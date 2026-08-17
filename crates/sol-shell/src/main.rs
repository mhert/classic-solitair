//! `sol-shell` — minimal winit development shell.
//!
//! Runs the presenter and wgpu renderer together outside of any real
//! frontend, for fast iteration before sol-qt or sol-win32 exist, and as
//! the seed for future wasm/Android shells. Keyboard shortcuts stand in
//! for menus; all game behavior lives behind the presenter's API — this
//! binary contains zero game logic.
//!
//! The board fills the window: on resize the presenter fits a
//! continuous scale to the surface (cards scale with the height, the
//! columns spread across the width like the original's), the renderer
//! stretches the logical frame by that factor, and pointer input maps
//! back through the same fit. Frames are
//! drawn into a persistent canvas texture, then blitted onto the surface
//! by a fullscreen-triangle pass: swapchain images do not survive
//! presentation, but the win cascade's don't-clear smear needs the
//! previous frame — the canvas is where it accumulates.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Instant;

use anyhow::{Context, anyhow};
use sol_engine::{DrawMode, ScoringMode, Seed};
use sol_presenter::{Presenter, Pt, Size};
use sol_render_wgpu::{AtlasBuildJob, BlitPipeline, BuiltAtlas, RenderError, Renderer};

/// Debug-label prefix for this host's blit objects, so a graphics debugger
/// names them for the frontend that created them.
const BLIT_LABEL: &str = "sol";
use sol_session::{Options, Session};
use sol_theme::{CardScaling, Theme};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowAttributes};

const HELP: &str = "\
classic-solitair dev shell

USAGE: sol-shell [--theme <dir-or-zip>] [--seed <0-32767>] [--xbrz]

  --theme <path>   theme package to load (default: the in-tree default theme)
  --seed <0-32767> deal this exact game instead of a random one
  --xbrz           upscale a PNG theme through xBRZ (no effect on vector themes)

KEYS
  F2                 deal a new random game
  G <digits> Enter   select game by seed (Esc cancels)
  Ctrl+Z / Ctrl+Y    undo / redo (rejected in Vegas)
  Ctrl+S / Ctrl+O    save / load the autosave slot
  D  toggle draw mode (next deal)    M  cycle scoring mode (next deal)
  T  toggle timed game (next deal)   O  toggle outline dragging
  B  cycle card back                 Esc  quit

Set SOL_SHELL_LOG_INPUT=1 to print pointer and window-configure
diagnostics (for input bug reports).
";

fn main() -> anyhow::Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print!("{HELP}");
        return Ok(());
    }

    let theme = match &args.theme {
        Some(path) => {
            Theme::load_path(path).with_context(|| format!("loading theme {}", path.display()))?
        }
        None => Theme::load_dir(default_theme_dir()?).context("loading the default theme")?,
    };
    let scaling = if args.xbrz {
        CardScaling::Xbrz
    } else {
        CardScaling::Original
    };

    let seed = args.seed.unwrap_or_else(sol_frontend::random_seed);
    let session = Session::new(Options::default(), seed);
    let presenter = Presenter::new(session, &theme);
    let design_1x = presenter.layout().design_size();

    print!("{HELP}");
    let event_loop = EventLoop::new().context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    // The one result channel every dispatched atlas build reports on; it
    // outlives every build, so no in-flight result is ever dropped unread.
    let (atlas_tx, atlas_rx) = mpsc::channel();
    let mut shell = Shell {
        theme,
        scaling,
        design_1x,
        presenter,
        gfx: None,
        last_frame: None,
        modifiers: ModifiersState::empty(),
        cursor: Pt::new(0, 0),
        seed_entry: None,
        title: String::new(),
        log_input: std::env::var_os("SOL_SHELL_LOG_INPUT").is_some(),
        failure: None,
        atlas_tx,
        atlas_rx,
    };
    event_loop.run_app(&mut shell).context("event loop")?;
    match shell.failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

/// Parsed command-line arguments.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    theme: Option<PathBuf>,
    seed: Option<Seed>,
    xbrz: bool,
    help: bool,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => parsed.help = true,
                "--xbrz" => parsed.xbrz = true,
                "--theme" => {
                    let value = args.next().ok_or_else(|| anyhow!("--theme needs a path"))?;
                    parsed.theme = Some(PathBuf::from(value));
                }
                "--seed" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow!("--seed needs a number"))?;
                    let seed = value
                        .parse::<Seed>()
                        .with_context(|| format!("--seed {value} is not a game number"))?;
                    parsed.seed = Some(seed);
                }
                other => return Err(anyhow!("unknown argument {other} (try --help)")),
            }
        }
        Ok(parsed)
    }
}

/// The in-tree default theme, resolved through the frontends' shared
/// discovery so this harness and the real frontends look in the same places.
fn default_theme_dir() -> anyhow::Result<PathBuf> {
    sol_frontend::themes::dev_default_dir()
        .ok_or_else(|| anyhow!("no theme given (--theme) and themes/default was not found"))
}

/// Window title: seed always visible (and copyable into "--seed"), plus
/// score, elapsed time, and the options the next deal will use.
fn title_line(presenter: &Presenter, seed_entry: Option<&str>) -> String {
    if let Some(digits) = seed_entry {
        return format!("classic-solitair — select game: {digits}_ (Enter deals, Esc cancels)");
    }
    let options = presenter.options();
    let score = match options.scoring {
        ScoringMode::Vegas => format!("${}", presenter.score()),
        ScoringMode::Standard => format!("score {}", presenter.score()),
        ScoringMode::None => "no scoring".to_owned(),
    };
    let elapsed = presenter.elapsed_secs();
    let draw = match options.draw_mode {
        DrawMode::One => "draw one",
        DrawMode::Three => "draw three",
    };
    let scoring = match options.scoring {
        ScoringMode::Standard => "standard",
        ScoringMode::Vegas => "vegas",
        ScoringMode::None => "none",
    };
    let won = if presenter.is_won() { " — WON" } else { "" };
    format!(
        "classic-solitair — seed {} — {} — {}:{:02} — {} / {}{}{}",
        presenter.seed().get(),
        score,
        elapsed / 60,
        elapsed % 60,
        draw,
        scoring,
        if options.timed { "" } else { " / untimed" },
        won,
    )
}

/// The canvas format: plain non-sRGB bytes, matching the renderer's
/// no-color-management contract.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Everything that exists only while the window does.
struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    canvas: wgpu::Texture,
    blit: BlitPipeline,
    blit_bind: wgpu::BindGroup,
}

impl Gfx {
    /// The persistent canvas the presenter frames accumulate in (the
    /// cascade smear lives here); each frame it is sampled onto the
    /// surface by the blit pipeline.
    fn create_canvas(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sol shell canvas"),
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
    /// Spawns a transient thread to run `job` off the event loop, so the
    /// CPU rebuild that a vector theme's factor crossing needs (resvg —
    /// a PNG theme's factor is fixed regardless of display scale, so a
    /// resize never crosses one) cannot stall a resize drag; rendering
    /// keeps using the atlas already loaded until the result comes back.
    /// The thread reports its outcome on a clone of the shell's one
    /// persistent result channel, so concurrent builds — a resize
    /// crossing several factors before any of them finishes — each
    /// deliver their own result; none is dropped unread, which would
    /// strand its factor as outstanding in the renderer and permanently
    /// block that factor's rebuild.
    ///
    /// A failed thread spawn is reported through [`Renderer::job_failed`]
    /// right here (no builder thread will ever exist to report it
    /// otherwise) and logged like any other rescale failure.
    fn dispatch_atlas_build(
        &mut self,
        job: AtlasBuildJob,
        results: &mpsc::Sender<(u32, Result<BuiltAtlas, RenderError>)>,
    ) {
        let factor = job.factor();
        let sender = results.clone();
        let spawned = thread::Builder::new()
            .name(String::from("sol-shell-atlas"))
            .spawn(move || {
                let result = job.run();
                let _ = sender.send((factor, result));
            });
        if let Err(error) = spawned {
            self.renderer.job_failed(factor);
            eprintln!("sol-shell: starting the atlas build thread: {error}");
        }
    }
}

/// The application: presenter state plus the windowed GPU state.
struct Shell {
    theme: Theme,
    scaling: CardScaling,
    design_1x: Size,
    presenter: Presenter,
    gfx: Option<Gfx>,
    last_frame: Option<Instant>,
    modifiers: ModifiersState,
    cursor: Pt,
    /// `Some(digits)` while "Select Game…" seed entry is active.
    seed_entry: Option<String>,
    title: String,
    /// `SOL_SHELL_LOG_INPUT`: print pointer/configure diagnostics.
    log_input: bool,
    /// A fatal setup/render failure, reported after the loop exits.
    failure: Option<anyhow::Error>,
    /// The one result channel every dispatched atlas build reports on,
    /// created with the shell and never replaced while it lives. Each
    /// dispatch hands a builder thread a clone of the sender; every frame
    /// `about_to_wait` drains the receiver, so no in-flight build's result
    /// is dropped unread — an undelivered result strands its factor as
    /// outstanding in the renderer, permanently blocking that factor's
    /// rebuild. The sender is retained here too, so the channel never
    /// disconnects while the shell runs.
    atlas_tx: mpsc::Sender<(u32, Result<BuiltAtlas, RenderError>)>,
    atlas_rx: mpsc::Receiver<(u32, Result<BuiltAtlas, RenderError>)>,
}

impl Shell {
    /// Fails the whole shell: remember the error and stop the loop.
    fn fail(&mut self, event_loop: &ActiveEventLoop, failure: anyhow::Error) {
        eprintln!("sol-shell: {failure:#}");
        self.failure = Some(failure);
        event_loop.exit();
    }

    fn init_gfx(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Gfx> {
        // Open at 2×: the 1× design client is postage-stamp sized on
        // modern displays. The fitted scale is recomputed from the real
        // window size below.
        let initial_scale = 2_u32;
        let size = PhysicalSize::new(
            self.design_1x.w.unsigned_abs() * initial_scale,
            self.design_1x.h.unsigned_abs() * initial_scale,
        );
        let attributes = WindowAttributes::default()
            .with_title("classic-solitair")
            .with_inner_size(size);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("creating the window")?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .context("creating the surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no compatible graphics adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sol shell device"),
            required_limits: Renderer::required_limits(&adapter),
            ..Default::default()
        }))
        .context("requesting the device")?;

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB surface format: the renderer is a byte-exact
        // blitter with no color management.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .context("the surface offers no formats")?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: Vec::new(),
        };
        surface.configure(&device, &config);

        let fit = self
            .presenter
            .fit_viewport(size.width.max(1), size.height.max(1));
        let renderer = Renderer::new(
            &device,
            &queue,
            CANVAS_FORMAT,
            self.theme.clone(),
            self.scaling,
            fit.scale,
        )
        .context("building the renderer")?;

        let blit = BlitPipeline::new(&device, format, BLIT_LABEL);
        let canvas = Gfx::create_canvas(&device, &config);
        let blit_bind = blit.bind(&device, &canvas, BLIT_LABEL);
        Ok(Gfx {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            canvas,
            blit,
            blit_bind,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let (width, height) = (size.width.max(1), size.height.max(1));
        if self.log_input {
            println!("input: configure {width}x{height}");
        }
        // Compositors deliver same-size configures for window activation
        // and focus changes; treat those as the no-ops they are — a real
        // reconfigure mid-drag would drop the drag.
        if self
            .gfx
            .as_ref()
            .is_some_and(|gfx| (gfx.config.width, gfx.config.height) == (width, height))
        {
            return;
        }
        let fit = self.presenter.fit_viewport(width, height);
        if let Some(gfx) = &mut self.gfx {
            gfx.config.width = width;
            gfx.config.height = height;
            gfx.surface.configure(&gfx.device, &gfx.config);
            gfx.canvas = Gfx::create_canvas(&gfx.device, &gfx.config);
            gfx.blit_bind = gfx.blit.bind(&gfx.device, &gfx.canvas, BLIT_LABEL);
            match gfx.renderer.adopt_scale(&gfx.device, &gfx.queue, fit.scale) {
                Ok(Some(job)) => gfx.dispatch_atlas_build(job, &self.atlas_tx),
                Ok(None) => {}
                Err(error) => eprintln!("sol-shell: rescale failed: {error}"),
            }
        }
    }

    /// Delivers every atlas build that has landed since the last frame to
    /// the renderer, draining the persistent result channel so nothing
    /// queued is left behind. Runs on the event loop, where the renderer
    /// lives, so builds computed on transient threads only ever *report*
    /// here — the renderer is never touched off-thread. A success goes to
    /// [`Renderer::apply_atlas`], whose follow-up job (the scale moved on
    /// mid-build) is dispatched the same way; a failure reports
    /// [`Renderer::job_failed`] and logs. Both paths resolve the job's
    /// factor in the renderer, so no crossing strands a factor as
    /// outstanding.
    fn drain_atlas_builds(&mut self) {
        let Some(gfx) = &mut self.gfx else {
            return;
        };
        while let Ok((factor, result)) = self.atlas_rx.try_recv() {
            match result {
                Ok(built) => {
                    if let Some(job) = gfx.renderer.apply_atlas(&gfx.device, &gfx.queue, built) {
                        gfx.dispatch_atlas_build(job, &self.atlas_tx);
                    }
                }
                Err(error) => {
                    gfx.renderer.job_failed(factor);
                    eprintln!("sol-shell: atlas build failed: {error}");
                }
            }
        }
    }

    fn redraw(&mut self) -> anyhow::Result<()> {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(0, |last| now.duration_since(last).as_millis());
        self.last_frame = Some(now);
        // A stall (window drag, suspend) is not hours of card time.
        self.presenter
            .advance(u32::try_from(dt).unwrap_or(u32::MAX).min(250));

        let frame = self.presenter.frame();
        let Some(gfx) = &mut self.gfx else {
            return Ok(());
        };

        let canvas_view = gfx
            .canvas
            .create_view(&wgpu::TextureViewDescriptor::default());
        gfx.renderer
            .render(
                &gfx.device,
                &gfx.queue,
                &canvas_view,
                (gfx.config.width, gfx.config.height),
                &frame,
            )
            .context("rendering the frame")?;

        let surface_texture = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // Skip this frame; the next redraw tries again.
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow!("surface texture acquisition failed validation"));
            }
        };
        let mut encoder = gfx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sol shell present"),
            });
        {
            let surface_view = surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sol shell blit pass"),
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
            pass.set_pipeline(&gfx.blit.pipeline);
            pass.set_bind_group(0, &gfx.blit_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        gfx.queue.submit([encoder.finish()]);
        // No `pre_present_notify` latency hint: it arms winit's Wayland
        // frame-callback gate, and on this wgpu/Mesa stack the callback
        // for the presented frame never arrives, parking the event loop
        // after one frame. Without the hint, redraw delivery is ungated
        // and the Fifo present paces the loop instead.
        gfx.queue.present(surface_texture);

        let title = title_line(&self.presenter, self.seed_entry.as_deref());
        if title != self.title {
            gfx.window.set_title(&title);
            self.title = title;
        }
        Ok(())
    }

    fn key_pressed(&mut self, event_loop: &ActiveEventLoop, code: KeyCode) {
        // Any key lands running animations first, like the original.
        self.presenter.key_down();

        if self.seed_entry.is_some() {
            self.seed_entry_key(code);
            return;
        }

        let ctrl = self.modifiers.control_key();
        match code {
            KeyCode::Escape => event_loop.exit(),
            KeyCode::F2 => self.presenter.deal_new(sol_frontend::random_seed()),
            KeyCode::KeyG if !ctrl => self.seed_entry = Some(String::new()),
            KeyCode::KeyZ if ctrl => report(self.presenter.undo(), "undo"),
            KeyCode::KeyY if ctrl => report(self.presenter.redo(), "redo"),
            KeyCode::KeyS if ctrl => self.save(),
            KeyCode::KeyO if ctrl => self.load(),
            KeyCode::KeyD => self.update_options(|options| {
                options.draw_mode = match options.draw_mode {
                    DrawMode::One => DrawMode::Three,
                    DrawMode::Three => DrawMode::One,
                };
            }),
            KeyCode::KeyM => self.update_options(|options| {
                options.scoring = match options.scoring {
                    ScoringMode::Standard => ScoringMode::Vegas,
                    ScoringMode::Vegas => ScoringMode::None,
                    ScoringMode::None => ScoringMode::Standard,
                };
            }),
            KeyCode::KeyT => self.update_options(|options| options.timed = !options.timed),
            KeyCode::KeyO => {
                self.update_options(|options| options.outline_dragging = !options.outline_dragging);
            }
            KeyCode::KeyB => {
                let count = self.presenter.back_count().max(1);
                let next = (self.presenter.back_index() + 1) % count;
                report(self.presenter.set_back(next), "cycling the card back");
            }
            _ => {}
        }
    }

    fn seed_entry_key(&mut self, code: KeyCode) {
        let Some(digits) = &mut self.seed_entry else {
            return;
        };
        match code {
            KeyCode::Escape => self.seed_entry = None,
            KeyCode::Backspace => {
                digits.pop();
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                // Out-of-range entries name no game, so the deal stands.
                let seed = digits.parse::<Seed>().ok();
                self.seed_entry = None;
                if let Some(seed) = seed {
                    self.presenter.deal_new(seed);
                }
            }
            other => {
                if let Some(digit) = digit_of(other)
                    && digits.len() < 10
                {
                    digits.push(digit);
                }
            }
        }
    }

    fn update_options(&mut self, change: impl FnOnce(&mut Options)) {
        let mut options = self.presenter.options().clone();
        change(&mut options);
        self.presenter.set_options(options);
    }

    fn save(&mut self) {
        match sol_session::storage::autosave(self.presenter.session()) {
            Ok(path) => println!("saved to {}", path.display()),
            Err(error) => eprintln!("save failed: {error}"),
        }
    }

    fn load(&mut self) {
        let path = match sol_session::paths::autosave_path() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("load failed: {error}");
                return;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("load failed: {} ({error})", path.display());
                return;
            }
        };
        match self.presenter.load_bytes(&bytes) {
            Ok(()) => println!("loaded {}", path.display()),
            Err(error) => eprintln!("load failed: {error}"),
        }
    }
}

/// Prints a rejected menu action; rejections (Vegas undo, nothing to
/// redo, no such back) are ordinary and never fatal.
fn report<E: std::fmt::Display>(result: Result<(), E>, what: &str) {
    if let Err(error) = result {
        println!("{what}: {error}");
    }
}

const fn digit_of(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::Digit0 | KeyCode::Numpad0 => '0',
        KeyCode::Digit1 | KeyCode::Numpad1 => '1',
        KeyCode::Digit2 | KeyCode::Numpad2 => '2',
        KeyCode::Digit3 | KeyCode::Numpad3 => '3',
        KeyCode::Digit4 | KeyCode::Numpad4 => '4',
        KeyCode::Digit5 | KeyCode::Numpad5 => '5',
        KeyCode::Digit6 | KeyCode::Numpad6 => '6',
        KeyCode::Digit7 | KeyCode::Numpad7 => '7',
        KeyCode::Digit8 | KeyCode::Numpad8 => '8',
        KeyCode::Digit9 | KeyCode::Numpad9 => '9',
        _ => return None,
    })
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match self.init_gfx(event_loop) {
            Ok(gfx) => self.gfx = Some(gfx),
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                // Window physical pixels map to logical presenter
                // pixels through the current fit.
                // The cast is exact for any real window: a cursor position
                // is a coordinate inside a surface, and every display size
                // a compositor reports stays far inside i32.
                #[allow(clippy::cast_possible_truncation)]
                let pt = self
                    .presenter
                    .fit()
                    .to_logical(position.x.floor() as i32, position.y.floor() as i32);
                self.cursor = pt;
                self.presenter.pointer_move(pt);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    if self.log_input {
                        println!("input: press at {:?}", self.cursor);
                    }
                    self.presenter.pointer_down(self.cursor);
                }
                ElementState::Released => {
                    let moves_before = self.presenter.session().game().log().len();
                    self.presenter.pointer_up(self.cursor);
                    if self.log_input {
                        let outcome = if self.presenter.session().game().log().len() > moves_before
                        {
                            "move applied"
                        } else if self.presenter.is_animating() {
                            "snapped back"
                        } else {
                            "no-op"
                        };
                        println!("input: release at {:?} -> {outcome}", self.cursor);
                    }
                }
            },
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => self.key_pressed(event_loop, code),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.redraw() {
                    self.fail(event_loop, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Animations and the timer need continuous frames; Fifo present
        // paces this to the display.
        self.drain_atlas_builds();
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn args(list: &[&str]) -> anyhow::Result<Args> {
        Args::parse(list.iter().map(ToString::to_string))
    }

    #[test]
    fn args_parse_theme_seed_and_help() {
        assert_eq!(args(&[]).unwrap(), Args::default());
        let parsed = args(&["--theme", "/tmp/t", "--seed", "42"]).unwrap();
        assert_eq!(parsed.theme, Some(PathBuf::from("/tmp/t")));
        assert_eq!(parsed.seed, Some(Seed::new(42).unwrap()));
        assert!(args(&["--help"]).unwrap().help);
        assert!(args(&["-h"]).unwrap().help);
        assert!(args(&["--xbrz"]).unwrap().xbrz);
    }

    #[test]
    fn args_reject_garbage() {
        assert!(args(&["--seed"]).is_err(), "missing value");
        assert!(args(&["--seed", "x"]).is_err(), "not a number");
        assert!(args(&["--seed", "32768"]).is_err(), "beyond the last game");
        assert!(args(&["--theme"]).is_err(), "missing value");
        assert!(args(&["--frobnicate"]).is_err(), "unknown flag");
    }

    #[test]
    fn digits_map_from_both_keyboard_rows() {
        assert_eq!(digit_of(KeyCode::Digit0), Some('0'));
        assert_eq!(digit_of(KeyCode::Numpad9), Some('9'));
        assert_eq!(digit_of(KeyCode::KeyA), None);
    }
}
