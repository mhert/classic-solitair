//! The cxx-qt bridge: `Playfield`, a `QQuickPaintedItem` the QML chrome
//! instantiates. It owns the whole application core ([`App`]) and is the
//! chrome's only door to it — every menu entry, dialog, and pointer
//! event lands in an invokable here, and every displayed string leaves
//! through a property.
//!
//! This is the crate's one unsafe-bearing module (Qt hands `paint` a raw
//! `QPainter` pointer, and `QImage::from_raw_bytes` trusts the buffer
//! layout); each exception carries its `// SAFETY:` invariant.

// The crate denies unsafe_code; this module is the sanctioned exception
// (cxx-qt bridges declare C++ types and virtual overrides, which are
// inherently unsafe declarations). The attribute must be file-scoped:
// cxx_qt::bridge rejects any outer attribute on its module.
#![allow(unsafe_code)]

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QByteArrayBase64Options, QImage, QImageFormat, QRect, QString, QStringList,
};
use sol_engine::{DrawMode, ScoringMode};
use sol_theme::CardScaling;

use sol_frontend::app::App as Core;
use sol_frontend::options::EditedOptions;
use sol_frontend::previews;

use crate::app::{self, App, BackPreviews};
use crate::worker::WorkerEvent;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        /// From cxx-qt-lib.
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        /// From cxx-qt-lib.
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qrect.h");
        /// From cxx-qt-lib.
        type QRect = cxx_qt_lib::QRect;
        include!("cxx-qt-lib/qimage.h");
        /// From cxx-qt-lib.
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qpainter.h");
        /// From cxx-qt-lib.
        type QPainter = cxx_qt_lib::QPainter;
    }

    unsafe extern "C++" {
        include!(<QtQuick/QQuickPaintedItem>);
        /// The playfield item's base class.
        type QQuickPaintedItem;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QQuickPaintedItem]
        #[qproperty(QString, seed_text, cxx_name = "seedText")]
        #[qproperty(QString, score_text, cxx_name = "scoreText")]
        #[qproperty(QString, time_text, cxx_name = "timeText")]
        #[qproperty(QString, status_message, cxx_name = "statusMessage")]
        #[qproperty(bool, can_undo, cxx_name = "canUndo")]
        #[qproperty(bool, can_redo, cxx_name = "canRedo")]
        #[qproperty(bool, won)]
        #[qproperty(i32, back_frame_epoch, cxx_name = "backFrameEpoch")]
        #[qproperty(i32, back_cell_width, cxx_name = "backCellWidth")]
        #[qproperty(i32, back_cell_height, cxx_name = "backCellHeight")]
        /// The QML-facing playfield item.
        type Playfield = super::PlayfieldRust;

        /// Draws the latest rendered frame.
        #[qinvokable]
        #[cxx_override]
        unsafe fn paint(self: Pin<&mut Self>, painter: *mut QPainter);

        /// Requests a scene-graph repaint (QQuickPaintedItem).
        #[inherit]
        fn update(self: Pin<&mut Self>);

        /// Advances animations/clock and renders the next frame.
        #[qinvokable]
        fn tick(self: Pin<&mut Self>);

        /// Adopts the item's size (logical px) and device pixel ratio.
        #[qinvokable]
        #[cxx_name = "viewResized"]
        fn view_resized(self: Pin<&mut Self>, width: f64, height: f64, dpr: f64);

        /// Pointer press at logical item coordinates.
        #[qinvokable]
        fn press(self: Pin<&mut Self>, x: f64, y: f64);

        /// Pointer move at logical item coordinates.
        #[qinvokable]
        #[cxx_name = "moveTo"]
        fn move_to(self: Pin<&mut Self>, x: f64, y: f64);

        /// Pointer release at logical item coordinates.
        #[qinvokable]
        fn release(self: Pin<&mut Self>, x: f64, y: f64);

        /// Any keyboard key: lands running animations, like the original.
        #[qinvokable]
        #[cxx_name = "anyKey"]
        fn any_key(self: Pin<&mut Self>);

        /// Menu "Deal": a new random game.
        #[qinvokable]
        fn deal(self: Pin<&mut Self>);

        /// "Select Game…": deals the seed in `digits`; `false` when
        /// `digits` is not a run of digits.
        #[qinvokable]
        #[cxx_name = "selectGame"]
        fn select_game(self: Pin<&mut Self>, digits: &QString) -> bool;

        /// Menu Undo (rejections land in `statusMessage`).
        #[qinvokable]
        fn undo(self: Pin<&mut Self>);

        /// Menu Redo (rejections land in `statusMessage`).
        #[qinvokable]
        fn redo(self: Pin<&mut Self>);

        /// Menu Save: writes the autosave slot.
        #[qinvokable]
        fn save(self: Pin<&mut Self>);

        /// Menu Load: restores the autosave slot.
        #[qinvokable]
        fn load(self: Pin<&mut Self>);

        /// Window closing: silent autosave.
        #[qinvokable]
        #[cxx_name = "autosaveOnExit"]
        fn autosave_on_exit(self: Pin<&mut Self>);

        /// The persisted logical window width; 0 when none was saved
        /// (QML keeps its own default then).
        #[qinvokable]
        #[cxx_name = "initialWindowWidth"]
        fn initial_window_width(self: &Self) -> i32;

        /// The persisted logical window height; 0 when none was saved
        /// (QML keeps its own default then).
        #[qinvokable]
        #[cxx_name = "initialWindowHeight"]
        fn initial_window_height(self: &Self) -> i32;

        /// Whether both `x` and `y` were persisted.
        #[qinvokable]
        #[cxx_name = "hasInitialWindowPosition"]
        fn has_initial_window_position(self: &Self) -> bool;

        /// The persisted window `x`; 0 without a position (QML gates on
        /// `hasInitialWindowPosition`).
        #[qinvokable]
        #[cxx_name = "initialWindowX"]
        fn initial_window_x(self: &Self) -> i32;

        /// The persisted window `y`; 0 without a position (QML gates on
        /// `hasInitialWindowPosition`).
        #[qinvokable]
        #[cxx_name = "initialWindowY"]
        fn initial_window_y(self: &Self) -> i32;

        /// Whether the persisted window was maximized.
        #[qinvokable]
        #[cxx_name = "initialWindowMaximized"]
        fn initial_window_maximized(self: &Self) -> bool;

        /// Records the reported geometry without writing anything — the
        /// close path's last chance to capture a change the settle
        /// debounce has not written yet; the exit persist carries it.
        #[qinvokable]
        #[cxx_name = "recordWindowGeometry"]
        fn record_window_geometry(
            self: Pin<&mut Self>,
            width: i32,
            height: i32,
            x: i32,
            y: i32,
            maximized: bool,
        );

        /// QML's debounced settle: records the reported geometry and
        /// persists the settings document.
        #[qinvokable]
        #[cxx_name = "windowGeometrySettled"]
        fn window_geometry_settled(
            self: Pin<&mut Self>,
            width: i32,
            height: i32,
            x: i32,
            y: i32,
            maximized: bool,
        );

        /// The discovered theme ids, picker order.
        #[qinvokable]
        #[cxx_name = "themeIds"]
        fn theme_ids(self: &Self) -> QStringList;

        /// The active theme id.
        #[qinvokable]
        #[cxx_name = "themeId"]
        fn theme_id(self: &Self) -> QString;

        /// The active theme's back names, declaration order.
        #[qinvokable]
        #[cxx_name = "backNames"]
        fn back_names(self: &Self) -> QStringList;

        /// The selected back index.
        #[qinvokable]
        #[cxx_name = "backIndex"]
        fn back_index(self: &Self) -> i32;

        /// Options dialog opened: capture the Cancel restore point.
        #[qinvokable]
        #[cxx_name = "beginPreview"]
        fn begin_preview(self: Pin<&mut Self>);

        /// Options dialog live theme preview; empty on success, error
        /// text otherwise.
        #[qinvokable]
        #[cxx_name = "previewTheme"]
        fn preview_theme(self: Pin<&mut Self>, id: &QString) -> QString;

        /// Options dialog live back preview.
        #[qinvokable]
        #[cxx_name = "previewBack"]
        fn preview_back(self: Pin<&mut Self>, index: i32);

        /// Rebuilds the card-back preview grid if the artwork it would
        /// show has changed since the last rebuild; empty on success
        /// (including an already-fresh cache), the failure text
        /// otherwise.
        #[qinvokable]
        #[cxx_name = "refreshBackPreviews"]
        fn refresh_back_previews(self: Pin<&mut Self>) -> QString;

        /// The `data:` URI of the frame card back `back` is showing right
        /// now; empty when previews are unavailable.
        #[qinvokable]
        #[cxx_name = "backFrameUri"]
        fn back_frame_uri(self: &Self, back: i32) -> QString;

        /// Whether the active theme has PNG art, and therefore whether the
        /// card-scaling picker means anything for it.
        #[qinvokable]
        #[cxx_name = "scalingIsAvailable"]
        fn scaling_is_available(self: &Self) -> bool;

        /// The active theme's card scaling as a picker index: 0 original,
        /// 1 xBRZ.
        #[qinvokable]
        #[cxx_name = "scalingIndex"]
        fn scaling_index(self: &Self) -> i32;

        /// Options dialog live card-scaling preview; empty on success,
        /// error text otherwise.
        #[qinvokable]
        #[cxx_name = "previewScaling"]
        fn preview_scaling(self: Pin<&mut Self>, index: i32) -> QString;

        /// Options dialog OK: commit every edited option.
        #[qinvokable]
        #[cxx_name = "commitOptions"]
        fn commit_options(
            self: Pin<&mut Self>,
            draw_three: bool,
            scoring: &QString,
            timed: bool,
            outline_dragging: bool,
            keep_vegas_score: bool,
            sounds: bool,
        );

        /// Options dialog Cancel: restore the previewed-away state.
        #[qinvokable]
        #[cxx_name = "cancelPreview"]
        fn cancel_preview(self: Pin<&mut Self>);

        /// Current option values, for populating the dialog.
        #[qinvokable]
        #[cxx_name = "optionDrawThree"]
        fn option_draw_three(self: &Self) -> bool;
        /// Scoring mode as `"standard"` / `"vegas"` / `"none"`.
        #[qinvokable]
        #[cxx_name = "optionScoring"]
        fn option_scoring(self: &Self) -> QString;
        /// Whether the game is timed.
        #[qinvokable]
        #[cxx_name = "optionTimed"]
        fn option_timed(self: &Self) -> bool;
        /// Whether dragging shows an outline.
        #[qinvokable]
        #[cxx_name = "optionOutline"]
        fn option_outline(self: &Self) -> bool;
        /// Whether the Vegas bankroll persists across deals.
        #[qinvokable]
        #[cxx_name = "optionKeepVegas"]
        fn option_keep_vegas(self: &Self) -> bool;
        /// Whether sounds are enabled (persisted; this frontend does not
        /// play them).
        #[qinvokable]
        #[cxx_name = "optionSounds"]
        fn option_sounds(self: &Self) -> bool;

        /// Whether `--smoke` self-test mode is active.
        #[qinvokable]
        #[cxx_name = "smokeMode"]
        fn smoke_mode(self: &Self) -> bool;
    }

    impl cxx_qt::Initialize for Playfield {}

    impl cxx_qt::Threading for Playfield {}
}

/// The Rust state behind [`qobject::Playfield`].
pub struct PlayfieldRust {
    seed_text: QString,
    score_text: QString,
    time_text: QString,
    status_message: QString,
    can_undo: bool,
    can_redo: bool,
    won: bool,
    /// The application core; `None` when startup failed (the error is
    /// shown via `status_message` and the playfield stays blank).
    app: Option<App>,
    /// The latest rendered frame, ready for `paint`.
    frame_image: Option<QImage>,
    /// The item's logical size, `paint`'s target rectangle.
    view_w: i32,
    view_h: i32,
    /// Logical → physical pixel factor of the hosting window.
    dpr: f64,
    /// The Options dialog's rebuilt card-back preview grid; `None` before
    /// the first rebuild, after a failed one, or once the theme or
    /// scaling it describes is no longer active. `tick` only does its
    /// per-frame comparison when this is `Some` — a closed dialog costs
    /// one `is_none` check.
    back_previews: Option<PreviewCache>,
    /// Bumped whenever the set of frames the cached backs are showing
    /// changes; QML thumbnail delegates re-read `backFrameUri` on this
    /// changing rather than on every render tick.
    back_frame_epoch: i32,
    /// One grid cell's logical width, from the last successful preview
    /// rebuild — themes size their own cards, so this is never a
    /// QML-side constant.
    back_cell_width: i32,
    /// One grid cell's logical height; see `back_cell_width`.
    back_cell_height: i32,
}

impl Default for PlayfieldRust {
    fn default() -> Self {
        Self {
            seed_text: QString::default(),
            score_text: QString::default(),
            time_text: QString::default(),
            status_message: QString::default(),
            can_undo: false,
            can_redo: false,
            won: false,
            app: None,
            frame_image: None,
            view_w: 0,
            view_h: 0,
            dpr: 1.0,
            back_previews: None,
            back_frame_epoch: 0,
            back_cell_width: 0,
            back_cell_height: 0,
        }
    }
}

/// The `(theme, scaling, sheet scale)` a rebuilt [`PreviewCache`] was
/// built for. [`qobject::Playfield::refresh_back_previews`] compares this
/// against the theme, scaling and DPR live right now; a match means the
/// cache already describes exactly this artwork, so the rebuild — a
/// worker round trip — is skipped entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewKey {
    theme_id: String,
    scaling: CardScaling,
    sheet_scale: u32,
}

/// The Options dialog's rebuilt card-back preview grid.
struct PreviewCache {
    /// The artwork this cache describes; see [`PreviewKey`].
    key: PreviewKey,
    /// `uris[back][frame]`: pre-built `data:image/png;base64,...` URIs,
    /// the theme's own back declaration order — empty for a back with no
    /// frames.
    uris: Vec<Vec<String>>,
    /// The frame each back was showing the moment this cache was last
    /// built or last found to have moved on — the baseline
    /// [`qobject::Playfield::tick`]'s next comparison diffs against, so
    /// only a genuine change bumps `backFrameEpoch`.
    shown: Vec<u32>,
}

/// `data:image/png;base64,...` for one PNG blob. `to_base64`'s default
/// options are the plain (non-URL) alphabet with padding — exactly what a
/// `data:` URI wants — and base64 output is pure ASCII, so the lossy
/// UTF-8 conversion below never actually loses anything.
fn data_uri(png: &[u8]) -> String {
    let base64 = QByteArray::from(png).to_base64(QByteArrayBase64Options::default());
    format!(
        "data:image/png;base64,{}",
        String::from_utf8_lossy(base64.as_ref())
    )
}

/// Turns one [`BackPreviews`] rebuild into the cache
/// [`qobject::Playfield::back_frame_uri`] reads from: every PNG blob
/// becomes its own `data:` URI, and the frame each back is showing right
/// now is captured as [`PreviewCache::shown`]'s starting baseline.
fn build_preview_cache(key: PreviewKey, previews: &BackPreviews, app: &App) -> PreviewCache {
    let uris = previews
        .frames
        .iter()
        .map(|frames| frames.iter().map(|png| data_uri(png)).collect())
        .collect();
    let presenter = app.core().presenter();
    let shown = (0..presenter.back_count())
        .map(|back| presenter.back_frame(back))
        .collect();
    PreviewCache { key, uris, shown }
}

impl cxx_qt::Initialize for qobject::Playfield {
    /// Boots the application core with the CLI's theme/seed overrides
    /// and the persisted settings, wiring the render worker's events
    /// back onto this Qt thread.
    fn initialize(mut self: Pin<&mut Self>) {
        let cli = crate::cli();
        let (settings, paths) = app::startup_state();
        let qt_thread = self.qt_thread();
        let deliver = move |event: WorkerEvent| {
            // A queue failure means the window is already gone.
            let _ = qt_thread.queue(move |playfield| {
                playfield.handle_worker_event(event);
            });
        };
        match App::start(cli.theme.clone(), cli.seed, settings, paths, deliver) {
            Ok(app) => self.as_mut().rust_mut().app = Some(app),
            Err(error) => {
                eprintln!("sol-qt: startup failed: {error:#}");
                let message = QString::from(format!("Startup failed: {error}").as_str());
                self.as_mut().set_status_message(message);
            }
        }
    }
}

/// Whether the running windowing system exposes a meaningful top-level
/// window position — the condition under which a captured `x`/`y` is
/// worth persisting at all.
///
/// A Wayland client can neither read nor set its own position, so the
/// coordinates Qt reports there describe nothing restorable and the
/// settings document keeps no `x`/`y` for such a run. cxx-qt-lib binds no
/// `QGuiApplication::platformName()`, so the session type is inferred
/// from `WAYLAND_DISPLAY` instead. The trade-off: an X11 run inside a
/// Wayland session (`-platform xcb`, or an X11-only build) sees that
/// variable too and therefore also persists no position — it forgets a
/// usable position rather than restoring a meaningless one.
fn platform_reports_window_position() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// Saturating `f64` → `i32` for window coordinates: non-finite → 0,
/// everything else clamped into range before the cast.
// False positive: the clamp above the cast makes truncation impossible
// (the fractional part is deliberately floored by the callers).
#[allow(clippy::cast_possible_truncation)]
fn saturating_i32(value: f64) -> i32 {
    if value.is_finite() {
        value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    } else {
        0
    }
}

impl qobject::Playfield {
    /// Physical-pixel point for a logical event position.
    fn physical(&self, x: f64, y: f64) -> (i32, i32) {
        let dpr = self.rust().dpr;
        (
            saturating_i32((x * dpr).floor()),
            saturating_i32((y * dpr).floor()),
        )
    }

    /// Refreshes every chrome-facing property from the presenter.
    fn refresh_status(mut self: Pin<&mut Self>) {
        let Some(app) = &self.rust().app else {
            return;
        };
        let presenter = app.core().presenter();
        let seed = QString::from(sol_frontend::status::seed_digits(presenter).as_str());
        let score = QString::from(sol_frontend::status::score_text(presenter).as_str());
        let time = QString::from(sol_frontend::status::time_text(presenter).as_str());
        // Already false under Vegas scoring — the engine owns that rule.
        let can_undo = presenter.can_undo();
        let can_redo = presenter.can_redo();
        // The generated setters emit notify signals; only touch what
        // actually changed so QML bindings stay quiet between changes.
        if *self.as_ref().seed_text() != seed {
            self.as_mut().set_seed_text(seed);
        }
        if *self.as_ref().score_text() != score {
            self.as_mut().set_score_text(score);
        }
        if *self.as_ref().time_text() != time {
            self.as_mut().set_time_text(time);
        }
        if *self.as_ref().can_undo() != can_undo {
            self.as_mut().set_can_undo(can_undo);
        }
        if *self.as_ref().can_redo() != can_redo {
            self.as_mut().set_can_redo(can_redo);
        }
    }

    /// Shows a transient status-bar message.
    fn show_status(mut self: Pin<&mut Self>, message: &str) {
        self.as_mut().set_status_message(QString::from(message));
    }

    /// Draws the latest rendered frame 1:1 into the item.
    ///
    /// # Safety
    ///
    /// Qt calls this with a valid, exclusively borrowed painter for the
    /// duration of the call.
    pub unsafe fn paint(self: Pin<&mut Self>, painter: *mut qobject::QPainter) {
        // SAFETY: per the contract above, `painter` is valid and unique.
        let Some(painter) = (unsafe { painter.as_mut() }) else {
            return;
        };
        // SAFETY: QPainter is never moved by us; Qt owns its storage.
        let mut painter = unsafe { Pin::new_unchecked(painter) };
        let rust = self.rust();
        if let Some(image) = &rust.frame_image {
            // The image holds physical pixels; the painter works in
            // logical units and its device transform scales by the DPR,
            // mapping the image texel-for-texel onto device pixels.
            painter
                .as_mut()
                .draw_image(&QRect::new(0, 0, rust.view_w, rust.view_h), image);
        }
    }

    /// Adopts one event from the render worker onto the Qt thread
    /// (queued there via [`Threading`]) — never exposed to QML.
    fn handle_worker_event(mut self: Pin<&mut Self>, event: WorkerEvent) {
        match event {
            WorkerEvent::Frame(frame) => {
                let (Ok(width), Ok(height)) =
                    (i32::try_from(frame.width), i32::try_from(frame.height))
                else {
                    // The SAFETY invariant below is that the buffer is
                    // exactly width × height pixels. Saturating a dimension
                    // would break it silently, handing QImage a size the
                    // bytes do not match, so an unrepresentable frame is
                    // dropped instead. Not unit-tested: the surrounding
                    // method needs a live QQuickPaintedItem, so the type
                    // change is what enforces this rather than a test.
                    return;
                };
                // SAFETY: `Frame` guarantees `rgba` is exactly
                // `width × height` tightly packed RGBA8888 pixels, and both
                // dimensions converted without saturating.
                let image = unsafe {
                    QImage::from_raw_bytes(frame.rgba, width, height, QImageFormat::Format_RGBA8888)
                };
                self.as_mut().rust_mut().frame_image = Some(image);
                self.as_mut().update();
            }
            WorkerEvent::Error(reason) => {
                eprintln!("sol-qt: render failed: {reason}");
                self.as_mut()
                    .show_status(&format!("Render failed: {reason}"));
            }
        }
    }

    /// Advances the game clock/animations and the won-dialog/status
    /// bookkeeping, surfacing any one-time status the core returns (e.g.
    /// the render worker going away) through the status bar. Rendering
    /// happens on the worker; repaints are driven by frame arrival
    /// (`handle_worker_event`), not from here.
    pub fn tick(mut self: Pin<&mut Self>) {
        let status = match self.as_mut().rust_mut().app.as_mut() {
            None => return,
            Some(app) => app.tick(),
        };
        if let Some(message) = status {
            self.as_mut().show_status(&message);
        }
        let won_settled = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .map(App::core_mut)
            .is_some_and(Core::won_just_settled);
        if won_settled {
            self.as_mut().set_won(true);
        }
        self.as_mut().refresh_status();
        self.as_mut().tick_back_previews();
    }

    /// Adopts the item's logical size and the window's DPR.
    pub fn view_resized(mut self: Pin<&mut Self>, width: f64, height: f64, dpr: f64) {
        let dpr = if dpr.is_finite() && dpr > 0.0 {
            dpr
        } else {
            1.0
        };
        {
            let mut rust = self.as_mut().rust_mut();
            rust.dpr = dpr;
            rust.view_w = saturating_i32(width.floor()).max(0);
            rust.view_h = saturating_i32(height.floor()).max(0);
        }
        let (phys_w, phys_h) = self.physical(width, height);
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.resize(phys_w.max(0).unsigned_abs(), phys_h.max(0).unsigned_abs());
        }
    }

    /// Pointer press (logical coordinates).
    pub fn press(mut self: Pin<&mut Self>, x: f64, y: f64) {
        let (x, y) = self.physical(x, y);
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().pointer_down(x, y);
        }
    }

    /// Pointer move (logical coordinates).
    pub fn move_to(mut self: Pin<&mut Self>, x: f64, y: f64) {
        let (x, y) = self.physical(x, y);
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().pointer_move(x, y);
        }
    }

    /// Pointer release (logical coordinates).
    pub fn release(mut self: Pin<&mut Self>, x: f64, y: f64) {
        let (x, y) = self.physical(x, y);
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().pointer_up(x, y);
        }
    }

    /// Any key lands running animations.
    pub fn any_key(mut self: Pin<&mut Self>) {
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().any_key();
        }
    }

    /// Menu "Deal".
    pub fn deal(mut self: Pin<&mut Self>) {
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().deal_random();
        }
        self.as_mut().set_won(false);
    }

    /// "Select Game…" with a digit string.
    pub fn select_game(mut self: Pin<&mut Self>, digits: &QString) -> bool {
        let digits = digits.to_string();
        let dealt = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .is_some_and(|app| app.core_mut().select_game(&digits));
        if dealt {
            self.as_mut().set_won(false);
        }
        dealt
    }

    /// Menu Undo.
    pub fn undo(mut self: Pin<&mut Self>) {
        let rejection = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .map(App::core_mut)
            .and_then(Core::undo);
        if let Some(message) = rejection {
            self.show_status(&message);
        }
    }

    /// Menu Redo.
    pub fn redo(mut self: Pin<&mut Self>) {
        let rejection = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .map(App::core_mut)
            .and_then(Core::redo);
        if let Some(message) = rejection {
            self.show_status(&message);
        }
    }

    /// Menu Save.
    pub fn save(mut self: Pin<&mut Self>) {
        let status = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .map(App::core_mut)
            .map(Core::save);
        if let Some(message) = status {
            self.show_status(&message);
        }
    }

    /// Menu Load.
    pub fn load(mut self: Pin<&mut Self>) {
        let status = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .map(App::core_mut)
            .map(Core::load);
        if let Some(message) = status {
            self.as_mut().show_status(&message);
        }
        self.as_mut().set_won(false);
    }

    /// Silent autosave for window close, followed by a settings persist.
    pub fn autosave_on_exit(mut self: Pin<&mut Self>) {
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app::report(app.core().autosave_on_exit());
            app::report(app.core().persist_settings());
        }
    }

    /// The persisted logical window width; 0 when none was saved.
    pub fn initial_window_width(&self) -> i32 {
        self.with_window(
            |geometry| i32::try_from(geometry.width).unwrap_or(i32::MAX),
            0,
        )
    }

    /// The persisted logical window height; 0 when none was saved.
    pub fn initial_window_height(&self) -> i32 {
        self.with_window(
            |geometry| i32::try_from(geometry.height).unwrap_or(i32::MAX),
            0,
        )
    }

    /// Whether both `x` and `y` were persisted.
    pub fn has_initial_window_position(&self) -> bool {
        self.with_window(
            |geometry| geometry.x.is_some() && geometry.y.is_some(),
            false,
        )
    }

    /// The persisted window `x`; 0 without a position.
    pub fn initial_window_x(&self) -> i32 {
        self.with_window(|geometry| geometry.x.unwrap_or(0), 0)
    }

    /// The persisted window `y`; 0 without a position.
    pub fn initial_window_y(&self) -> i32 {
        self.with_window(|geometry| geometry.y.unwrap_or(0), 0)
    }

    /// Whether the persisted window was maximized.
    pub fn initial_window_maximized(&self) -> bool {
        self.with_window(|geometry| geometry.maximized, false)
    }

    /// Records the reported geometry without writing anything: the
    /// close path captures the window's true final placement this way,
    /// and the exit persist writes it once.
    ///
    /// A non-positive or unrepresentable size floors to 1, keeping the
    /// stored geometry positive — the same fallible-conversion shape the
    /// win32 frontend uses so a bad report can never panic, though its
    /// own floor is 0. The position is dropped where the platform
    /// exposes none (see [`platform_reports_window_position`]), which
    /// leaves the settings document without `x`/`y` there.
    pub fn record_window_geometry(
        mut self: Pin<&mut Self>,
        width: i32,
        height: i32,
        x: i32,
        y: i32,
        maximized: bool,
    ) {
        let width = u32::try_from(width).unwrap_or(1).max(1);
        let height = u32::try_from(height).unwrap_or(1).max(1);
        let position = platform_reports_window_position().then_some((x, y));
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut()
                .record_window_geometry(width, height, position, maximized);
        }
    }

    /// QML's debounced settle: records the reported geometry (see
    /// [`Self::record_window_geometry`]) and persists the settings
    /// document.
    pub fn window_geometry_settled(
        mut self: Pin<&mut Self>,
        width: i32,
        height: i32,
        x: i32,
        y: i32,
        maximized: bool,
    ) {
        self.as_mut()
            .record_window_geometry(width, height, x, y, maximized);
        if let Some(app) = self.rust().app.as_ref() {
            app::report(app.core().persist_settings());
        }
    }

    /// The discovered theme ids.
    pub fn theme_ids(&self) -> QStringList {
        self.rust()
            .app
            .as_ref()
            .map(|app| {
                app.core()
                    .theme_ids()
                    .into_iter()
                    .map(|id| QString::from(id.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The active theme id.
    pub fn theme_id(&self) -> QString {
        self.rust()
            .app
            .as_ref()
            .map(|app| QString::from(app.core().theme_id()))
            .unwrap_or_default()
    }

    /// The active theme's back names.
    pub fn back_names(&self) -> QStringList {
        self.rust()
            .app
            .as_ref()
            .map(|app| {
                app.core()
                    .back_names()
                    .into_iter()
                    .map(|name| QString::from(name.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The selected back index.
    pub fn back_index(&self) -> i32 {
        self.rust()
            .app
            .as_ref()
            .and_then(|app| i32::try_from(app.core().back_index()).ok())
            .unwrap_or(0)
    }

    /// Options dialog opened.
    pub fn begin_preview(mut self: Pin<&mut Self>) {
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.core_mut().begin_preview();
        }
    }

    /// Live theme preview; empty result on success.
    pub fn preview_theme(mut self: Pin<&mut Self>, id: &QString) -> QString {
        let id = id.to_string();
        let error = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .and_then(|app| app.select_theme_live(&id));
        if error.is_none() {
            // The board's artwork just changed: any cached preview grid
            // now describes a theme that is no longer active.
            self.as_mut().rust_mut().back_previews = None;
        }
        error.map_or_else(QString::default, |message| QString::from(message.as_str()))
    }

    /// Live back preview.
    pub fn preview_back(mut self: Pin<&mut Self>, index: i32) {
        let index = usize::try_from(index).unwrap_or(0);
        let error = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .and_then(|app| app.core_mut().set_back(index));
        if let Some(message) = error {
            self.show_status(&message);
        }
    }

    /// Rebuilds the Options dialog's card-back preview grid when the
    /// active theme, its card scaling, or the sheet's render scale (see
    /// [`previews::sheet_scale`]) differ from what is cached; a matching key
    /// returns immediately, so reopening the dialog on unchanged artwork
    /// costs nothing. Empty result on success (including the no-op
    /// case); otherwise the failure text a status bar can show, and any
    /// existing cache is dropped, since it would describe artwork that
    /// failed to render rather than what is actually active.
    pub fn refresh_back_previews(mut self: Pin<&mut Self>) -> QString {
        let rust = self.rust();
        let dpr = rust.dpr;
        let Some(app) = rust.app.as_ref() else {
            return QString::default();
        };
        let key = PreviewKey {
            theme_id: String::from(app.core().theme_id()),
            scaling: app.core().scaling(),
            sheet_scale: previews::sheet_scale(dpr),
        };
        if rust
            .back_previews
            .as_ref()
            .is_some_and(|cache| cache.key == key)
        {
            return QString::default();
        }

        let outcome = app.back_previews(dpr).map(|previews| {
            let cache = build_preview_cache(key.clone(), &previews, app);
            (previews.cell, cache)
        });
        match outcome {
            Ok((cell, cache)) => {
                // The generated setters emit notify signals; only touch what
                // actually changed so QML bindings stay quiet between
                // changes. A raw `rust_mut()` field write would skip that
                // notify entirely, leaving `backGrid.cellWidth`/`cellHeight`
                // and the delegate `Image`'s size frozen at whatever they
                // evaluated to before the first successful rebuild.
                if *self.as_ref().back_cell_width() != cell.w {
                    self.as_mut().set_back_cell_width(cell.w);
                }
                if *self.as_ref().back_cell_height() != cell.h {
                    self.as_mut().set_back_cell_height(cell.h);
                }
                self.as_mut().rust_mut().back_previews = Some(cache);
                QString::default()
            }
            Err(error) => {
                self.as_mut().rust_mut().back_previews = None;
                QString::from(error.as_str())
            }
        }
    }

    /// The `data:` URI of the frame card back `back` is showing right
    /// now; empty when no preview grid is cached, or `back` is out of
    /// range — the picker falls back to showing the back's name in that
    /// case.
    ///
    /// Reads [`sol_presenter::Presenter::back_frame`] directly rather
    /// than a cached snapshot, so a thumbnail always names the same
    /// frame the board's own back sprites are drawn with at this exact
    /// instant — the same clock law, read live.
    pub fn back_frame_uri(&self, back: i32) -> QString {
        let Ok(back) = usize::try_from(back) else {
            return QString::default();
        };
        let rust = self.rust();
        let (Some(app), Some(cache)) = (rust.app.as_ref(), rust.back_previews.as_ref()) else {
            return QString::default();
        };
        let frame = usize::try_from(app.core().presenter().back_frame(back)).unwrap_or(0);
        cache
            .uris
            .get(back)
            .and_then(|frames| frames.get(frame))
            .map_or_else(QString::default, |uri| QString::from(uri.as_str()))
    }

    /// Recomputes which frame each cached back is showing and bumps
    /// `backFrameEpoch` when that set changed since the last check; a
    /// closed dialog (no cache) costs exactly the one `is_none` this
    /// falls through to below. QML's thumbnail delegates re-read
    /// [`Self::back_frame_uri`] when the epoch changes, so the cost
    /// tracks how often the shown frames actually move — a handful of
    /// times a second, set by each back's own declared timing — rather
    /// than this method's own per-tick call rate.
    fn tick_back_previews(mut self: Pin<&mut Self>) {
        let current = {
            let rust = self.rust();
            let (Some(app), Some(cache)) = (rust.app.as_ref(), rust.back_previews.as_ref()) else {
                return;
            };
            let presenter = app.core().presenter();
            let current: Vec<u32> = (0..presenter.back_count())
                .map(|back| presenter.back_frame(back))
                .collect();
            if current == cache.shown {
                return;
            }
            current
        };
        let epoch = self.rust().back_frame_epoch.wrapping_add(1);
        if let Some(cache) = self.as_mut().rust_mut().back_previews.as_mut() {
            cache.shown = current;
        }
        self.as_mut().set_back_frame_epoch(epoch);
    }

    /// Whether the active theme has PNG art.
    pub fn scaling_is_available(&self) -> bool {
        self.rust()
            .app
            .as_ref()
            .is_some_and(|app| app.core().theme_is_png())
    }

    /// The active theme's card scaling as a picker index.
    pub fn scaling_index(&self) -> i32 {
        match self.rust().app.as_ref().map(|app| app.core().scaling()) {
            Some(CardScaling::Xbrz) => 1,
            Some(CardScaling::Original) | None => 0,
        }
    }

    /// Live card-scaling preview; empty result on success.
    pub fn preview_scaling(mut self: Pin<&mut Self>, index: i32) -> QString {
        let scaling = if index == 1 {
            CardScaling::Xbrz
        } else {
            CardScaling::Original
        };
        let error = self
            .as_mut()
            .rust_mut()
            .app
            .as_mut()
            .and_then(|app| app.select_scaling_live(scaling));
        if error.is_none() {
            // The board's artwork just changed: any cached preview grid
            // now describes a scaling choice that is no longer active.
            self.as_mut().rust_mut().back_previews = None;
        }
        error.map_or_else(QString::default, |message| QString::from(message.as_str()))
    }

    /// Options dialog OK.
    // QML hands over the dialog's six controls in one call; a struct
    // cannot cross the QML invokable boundary, so the bools stay.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn commit_options(
        mut self: Pin<&mut Self>,
        draw_three: bool,
        scoring: &QString,
        timed: bool,
        outline_dragging: bool,
        keep_vegas_score: bool,
        sounds: bool,
    ) {
        let scoring = match scoring.to_string().as_str() {
            "vegas" => ScoringMode::Vegas,
            "none" => ScoringMode::None,
            _ => ScoringMode::Standard,
        };
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app::report(app.core_mut().commit_options(EditedOptions {
                draw_three,
                scoring,
                timed,
                outline_dragging,
                keep_vegas_score,
                sounds,
            }));
        }
        self.as_mut().refresh_status();
    }

    /// Options dialog Cancel.
    pub fn cancel_preview(mut self: Pin<&mut Self>) {
        if let Some(app) = self.as_mut().rust_mut().app.as_mut() {
            app.cancel_preview();
        }
        // A cancel can revert the theme or scaling live (see
        // `App::cancel_preview`) without going through `preview_theme` /
        // `preview_scaling`, so the cache is dropped here too — it can no
        // longer be trusted to describe what is now active.
        self.as_mut().rust_mut().back_previews = None;
    }

    /// Current draw mode, for the dialog.
    pub fn option_draw_three(&self) -> bool {
        self.with_options(|options| options.draw_mode == DrawMode::Three, true)
    }

    /// Current scoring mode, for the dialog.
    pub fn option_scoring(&self) -> QString {
        QString::from(self.with_options(
            |options| match options.scoring {
                ScoringMode::Standard => "standard",
                ScoringMode::Vegas => "vegas",
                ScoringMode::None => "none",
            },
            "standard",
        ))
    }

    /// Current timed flag, for the dialog.
    pub fn option_timed(&self) -> bool {
        self.with_options(|options| options.timed, true)
    }

    /// Current outline-dragging flag, for the dialog.
    pub fn option_outline(&self) -> bool {
        self.with_options(|options| options.outline_dragging, false)
    }

    /// Current keep-Vegas-score flag, for the dialog.
    pub fn option_keep_vegas(&self) -> bool {
        self.with_options(|options| options.keep_vegas_score, false)
    }

    /// Current sounds flag, for the dialog.
    pub fn option_sounds(&self) -> bool {
        self.with_options(|options| options.sounds, true)
    }

    /// Whether `--smoke` self-test mode is active.
    // A QML invokable is necessarily an instance method, even when the
    // answer is process-global.
    #[allow(clippy::unused_self)]
    pub fn smoke_mode(&self) -> bool {
        crate::cli().smoke
    }

    /// Reads one value out of the current options, with a fallback for
    /// the failed-startup state.
    fn with_options<T>(&self, read: impl FnOnce(&sol_session::Options) -> T, fallback: T) -> T {
        self.rust()
            .app
            .as_ref()
            .map_or(fallback, |app| read(app.core().presenter().options()))
    }

    /// Reads one value out of the recorded window geometry, with a
    /// fallback for the failed-startup state or no geometry recorded yet.
    fn with_window<T>(
        &self,
        read: impl FnOnce(&sol_session::WindowGeometry) -> T,
        fallback: T,
    ) -> T {
        self.rust()
            .app
            .as_ref()
            .map(App::core)
            .and_then(Core::window_geometry)
            .map_or(fallback, read)
    }
}
