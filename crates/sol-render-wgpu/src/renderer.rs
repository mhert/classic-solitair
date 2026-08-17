//! [`Renderer`]: the batched wgpu sprite renderer.
//!
//! One textured-quad pipeline, one atlas texture, one vertex/index buffer
//! pair rebuilt per frame from the presenter's [`DisplayList`]. The
//! target and projection are pixel-space orthographic; a frame is drawn
//! by clearing to the list's clear color — or loading the previous
//! contents when the list says not to clear, which is what the win
//! cascade's smear trail is made of.

use sol_presenter::DisplayList;
use sol_theme::{CardScaling, Theme};

use crate::atlas::{self, Atlas};
use crate::error::RenderError;
use crate::scale::{ceil_factor, content_factor, pixel_aa};
use crate::vertex::{Vertex, build_batch};

/// The atlas texture's pixel format. Plain (non-sRGB) so the theme's
/// bytes pass through the pipeline untouched — this renderer is a blitter
/// with no color management, like the original.
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Initial buffer capacity in quads: a full dealt board is ~53 sprites,
/// so a typical frame never grows the buffers.
const INITIAL_QUADS: usize = 64;

/// Byte size of the shader's `Globals` uniform (one `vec4<f32>`); the
/// buffer size and the bind-group layout's `min_binding_size` must both
/// match `shader.wgsl`.
const GLOBALS_BYTES: u64 = 16;

/// A self-contained atlas rebuild, produced by [`Renderer::adopt_scale`]
/// or [`Renderer::apply_atlas`] when the loaded atlas needs to change.
///
/// Holds its own copy of the theme and carries no GPU handles, so
/// [`Self::run`] can execute on any thread — the renderer itself never
/// spawns one or otherwise touches threads. Hand a successful result back
/// through [`Renderer::apply_atlas`]; if [`Self::run`] fails, report it
/// through [`Renderer::job_failed`] so the factor is not leaked.
#[derive(Debug)]
pub struct AtlasBuildJob {
    theme: Theme,
    factor: u32,
    max_dim: u32,
}

impl AtlasBuildJob {
    /// The content factor this job rasterizes at — already resolved
    /// against the device's texture limit, so running it cannot yield a
    /// different factor than this one.
    #[must_use]
    pub const fn factor(&self) -> u32 {
        self.factor
    }

    /// Rasterizes the theme at [`Self::factor`] on the CPU — xBRZ for a
    /// PNG theme whose player chose it, resvg for a vector theme —
    /// exactly like the synchronous path, just not tied to the renderer
    /// or its device — safe to run on a frontend's own thread while
    /// rendering continues on the atlas already loaded.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] if an asset fails to decode or rasterize, or the
    /// packed sheet cannot fit the device's texture limit.
    pub fn run(self) -> Result<BuiltAtlas, RenderError> {
        atlas::build(&self.theme, self.factor, self.max_dim).map(BuiltAtlas)
    }
}

/// An atlas built by [`AtlasBuildJob::run`], ready for
/// [`Renderer::apply_atlas`].
///
/// Opaque: its shape is a renderer implementation detail. Send and
/// `'static` so a frontend can hand it back across a thread boundary.
#[derive(Debug)]
pub struct BuiltAtlas(Atlas);

/// The batched wgpu sprite renderer for one loaded theme.
#[derive(Debug)]
pub struct Renderer {
    theme: Theme,
    scaling: CardScaling,
    max_dim: u32,
    /// The content factor the loaded atlas should hold, already clamped
    /// through [`atlas::plan_factor`] against `max_dim`.
    wanted: u32,
    /// The content factors of every build job handed out but not yet
    /// resolved — by [`Renderer::apply_atlas`] landing or discarding it, or
    /// by [`Renderer::job_failed`] reporting its build failed. The dedup
    /// ledger, so a repeated [`Renderer::adopt_scale`] toward any factor
    /// already building hands out no second job. A set rather than a single
    /// slot because adopts can cross several boundaries before any build
    /// lands, leaving more than one job outstanding at once; the realistic
    /// factor range is tiny, so a plain `Vec` used as a set is the whole
    /// thing.
    pending: Vec<u32>,
    /// The content factors whose most recent build was reported failed
    /// through [`Renderer::job_failed`]. While a factor is in here `next_step`
    /// refuses to re-issue a job for it; [`Renderer::adopt_scale`] drops every
    /// factor except the one it plans (`retain(|&f| f == planned)`), so
    /// planning any *different* factor lifts that factor's damping and
    /// returning to it later builds again, while re-planning the same factor
    /// keeps it damped. A set rather than a single slot for the same reason
    /// `pending` is: several factors can fail independently while their jobs
    /// are outstanding, and one failure must not clobber another's damping.
    damped: Vec<u32>,
    /// The one atlas most recently displaced by an applied build or a
    /// cache swap, CPU bytes retained so oscillating back to it needs
    /// only a re-upload, never a rebuild.
    previous: Option<Atlas>,
    display_scale: f32,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    atlas: Atlas,
    atlas_texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
}

impl Renderer {
    /// The device limits this renderer needs: the WebGL2-compatible
    /// downlevel baseline, with the adapter's actual resolution limits so
    /// capable hardware can hold larger atlases.
    #[must_use]
    pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    }

    /// Builds the pipeline and the atlas for `theme` at the continuous
    /// display `scale` (non-finite or non-positive values fall back to
    /// 1.0), rendering into `target_format` targets.
    ///
    /// `target_format` should be a non-sRGB format: this renderer does no
    /// color management, and a linear target passes the theme's bytes
    /// through unchanged.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] if a theme asset fails to decode or rasterize,
    /// or the assets cannot fit one atlas texture on this device.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        theme: Theme,
        scaling: CardScaling,
        scale: f32,
    ) -> Result<Self, RenderError> {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let mode = theme.manifest.render_mode;
        let max_dim = device.limits().max_texture_dimension_2d;

        let bind_group_layout = create_bind_group_layout(device);
        // PNG themes at Original get their hard texel edges from the AA
        // entry point, not the sampler, so every combination samples
        // linearly.
        let fragment_entry = if pixel_aa(mode, scaling) {
            "fs_pixel_aa"
        } else {
            "fs_main"
        };
        let pipeline = create_pipeline(device, target_format, &bind_group_layout, fragment_entry);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sol atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sol globals"),
            size: GLOBALS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let content = content_factor(mode, scaling, ceil_factor(scale));
        let atlas = atlas::build(&theme, content, max_dim)?;
        // build() already resolved `content` through plan_factor, so the
        // loaded atlas satisfies the want the moment construction finishes.
        let wanted = atlas.factor;
        let (atlas_texture, bind_group) = upload_atlas(
            device,
            queue,
            &bind_group_layout,
            &sampler,
            &uniforms,
            &atlas,
        );

        let vertex_buffer = create_vertex_buffer(device, INITIAL_QUADS * 4);
        let index_buffer = create_index_buffer(device, INITIAL_QUADS * 6);

        Ok(Self {
            theme,
            scaling,
            max_dim,
            wanted,
            pending: Vec::new(),
            damped: Vec::new(),
            previous: None,
            display_scale: scale,
            pipeline,
            bind_group_layout,
            sampler,
            uniforms,
            atlas,
            atlas_texture,
            bind_group,
            vertex_buffer,
            index_buffer,
        })
    }

    /// The content factor the atlas currently holds (test observability
    /// and diagnostics): 1 for PNG themes at Original, xBRZ's ceiling at
    /// Xbrz, the resvg factor for vector themes.
    #[must_use]
    pub const fn atlas_factor(&self) -> u32 {
        self.atlas.factor
    }

    /// The device's texture size ceiling (`max_texture_dimension_2d`),
    /// the same limit this renderer already plans atlas content factors
    /// against. Exposed for callers laying out an image that must fit one
    /// texture — a card-back contact sheet's `max_side`, say.
    #[must_use]
    pub const fn max_texture_dim(&self) -> u32 {
        self.max_dim
    }

    /// Adopts a continuous display scale immediately: `render` stretches
    /// the currently loaded atlas by `scale` starting with the very next
    /// call — the scene transform is scale-driven, independent of which
    /// atlas is loaded. Plans the content factor that scale wants
    /// (`content_factor(render_mode, scaling, ceil_factor(scale))`,
    /// clamped through [`atlas::plan_factor`] against this device's
    /// texture limit) and returns a job to rasterize it when,
    /// and only when, all of these hold:
    ///
    /// - the planned factor differs from the loaded atlas's factor;
    /// - the one-slot previous-atlas cache does not already hold it (a
    ///   cache hit is swapped in immediately instead: upload only, no
    ///   job, no rasterization);
    /// - an identical job has not already been handed out and left
    ///   unresolved (repeated adopts across the same crossing return
    ///   `None`, so callers never get duplicate builds);
    /// - the planned factor is not currently damped by a reported build
    ///   failure ([`Self::job_failed`]).
    ///
    /// Planning a factor first lifts the retry damping of every *other*
    /// factor, so a factor whose build once failed builds again as soon as
    /// the plan moves away and later returns; re-planning the same damped
    /// factor keeps it damped. Non-finite or non-positive scales fall back
    /// to 1.0.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] if planning fails (the theme cannot fit even
    /// factor 1 on this device — only possible if [`Self::new`] itself
    /// would now fail); the loaded atlas and scale are left unchanged.
    pub fn adopt_scale(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scale: f32,
    ) -> Result<Option<AtlasBuildJob>, RenderError> {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let content = content_factor(
            self.theme.manifest.render_mode,
            self.scaling,
            ceil_factor(scale),
        );
        // Plan first (pure integer math, no rasterization) so a failure
        // leaves both the scale and the want untouched.
        let planned = atlas::plan_factor(&self.theme, content, self.max_dim)?;
        // Planning any factor other than a damped one lifts that factor's
        // retry damping: it builds again once the plan has moved away from it
        // and later returns. Re-planning the same factor keeps it damped.
        self.damped.retain(|&factor| factor == planned);
        self.wanted = planned;
        self.display_scale = scale;
        Ok(self.next_step(device, queue))
    }

    /// Resolves a build from a job this renderer handed out.
    ///
    /// If `built` still matches the wanted factor and differs from the one
    /// already loaded: uploads it, swaps it in, retains the outgoing atlas
    /// in the one-slot cache, and returns `None`. Otherwise the result is
    /// redundant — the want moved on while it built, or that factor is
    /// already on screen — so `built` is discarded (never overwriting a
    /// fresher atlas, never disturbing the cache) and this returns
    /// whatever step is still needed toward the current want: a follow-up
    /// job, or `None` if none is needed.
    #[must_use]
    pub fn apply_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        built: BuiltAtlas,
    ) -> Option<AtlasBuildJob> {
        let atlas = built.0;
        // This specific job is resolved either way: applied below, or
        // discarded by one of the guards. Dropping it from the outstanding
        // set here (rather than only on the fresh branch) is what lets a
        // later adopt toward this same factor try again instead of
        // deduping against a job that has already called back in.
        self.pending.retain(|&factor| factor != atlas.factor);
        // Already loaded: a result at the factor already on screen adds
        // nothing. Unreachable through the public API while the dedup
        // invariant holds — a job is only ever issued for a factor differing
        // from the loaded one, and each factor keeps at most one job
        // outstanding — but kept as defense-in-depth: re-swapping an
        // identical atlas would evict the genuinely different atlas the
        // one-slot cache is holding. Discard it.
        if atlas.factor == self.atlas.factor {
            return self.next_step(device, queue);
        }
        // Stale: the want moved on while it built. Never overwrite a
        // fresher atlas; take whatever step the current want still needs.
        if atlas.factor != self.wanted {
            return self.next_step(device, queue);
        }
        let outgoing = std::mem::replace(&mut self.atlas, atlas);
        self.upload_current(device, queue);
        self.previous = Some(outgoing);
        None
    }

    /// Reports that the [`AtlasBuildJob`] for `factor` failed to
    /// [`run`](AtlasBuildJob::run) — the failure-report channel for callers
    /// driving [`Self::adopt_scale`] / [`Self::apply_atlas`] off-thread.
    ///
    /// Callers MUST invoke this whenever a job's `run()` returns `Err`,
    /// passing the failed job's [`factor`](AtlasBuildJob::factor). A job that
    /// is neither applied through [`Self::apply_atlas`] nor reported here
    /// stays recorded as outstanding, and the renderer would never hand out
    /// another job for that factor.
    ///
    /// Has an effect only when `factor` is a currently outstanding job: it is
    /// then dropped from the outstanding set so the factor is not leaked, and
    /// retry is damped — the same `factor` is not re-issued until an
    /// [`Self::adopt_scale`] first plans a *different* factor (after which
    /// returning to `factor` plans it afresh and may build again). Reporting a
    /// `factor` that is not outstanding (never issued, or already resolved) is
    /// caller misuse and does nothing, so a stray or duplicate report cannot
    /// corrupt the retry state of a genuinely outstanding factor. The loaded
    /// atlas is untouched — rendering continues on it at the already-adopted
    /// continuous scale.
    pub fn job_failed(&mut self, factor: u32) {
        // Only a job that is actually outstanding can fail. A `factor` that
        // was never issued (or already resolved) is caller misuse: doing
        // nothing keeps a stray or duplicate report from damping a factor no
        // job ever planned, or from clobbering another factor's damping.
        if !self.pending.contains(&factor) {
            return;
        }
        self.pending.retain(|&outstanding| outstanding != factor);
        self.damped.push(factor);
    }

    /// Adopts a continuous display scale and settles the atlas before
    /// returning: equivalent to calling [`Self::adopt_scale`], running
    /// any returned job inline, and feeding the result to
    /// [`Self::apply_atlas`], repeated until no job remains. The CPU cost
    /// of a factor change — xBRZ for a PNG theme at Xbrz, resvg for a
    /// vector theme — lands on the caller's thread; frontends that want
    /// that work off-thread should drive
    /// `adopt_scale`/`apply_atlas` directly instead of this method.
    ///
    /// Only rebuilds when the planned factor actually changes — never
    /// per resize tick.
    ///
    /// Non-finite or non-positive scales fall back to 1.0.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] if planning or a build job fails; the
    /// previously loaded atlas stays in use either way. If planning
    /// itself fails, the scale is not adopted either (see
    /// [`Self::adopt_scale`]); if a build fails after the scale was
    /// already adopted, the scale stays adopted (the transform stretches
    /// the still-current atlas by it), but the atlas is not replaced. As
    /// with the async API this wraps, a factor whose build failed is not
    /// retried on a later call until the planned factor changes again in
    /// between.
    pub fn set_display_scale(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scale: f32,
    ) -> Result<(), RenderError> {
        let mut job = self.adopt_scale(device, queue, scale)?;
        while let Some(current) = job {
            let factor = current.factor();
            let built = match current.run() {
                Ok(built) => built,
                // Report the failure through the same channel the async path
                // uses before propagating: otherwise this factor would stay
                // recorded as outstanding and never rebuild, breaking the
                // documented "retried once the plan changes again" contract.
                Err(err) => {
                    self.job_failed(factor);
                    return Err(err);
                }
            };
            job = self.apply_atlas(device, queue, built);
        }
        Ok(())
    }

    /// The step still needed to bring the loaded atlas to `self.wanted`:
    /// `None` if it is already loaded, an immediate cache swap if the
    /// one-slot cache already holds it, `None` again if a job for it is
    /// already out or if it is damped after a reported failure, or else a
    /// fresh build job.
    fn next_step(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<AtlasBuildJob> {
        if self.wanted == self.atlas.factor {
            return None;
        }
        if self
            .previous
            .as_ref()
            .is_some_and(|cached| cached.factor == self.wanted)
        {
            self.swap_cache(device, queue);
            return None;
        }
        if self.pending.contains(&self.wanted) {
            return None;
        }
        // Damped: this factor's last build was reported failed and the plan
        // has not moved off it since, so hand out no fresh job yet.
        if self.damped.contains(&self.wanted) {
            return None;
        }
        self.pending.push(self.wanted);
        Some(AtlasBuildJob {
            theme: self.theme.clone(),
            factor: self.wanted,
            max_dim: self.max_dim,
        })
    }

    /// Swaps the one-slot cache into place: it becomes the loaded atlas,
    /// re-uploaded from its retained CPU bytes (no rasterization), and
    /// the previously loaded atlas takes its place in the cache.
    fn swap_cache(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(cached) = self.previous.take() else {
            return;
        };
        let outgoing = std::mem::replace(&mut self.atlas, cached);
        self.upload_current(device, queue);
        self.previous = Some(outgoing);
    }

    /// Re-uploads `self.atlas` into a fresh GPU texture and bind group,
    /// replacing the ones currently bound.
    fn upload_current(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (texture, bind_group) = upload_atlas(
            device,
            queue,
            &self.bind_group_layout,
            &self.sampler,
            &self.uniforms,
            &self.atlas,
        );
        self.atlas_texture = texture;
        self.bind_group = bind_group;
    }

    /// Draws one frame of `list` into `target` (a `viewport`-sized render
    /// target in pixels) at this renderer's own adopted display scale,
    /// encoding and submitting its own command buffer. A thin delegate to
    /// [`Self::render_at`]; see it for the full drawing contract.
    ///
    /// A `list.clear` color clears the target first; `None` loads the
    /// previous contents and draws over them (the cascade's smear).
    ///
    /// # Errors
    ///
    /// [`RenderError::UnknownTexture`] if the list references a texture
    /// this renderer's theme does not provide. Nothing is drawn.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
        list: &DisplayList,
    ) -> Result<(), RenderError> {
        let scale = self.display_scale;
        self.render_at(device, queue, target, viewport, scale, list)
    }

    /// Draws one frame of `list` into `target` (a `viewport`-sized render
    /// target in pixels) at an explicit display `scale`, leaving every
    /// piece of adopted renderer state untouched: the adopted display
    /// scale, the loaded atlas, the planned content factor. [`Self::render`]
    /// is this method called with the adopted scale; a caller that needs to
    /// draw at a scale of its own — a card-back contact sheet, sized for
    /// the Options dialog rather than the board's window fit — calls this
    /// directly instead, without disturbing what the board renders next.
    /// The scale is nothing but a uniform in the vertex transform, so
    /// parameterizing it costs nothing extra.
    ///
    /// Non-finite or non-positive scales fall back to `1.0`, matching
    /// [`Self::adopt_scale`].
    ///
    /// A `list.clear` color clears the target first; `None` loads the
    /// previous contents and draws over them (the cascade's smear).
    ///
    /// # Errors
    ///
    /// [`RenderError::UnknownTexture`] if the list references a texture
    /// this renderer's theme does not provide. Nothing is drawn.
    pub fn render_at(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
        scale: f32,
        list: &DisplayList,
    ) -> Result<(), RenderError> {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let (vertices, indices) = build_batch(list, &self.atlas)?;
        if list.clear.is_none() && indices.is_empty() {
            return Ok(());
        }

        let (width, height) = (viewport.0.max(1), viewport.1.max(1));
        #[allow(clippy::cast_precision_loss)] // window sizes are far below 2^24
        let transform = [
            2.0 * scale / width as f32,
            -2.0 * scale / height as f32,
            -1.0_f32,
            1.0_f32,
        ];
        queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&transform));

        if !indices.is_empty() {
            let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);
            let index_bytes: &[u8] = bytemuck::cast_slice(&indices);
            let needed_vertex = u64::try_from(vertex_bytes.len()).unwrap_or(u64::MAX);
            let needed_index = u64::try_from(index_bytes.len()).unwrap_or(u64::MAX);
            if self.vertex_buffer.size() < needed_vertex {
                self.vertex_buffer = create_vertex_buffer(device, vertices.len() * 2);
            }
            if self.index_buffer.size() < needed_index {
                self.index_buffer = create_index_buffer(device, indices.len() * 2);
            }
            queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
            queue.write_buffer(&self.index_buffer, 0, index_bytes);
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sol frame"),
        });
        {
            let load = match list.clear {
                Some(color) => wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(color.r) / 255.0,
                    g: f64::from(color.g) / 255.0,
                    b: f64::from(color.b) / 255.0,
                    a: f64::from(color.a) / 255.0,
                }),
                // The cascade's don't-clear mode: keep the previous frame
                // and smear over it.
                None => wgpu::LoadOp::Load,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sol playfield pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !indices.is_empty() {
                let vertex_len =
                    u64::try_from(size_of_val(vertices.as_slice())).unwrap_or(u64::MAX);
                let index_len = u64::try_from(size_of_val(indices.as_slice())).unwrap_or(u64::MAX);
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..vertex_len));
                pass.set_index_buffer(
                    self.index_buffer.slice(..index_len),
                    wgpu::IndexFormat::Uint32,
                );
                let count = u32::try_from(indices.len()).unwrap_or(u32::MAX);
                pass.draw_indexed(0..count, 0, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
        Ok(())
    }
}

/// The pipeline's one bind group layout: atlas texture, sampler, and the
/// projection uniform.
fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sol sprite bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(GLOBALS_BYTES),
                },
                count: None,
            },
        ],
    })
}

/// The one textured-quad render pipeline, with the fragment entry point
/// chosen by `pixel_aa`'s `(render_mode, scaling)` pair (plain sampling or
/// pixel-art AA).
fn create_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    bind_group_layout: &wgpu::BindGroupLayout,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sol sprite shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sol sprite pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sol sprite pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x2,
                    1 => Float32x2,
                    2 => Float32x4,
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                // Everything in this pipeline is premultiplied.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Uploads `atlas` into a fresh texture and rebinds the bind group.
fn upload_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
    atlas: &Atlas,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let size = wgpu::Extent3d {
        width: atlas.width,
        height: atlas.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sol atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &atlas.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(atlas.width * 4),
            rows_per_image: Some(atlas.height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sol sprite bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    });
    (texture, bind_group)
}

fn create_vertex_buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sol vertices"),
        size: (vertices.max(4) * size_of::<Vertex>()) as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_index_buffer(device: &wgpu::Device, indices: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sol indices"),
        size: (indices.max(6) * size_of::<u32>()) as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
