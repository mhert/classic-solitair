//! The Win32 chrome: main window, real menu bar, status bar, the
//! playfield child canvas, the native dialogs, and all event wiring.
//! Every action calls into the platform-free [`App`] core — no game
//! logic lives here.
//!
//! Concurrency model: the chrome and all game state run on the one
//! GUI thread; rendering runs on a dedicated thread behind
//! [`RenderHandle`] and the GUI thread never waits for it (frames are
//! dropped when the renderer is behind — see `gfx` for why a message
//! pump must never block on presentation). Shared state sits in
//! `RefCell`s inside one `Rc<Ui>` the handler closures clone. Two
//! rules keep the borrows sound:
//!
//! 1. Handlers match the event *before* borrowing anything (the
//!    subclass procs run for every message of every control).
//! 2. No borrow is held across a call that pumps messages (message
//!    boxes). The tick additionally holds a re-entrancy guard, because
//!    the animation timer's tick is delivered as a *sent* message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use native_windows_gui as nwg;
use winapi::shared::windef::RECT;
use winapi::um::commctrl::SB_SETPARTS;
use winapi::um::winuser::{
    GetClientRect, GetKeyState, GetWindowRect, IsIconic, IsZoomed, SW_MAXIMIZE, SWP_NOACTIVATE,
    SWP_NOZORDER, SendMessageW, SetWindowPos, ShowWindow, VK_CONTROL, VK_F2, WM_EXITSIZEMOVE,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

use crate::Cli;
use crate::dialogs::{
    BackPreviews, OptionsDialog, SelectGameDialog, index_to_scaling, scaling_to_index,
};
use crate::gfx::RenderHandle;
use crate::placement::{
    DEFAULT_WINDOW_SIZE, GEOMETRY_SETTLE_POLL_MS, apply_outer_placement, outer_window_rect,
    placement_settled, startup_placement,
};
use sol_frontend::app::{App, StatePaths};
use sol_frontend::previews;
use sol_frontend::status::{score_text, seed_digits, seed_label, time_text};
use sol_presenter::{BackSheet, Rgba};
use sol_session::Settings;
use sol_theme::CardScaling;

/// How long a transient status message stays up.
const STATUS_MESSAGE_SECS: u64 = 4;

/// Status-bar part widths, DPI-logical pixels (message part fills).
const PART_SEED_W: i32 = 150;
const PART_SCORE_W: i32 = 120;
const PART_TIME_W: i32 = 100;

/// Raw-handler ids; must be `> 0xFFFF` (nwg reserves the low range).
const RAW_ID_CANVAS: usize = 0x1_0001;
const RAW_ID_WINDOW_KEYS: usize = 0x1_0002;
const RAW_ID_STATUS_CLICK: usize = 0x1_0003;
const RAW_ID_WINDOW_SIZEMOVE: usize = 0x1_0004;

/// Resolves the settings the core should boot from and the paths it should
/// read and write back through. `--smoke` skips real state entirely: unlike
/// the autosave and theme paths (redirectable through a `tempfile` directory
/// in tests), the Windows settings location resolves through the platform's
/// known-folder API, which no environment variable can override — so the
/// self-test stays hermetic by using in-memory defaults and no paths at all.
fn startup_state(smoke: bool) -> (Settings, StatePaths) {
    if smoke {
        return (Settings::default(), StatePaths::default());
    }
    let (paths, notices) = StatePaths::resolve();
    let (settings, notice) = sol_frontend::app::load_settings(&paths);
    for notice in notices.iter().chain(notice.iter()) {
        report(Some(notice.clone()));
    }
    (settings, paths)
}

/// Logs a notice the platform-free core handed back. The core never prints —
/// so that its behaviour stays observable to a test — and each frontend
/// labels what it logs with its own name.
fn report(notice: Option<String>) {
    if let Some(notice) = notice {
        eprintln!("sol-win32: {notice}");
    }
}

/// Runs the frontend: builds the chrome, wires events, and either
/// enters the message loop or (in `--smoke` mode) exercises the whole
/// build headlessly and returns.
///
/// # Errors
///
/// Startup failures: theme loading, window construction, or the wgpu
/// path refusing the window.
pub fn run(cli: &Cli) -> anyhow::Result<()> {
    let log = std::env::var_os("SOL_WIN32_LOG").is_some();
    // SAFETY: process-wide DPI opt-in, called before any window
    // exists; makes window pixels real pixels so the continuous fit
    // works against the true surface. nwg deprecates this in favor of
    // a manifest, but a manifest needs an embedded resource compile
    // step this build intentionally avoids.
    #[allow(unsafe_code, deprecated)]
    unsafe {
        nwg::set_dpi_awareness();
    }
    nwg::init().map_err(|error| anyhow!("initializing native-windows-gui: {error}"))?;
    // Without this every control renders in the pre-Win95 bitmap font.
    nwg::Font::set_global_family("Segoe UI")
        .map_err(|error| anyhow!("setting the default font: {error}"))?;

    let (settings, paths) = startup_state(cli.smoke);
    let started = App::start(cli.theme.clone(), cli.seed, settings, paths)
        .context("starting the application")?;
    report(started.notice);
    let ui = Rc::new(build_ui(started.app).context("building the window")?);
    let handlers = bind_handlers(&ui);

    // First layout, then the render thread onto the laid-out canvas.
    ui.layout();
    let render = {
        let app = ui.app.borrow();
        let hwnd = ui
            .canvas
            .handle
            .hwnd()
            .ok_or_else(|| anyhow!("the playfield canvas has no window handle"))?;
        RenderHandle::start(
            hwnd,
            ui.canvas.physical_size(),
            app.theme().clone(),
            app.scaling(),
            app.scale(),
        )
        .context("starting the wgpu render path")?
    };
    *ui.render.borrow_mut() = Some(render);

    let result = if cli.smoke {
        smoke(&ui)
    } else {
        ui.show_at_startup();
        ui.window.set_focus();
        ui.timer.start();
        nwg::dispatch_thread_events();
        if log {
            eprintln!("sol-win32 log: message loop exited");
        }
        report(ui.app.borrow().autosave_on_exit());
        report(ui.app.borrow().persist_settings());
        if log {
            eprintln!("sol-win32 log: autosave done");
        }
        Ok(())
    };

    // Unbind explicitly so the handler closures (each holding an
    // `Rc<Ui>`) are dropped and `ui` below is the last owner; the
    // render thread is then shut down while the control windows still
    // exist (its surface targets the canvas HWND).
    for handler in handlers.events {
        nwg::unbind_event_handler(&handler);
    }
    for handler in handlers.raw {
        drop(nwg::unbind_raw_event_handler(&handler));
    }
    if log {
        eprintln!("sol-win32 log: handlers unbound");
    }
    if let Some(render) = ui.render.borrow_mut().take() {
        render.shutdown();
    }
    if log {
        eprintln!("sol-win32 log: render thread down");
    }
    result
}

/// Blocks (bounded) until the render thread has finished at least one
/// frame beyond `baseline` — proof that a forced render actually reached
/// the render thread, not merely that bookkeeping changed. An error when
/// no such frame lands within 10 seconds.
fn wait_for_a_rendered_frame_past(ui: &Rc<Ui>, baseline: u64) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rendered = ui
            .render
            .borrow()
            .as_ref()
            .map_or(0, RenderHandle::frames_rendered);
        if rendered > baseline {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "smoke: expected a rendered frame past {baseline}, still at {rendered} after 10s"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The `--smoke` self-test: everything is already built; render a few
/// frames through the real on-window path, exercise the dialog
/// populate/read-back, theme-switch, and card-scaling live-preview/Cancel
/// paths, and report. The window stays hidden and the message loop is
/// never entered.
fn smoke(ui: &Rc<Ui>) -> anyhow::Result<()> {
    for _ in 0..3 {
        // `frame`, not `take_frame_if_changed`: the point here is to push
        // three frames through the real surface path, and an idle board
        // produces three identical display lists — exactly what the change
        // gate suppresses. Skipping them is right in the message loop and
        // wrong in a test of the path itself.
        let list = {
            let mut app = ui.app.borrow_mut();
            app.advance();
            app.frame()
        };
        let list = list.ok_or_else(|| anyhow!("smoke: no frame despite a laid-out canvas"))?;
        if let Some(render) = ui.render.borrow_mut().as_mut() {
            render.frame(list);
        }
        ui.update_status();
    }
    // The render thread draws asynchronously; wait (bounded) until all
    // three frames actually went through the real surface path.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rendered = ui
            .render
            .borrow()
            .as_ref()
            .map_or(0, RenderHandle::frames_rendered);
        if rendered >= 3 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "smoke: renderer finished only {rendered} of 3 frames in 10s"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Card-scaling live preview + Cancel through the real path. A scaling
    // change alone leaves the display list byte-identical to the last one
    // submitted (positions never move), so only a forced render — not the
    // per-tick change-gated path — can put a rebuilt atlas on screen; the
    // same is true of Cancel's restore. Checking `scaling()` alone would
    // pass even against a Cancel that reverted bookkeeping without
    // driving a render, so each step below also waits for a frame the
    // render thread actually finished.
    ui.app.borrow_mut().begin_preview();
    let frames_before_scaling = ui
        .render
        .borrow()
        .as_ref()
        .map_or(0, RenderHandle::frames_rendered);
    if let Some(error) = ui.switch_scaling(CardScaling::Xbrz) {
        return Err(anyhow!("smoke: live scaling preview failed: {error}"));
    }
    if ui.app.borrow().scaling() != CardScaling::Xbrz {
        return Err(anyhow!(
            "smoke: live scaling preview did not update app state"
        ));
    }
    wait_for_a_rendered_frame_past(ui, frames_before_scaling)?;

    let frames_before_cancel = ui
        .render
        .borrow()
        .as_ref()
        .map_or(0, RenderHandle::frames_rendered);
    ui.cancel_options();
    if ui.app.borrow().scaling() != CardScaling::Original {
        return Err(anyhow!(
            "smoke: cancel did not restore the scaling bookkeeping"
        ));
    }
    wait_for_a_rendered_frame_past(ui, frames_before_cancel)?;

    // Options dialog: populate from the live options — including a real
    // card-back grid build through the render thread — read back, commit.
    ui.populate_options();
    let edited = ui.options.read();
    report(ui.app.borrow_mut().commit_options(edited));
    // Theme switch through the real path. Ordinarily (no `--theme`
    // override) this re-selects the already-active "default" — a no-op
    // switch that still exercises the render dispatch. Given a second,
    // distinct theme via `--theme` (see `tests/smoke.rs`'s
    // coincident-display-list fixture), this instead performs a genuine
    // cross-theme switch, and when the two themes' manifests happen to
    // produce the same display list — guaranteed here, since the fixture
    // is a byte-for-byte copy of "default" under another name — it is
    // exactly the scenario `push_frame` exists for: `adopt_theme` returns
    // `true` but nothing about the board's geometry changed, so only the
    // forced push, not the per-tick change-gated path, can put the
    // freshly rebuilt atlas on screen.
    let theme_before_switch = ui.app.borrow().theme_id().to_owned();
    let frames_before_theme_switch = ui
        .render
        .borrow()
        .as_ref()
        .map_or(0, RenderHandle::frames_rendered);
    if let Some(error) = ui.switch_theme("default") {
        return Err(anyhow!("smoke: theme switch failed: {error}"));
    }
    if ui.app.borrow().theme_id() != "default" {
        return Err(anyhow!("smoke: theme switch did not adopt \"default\""));
    }
    wait_for_a_rendered_frame_past(ui, frames_before_theme_switch)?;
    if theme_before_switch != "default" {
        println!(
            "smoke: cross-theme switch from {theme_before_switch:?} to \"default\" \
             verified (frame re-rendered)"
        );
    }
    // Select Game dialog: populate and read the seed field back.
    ui.select.populate(&ui.app);
    let digits = ui.select.input.text();
    if !ui.app.borrow_mut().select_game(&digits) {
        return Err(anyhow!("smoke: seed field round-trip failed ({digits:?})"));
    }
    println!(
        "smoke: chrome built, {0} frames rendered, dialogs exercised, \
         scaling preview and cancel verified",
        3
    );
    Ok(())
}

/// The bound handler tokens; kept alive until the loop ends.
struct Handlers {
    events: Vec<nwg::EventHandler>,
    raw: Vec<nwg::RawEventHandler>,
}

/// Everything the chrome owns. Rendering lives on its own thread (see
/// `gfx`); the handle here never blocks the GUI thread. On shutdown
/// `run` disconnects the handle and waits (bounded) for the thread to
/// drop its surface before the control windows are destroyed.
pub struct Ui {
    app: RefCell<App>,
    render: RefCell<Option<RenderHandle>>,

    window: nwg::Window,
    canvas: nwg::ExternCanvas,
    status: nwg::StatusBar,
    timer: nwg::AnimationTimer,
    /// Polls [`Ui::placement_changed_at`] while a keyboard-driven
    /// placement change (Win+arrow snap, maximize/restore via keyboard)
    /// is pending. It is not itself a debounce — restarting an
    /// `nwg::AnimationTimer` does not push its next tick back — so the
    /// debouncing lives in the deadline it polls; see
    /// [`GEOMETRY_SETTLE_POLL_MS`] and [`Ui::settle_tick`].
    settle_timer: nwg::AnimationTimer,

    menu_game: nwg::Menu,
    item_deal: nwg::MenuItem,
    item_select: nwg::MenuItem,
    _sep_game_1: nwg::MenuSeparator,
    item_undo: nwg::MenuItem,
    item_redo: nwg::MenuItem,
    _sep_game_2: nwg::MenuSeparator,
    item_save: nwg::MenuItem,
    item_load: nwg::MenuItem,
    _sep_game_3: nwg::MenuSeparator,
    item_options: nwg::MenuItem,
    _sep_game_4: nwg::MenuSeparator,
    item_exit: nwg::MenuItem,
    _menu_help: nwg::Menu,
    item_about: nwg::MenuItem,

    options: OptionsDialog,
    select: SelectGameDialog,

    /// Transient status text and when it expires.
    status_message: RefCell<(String, Option<Instant>)>,
    /// Last text pushed per status part, to skip redundant `SB_SETTEXT`s.
    status_cache: RefCell<[String; 4]>,
    /// The seed part's physical x-range, for click-to-copy hit tests.
    seed_part_range: Cell<(i32, i32)>,
    /// When the last placement change arrived, while a settle capture is
    /// pending; `None` when none is. This is the debounce: every change
    /// pushes it forward, and the capture only runs once it is
    /// [`GEOMETRY_SETTLE_MS`] old, so a burst writes settings once.
    placement_changed_at: Cell<Option<Instant>>,
    /// Re-entrancy guard: DXGI's `Present` may pump sent messages, and
    /// the timer tick arrives as one.
    ticking: Cell<bool>,
    /// Set when the window close begins: ticks already in flight must
    /// not render anymore (see [`Ui::tick`]).
    closing: Cell<bool>,
}

/// Maps an nwg builder error into a labelled startup error.
pub(crate) fn build_error(what: &str, error: &nwg::NwgError) -> anyhow::Error {
    anyhow!("building the {what}: {error}")
}

#[allow(clippy::too_many_lines)] // one linear builder sequence, split brings nothing
fn build_ui(app: App) -> Result<Ui, anyhow::Error> {
    let placement = startup_placement(app.window_geometry());

    let mut window = nwg::Window::default();
    // Hidden until the first frame is ready (the canvas class paints
    // nothing, so an early show would flash an unerased client area).
    // Freely resizable either way — the felt absorbs slack, like the
    // original.
    let window_builder = nwg::Window::builder()
        .flags(nwg::WindowFlags::MAIN_WINDOW | nwg::WindowFlags::RESIZABLE)
        .title("classic-solitair")
        // The builder reads this as a client size, but a restored
        // placement's exact outer rect is forced below, so the only
        // thing this steers there is the centering fallback — whose
        // math uses the value verbatim, and therefore centers the
        // final outer rect exactly.
        .size(placement.as_ref().map_or(DEFAULT_WINDOW_SIZE, |p| p.size));
    let window_builder = match placement.as_ref().and_then(|p| p.position) {
        Some(position) => window_builder.position(position),
        None => window_builder.center(true),
    };
    window_builder
        .build(&mut window)
        .map_err(|error| build_error("main window", &error))?;

    let mut canvas = nwg::ExternCanvas::default();
    nwg::ExternCanvas::builder()
        .flags(nwg::ExternCanvasFlags::VISIBLE)
        .position((0, 0))
        // A placeholder: `layout()` sizes the canvas to the client area
        // minus the status bar on the first resize, which arrives before
        // anything is shown. The builder value only has to be
        // non-degenerate, so it reuses the default window's own width.
        .size((DEFAULT_WINDOW_SIZE.0, 768))
        .parent(Some(&window))
        .build(&mut canvas)
        .map_err(|error| build_error("playfield canvas", &error))?;

    let mut status = nwg::StatusBar::default();
    nwg::StatusBar::builder()
        .text("")
        .parent(&window)
        .build(&mut status)
        .map_err(|error| build_error("status bar", &error))?;

    let mut timer = nwg::AnimationTimer::default();
    nwg::AnimationTimer::builder()
        .parent(&window)
        .interval(Duration::from_millis(16))
        .active(false)
        .build(&mut timer)
        .map_err(|error| build_error("frame timer", &error))?;

    let mut settle_timer = nwg::AnimationTimer::default();
    nwg::AnimationTimer::builder()
        .parent(&window)
        .interval(Duration::from_millis(GEOMETRY_SETTLE_POLL_MS))
        .active(false)
        .build(&mut settle_timer)
        .map_err(|error| build_error("geometry settle timer", &error))?;

    // Menu bar: Game · Help, exactly the original's structure. The \t
    // labels are display-only; the raw key handlers implement them.
    let mut menu_game = nwg::Menu::default();
    nwg::Menu::builder()
        .text("&Game")
        .parent(&window)
        .build(&mut menu_game)
        .map_err(|error| build_error("Game menu", &error))?;
    let mut item_deal = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Deal\tF2")
        .parent(&menu_game)
        .build(&mut item_deal)
        .map_err(|error| build_error("Deal item", &error))?;
    let mut item_select = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Select Game…")
        .parent(&menu_game)
        .build(&mut item_select)
        .map_err(|error| build_error("Select Game item", &error))?;
    let mut sep_game_1 = nwg::MenuSeparator::default();
    nwg::MenuSeparator::builder()
        .parent(&menu_game)
        .build(&mut sep_game_1)
        .map_err(|error| build_error("menu separator", &error))?;
    let mut item_undo = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Undo\tCtrl+Z")
        .parent(&menu_game)
        .build(&mut item_undo)
        .map_err(|error| build_error("Undo item", &error))?;
    let mut item_redo = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Redo\tCtrl+Y")
        .parent(&menu_game)
        .build(&mut item_redo)
        .map_err(|error| build_error("Redo item", &error))?;
    let mut sep_game_2 = nwg::MenuSeparator::default();
    nwg::MenuSeparator::builder()
        .parent(&menu_game)
        .build(&mut sep_game_2)
        .map_err(|error| build_error("menu separator", &error))?;
    let mut item_save = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("Sa&ve")
        .parent(&menu_game)
        .build(&mut item_save)
        .map_err(|error| build_error("Save item", &error))?;
    let mut item_load = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Load")
        .parent(&menu_game)
        .build(&mut item_load)
        .map_err(|error| build_error("Load item", &error))?;
    let mut sep_game_3 = nwg::MenuSeparator::default();
    nwg::MenuSeparator::builder()
        .parent(&menu_game)
        .build(&mut sep_game_3)
        .map_err(|error| build_error("menu separator", &error))?;
    let mut item_options = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&Options…")
        .parent(&menu_game)
        .build(&mut item_options)
        .map_err(|error| build_error("Options item", &error))?;
    let mut sep_game_4 = nwg::MenuSeparator::default();
    nwg::MenuSeparator::builder()
        .parent(&menu_game)
        .build(&mut sep_game_4)
        .map_err(|error| build_error("menu separator", &error))?;
    let mut item_exit = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("E&xit")
        .parent(&menu_game)
        .build(&mut item_exit)
        .map_err(|error| build_error("Exit item", &error))?;

    let mut menu_help = nwg::Menu::default();
    nwg::Menu::builder()
        .text("&Help")
        .parent(&window)
        .build(&mut menu_help)
        .map_err(|error| build_error("Help menu", &error))?;
    let mut item_about = nwg::MenuItem::default();
    nwg::MenuItem::builder()
        .text("&About")
        .parent(&menu_help)
        .build(&mut item_about)
        .map_err(|error| build_error("About item", &error))?;

    // The whole chrome exists now, menu bar included, so this is the
    // window a later capture will measure: force the restored outer
    // rect onto it in one call. Doing it here rather than through the
    // builder is what keeps apply and capture exact inverses (see
    // [`StartupPlacement`]).
    if let Some(placement) = &placement
        && let Some(hwnd) = window.handle.hwnd()
    {
        apply_outer_placement(hwnd, placement);
    }

    let options = OptionsDialog::build(&window)?;
    let select = SelectGameDialog::build(&window)?;

    Ok(Ui {
        app: RefCell::new(app),
        render: RefCell::new(None),
        window,
        canvas,
        status,
        timer,
        settle_timer,
        menu_game,
        item_deal,
        item_select,
        _sep_game_1: sep_game_1,
        item_undo,
        item_redo,
        _sep_game_2: sep_game_2,
        item_save,
        item_load,
        _sep_game_3: sep_game_3,
        item_options,
        _sep_game_4: sep_game_4,
        item_exit,
        _menu_help: menu_help,
        item_about,
        options,
        select,
        status_message: RefCell::new((String::new(), None)),
        status_cache: RefCell::new(std::array::from_fn(|_| String::from("\u{1}"))),
        seed_part_range: Cell::new((0, 0)),
        placement_changed_at: Cell::new(None),
        ticking: Cell::new(false),
        closing: Cell::new(false),
    })
}

impl Ui {
    /// Positions the canvas over the client area between the menu bar
    /// (non-client, above) and the status bar, then refits scale,
    /// viewport, surface, and the status-bar part edges. All math in
    /// physical pixels — the surface's currency.
    fn layout(&self) {
        let (Some(window), Some(canvas), Some(status)) = (
            self.window.handle.hwnd(),
            self.canvas.handle.hwnd(),
            self.status.handle.hwnd(),
        ) else {
            return;
        };
        let mut client = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut status_rect = client;
        // SAFETY: all three HWNDs come live from our own controls on
        // this thread; the out-pointers are to stack RECTs.
        #[allow(unsafe_code)]
        let (client_size, status_height) = unsafe {
            GetClientRect(window, &raw mut client);
            GetWindowRect(status, &raw mut status_rect);
            (
                (client.right, client.bottom),
                status_rect.bottom - status_rect.top,
            )
        };
        let width = client_size.0.max(1);
        let height = (client_size.1 - status_height).max(1);
        // SAFETY: same live canvas HWND; SetWindowPos with physical
        // pixels sidesteps nwg's DPI-logical rounding so the canvas
        // and the surface agree exactly.
        #[allow(unsafe_code)]
        unsafe {
            SetWindowPos(
                canvas,
                std::ptr::null_mut(),
                0,
                0,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.set_status_parts(client_size.0);

        let (width, height) = (width.unsigned_abs(), height.unsigned_abs());
        let resized = self.app.borrow_mut().resize(width, height);
        if resized && let Some(render) = self.render.borrow().as_ref() {
            render.resize((width, height));
            render.set_scale(self.app.borrow().scale());
        }
    }

    /// Shows the window for the first time. A persisted `maximized` flag
    /// shows and maximizes it in one call (`ShowWindow` with
    /// `SW_MAXIMIZE`) so the window — deliberately kept hidden until now
    /// — never flashes a restored frame first; otherwise exactly today's
    /// plain show.
    fn show_at_startup(&self) {
        let maximized = self
            .app
            .borrow()
            .window_geometry()
            .is_some_and(|geometry| geometry.maximized);
        let hwnd = maximized.then(|| self.window.handle.hwnd()).flatten();
        if let Some(hwnd) = hwnd {
            // SAFETY: live top-level window handle from our own control;
            // SW_MAXIMIZE both shows and maximizes in one call.
            #[allow(unsafe_code)]
            unsafe {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
        } else {
            self.window.set_visible(true);
        }
    }

    /// Reads the live window placement into the recorded geometry, ready
    /// for the next persist, and reports whether it recorded anything.
    /// Skipped entirely while minimized: the window rect reports the
    /// iconic geometry then, not the restored one worth keeping. While
    /// maximized, `maximized` is reported `true` and the (maximized, not
    /// very meaningful) rect rides along regardless — the `App` merge
    /// keeps whatever restored size was already stored, or synthesizes
    /// one from this report only if nothing was stored yet.
    fn record_geometry(&self) -> bool {
        let Some(hwnd) = self.window.handle.hwnd() else {
            return false;
        };
        // SAFETY: plain state query on our own live window handle.
        #[allow(unsafe_code)]
        if unsafe { IsIconic(hwnd) } != 0 {
            return false;
        }
        // SAFETY: plain state query on our own live window handle.
        #[allow(unsafe_code)]
        let maximized = unsafe { IsZoomed(hwnd) } != 0;
        // The outer rect, in the same space `apply_outer_placement`
        // restores, so apply and capture are exact inverses.
        let (x, y, width, height) = outer_window_rect(hwnd);
        self.app
            .borrow_mut()
            .record_window_geometry(width, height, Some((x, y)), maximized);
        true
    }

    /// [`Self::record_geometry`] plus an immediate write — the two live
    /// write points (an interactive move/resize ending, a keyboard-driven
    /// change settling). Recording nothing (a minimized window) writes
    /// nothing either. The recording borrow is dropped before the
    /// persisting one.
    fn capture_geometry(&self) {
        if self.record_geometry() {
            report(self.app.borrow().persist_settings());
        }
    }

    /// (Re)arms the settle debounce after a placement change that raises
    /// no `WM_EXITSIZEMOVE`: pushes the deadline out and makes sure the
    /// timer that polls it is running.
    fn arm_settle(&self) {
        self.placement_changed_at.set(Some(Instant::now()));
        self.settle_timer.start();
    }

    /// Drops a pending settle capture (an immediate write took it over,
    /// or the window is closing).
    fn cancel_settle(&self) {
        self.settle_timer.stop();
        self.placement_changed_at.set(None);
    }

    /// One settle-timer poll: capture only once the last placement change
    /// has been quiet for [`GEOMETRY_SETTLE_MS`], otherwise leave the
    /// timer running and wait. This is what keeps a burst of changes —
    /// or a whole interactive resize, which raises `OnResize` throughout
    /// — down to a single settings write.
    fn settle_tick(&self) {
        let Some(changed_at) = self.placement_changed_at.get() else {
            self.settle_timer.stop();
            return;
        };
        if !placement_settled(changed_at, Instant::now()) {
            return;
        }
        self.cancel_settle();
        self.capture_geometry();
    }

    /// (Re)creates the four status-bar parts: message (fills) · seed ·
    /// score · time. `width` is the bar's physical width.
    fn set_status_parts(&self, width: i32) {
        let Some(status) = self.status.handle.hwnd() else {
            return;
        };
        #[allow(clippy::cast_possible_truncation)] // DPI factors are small
        let scale =
            |logical: i32| -> i32 { (f64::from(logical) * nwg::scale_factor()).round() as i32 };
        let (seed_w, score_w, time_w) =
            (scale(PART_SEED_W), scale(PART_SCORE_W), scale(PART_TIME_W));
        let edges = [
            (width - seed_w - score_w - time_w).max(0),
            (width - score_w - time_w).max(0),
            (width - time_w).max(0),
            -1,
        ];
        self.seed_part_range.set((edges[0], edges[1]));
        // SAFETY: live status-bar HWND; SB_SETPARTS reads `edges.len()`
        // i32s from the pointer, which points at exactly that array.
        #[allow(unsafe_code)]
        unsafe {
            SendMessageW(status, SB_SETPARTS, edges.len(), edges.as_ptr() as isize);
        }
        // Parts were rebuilt: force the next update to rewrite them.
        *self.status_cache.borrow_mut() = std::array::from_fn(|_| String::from("\u{1}"));
    }

    /// Pushes the current message/seed/score/time into the status-bar
    /// parts, skipping parts whose text is unchanged.
    fn update_status(&self) {
        {
            let mut message = self.status_message.borrow_mut();
            if message.1.is_some_and(|expires| Instant::now() >= expires) {
                *message = (String::new(), None);
            }
        }
        let (seed, score, time) = {
            let app = self.app.borrow();
            let presenter = app.presenter();
            (
                seed_label(presenter),
                score_text(presenter),
                time_text(presenter),
            )
        };
        let texts = [self.status_message.borrow().0.clone(), seed, score, time];
        let mut cache = self.status_cache.borrow_mut();
        for (index, text) in texts.iter().enumerate() {
            if cache.get(index).is_some_and(|cached| cached != text) {
                #[allow(clippy::cast_possible_truncation)] // 4 parts
                self.status.set_text(index as u8, text);
                if let Some(slot) = cache.get_mut(index) {
                    slot.clone_from(text);
                }
            }
        }
    }

    /// Shows a transient status message (rejections, save/load
    /// results); it clears itself after a few seconds.
    fn show_status(&self, text: String) {
        *self.status_message.borrow_mut() = (
            text,
            Some(Instant::now() + Duration::from_secs(STATUS_MESSAGE_SECS)),
        );
        self.update_status();
    }

    /// One frame: advance the clock/animations, render, refresh the
    /// status bar, and pop the Game Won dialog when a win settles.
    fn tick(self: &Rc<Self>) {
        if self.ticking.get() || self.closing.get() {
            return;
        }
        self.ticking.set(true);
        let list = {
            let mut app = self.app.borrow_mut();
            app.advance();
            app.take_frame_if_changed()
        };
        // Frames go to the render thread, which is the only place
        // allowed to wait on the GPU; a busy or wedged renderer means
        // dropped frames here, never a blocked GUI thread. Skip even
        // handing frames over while hidden/minimized (nothing will
        // present them) — the clock keeps advancing above, like the
        // original's timer.
        if self.presentable()
            && let Some(list) = list
            && let Some(render) = self.render.borrow_mut().as_mut()
        {
            render.frame(list);
        }
        // Card-back grid animation: cheap when the Options dialog is
        // closed (one visibility check, nothing else), so unconditional
        // here rather than gated a second time on top of the check
        // `animate_backs` already does itself.
        self.options.animate_backs(&self.app);
        self.update_status();
        self.ticking.set(false);

        // Game Won — never while a dialog is open: the MessageBox
        // would re-enable the deliberately disabled main window when
        // it closes, breaking the dialog's modality.
        if !self.dialog_open() && self.app.borrow_mut().won_just_settled() {
            let score = { score_text(self.app.borrow().presenter()) };
            let content = if score.is_empty() {
                String::from("Congratulations, you won!\r\n\r\nDeal another game?")
            } else {
                format!("Congratulations, you won!\r\n{score}\r\n\r\nDeal another game?")
            };
            // No borrows held: the message box pumps messages.
            let choice = nwg::modal_message(
                &self.window,
                &nwg::MessageParams {
                    title: "Game Won",
                    content: &content,
                    buttons: nwg::MessageButtons::YesNo,
                    icons: nwg::MessageIcons::None,
                },
            );
            if choice == nwg::MessageChoice::Yes {
                self.app.borrow_mut().deal_random();
            }
        }
    }

    /// Whether one of the custom dialogs is up (the main window is
    /// disabled then).
    fn dialog_open(&self) -> bool {
        self.options.window.visible() || self.select.window.visible()
    }

    /// Whether the playfield can safely present a frame: the window is
    /// visible and not minimized. (`--smoke` renders with the window
    /// never shown, but it calls the render path directly and never
    /// enters the message loop, so this gate does not apply there —
    /// the hazard is a *withdrawn* or iconic window whose swapchain
    /// the compositor abandoned mid-session.)
    fn presentable(&self) -> bool {
        if !self.window.visible() {
            return false;
        }
        let Some(window) = self.window.handle.hwnd() else {
            return false;
        };
        // SAFETY: plain state query on our own live window handle.
        #[allow(unsafe_code)]
        let iconic = unsafe { IsIconic(window) } != 0;
        !iconic
    }

    /// Pushes the current presenter frame through the render path
    /// unconditionally, bypassing the per-tick change gate
    /// (`take_frame_if_changed`). A theme or scaling swap rebuilds the
    /// renderer's atlas but draws nothing by itself — the canvas is only
    /// ever written by a `RenderCmd::Frame`, and the per-tick path sends
    /// one only when the display list actually differs from the last one
    /// submitted. A scaling change never touches card geometry, and two
    /// themes that happen to share both `base_size` and every card
    /// position (plausible for two PNG themes converted from the same
    /// source layout) produce the same list too, so on an idle board the
    /// change-gated path can suppress forever and the canvas would go on
    /// showing the atlas just replaced. Never blocks:
    /// `RenderHandle::frame` only ever drops a frame, it does not wait.
    fn push_frame(&self) {
        let list = self.app.borrow().frame();
        if let Some(list) = list
            && let Some(render) = self.render.borrow_mut().as_mut()
        {
            render.frame(list);
        }
    }

    /// Switches the active theme through the render path, keeping app
    /// state and renderer in lock-step; the error text comes back for
    /// the status bar (the previous theme stays fully active then).
    fn switch_theme(&self, id: &str) -> Option<String> {
        let theme = match self.app.borrow().load_theme(id) {
            Ok(theme) => theme,
            Err(error) => return Some(error),
        };
        if let Some(render) = self.render.borrow().as_ref() {
            let scale = self.app.borrow().scale();
            let scaling = self.app.borrow().scaling_of(id);
            if let Err(error) = render.set_theme(theme.clone(), scaling, scale) {
                return Some(error);
            }
        }
        let adopted = self.app.borrow_mut().adopt_theme(id, &theme);
        if adopted && let Some(render) = self.render.borrow().as_ref() {
            render.set_scale(self.app.borrow().scale());
        }
        self.push_frame();
        None
    }

    /// Applies a card-scaling choice through the render path, keeping app
    /// state and renderer in lock-step; the error text comes back for the
    /// status bar (the previous scaling stays fully active then).
    fn switch_scaling(&self, scaling: CardScaling) -> Option<String> {
        if scaling == self.app.borrow().scaling() {
            return None;
        }
        let theme = self.app.borrow().theme().clone();
        if let Some(render) = self.render.borrow().as_ref() {
            let scale = self.app.borrow().scale();
            if let Err(error) = render.set_theme(theme, scaling, scale) {
                return Some(error);
            }
        }
        self.app.borrow_mut().set_scaling(scaling);
        // A scaling change cannot alter base_size, so unlike a theme swap
        // no refit is needed — but the rebuilt atlas still has to reach
        // the screen; see `push_frame` for why this cannot be left to the
        // next tick.
        self.push_frame();
        None
    }

    /// Populates the Options dialog from current app state and refreshes
    /// its card-back grid — the two steps every dialog open needs, kept
    /// together so no caller can do one without the other. `populate`
    /// itself cannot refresh the grid: filling it means rendering through
    /// the render thread, which only this `Ui`, not the dialog, can
    /// reach.
    fn populate_options(&self) {
        self.options.populate(&self.app);
        self.refresh_back_previews();
    }

    /// Rebuilds the Options dialog's card-back grid: renders the active
    /// theme's card-back contact sheet and hands the resulting
    /// thumbnails to the dialog, or, on any failure, reports the reason
    /// on the status bar and falls the grid back to plain back names.
    ///
    /// Called wherever the grid needs refreshing: opening the dialog
    /// ([`Self::populate_options`]), and after a theme or card-scaling
    /// change — both change what the sheet renders.
    fn refresh_back_previews(&self) {
        match self.build_back_previews() {
            Ok(previews) => self.options.refresh_back_grid(&self.app, Some(previews)),
            Err(reason) => {
                self.options.refresh_back_grid(&self.app, None);
                self.show_status(reason);
            }
        }
    }

    /// Renders the active theme's card-back contact sheet through the
    /// render thread and cuts it into per-back, per-frame bitmaps ready
    /// for the dialog's image list.
    ///
    /// The sheet's scale is the display's own scale factor
    /// ([`nwg::scale_factor`]), rounded up to an integer and clamped to
    /// `1..=4` ([`previews::sheet_scale`]) so every cell rectangle
    /// multiplies to exact physical pixels; its `max_side` is the render thread's
    /// texture ceiling divided by that scale, since the presenter itself
    /// lays the sheet out in logical pixels. The background is opaque —
    /// the dialog's own list background — unlike the board's transparent
    /// clear: an image-list bitmap is composited by a common control, not
    /// a place to rely on per-pixel alpha, and a card's transparent
    /// corners should show the control's own background instead.
    ///
    /// # Errors
    ///
    /// Text naming what failed: the render thread is not ready yet, the
    /// active theme's card backs do not fit one preview image, the
    /// render itself failed or did not answer in time, or the rendered
    /// pixels and the sheet's own layout disagreed.
    fn build_back_previews(&self) -> Result<BackPreviews, String> {
        let render = self.render.borrow();
        let Some(render) = render.as_ref() else {
            return Err(String::from("the renderer is not ready yet"));
        };
        let scale = previews::sheet_scale(nwg::scale_factor());
        let max_side = render.max_texture_dim() / scale;
        let background = {
            let [r, g, b] = self.options.list_back.background_color();
            Rgba::opaque(r, g, b)
        };

        let app = self.app.borrow();
        let sheet = app
            .presenter()
            .back_sheet(background, max_side)
            .ok_or_else(|| {
                String::from("the active theme's card backs do not fit one preview image")
            })?;
        let back_count = app.presenter().back_count();
        drop(app);

        let BackSheet {
            size,
            cell,
            cells,
            list,
        } = sheet;
        let width = u32::try_from(size.w).unwrap_or(0).saturating_mul(scale);
        let height = u32::try_from(size.h).unwrap_or(0).saturating_mul(scale);
        #[allow(clippy::cast_precision_loss)] // scale is 1..=4, exact in f32
        let scale_f32 = scale as f32;
        let pixels = render.render_sheet(list, (width, height), scale_f32)?;
        let frames = previews::png_frames(&pixels, (width, height), &cells, scale, back_count)
            .map_err(|error| error.to_string())?;

        let mut bitmaps: Vec<Vec<nwg::Bitmap>> = Vec::with_capacity(frames.len());
        for back_frames in frames {
            let mut decoded = Vec::with_capacity(back_frames.len());
            for png in back_frames {
                let bitmap = nwg::Bitmap::from_bin(&png)
                    .map_err(|error| format!("decoding a card-back thumbnail: {error}"))?;
                decoded.push(bitmap);
            }
            bitmaps.push(decoded);
        }

        let cell_physical = (
            u32::try_from(cell.w).unwrap_or(0).saturating_mul(scale),
            u32::try_from(cell.h).unwrap_or(0).saturating_mul(scale),
        );
        Ok(BackPreviews {
            frames: bitmaps,
            cell: cell_physical,
        })
    }

    /// Opens a dialog window: the main window is disabled for the
    /// duration, which is what makes the dialog modal.
    fn open_dialog(&self, dialog: &nwg::Window) {
        self.window.set_enabled(false);
        dialog.set_visible(true);
        dialog.set_focus();
    }

    /// Closes a dialog window and gives the main window back.
    fn close_dialog(&self, dialog: &nwg::Window) {
        dialog.set_visible(false);
        self.window.set_enabled(true);
        self.window.set_focus();
    }

    /// The Options dialog's Cancel/close path: put the previewed theme,
    /// back and card scaling back, then close.
    fn cancel_options(&self) {
        let restore = self.app.borrow_mut().take_preview_restore();
        if let Some(restore) = restore {
            if restore.theme_id == self.app.borrow().theme_id() {
                // The theme replay below never runs in this branch, so a
                // scaling-only live preview against this same theme (no
                // theme switch involved) rebuilt the render path directly
                // and needs its own undo here. The target has to come
                // from the restore snapshot rather than from
                // `app.scaling()` after the bookkeeping restore below
                // runs: at that point it would already equal the target,
                // defeating `switch_scaling`'s own no-op guard and
                // leaving the renderer's atlas on the previewed value.
                let restored_scaling = restore.scaling_of(&restore.theme_id);
                if let Some(error) = self.switch_scaling(restored_scaling) {
                    eprintln!("sol-win32: restoring scaling after cancel failed: {error}");
                }
                self.app.borrow_mut().restore_scaling(&restore);
            } else {
                // Restore the scaling bookkeeping before the replay
                // below reads it, so the theme swap rebuilds under the
                // scaling recorded when the dialog opened rather than
                // one edited mid-session on a theme since switched away
                // from.
                self.app.borrow_mut().restore_scaling(&restore);
                // Restoring what was active before cannot introduce a
                // new failure mode worth surfacing on a Cancel; stderr
                // is enough.
                if let Some(error) = self.switch_theme(&restore.theme_id) {
                    eprintln!("sol-win32: restoring theme after cancel failed: {error}");
                }
                self.options.refresh_scaling(&self.app);
            }
            if let Some(error) = self.app.borrow_mut().set_back(restore.back_index) {
                eprintln!("sol-win32: restoring back after cancel failed: {error}");
            }
        }
        self.close_dialog(&self.options.window);
    }

    /// A key press anywhere on the window or canvas. Lands running
    /// animations first (like the original), then dispatches the menu
    /// accelerators nwg cannot: F2, Ctrl+Z, Ctrl+Y. Returns `true`
    /// when the key was one of those.
    fn handle_key(&self, key: usize) -> bool {
        self.app.borrow_mut().any_key();
        // SAFETY: plain keyboard-state query, no pointers involved.
        #[allow(unsafe_code)]
        let control_down = unsafe { GetKeyState(VK_CONTROL) } < 0;
        #[allow(clippy::cast_sign_loss)] // VK constants are small positives
        match key {
            key if key == VK_F2 as usize => {
                self.app.borrow_mut().deal_random();
                true
            }
            0x5A if control_down => {
                // Ctrl+Z
                let rejection = self.app.borrow_mut().undo();
                if let Some(error) = rejection {
                    self.show_status(error);
                }
                true
            }
            0x59 if control_down => {
                // Ctrl+Y
                let rejection = self.app.borrow_mut().redo();
                if let Some(error) = rejection {
                    self.show_status(error);
                }
                true
            }
            _ => false,
        }
    }

    /// A click on the status bar: inside the seed part it copies the
    /// bare seed digits to the clipboard.
    fn status_clicked(&self, x: i32) {
        let (from, to) = self.seed_part_range.get();
        if x < from || x >= to {
            return;
        }
        let digits = { seed_digits(self.app.borrow().presenter()) };
        nwg::Clipboard::set_data_text(&self.window, &digits);
        self.show_status(format!("Seed {digits} copied to the clipboard"));
    }
}

/// Splits a mouse lparam into signed client coordinates (they go
/// negative while the capture drags the pointer left of or above the
/// canvas).
// The `as u16 as i16 as i32` chain is the documented Win32 lparam unpack:
// each coordinate is a signed 16-bit value packed into half of the word, so
// the truncation to u16, the reinterpretation as i16 and the widening back
// to i32 are the decode, not a lossy conversion clippy should warn about.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
const fn point_of(lparam: isize) -> (i32, i32) {
    let x = (lparam as usize & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam as usize >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Wires every handler and returns their tokens.
///
/// One call per surface: each `bind_*` below owns one independent piece of
/// wiring, and this is the list of them.
fn bind_handlers(ui: &Rc<Ui>) -> Handlers {
    let mut events = Vec::new();
    let mut raw = Vec::new();

    bind_main_window(ui, &mut events);
    bind_playfield(ui, &mut raw);
    bind_window_keys(ui, &mut raw);
    bind_size_move(ui, &mut raw);
    bind_status_bar(ui, &mut raw);
    bind_options_dialog(ui, &mut events);
    bind_select_dialog(ui, &mut events);

    Handlers { events, raw }
}

/// Wires the main window: menu commands, resize, close, and the animation timer.
fn bind_main_window(ui: &Rc<Ui>, events: &mut Vec<nwg::EventHandler>) {
    // Main window: menu, resize, close, timer.
    let handler_ui = Rc::clone(ui);
    events.push(nwg::full_bind_event_handler(
        &ui.window.handle,
        move |event, _data, handle| {
            let ui = &handler_ui;
            match event {
                nwg::Event::OnTimerTick if handle == ui.timer.handle => ui.tick(),
                nwg::Event::OnTimerTick if handle == ui.settle_timer.handle => ui.settle_tick(),
                nwg::Event::OnResize | nwg::Event::OnWindowMaximize
                    if handle == ui.window.handle =>
                {
                    ui.layout();
                    // Keyboard-driven placement changes (Win+arrow snap,
                    // maximize/restore) raise no WM_EXITSIZEMOVE; push the
                    // settle deadline out so the new placement still gets
                    // captured once it stops changing — and only then, no
                    // matter how many of these arrive.
                    ui.arm_settle();
                }
                // Win32 idiom: refresh enabled-states just before the
                // menu drops down. Undo/Redo bind to the presenter's
                // `can_undo`/`can_redo`, which the engine already holds
                // `false` under Vegas — the original's "disabled in
                // Vegas" needs no frontend rule.
                nwg::Event::OnMenuOpen if handle == ui.menu_game.handle => {
                    let model = ui.app.borrow().menu_model();
                    ui.item_undo.set_enabled(model.undo_enabled);
                    ui.item_redo.set_enabled(model.redo_enabled);
                }
                nwg::Event::OnMenuItemSelected => {
                    if handle == ui.item_deal.handle {
                        ui.app.borrow_mut().deal_random();
                    } else if handle == ui.item_select.handle {
                        ui.select.populate(&ui.app);
                        ui.open_dialog(&ui.select.window);
                        ui.select.input.set_focus();
                    } else if handle == ui.item_undo.handle {
                        let rejection = ui.app.borrow_mut().undo();
                        if let Some(error) = rejection {
                            ui.show_status(error);
                        }
                    } else if handle == ui.item_redo.handle {
                        let rejection = ui.app.borrow_mut().redo();
                        if let Some(error) = rejection {
                            ui.show_status(error);
                        }
                    } else if handle == ui.item_save.handle {
                        let status = ui.app.borrow_mut().save();
                        ui.show_status(status);
                    } else if handle == ui.item_load.handle {
                        let status = ui.app.borrow_mut().load();
                        ui.show_status(status);
                    } else if handle == ui.item_options.handle {
                        ui.populate_options();
                        ui.app.borrow_mut().begin_preview();
                        ui.open_dialog(&ui.options.window);
                    } else if handle == ui.item_exit.handle {
                        ui.window.close();
                    } else if handle == ui.item_about.handle {
                        // No borrows held: the message box pumps.
                        nwg::modal_info_message(
                            &ui.window,
                            "About classic-solitair",
                            "classic-solitair\r\n\r\n\
                             A faithful reproduction of Windows 98 Klondike Solitaire, \
                             extended with save/load, undo/redo, themes, and seed-based \
                             game selection.\r\n\r\n\
                             Free software under the GNU GPL 3.0 or later. \
                             No original Microsoft artwork is included; use \
                             soltool extract to build a theme from your own files.",
                        );
                    }
                }
                nwg::Event::OnWindowClose if handle == ui.window.handle => {
                    // Stop presenting before nwg's default close
                    // handling hides the window: a tick already in
                    // flight would otherwise render to a window the
                    // compositor abandoned and park the GUI thread in
                    // an unbounded GPU wait — the app froze exactly
                    // that way under wine.
                    if std::env::var_os("SOL_WIN32_LOG").is_some() {
                        eprintln!("sol-win32 log: OnWindowClose entered");
                    }
                    ui.closing.set(true);
                    // Record before dropping the pending settle capture:
                    // the window still exists here, so this is the last
                    // chance to see its true final placement, and a change
                    // made inside the settle window would otherwise be
                    // thrown away. `run`'s persist after the loop writes
                    // it.
                    ui.record_geometry();
                    ui.cancel_settle();
                    ui.timer.stop();
                    nwg::stop_thread_dispatch();
                    if std::env::var_os("SOL_WIN32_LOG").is_some() {
                        eprintln!("sol-win32 log: OnWindowClose done");
                    }
                }
                _ => {}
            }
        },
    ));
}

/// Wires the playfield canvas: raw pointer messages, for exact physical coordinates.
fn bind_playfield(ui: &Rc<Ui>, raw: &mut Vec<nwg::RawEventHandler>) {
    // Playfield input: raw messages give exact physical coordinates
    // and full control over capture; presenter semantics (drag pickup
    // on down, double-click detection) live behind `pointer_down`.
    let canvas_ui = Rc::clone(ui);
    if let Ok(handler) = nwg::bind_raw_event_handler(
        &ui.canvas.handle,
        RAW_ID_CANVAS,
        move |_hwnd, msg, wparam, lparam| {
            let ui = &canvas_ui;
            match msg {
                WM_LBUTTONDOWN => {
                    let (x, y) = point_of(lparam);
                    // Capture so the drag keeps tracking (and can
                    // release) outside the canvas, like the original.
                    nwg::GlobalCursor::set_capture(&ui.canvas.handle);
                    ui.app.borrow_mut().pointer_down(x, y);
                }
                WM_MOUSEMOVE => {
                    let (x, y) = point_of(lparam);
                    ui.app.borrow_mut().pointer_move(x, y);
                }
                WM_LBUTTONUP => {
                    let (x, y) = point_of(lparam);
                    nwg::GlobalCursor::release();
                    ui.app.borrow_mut().pointer_up(x, y);
                }
                WM_KEYDOWN => {
                    ui.handle_key(wparam);
                }
                _ => {}
            }
            None
        },
    ) {
        raw.push(handler);
    }
}

/// Wires keyboard accelerators reaching the window itself.
fn bind_window_keys(ui: &Rc<Ui>, raw: &mut Vec<nwg::RawEventHandler>) {
    // Keyboard accelerators when focus sits on the window itself.
    let keys_ui = Rc::clone(ui);
    if let Ok(handler) = nwg::bind_raw_event_handler(
        &ui.window.handle,
        RAW_ID_WINDOW_KEYS,
        move |_hwnd, msg, wparam, _lparam| {
            if msg == WM_KEYDOWN {
                keys_ui.handle_key(wparam);
            }
            None
        },
    ) {
        raw.push(handler);
    }
}

/// Wires the end of an interactive move or resize.
fn bind_size_move(ui: &Rc<Ui>, raw: &mut Vec<nwg::RawEventHandler>) {
    // End of an interactive move or resize: capture and persist the new
    // placement immediately, and cancel any pending keyboard-driven
    // settle capture so a drag followed right after by a snap doesn't
    // write twice.
    let sizemove_ui = Rc::clone(ui);
    if let Ok(handler) = nwg::bind_raw_event_handler(
        &ui.window.handle,
        RAW_ID_WINDOW_SIZEMOVE,
        move |_hwnd, msg, _wparam, _lparam| {
            if msg == WM_EXITSIZEMOVE {
                sizemove_ui.cancel_settle();
                sizemove_ui.capture_geometry();
            }
            None
        },
    ) {
        raw.push(handler);
    }
}

/// Wires click-to-copy on the status bar's seed part.
fn bind_status_bar(ui: &Rc<Ui>, raw: &mut Vec<nwg::RawEventHandler>) {
    // Click-to-copy on the status bar's seed part.
    let status_ui = Rc::clone(ui);
    if let Ok(handler) = nwg::bind_raw_event_handler(
        &ui.status.handle,
        RAW_ID_STATUS_CLICK,
        move |_hwnd, msg, _wparam, lparam| {
            if msg == WM_LBUTTONDOWN {
                let (x, _y) = point_of(lparam);
                status_ui.status_clicked(x);
            }
            None
        },
    ) {
        raw.push(handler);
    }
}

/// Wires the Options dialog.
fn bind_options_dialog(ui: &Rc<Ui>, events: &mut Vec<nwg::EventHandler>) {
    // Options dialog.
    let options_ui = Rc::clone(ui);
    events.push(nwg::full_bind_event_handler(
        &ui.options.window.handle,
        move |event, _data, handle| {
            let ui = &options_ui;
            match event {
                nwg::Event::OnButtonClick => {
                    if handle == ui.options.ok.handle {
                        let edited = ui.options.read();
                        report(ui.app.borrow_mut().commit_options(edited));
                        ui.close_dialog(&ui.options.window);
                    } else if handle == ui.options.cancel.handle {
                        ui.cancel_options();
                    } else if handle == ui.options.radio_standard.handle
                        || handle == ui.options.radio_vegas.handle
                        || handle == ui.options.radio_none.handle
                    {
                        ui.options.sync_keep_vegas();
                    }
                }
                nwg::Event::OnComboxBoxSelection => {
                    if handle == ui.options.combo_theme.handle {
                        if let Some(id) = ui.options.combo_theme.selection_string() {
                            if let Some(error) = ui.switch_theme(&id) {
                                ui.show_status(error);
                                // Snap the picker back to the theme
                                // that is actually active.
                                let active = ui.app.borrow().theme_id().to_owned();
                                ui.options.combo_theme.set_selection_string(&active);
                            }
                            ui.options.refresh_scaling(&ui.app);
                            ui.refresh_back_previews();
                        }
                    } else if handle == ui.options.combo_scaling.handle {
                        let scaling = index_to_scaling(ui.options.combo_scaling.selection());
                        if let Some(error) = ui.switch_scaling(scaling) {
                            ui.show_status(error);
                            // Snap the picker back to the scaling that
                            // is actually active.
                            let active = ui.app.borrow().scaling();
                            ui.options
                                .combo_scaling
                                .set_selection(Some(scaling_to_index(active)));
                        }
                        // The rendered art itself may have changed (a
                        // PNG theme's xBRZ smoothing is a scaling-level
                        // choice), so the grid needs the same refresh a
                        // theme change gets — whether the switch above
                        // succeeded or the picker just snapped back.
                        ui.refresh_back_previews();
                    }
                }
                nwg::Event::OnListViewItemChanged if handle == ui.options.list_back.handle => {
                    if let Some(index) = ui.options.list_back.selected_item() {
                        let rejection = ui.app.borrow_mut().set_back(index);
                        if let Some(error) = rejection {
                            ui.show_status(error);
                        }
                    }
                }
                // OnKeyEsc arrives with the focused control's handle,
                // and only this dialog's window can emit OnWindowClose
                // here — neither needs a handle guard.
                nwg::Event::OnKeyEsc | nwg::Event::OnWindowClose => {
                    ui.cancel_options();
                }
                _ => {}
            }
        },
    ));
}

/// Wires the Select Game dialog.
fn bind_select_dialog(ui: &Rc<Ui>, events: &mut Vec<nwg::EventHandler>) {
    // Select Game dialog.
    let select_ui = Rc::clone(ui);
    events.push(nwg::full_bind_event_handler(
        &ui.select.window.handle,
        move |event, _data, handle| {
            let ui = &select_ui;
            let accept = matches!(event, nwg::Event::OnKeyEnter)
                || (matches!(event, nwg::Event::OnButtonClick) && handle == ui.select.ok.handle);
            if accept {
                let digits = ui.select.input.text();
                if !ui.app.borrow_mut().select_game(&digits) {
                    ui.show_status(String::from("Enter a game number from 0 to 32767"));
                }
                ui.close_dialog(&ui.select.window);
            } else if matches!(event, nwg::Event::OnKeyEsc | nwg::Event::OnWindowClose)
                || (matches!(event, nwg::Event::OnButtonClick) && handle == ui.select.cancel.handle)
            {
                ui.close_dialog(&ui.select.window);
            }
        },
    ));
}
