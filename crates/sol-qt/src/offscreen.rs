//! Headless wgpu playfield rendering: display lists in, RGBA8 frames out.
//!
//! This is the embedding decision made concrete: the playfield never owns
//! a window or surface. Frames render into a persistent canvas texture
//! (persistent so the win cascade's don't-clear smear accumulates, same
//! as `sol-shell`'s canvas) and are read back over a cached staging
//! buffer for the QML layer to draw.

use sol_presenter::DisplayList;
use sol_render_wgpu::{AtlasBuildJob, BuiltAtlas, RenderError, Renderer, render_to_rgba};
use sol_theme::{CardScaling, Theme};

/// The canvas format: plain non-sRGB bytes, matching the renderer's
/// no-color-management contract.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One rendered playfield frame in tightly packed RGBA8 rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Errors from the offscreen render path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OffscreenError {
    /// No Vulkan or GL adapter (hardware or software) exists.
    #[error("no graphics adapter available (Vulkan or GL, hardware or software)")]
    NoAdapter,
    /// The adapter refused to give out a device.
    #[error("requesting the graphics device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// The sprite renderer failed (atlas build, rescale, or draw).
    #[error(transparent)]
    Render(#[from] RenderError),
    /// Waiting for the readback copy to finish failed.
    #[error("polling the graphics device")]
    Poll(#[from] wgpu::PollError),
    /// Mapping the readback buffer failed.
    #[error("mapping the readback buffer")]
    Map(#[from] wgpu::BufferAsyncError),
    /// Viewing the mapped readback range failed.
    #[error("viewing the mapped readback range")]
    MapRange(#[from] wgpu::MapRangeError),
    /// The readback completion callback never delivered a result.
    #[error("the readback completion callback was dropped unresolved")]
    MapLost,
    /// The mapped readback bytes do not match the expected row layout.
    #[error("readback buffer layout mismatch")]
    RowLayout,
}

/// The persistent per-size canvas texture. Recreated only when the
/// viewport size actually changes, never per frame — it carries the win
/// cascade's don't-clear smear across frames.
struct Canvas {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

/// The grow-only GPU→CPU readback staging buffer. Reused across
/// `render` calls whenever its capacity already covers the current
/// frame's padded byte size, reallocated only when it is too small.
/// Sized to the frame, not the canvas or the theme, so it survives both
/// a shrink-then-regrow and a `set_theme` swap untouched.
struct Staging {
    buffer: wgpu::Buffer,
    /// The buffer's allocated size in bytes — not necessarily what the
    /// current frame needs, only an upper bound on it.
    capacity: u64,
}

/// A headless wgpu device driving [`Renderer`] into readable frames.
pub struct Offscreen {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    canvas: Option<Canvas>,
    staging: Option<Staging>,
}

impl Offscreen {
    /// Acquires a headless device (preferring a hardware adapter,
    /// falling back to a software rasterizer) and builds the sprite
    /// renderer for `theme` at the continuous display `scale`.
    ///
    /// # Errors
    ///
    /// [`OffscreenError::NoAdapter`] when no adapter exists at all,
    /// [`OffscreenError::RequestDevice`] when the device request fails,
    /// or a [`RenderError`] from building the atlas.
    pub fn new(theme: Theme, scaling: CardScaling, scale: f32) -> Result<Self, OffscreenError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapters = pollster::block_on(
            instance.enumerate_adapters(wgpu::Backends::VULKAN | wgpu::Backends::GL),
        );
        let adapter = adapters
            .iter()
            .find(|adapter| adapter.get_info().device_type != wgpu::DeviceType::Cpu)
            .or_else(|| adapters.first())
            .ok_or(OffscreenError::NoAdapter)?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("sol qt device"),
                required_limits: Renderer::required_limits(adapter),
                ..Default::default()
            }))?;
        let renderer = Renderer::new(&device, &queue, CANVAS_FORMAT, theme, scaling, scale)?;
        Ok(Self {
            device,
            queue,
            renderer,
            canvas: None,
            staging: None,
        })
    }

    /// The device's texture size ceiling
    /// ([`Renderer::max_texture_dim`]): the largest a texture — including
    /// a card-back contact sheet drawn through [`Self::render_sheet`] —
    /// can be on this device.
    #[must_use]
    pub const fn max_texture_dim(&self) -> u32 {
        self.renderer.max_texture_dim()
    }

    /// Adopts a continuous display scale for the sprite renderer,
    /// planning (but not yet rasterizing) the atlas rebuild it needs.
    ///
    /// A thin pass-through to [`Renderer::adopt_scale`]: the returned
    /// job, if any, is self-contained and can run anywhere before its
    /// result comes back through [`Self::apply_atlas`].
    ///
    /// # Errors
    ///
    /// A [`RenderError`] from planning the retarget (the previous scale
    /// stays active on failure).
    pub fn adopt_scale(&mut self, scale: f32) -> Result<Option<AtlasBuildJob>, OffscreenError> {
        let job = self
            .renderer
            .adopt_scale(&self.device, &self.queue, scale)?;
        Ok(job)
    }

    /// Resolves a build from a job [`Self::adopt_scale`] handed out. A
    /// thin pass-through to [`Renderer::apply_atlas`] — see it for the
    /// full stale/fresh/cache-hit contract. `Some` means another job is
    /// still needed toward the current want.
    #[must_use]
    pub fn apply_atlas(&mut self, built: BuiltAtlas) -> Option<AtlasBuildJob> {
        self.renderer.apply_atlas(&self.device, &self.queue, built)
    }

    /// Reports that an outstanding job's [`AtlasBuildJob::run`] failed.
    /// A thin pass-through to [`Renderer::job_failed`] — callers MUST
    /// call this on every `run()` error, passing the failed job's
    /// [`factor`](AtlasBuildJob::factor) captured before the call.
    pub fn job_failed(&mut self, factor: u32) {
        self.renderer.job_failed(factor);
    }

    /// Replaces the theme by rebuilding the sprite renderer (the atlas
    /// is theme-shaped, so a swap is a rebuild). The canvas is dropped:
    /// the first frame after a theme switch starts clean. The readback
    /// staging buffer is left in place — it is sized to the frame, not
    /// the theme, so a swap has no reason to touch it.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] from building the new atlas; the offscreen
    /// state is unusable for rendering the new theme until a successful
    /// call (the caller re-tries or reverts by calling again with the
    /// previous theme).
    pub fn set_theme(
        &mut self,
        theme: Theme,
        scaling: CardScaling,
        scale: f32,
    ) -> Result<(), OffscreenError> {
        self.renderer = Renderer::new(
            &self.device,
            &self.queue,
            CANVAS_FORMAT,
            theme,
            scaling,
            scale,
        )?;
        self.canvas = None;
        Ok(())
    }

    /// Renders `list` into the persistent canvas and reads the result
    /// back as a tightly packed RGBA8 frame.
    ///
    /// The canvas is recreated only when `width`×`height` changes; a
    /// don't-clear list therefore paints over the previous frame's
    /// pixels, which is what produces the win cascade's smear trail.
    /// The readback staging buffer is grow-only and independent of the
    /// canvas: it is reused whenever its capacity already covers this
    /// frame, and only reallocated when it is too small.
    ///
    /// # Errors
    ///
    /// A [`RenderError`] from drawing, or a readback failure
    /// ([`OffscreenError::Poll`] / [`Map`](OffscreenError::Map) /
    /// [`MapLost`](OffscreenError::MapLost) /
    /// [`RowLayout`](OffscreenError::RowLayout)).
    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        list: &DisplayList,
    ) -> Result<Frame, OffscreenError> {
        let (width, height) = (width.max(1), height.max(1));
        // The canvas is kept across frames and rebuilt only on an actual
        // size change (it carries the cascade smear between frames).
        if self
            .canvas
            .as_ref()
            .is_none_or(|canvas| (canvas.width, canvas.height) != (width, height))
        {
            self.canvas = None;
        }
        let canvas = self
            .canvas
            .get_or_insert_with(|| Self::build_canvas(&self.device, width, height));
        let view = canvas
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer
            .render(&self.device, &self.queue, &view, (width, height), list)?;

        // Grow-only: the staging buffer is size-shaped, not
        // canvas-shaped — it survives shrinks and is only rebuilt when
        // too small for this frame's padded byte size.
        let bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let needed = u64::from(bytes_per_row) * u64::from(height);
        if self
            .staging
            .as_ref()
            .is_none_or(|staging| staging.capacity < needed)
        {
            self.staging = None;
        }
        let staging = self
            .staging
            .get_or_insert_with(|| Self::build_staging(&self.device, needed));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sol qt readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &canvas.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        // Map only the range this frame actually uses: a buffer left
        // over from a larger previous frame must not read back its
        // stale tail as if it were part of this one.
        staging
            .buffer
            .map_async(wgpu::MapMode::Read, 0..needed, move |result| {
                // The receiver only ever sees this one send; a dropped
                // receiver means the poll below already failed.
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv().map_err(|_| OffscreenError::MapLost)??;
        // Unmap unconditionally: a mapped-range failure must not leave
        // the buffer mapped, or every later frame would fail too.
        let rgba = staging
            .buffer
            .get_mapped_range(0..needed)
            .map(|mapped| unpad_rows(&mapped, width, height, bytes_per_row));
        staging.buffer.unmap();
        Ok(Frame {
            width,
            height,
            rgba: rgba?.ok_or(OffscreenError::RowLayout)?,
        })
    }

    /// One-shot render of `list` at `scale` into a fresh `size` (physical
    /// pixels) target, read back as tightly packed RGBA8 rows — a thin
    /// pass-through to [`render_to_rgba`].
    ///
    /// Deliberately not [`Self::render`]: that method paints into the
    /// persistent canvas the win cascade's don't-clear smear depends on
    /// surviving frame to frame, sized to the playfield's own viewport. A
    /// card-back contact sheet is a different shape entirely — sized for
    /// the Options dialog rather than the board, drawn once whenever the
    /// dialog wants a fresh preview, and never touched again. This method
    /// allocates and drops its own target and readback buffer inside the
    /// call, touching neither the canvas nor the staging buffer
    /// [`Self::render`] keeps alive.
    ///
    /// # Errors
    ///
    /// An [`OffscreenError::Render`] wrapping whatever [`render_to_rgba`]
    /// reports: the draw itself failing, or a readback failure.
    pub fn render_sheet(
        &mut self,
        list: &DisplayList,
        size: (u32, u32),
        scale: f32,
    ) -> Result<Vec<u8>, OffscreenError> {
        Ok(render_to_rgba(
            &self.device,
            &self.queue,
            &mut self.renderer,
            list,
            size,
            scale,
        )?)
    }

    /// Allocates the canvas texture for `width`×`height`.
    fn build_canvas(device: &wgpu::Device, width: u32, height: u32) -> Canvas {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sol qt canvas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Canvas {
            texture,
            width,
            height,
        }
    }

    /// Allocates a readback staging buffer with `size` bytes of capacity.
    fn build_staging(device: &wgpu::Device, size: u64) -> Staging {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sol qt readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Staging {
            buffer,
            capacity: size,
        }
    }
}

/// Strips the copy alignment padding: `width * 4`-byte rows out of
/// `bytes_per_row`-strided input. `None` when `mapped` is too short for
/// the claimed layout.
fn unpad_rows(mapped: &[u8], width: u32, height: u32, bytes_per_row: u32) -> Option<Vec<u8>> {
    let row_bytes = (width as usize).checked_mul(4)?;
    let stride = bytes_per_row as usize;
    let mut rgba = Vec::with_capacity(row_bytes.checked_mul(height as usize)?);
    for row in 0..height as usize {
        let start = row.checked_mul(stride)?;
        rgba.extend_from_slice(mapped.get(start..start.checked_add(row_bytes)?)?);
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The in-tree default theme, like the renderer's own tests use.
    fn default_theme() -> Theme {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes/default");
        Theme::load_dir(dir).unwrap()
    }

    /// Renders a presenter frame and checks the readback: correct
    /// dimensions, opaque pixels, and the persistent-canvas contract the
    /// win cascade needs (a don't-clear list keeps prior pixels).
    /// Skips when the machine has no graphics adapter at all — the
    /// renderer's own gates run under CI-installed Mesa.
    #[test]
    fn renders_and_reads_back_a_presenter_frame() {
        use sol_presenter::{Presenter, Rgba};
        use sol_session::{Options, Session};

        let theme = default_theme();
        let mut offscreen = match Offscreen::new(theme.clone(), CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };

        let presenter = Presenter::new(
            Session::new(Options::default(), sol_engine::Seed::new(1).unwrap()),
            &theme,
        );
        let (width, height) = (585, 384);
        let frame = offscreen.render(width, height, &presenter.frame()).unwrap();
        assert_eq!((frame.width, frame.height), (width, height));
        assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
        assert!(
            frame.rgba.chunks_exact(4).all(|px| px.last() == Some(&255)),
            "playfield frames are fully opaque"
        );

        // A don't-clear empty list must leave the previous frame intact
        // (the cascade smear relies on the canvas persisting)...
        let empty_no_clear = sol_presenter::DisplayList {
            clear: None,
            sprites: Vec::new(),
        };
        let kept = offscreen.render(width, height, &empty_no_clear).unwrap();
        assert_eq!(kept, frame, "no-clear frame keeps the canvas");

        // ...while a clearing list wipes it to the given color.
        let clear_red = sol_presenter::DisplayList {
            clear: Some(Rgba::opaque(255, 0, 0)),
            sprites: Vec::new(),
        };
        let wiped = offscreen.render(width, height, &clear_red).unwrap();
        assert!(
            wiped.rgba.chunks_exact(4).all(|px| px == [255, 0, 0, 255]),
            "clearing frame wipes the canvas"
        );

        // A size change rebuilds the canvas at the new dimensions.
        let resized = offscreen.render(300, 200, &clear_red).unwrap();
        assert_eq!((resized.width, resized.height), (300, 200));
    }

    /// Crossing a factor boundary (`adopt_scale` → run the job → `apply_atlas`)
    /// settles the retarget, and a frame rendered afterward still reads back
    /// at exactly the requested size with the expected content. The default
    /// theme is `vector` mode, so scale 1.0 → 2.0 always crosses a boundary
    /// (no per-mode clamping to dodge).
    #[test]
    fn adopt_scale_across_a_boundary_applies_and_renders() {
        use sol_presenter::{DisplayList, Rgba};

        let theme = default_theme();
        let mut offscreen = match Offscreen::new(theme, CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };

        let job = offscreen
            .adopt_scale(2.0)
            .unwrap()
            .expect("crossing from factor 1 to 2 yields a job");
        assert_eq!(job.factor(), 2);

        let built = job.run().unwrap();
        assert!(
            offscreen.apply_atlas(built).is_none(),
            "the want is satisfied after one apply"
        );

        let (width, height) = (300, 200);
        let clear_red = DisplayList {
            clear: Some(Rgba::opaque(255, 0, 0)),
            sprites: Vec::new(),
        };
        let frame = offscreen.render(width, height, &clear_red).unwrap();
        assert_eq!((frame.width, frame.height), (width, height));
        assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
        assert!(
            frame.rgba.chunks_exact(4).all(|px| px == [255, 0, 0, 255]),
            "frame at the retargeted factor still reads back correctly"
        );
    }

    /// Renders a large frame, then a smaller one, then the large size
    /// again: exercises the staging buffer's grow-only reuse. A
    /// used-range mapping bug would show up here as padding garbage
    /// bleeding into the unpadded result, or a length mismatch, once the
    /// buffer is bigger than what the smaller frame needs.
    #[test]
    fn staging_buffer_reuse_survives_shrink_and_regrow() {
        use sol_presenter::{DisplayList, Rgba};

        let theme = default_theme();
        let mut offscreen = match Offscreen::new(theme, CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };

        let sizes = [
            (640_u32, 480_u32, Rgba::opaque(255, 0, 0)),
            (64, 48, Rgba::opaque(0, 255, 0)),
            (640, 480, Rgba::opaque(0, 0, 255)),
        ];
        for (width, height, color) in sizes {
            let list = DisplayList {
                clear: Some(color),
                sprites: Vec::new(),
            };
            let frame = offscreen.render(width, height, &list).unwrap();
            assert_eq!((frame.width, frame.height), (width, height));
            assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
            assert!(
                frame
                    .rgba
                    .chunks_exact(4)
                    .all(|px| px == [color.r, color.g, color.b, 255]),
                "readback at {width}x{height} must be exactly the clear color, \
                 with no stale-buffer padding leaking in"
            );
        }
    }

    /// `render_sheet` draws at an explicit scale into a fresh target of
    /// the caller's own size, independent of the renderer's adopted
    /// display scale and the persistent canvas `render` uses — a second
    /// `render` call at the canvas's own size afterward must still read
    /// back the canvas's own color, proving the sheet render touched
    /// neither.
    #[test]
    fn render_sheet_draws_a_one_shot_target_independent_of_the_canvas() {
        use sol_presenter::{DisplayList, Rgba};

        let theme = default_theme();
        let mut offscreen = match Offscreen::new(theme, CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };

        let canvas_list = DisplayList {
            clear: Some(Rgba::opaque(1, 2, 3)),
            sprites: Vec::new(),
        };
        let canvas_frame = offscreen.render(50, 40, &canvas_list).unwrap();

        let sheet_list = DisplayList {
            clear: Some(Rgba::opaque(9, 8, 7)),
            sprites: Vec::new(),
        };
        let sheet = offscreen.render_sheet(&sheet_list, (12, 8), 2.0).unwrap();
        assert_eq!(
            sheet.len(),
            12 * 8 * 4,
            "tightly packed RGBA8 at the requested size"
        );
        assert!(
            sheet.chunks_exact(4).all(|px| px == [9, 8, 7, 255]),
            "the sheet clears to its own color, not the canvas's"
        );

        let replayed = offscreen.render(50, 40, &canvas_list).unwrap();
        assert_eq!(
            replayed, canvas_frame,
            "the canvas is unaffected by the sheet render in between"
        );
    }

    #[test]
    fn max_texture_dim_is_a_plausible_device_limit() {
        let theme = default_theme();
        let offscreen = match Offscreen::new(theme, CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };
        // The WebGL2-downlevel floor `Renderer::required_limits` itself
        // plans atlas factors against.
        assert!(offscreen.max_texture_dim() >= 2048);
    }

    #[test]
    fn unpad_rows_strips_alignment_padding() {
        // 2×2 image, rows padded to 12 bytes (stride > 8 real bytes).
        let mut mapped = Vec::new();
        for row in 0..2_u8 {
            for x in 0..2_u8 {
                mapped.extend_from_slice(&[row, x, 0xAA, 0xFF]);
            }
            mapped.extend_from_slice(&[0xEE; 4]); // padding
        }
        let rgba = unpad_rows(&mapped, 2, 2, 12).unwrap();
        assert_eq!(
            rgba,
            &[
                0, 0, 0xAA, 0xFF, 0, 1, 0xAA, 0xFF, 1, 0, 0xAA, 0xFF, 1, 1, 0xAA, 0xFF
            ]
        );
    }

    #[test]
    fn unpad_rows_is_identity_without_padding() {
        let mapped: Vec<u8> = (0..16).collect();
        assert_eq!(unpad_rows(&mapped, 2, 2, 8).unwrap(), mapped);
    }

    #[test]
    fn unpad_rows_rejects_short_input() {
        assert!(unpad_rows(&[0; 15], 2, 2, 8).is_none());
        assert!(unpad_rows(&[], 1, 1, 4).is_none());
    }
}
