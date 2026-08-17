//! The on-window render path: a wgpu surface on the playfield child
//! window, a persistent canvas texture the presenter frames accumulate
//! in (the win cascade's don't-clear smear lives there), and a
//! fullscreen-triangle blit that puts the canvas on the surface each
//! frame — swapchain images do not survive presentation, so the canvas
//! is where history accumulates. Same model as sol-shell; only the
//! surface construction differs, because the window handle comes from
//! native-windows-gui as a bare `HWND` instead of a winit window.
//!
//! # Why a dedicated render thread
//!
//! All of this runs on a worker thread behind [`RenderHandle`], never
//! on the GUI thread. Swapchain acquire/present are allowed to block —
//! and the presentation engine is free to never satisfy them (observed
//! under wine/KWin: the window-close interaction can stall the X11
//! present event stream mid-acquire, which froze the whole app when
//! rendering lived on the message-pump thread). A Win32 message pump
//! must never wait on presentation; here a stalled swapchain only
//! pauses drawing, while menus, input, and above all closing keep
//! working. The UI thread communicates through a channel and drops
//! frames instead of ever waiting: if the renderer falls more than a
//! couple of frames behind, new frames are simply skipped.
//!
//! Atlas rebuilds triggered by a scale change follow the same rule: a
//! vector theme's resvg rebuild can take hundreds of milliseconds, long
//! enough to stall frames if it ran inline here, so it always runs on
//! its own transient thread and reports back through this thread's own
//! command channel — the same one the UI thread sends through — rather
//! than ever touching the renderer off this thread. A PNG theme's atlas
//! factor never depends on the display scale (fixed at native pixels or
//! xBRZ's ceiling, whichever the player chose), so a scale change never
//! rebuilds one at all.

use std::num::NonZeroIsize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use sol_frontend::previews;
use sol_presenter::DisplayList;
use sol_render_wgpu::{
    AtlasBuildJob, BlitPipeline, BuiltAtlas, RenderError, Renderer, render_to_rgba,
};

/// Debug-label prefix for this host's blit objects, so a graphics debugger
/// names them for the frontend that created them.
const BLIT_LABEL: &str = "sol win32";
use sol_theme::{CardScaling, Theme};
use winapi::shared::windef::HWND;
use winapi::um::winuser::{GWLP_HINSTANCE, GetWindowLongPtrW};

/// Errors from the render path; fatal at startup, reported and skipped
/// per-frame afterwards.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GfxError {
    /// The playfield window has no valid `HWND` (destroyed, or not a
    /// window-backed control).
    #[error("the playfield window handle is not usable")]
    BadWindowHandle,
    /// wgpu rejected the window as a surface target.
    #[error("creating the playfield surface")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    /// No graphics adapter can drive the surface.
    #[error("no compatible graphics adapter")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    /// The adapter refused a device.
    #[error("requesting the graphics device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// The surface advertises no texture formats.
    #[error("the surface offers no formats")]
    NoFormats,
    /// Acquiring the next surface texture failed validation.
    #[error("surface texture acquisition failed validation")]
    SurfaceValidation,
    /// Building or rescaling the sprite renderer failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The render thread could not be spawned, or died before
    /// reporting readiness.
    #[error("starting the render thread")]
    RenderThread(#[source] std::io::Error),
}

/// The canvas format: plain non-sRGB bytes, matching the renderer's
/// no-color-management contract.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Everything that exists only while the playfield window does.
pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    canvas: wgpu::Texture,
    blit: BlitPipeline,
    blit_bind: wgpu::BindGroup,
    /// Whether `SOL_WIN32_LOG` was set when this path was built. Read once:
    /// the variable cannot change during the process's life, and the draw
    /// path consults it every frame.
    log: bool,
}

impl Gfx {
    /// Builds the whole render path onto the window behind `hwnd`,
    /// sized `width`×`height` physical pixels.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] when the surface, adapter, device, or renderer
    /// cannot be created.
    ///
    /// # Safety contract (upheld by the caller structurally)
    ///
    /// `hwnd` must stay a valid window for this value's whole lifetime.
    /// The render thread owns the [`Gfx`]; [`RenderHandle::shutdown`]
    /// waits (bounded) for its drop before the UI lets the control
    /// windows be destroyed, and the abandoned-thread case never drops
    /// at all (process exit reclaims everything).
    pub fn new(
        hwnd: HWND,
        (width, height): (u32, u32),
        theme: Theme,
        scaling: CardScaling,
        scale: f32,
    ) -> Result<Self, GfxError> {
        let hwnd_value = NonZeroIsize::new(hwnd as isize).ok_or(GfxError::BadWindowHandle)?;
        let mut window_handle = Win32WindowHandle::new(hwnd_value);
        // The Vulkan backend wants the owning module; DX12 ignores it.
        // SAFETY: `hwnd` is a live window created by this process (the
        // caller's contract above); reading its window data is sound.
        #[allow(unsafe_code)]
        let hinstance = unsafe { GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) };
        window_handle.hinstance = NonZeroIsize::new(hinstance);

        // No display handle: on Windows the surface target alone
        // carries everything the backends need. The `_from_env` form
        // honors wgpu's standard overrides (WGPU_BACKEND=…), which is
        // how field reports can pin a backend without a rebuild.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // SAFETY: the handles are valid for the caller-guaranteed
        // lifetime of `hwnd` (see the safety contract above).
        #[allow(unsafe_code)]
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
                raw_window_handle: RawWindowHandle::Win32(window_handle),
            })
        }?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let log = std::env::var_os("SOL_WIN32_LOG").is_some();
        if log {
            let info = adapter.get_info();
            eprintln!(
                "sol-win32 log: adapter \"{}\" backend {:?} type {:?}",
                info.name, info.backend, info.device_type
            );
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("sol win32 device"),
                required_limits: Renderer::required_limits(&adapter),
                ..Default::default()
            }))?;

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB surface format: the renderer is a byte-exact
        // blitter with no color management.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(GfxError::NoFormats)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            // No-vsync presentation, NOT Fifo: frame pacing already
            // comes from the UI's 16 ms timer (the same model as
            // sol-qt), and a Fifo acquire parks the GUI thread in an
            // unbounded vsync wait the presentation engine is free to
            // never satisfy — observed under wine/KWin, where the
            // close interaction perturbs the X11 present event stream
            // mid-acquire and the app freezes before WM_CLOSE can even
            // be dispatched. A message-pump thread must never block on
            // presentation. The desktop compositor absorbs tearing in
            // windowed mode on every target.
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: Vec::new(),
        };
        surface.configure(&device, &config);

        let renderer = Renderer::new(&device, &queue, CANVAS_FORMAT, theme, scaling, scale)?;
        let blit = BlitPipeline::new(&device, format, BLIT_LABEL);
        let canvas = create_canvas(&device, &config);
        let blit_bind = blit.bind(&device, &canvas, BLIT_LABEL);
        Ok(Self {
            surface,
            device,
            queue,
            config,
            renderer,
            canvas,
            blit,
            blit_bind,
            log,
        })
    }

    /// Adopts a new surface size in physical pixels. Gated on actual
    /// change — Win32 repeats `WM_SIZE` freely, and reconfiguring
    /// mid-drag for a same-size event would wipe the canvas.
    pub fn resize(&mut self, (width, height): (u32, u32)) {
        let (width, height) = (width.max(1), height.max(1));
        if (self.config.width, self.config.height) == (width, height) {
            return;
        }
        if self.log {
            eprintln!("sol-win32 log: reconfigure {width}x{height}");
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.canvas = create_canvas(&self.device, &self.config);
        self.blit_bind = self.blit.bind(&self.device, &self.canvas, BLIT_LABEL);
    }

    /// Adopts a new display scale immediately: the scene transform
    /// stretches whichever atlas is already loaded from this call
    /// onward. Returns a build job when the planned atlas factor needs
    /// rasterizing; `None` when it is unchanged, already cached, or
    /// already building.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] when planning the new atlas factor fails; the
    /// previous atlas and scale stay in use.
    pub fn adopt_scale(&mut self, scale: f32) -> Result<Option<AtlasBuildJob>, GfxError> {
        Ok(self
            .renderer
            .adopt_scale(&self.device, &self.queue, scale)?)
    }

    /// Resolves a finished atlas build: applies it if it still matches
    /// what the renderer wants, or discards it (never overwriting a
    /// fresher atlas) if the want moved on while it built. Returns a
    /// follow-up job in that case; `None` when no further build is
    /// needed.
    #[must_use]
    pub fn apply_atlas(&mut self, built: BuiltAtlas) -> Option<AtlasBuildJob> {
        self.renderer.apply_atlas(&self.device, &self.queue, built)
    }

    /// Reports that a dispatched atlas build failed to run, or could not
    /// be dispatched at all, so the renderer stops treating its factor
    /// as outstanding and can build it again once a later adopt plans a
    /// different factor and returns to it.
    pub fn job_failed(&mut self, factor: u32) {
        self.renderer.job_failed(factor);
    }

    /// Swaps the theme by rebuilding the renderer at `scale`. On
    /// failure the previous renderer (and theme) stay fully in place —
    /// which is what lets the Options dialog reject a broken theme
    /// without visual damage.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] when the new theme's atlas cannot be built.
    pub fn set_theme(
        &mut self,
        theme: Theme,
        scaling: CardScaling,
        scale: f32,
    ) -> Result<(), GfxError> {
        self.renderer = Renderer::new(
            &self.device,
            &self.queue,
            CANVAS_FORMAT,
            theme,
            scaling,
            scale,
        )?;
        Ok(())
    }

    /// Renders `list` at `scale` into a fresh `size` target and reads it
    /// back as tightly packed RGBA8 rows: a one-shot render (the Options
    /// dialog's card-back contact sheet), touching neither the persistent
    /// on-window canvas nor the surface — see
    /// [`sol_render_wgpu::render_to_rgba`], which this wraps.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] when the draw or the readback fails.
    pub fn render_sheet(
        &mut self,
        list: &DisplayList,
        size: (u32, u32),
        scale: f32,
    ) -> Result<Vec<u8>, GfxError> {
        Ok(render_to_rgba(
            &self.device,
            &self.queue,
            &mut self.renderer,
            list,
            size,
            scale,
        )?)
    }

    /// The device's texture size ceiling
    /// ([`Renderer::max_texture_dim`]), for laying out a card-back
    /// contact sheet that must fit one texture.
    #[must_use]
    pub fn max_texture_dim(&self) -> u32 {
        self.renderer.max_texture_dim()
    }

    /// Draws `list` into the persistent canvas, then blits the canvas
    /// onto the surface and presents. A lost/outdated surface is
    /// reconfigured and the frame skipped; the next tick retries.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] from drawing or a failed surface acquisition.
    pub fn render(&mut self, list: &DisplayList) -> Result<(), GfxError> {
        let log = self.log;
        if log {
            eprintln!("sol-win32 log: render begin");
        }
        let canvas_view = self
            .canvas
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer.render(
            &self.device,
            &self.queue,
            &canvas_view,
            (self.config.width, self.config.height),
            list,
        )?;

        if log {
            eprintln!("sol-win32 log: acquire begin");
        }
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // Skip this frame; the next tick tries again.
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(GfxError::SurfaceValidation);
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sol win32 present"),
            });
        {
            let surface_view = surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sol win32 blit pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit.pipeline);
            pass.set_bind_group(0, &self.blit_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        if log {
            eprintln!("sol-win32 log: present begin");
        }
        self.queue.present(surface_texture);
        if log {
            eprintln!("sol-win32 log: present done");
        }
        Ok(())
    }
}

/// The persistent canvas the presenter frames accumulate in.
fn create_canvas(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sol win32 canvas"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CANVAS_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// The canvas → surface blit pipeline (fullscreen triangle).
/// What the UI thread — or a transient atlas-builder thread reporting
/// back — asks of the render thread.
enum RenderCmd {
    /// Draw and present one frame.
    Frame(DisplayList),
    /// The playfield's physical size changed.
    Resize((u32, u32)),
    /// The fitted continuous scale changed.
    SetScale(f32),
    /// Swap the theme; the reply carries the rebuild verdict (the
    /// Options dialog shows failures and keeps the previous theme).
    /// Boxed: a Theme is by far the largest payload on this channel.
    SetTheme(
        Box<Theme>,
        CardScaling,
        f32,
        mpsc::Sender<Result<(), String>>,
    ),
    /// Render one card-back contact sheet immediately and reply with its
    /// pixels: the Options dialog's card-back grid, refreshed on open and
    /// after a theme or scaling change. A bounded request/reply, the same
    /// shape as `SetTheme`, because the dialog waiting on it needs this
    /// exact result rather than whatever the playfield's own next frame
    /// happens to draw.
    RenderSheet(
        DisplayList,
        (u32, u32),
        f32,
        mpsc::Sender<Result<Vec<u8>, String>>,
    ),
    /// A transient atlas-builder thread finished the job for this
    /// factor (captured before `run` consumed it): the built atlas on
    /// success, or the build error on failure. The leading counter is
    /// the theme generation live when the build was dispatched; a
    /// result stamped with any other generation is for a theme swapped
    /// away since and is dropped before it can touch the renderer.
    BuildDone(u64, u32, Result<BuiltAtlas, RenderError>),
    /// Stop the render loop. The render thread holds a clone of its own
    /// sender for its whole life (so atlas builders can report back on
    /// it), which means the channel can no longer disconnect on its
    /// own — this explicit request is now the only way
    /// [`RenderHandle::shutdown`] has to end the loop.
    Shutdown,
}

/// Spawns a transient thread to run `job` off the render thread: a
/// vector theme's resvg rebuild (the only factor crossing a scale
/// change can trigger — a PNG theme's factor is fixed regardless of
/// display scale) can take hundreds of milliseconds, long enough to
/// stall frames if it ran inline on a factor crossing. The result
/// reports back on `sender` — a clone of the render thread's own
/// command sender — as a [`RenderCmd::BuildDone`] stamped with
/// `generation`, the theme generation live when the build was
/// dispatched, so a theme swap that lands before the build finishes can
/// be recognized and the stale result dropped instead of applied.
///
/// A failed SPAWN (not a failed build) is reported through
/// [`Gfx::job_failed`] right here instead: no builder thread will ever
/// exist to report it otherwise, and `job`'s factor must not be left
/// recorded as outstanding forever.
fn spawn_atlas_build(
    job: AtlasBuildJob,
    generation: u64,
    sender: &mpsc::Sender<RenderCmd>,
    gfx: &mut Gfx,
) {
    let factor = job.factor();
    let sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name(String::from("sol-win32-atlas"))
        .spawn(move || {
            let result = job.run();
            drop(sender.send(RenderCmd::BuildDone(generation, factor, result)));
        });
    if let Err(error) = spawned {
        gfx.job_failed(factor);
        eprintln!("sol-win32: starting the atlas build thread: {error}");
    }
}

/// The UI thread's grip on the render thread. Every method returns
/// promptly: frames are dropped rather than queued when the renderer
/// falls behind, and the one call that needs an answer (theme swaps)
/// waits with a bounded timeout.
pub struct RenderHandle {
    sender: mpsc::Sender<RenderCmd>,
    /// Frames handed over so far; compared against `rendered` to bound
    /// the queue instead of ever blocking on it.
    sent: u64,
    /// Frames the render thread has finished (successfully or not).
    rendered: Arc<AtomicU64>,
    /// Set by the render thread after it dropped the [`Gfx`] — the
    /// clean-shutdown handshake [`RenderHandle::shutdown`] waits on.
    stopped: Arc<AtomicBool>,
    /// The device's texture size ceiling, captured once by the render
    /// thread right after it builds [`Gfx`] — `0` until then, resolved
    /// through [`RenderHandle::max_texture_dim`].
    max_texture_dim: Arc<AtomicU32>,
}

impl RenderHandle {
    /// Spawns the render thread and builds the whole [`Gfx`] on it,
    /// reporting startup errors synchronously.
    ///
    /// # Errors
    ///
    /// A [`GfxError`] from [`Gfx::new`], or [`GfxError::RenderThread`]
    /// when the thread cannot start (or dies before reporting).
    pub fn start(
        hwnd: HWND,
        size: (u32, u32),
        theme: Theme,
        scaling: CardScaling,
        scale: f32,
    ) -> Result<Self, GfxError> {
        // An HWND is a plain kernel handle value; only window
        // *procedures* are thread-affine, and creating a surface for a
        // window owned by another thread is ordinary Win32. Ferried as
        // an integer because raw pointers are not `Send`.
        let hwnd_value = hwnd as isize;
        let (sender, receiver) = mpsc::channel::<RenderCmd>();
        let (ready_sender, ready_receiver) = mpsc::channel::<Result<(), GfxError>>();
        let rendered = Arc::new(AtomicU64::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let max_texture_dim = Arc::new(AtomicU32::new(0));
        let thread_rendered = Arc::clone(&rendered);
        let thread_stopped = Arc::clone(&stopped);
        let thread_max_texture_dim = Arc::clone(&max_texture_dim);
        // Cloned before the thread spawns, so the loop can hold a sender
        // of its own for its whole life: atlas builders it dispatches
        // clone theirs from this one to report results back on the same
        // channel the UI thread sends through. Holding this clone here
        // is what lets `RenderCmd::Shutdown` — not channel disconnection
        // — be the only way the loop ends; see the loop's tail comment.
        let self_sender = sender.clone();

        let spawned = std::thread::Builder::new()
            .name(String::from("sol-render"))
            .spawn(move || {
                let mut gfx = match Gfx::new(hwnd_value as HWND, size, theme, scaling, scale) {
                    Ok(gfx) => {
                        // A closed ready-channel means `start` already
                        // returned; nothing useful remains to do then.
                        if ready_sender.send(Ok(())).is_err() {
                            return;
                        }
                        gfx
                    }
                    Err(error) => {
                        drop(ready_sender.send(Err(error)));
                        thread_stopped.store(true, Ordering::Release);
                        return;
                    }
                };
                // Captured once, here, rather than per render: the
                // device this `Gfx` was built against never changes for
                // its whole life, so the ceiling never changes either.
                thread_max_texture_dim.store(gfx.max_texture_dim(), Ordering::Release);
                // Bumped only on a successful `SetTheme`: a build
                // dispatched before that swap carries the generation it
                // was dispatched under, so its result — arriving after —
                // is recognized as stale by comparing against this and
                // dropped instead of painting old-theme artwork into the
                // freshly swapped renderer.
                let mut generation: u64 = 0;
                while let Ok(cmd) = receiver.recv() {
                    match cmd {
                        RenderCmd::Frame(list) => {
                            if let Err(error) = gfx.render(&list) {
                                eprintln!("sol-win32: render failed: {error}");
                            }
                            thread_rendered.fetch_add(1, Ordering::Release);
                        }
                        RenderCmd::Resize(size) => gfx.resize(size),
                        RenderCmd::SetScale(scale) => match gfx.adopt_scale(scale) {
                            Ok(Some(job)) => {
                                spawn_atlas_build(job, generation, &self_sender, &mut gfx);
                            }
                            Ok(None) => {}
                            Err(error) => eprintln!("sol-win32: rescale failed: {error}"),
                        },
                        RenderCmd::SetTheme(theme, scaling, scale, reply) => {
                            let verdict = gfx
                                .set_theme(*theme, scaling, scale)
                                .map_err(|error| error.to_string());
                            // Only a successful swap replaces the
                            // renderer, so only then does advancing the
                            // generation correctly obsolete every build
                            // dispatched against the theme it replaced.
                            if verdict.is_ok() {
                                generation += 1;
                            }
                            drop(reply.send(verdict));
                        }
                        RenderCmd::RenderSheet(list, size, scale, reply) => {
                            let result = gfx
                                .render_sheet(&list, size, scale)
                                .map_err(|error| error.to_string());
                            drop(reply.send(result));
                        }
                        // Stale: this theme has since been swapped away,
                        // so the current renderer never issued this job.
                        // No apply, no `job_failed`, no log —
                        // obsolescence is not an error.
                        RenderCmd::BuildDone(build_generation, ..)
                            if build_generation != generation => {}
                        RenderCmd::BuildDone(_, _, Ok(built)) => {
                            if let Some(job) = gfx.apply_atlas(built) {
                                spawn_atlas_build(job, generation, &self_sender, &mut gfx);
                            }
                        }
                        RenderCmd::BuildDone(_, factor, Err(error)) => {
                            gfx.job_failed(factor);
                            eprintln!("sol-win32: atlas build failed: {error}");
                        }
                        RenderCmd::Shutdown => break,
                    }
                }
                // Channel disconnected (never expected while
                // `self_sender` above stays alive for the loop's whole
                // life — kept as a safe exit instead of an infinite spin
                // should that invariant ever be violated) or an explicit
                // shutdown was seen: drop the surface while the window
                // still exists, then report done.
                drop(gfx);
                thread_stopped.store(true, Ordering::Release);
            });
        if let Err(error) = spawned {
            return Err(GfxError::RenderThread(error));
        }

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                sent: 0,
                rendered,
                stopped,
                max_texture_dim,
            }),
            Ok(Err(error)) => Err(error),
            // The thread died without reporting; with panics denied
            // this is theoretical, but stay an error, not a hang.
            Err(_) => Err(GfxError::RenderThread(std::io::Error::other(
                "the render thread exited before reporting readiness",
            ))),
        }
    }

    /// Hands over one frame — or drops it when the renderer is more
    /// than two frames behind (busy, or wedged in the presentation
    /// engine): the UI thread never waits for rendering.
    pub fn frame(&mut self, list: DisplayList) {
        if self
            .sent
            .saturating_sub(self.rendered.load(Ordering::Acquire))
            > 2
        {
            return;
        }
        if self.sender.send(RenderCmd::Frame(list)).is_ok() {
            self.sent += 1;
        }
    }

    /// Forwards a physical-size change.
    pub fn resize(&self, size: (u32, u32)) {
        drop(self.sender.send(RenderCmd::Resize(size)));
    }

    /// Forwards a continuous-scale change.
    pub fn set_scale(&self, scale: f32) {
        drop(self.sender.send(RenderCmd::SetScale(scale)));
    }

    /// Swaps the theme and waits (bounded) for the verdict.
    ///
    /// # Errors
    ///
    /// The rebuild's error text, or a note that the renderer did not
    /// answer in time (a stalled presentation engine) — the caller
    /// keeps the previous theme active in both cases.
    pub fn set_theme(&self, theme: Theme, scaling: CardScaling, scale: f32) -> Result<(), String> {
        let (reply_sender, reply_receiver) = mpsc::channel();
        if self
            .sender
            .send(RenderCmd::SetTheme(
                Box::new(theme),
                scaling,
                scale,
                reply_sender,
            ))
            .is_err()
        {
            return Err(String::from("the render thread is gone"));
        }
        match reply_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(verdict) => verdict,
            Err(_) => Err(String::from(
                "the renderer did not respond; keeping the current theme",
            )),
        }
    }

    /// Renders one card-back contact sheet and waits (bounded) for its
    /// pixels — modelled on [`Self::set_theme`]'s bounded request/reply
    /// shape: the Options dialog waiting on this needs this exact
    /// result, not whatever the playfield's own next frame happens to
    /// draw.
    ///
    /// # Errors
    ///
    /// The render's error text, or a note that the renderer did not
    /// respond in time and card back previews are unavailable.
    pub fn render_sheet(
        &self,
        list: DisplayList,
        size: (u32, u32),
        scale: f32,
    ) -> Result<Vec<u8>, String> {
        let (reply_sender, reply_receiver) = mpsc::channel();
        if self
            .sender
            .send(RenderCmd::RenderSheet(list, size, scale, reply_sender))
            .is_err()
        {
            return Err(String::from("the render thread is gone"));
        }
        match reply_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(_) => Err(String::from(
                "the renderer did not respond; card back previews are unavailable",
            )),
        }
    }

    /// Frames finished so far (the smoke test's progress probe).
    #[must_use]
    pub fn frames_rendered(&self) -> u64 {
        self.rendered.load(Ordering::Acquire)
    }

    /// The device's texture size ceiling, for laying out a card-back
    /// contact sheet that must fit one texture. Conservatively the
    /// guaranteed floor (see [`previews::resolve_max_texture_dim`]) until
    /// the render thread has captured the device's real limit.
    #[must_use]
    pub fn max_texture_dim(&self) -> u32 {
        previews::resolve_max_texture_dim(self.max_texture_dim.load(Ordering::Acquire))
    }

    /// Requests the render thread stop, then waits (bounded) for it to
    /// drop its surface while the windows still exist. The render
    /// thread holds a clone of its own sender (so atlas builders can
    /// report back on it), so simply dropping this handle's sender can
    /// no longer end the loop the way it used to — this explicit
    /// request is what does it now. A renderer wedged in the
    /// presentation engine never answers; after the timeout it is
    /// simply abandoned — the process exit that follows terminates it,
    /// and the OS reclaims what its `Drop`s would have.
    pub fn shutdown(self) {
        drop(self.sender.send(RenderCmd::Shutdown));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.stopped.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
