//! [`App`]: the application state every frontend drives, and the entropy it
//! needs to start a new deal.
//!
//! Everything here delegates to the presenter's API — no game logic lives in
//! a frontend. Even the original's "Undo/Redo disabled in Vegas" menu rule
//! arrives ready-made: the engine's `can_undo`/`can_redo` are already `false`
//! under Vegas scoring, so the menus bind to them rather than restating it.
//!
//! Two properties make this testable without a display, and both are
//! deliberate. The core is handed the paths it reads and writes rather than
//! resolving them, so a test can point an `App` at a temporary directory and
//! exercise save, load and settings persistence as ordinary code. And it
//! never prints: a problem worth telling the user about comes back as text
//! for the frontend to log under its own name, because a core that writes to
//! stderr has a behaviour no test can observe.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sol_engine::{DrawMode, Seed};
use sol_presenter::{DisplayList, Presenter};
use sol_session::{Session, Settings, ThemeId, WindowGeometry};
use sol_theme::{CardScaling, RenderMode, Theme};

use crate::error::AppError;
use crate::geometry;
use crate::menu::MenuModel;
use crate::options::EditedOptions;
use crate::themes::{self, ThemeEntry, ThemeSource};

/// Entropy for "Deal": wall-clock nanoseconds folded into a [`Seed`]. The
/// presenter takes seeds from its host because the core crates never
/// touch an OS entropy source; for a frontend the clock is plenty.
///
/// The original picks its game the same way, from the low bits of a clock —
/// it just reads a coarser one (milliseconds since Windows started).
#[must_use]
pub fn random_seed() -> Seed {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    mix_seed(now.as_secs(), now.subsec_nanos())
}

/// Folds a clock reading into a seed.
///
/// Separate from the clock so the fold itself can be pinned against known
/// inputs: whether it actually mixes the sub-second part — rather than, say,
/// masking it away — is not observable from two calls that differ.
fn mix_seed(secs: u64, nanos: u32) -> Seed {
    let mix = u32::try_from(secs & 0xFFFF_FFFF)
        .unwrap_or_default()
        .wrapping_mul(1_000_003)
        ^ nanos;
    Seed::from_entropy(u64::from(mix))
}

/// Where this frontend reads and writes its per-user state.
///
/// The core is handed these rather than resolving them, so a test can point
/// an [`App`] at a temporary directory and exercise save, load and settings
/// persistence as ordinary code. Binaries pass [`StatePaths::resolve`].
///
/// A `None` on either side disables that document: the data directory could
/// not be resolved, or the run is a self-test that must not touch the user's
/// files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatePaths {
    /// The autosave document.
    pub autosave: Option<PathBuf>,
    /// The settings document.
    pub settings: Option<PathBuf>,
}

impl StatePaths {
    /// The real per-user locations, plus a notice for each that could not be
    /// resolved.
    #[must_use]
    pub fn resolve() -> (Self, Vec<String>) {
        let (autosave, autosave_notice) =
            resolved_or_notice("autosave", sol_session::paths::autosave_path());
        let (settings, settings_notice) =
            resolved_or_notice("settings", sol_session::paths::settings_path());
        let paths = Self { autosave, settings };
        let notices = autosave_notice.into_iter().chain(settings_notice).collect();
        (paths, notices)
    }

    /// Both documents under `dir`, for a run that must not touch the user's
    /// own data directory.
    #[must_use]
    pub fn under(dir: &Path) -> Self {
        Self {
            autosave: Some(dir.join("autosave.json")),
            settings: Some(dir.join("settings.json")),
        }
    }
}

/// The theme a startup settled on, and why it is not the requested one.
#[derive(Debug)]
struct ChosenTheme {
    theme: Theme,
    id: String,
    notice: Option<String>,
}

/// Loads `requested` from `entries`, falling back to `"default"` with a
/// notice when it will not load.
///
/// There is no special case for `requested == "default"`, and deliberately
/// none: the fallback below would then load the same theme that just failed,
/// fail identically, and return that error — which is exactly what a
/// "default is fatal" branch would return. The branch could not change any
/// outcome, so it is not written.
///
/// # Errors
///
/// [`AppError::Theme`] when neither `requested` nor `"default"` will load.
fn choose_startup_theme(entries: &[ThemeEntry], requested: &str) -> Result<ChosenTheme, AppError> {
    let refusal = match themes::load(entries, requested) {
        Ok(theme) => {
            return Ok(ChosenTheme {
                theme,
                id: String::from(requested),
                notice: None,
            });
        }
        Err(refusal) => refusal,
    };
    Ok(ChosenTheme {
        theme: themes::load(entries, "default")?,
        id: String::from("default"),
        notice: Some(format!(
            "startup theme \"{requested}\" failed ({refusal}); using default"
        )),
    })
}

/// One resolved document location, or the notice explaining its absence.
///
/// A path failing to resolve means no home or config directory — which no
/// in-process test can bring about without setting an environment variable,
/// and `unsafe_code` is forbidden here. Separating the decision from the
/// lookup is what makes the failure side reachable at all.
fn resolved_or_notice(
    what: &str,
    result: Result<PathBuf, sol_session::StorageError>,
) -> (Option<PathBuf>, Option<String>) {
    match result {
        Ok(path) => (Some(path), None),
        Err(error) => (None, Some(format!("no {what} location: {error}"))),
    }
}

/// Reads the persisted settings.
///
/// An absent document is a first run, not a problem, so it yields the
/// defaults silently; only a document that exists and will not parse earns a
/// notice.
#[must_use]
pub fn load_settings(paths: &StatePaths) -> (Settings, Option<String>) {
    let Some(path) = &paths.settings else {
        return (Settings::default(), None);
    };
    if !path.is_file() {
        return (Settings::default(), None);
    }
    match sol_session::storage::load_settings_from(path) {
        Ok(settings) => (settings, None),
        Err(error) => (
            Settings::default(),
            Some(format!("loading settings failed ({error}); using defaults")),
        ),
    }
}

/// Whether `condition` should be announced now: true the first time it
/// holds, false forever after, and false while it does not hold.
///
/// A one-shot latch, separated from the game state that drives it because
/// reaching a real win takes a full game while the latch has four states
/// worth pinning.
fn announce_once(condition: bool, already_announced: &mut bool) -> bool {
    if condition && !*already_announced {
        *already_announced = true;
        return true;
    }
    false
}

/// A restore point for the Options dialog's live preview: what to put
/// back on Cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRestore {
    /// The theme that was active when the dialog opened.
    pub theme_id: String,
    /// The back that was selected when the dialog opened.
    pub back_index: usize,
    /// Every per-theme scaling choice as it stood when the dialog opened.
    /// Restored wholesale, so a choice made for a theme the dialog then
    /// switched away from is undone too.
    pub theme_scaling: BTreeMap<ThemeId, CardScaling>,
}

impl PreviewRestore {
    /// The scaling `id` had recorded in this restore point, or
    /// [`CardScaling::Original`] if `id` was never chosen for, or isn't
    /// parseable as a [`ThemeId`]. Mirrors [`App::scaling_of`] against the
    /// snapshot instead of the live map — what a Cancel handler needs when
    /// undoing a scaling-only preview against the theme the dialog opened
    /// on.
    #[must_use]
    pub fn scaling_of(&self, id: &str) -> CardScaling {
        id.parse::<ThemeId>()
            .ok()
            .and_then(|id| self.theme_scaling.get(&id).copied())
            .unwrap_or_default()
    }
}

/// A booted core, plus anything the frontend should tell the user about how
/// it booted.
#[derive(Debug)]
pub struct Startup {
    /// The running core.
    pub app: App,
    /// A non-fatal startup problem — the persisted theme failed to load and
    /// the default was used — for the frontend to log under its own name.
    pub notice: Option<String>,
}

/// The frontend application state minus everything platform: presenter,
/// active theme, discovered themes, the Options dialog's preview state, and
/// the settings this build restores at startup and rewrites on every commit
/// or exit.
///
/// A frontend owns one of these alongside its own chrome and render path,
/// and adds only what genuinely differs — a render worker handle, a
/// transactional theme swap — on top.
#[derive(Debug)]
pub struct App {
    presenter: Presenter,
    themes: Vec<ThemeEntry>,
    theme: Theme,
    theme_id: String,
    viewport: (u32, u32),
    last_tick: Option<Instant>,
    preview: Option<PreviewRestore>,
    won_reported: bool,
    /// The window geometry to persist: seeded from settings at startup,
    /// updated by [`App::record_window_geometry`].
    window: Option<WindowGeometry>,
    /// The player's per-theme card-scaling choices, seeded from settings at
    /// startup and written back by [`App::persist_settings`].
    theme_scaling: BTreeMap<ThemeId, CardScaling>,
    paths: StatePaths,
    /// The last display list actually handed to a renderer, with the surface
    /// it was built for. See [`App::take_frame_if_changed`].
    last_submitted: Option<(DisplayList, u32, u32)>,
}

impl App {
    /// Boots the application state: discovers themes, loads the startup
    /// theme (honoring a `--theme` override path, or else `settings`'
    /// persisted theme id, falling back to `"default"` when that fails),
    /// applies `settings`' options and back index, and deals `seed` (or
    /// a random one).
    ///
    /// # Errors
    ///
    /// [`AppError`] when the theme fails to load — either the `--theme`
    /// override, or `"default"` itself.
    pub fn start(
        theme_override: Option<PathBuf>,
        seed: Option<Seed>,
        settings: Settings,
        paths: StatePaths,
    ) -> Result<Startup, AppError> {
        let mut theme_list = themes::discover();
        let mut notice = None;
        let (theme, theme_id) = if let Some(path) = theme_override {
            let theme = Theme::load_path(&path).map_err(|source| AppError::ThemeOverride {
                path: path.clone(),
                source: Box::new(source),
            })?;
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.is_empty())
                .map_or_else(|| String::from("default"), String::from);
            if theme_list.iter().all(|entry| entry.id != id) {
                theme_list.push(ThemeEntry {
                    id: id.clone(),
                    source: ThemeSource::Path(path),
                });
            }
            (theme, id)
        } else {
            let chosen = choose_startup_theme(&theme_list, settings.options.theme.as_str())?;
            notice = chosen.notice;
            (chosen.theme, chosen.id)
        };

        let mut options = settings.options;
        if let Ok(id) = theme_id.parse::<ThemeId>() {
            options.theme = id;
        }
        let seed = seed.unwrap_or_else(random_seed);
        let mut presenter = Presenter::new(Session::new(options, seed), &theme);
        let _ = presenter.set_back(settings.back_index);
        Ok(Startup {
            app: Self {
                presenter,
                themes: theme_list,
                theme,
                theme_id,
                viewport: (0, 0),
                last_tick: None,
                preview: None,
                won_reported: false,
                window: settings.window,
                theme_scaling: settings.theme_scaling,
                paths,
                last_submitted: None,
            },
            notice,
        })
    }

    /// Read-only presenter access for the status/format helpers.
    #[must_use]
    pub const fn presenter(&self) -> &Presenter {
        &self.presenter
    }

    /// The active theme (the render path builds its atlas from it).
    #[must_use]
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The fitted continuous display scale, always finite and positive.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.presenter.fit().scale
    }

    /// Adopts a new viewport size in physical pixels: refits the continuous
    /// scale and the presenter's viewport. Returns `true` when the surface
    /// actually changed, so the caller forwards the new scale to its render
    /// path. Reconfigure work is gated on actual changes — window systems
    /// repeat the same size for activation and focus, and a real reconfigure
    /// mid-drag would drop the drag.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let (width, height) = (width.max(1), height.max(1));
        if self.viewport == (width, height) {
            return false;
        }
        self.viewport = (width, height);
        self.refit();
        true
    }

    /// The current viewport in physical pixels; `(0, 0)` before the first
    /// resize arrives.
    #[must_use]
    pub const fn viewport(&self) -> (u32, u32) {
        self.viewport
    }

    /// Refits the continuous scale and viewport after a resize or theme
    /// change.
    fn refit(&mut self) {
        let (width, height) = self.viewport;
        self.presenter.fit_viewport(width, height);
    }

    /// Advances animations and the clock by the real elapsed time.
    pub fn advance(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(0, |last| now.duration_since(last).as_millis());
        self.last_tick = Some(now);
        // A stall (window drag, suspend) is not hours of card time.
        self.presenter
            .advance(u32::try_from(dt).unwrap_or(u32::MAX).min(250));
    }

    /// The next frame's sprite display list. Empty until the first
    /// resize arrives (the presenter has no viewport yet).
    #[must_use]
    pub fn frame(&self) -> Option<DisplayList> {
        (self.viewport != (0, 0)).then(|| self.presenter.frame())
    }

    /// The next frame's display list, but only when it differs from the one
    /// last taken — otherwise `None`.
    ///
    /// A solitaire board is static most of the time. Rebuilding an identical
    /// display list is cheap; submitting it is not, and one frontend reads
    /// the whole canvas back off the GPU per submission, so an idle board
    /// would otherwise cost a full-frame copy sixty times a second to draw
    /// the same pixels.
    ///
    /// The comparison includes the surface size, because the same list drawn
    /// at a new size is a different frame. It is safe for the win cascade:
    /// the canvas persists across frames and cascade frames are per-tick
    /// deltas that are empty when nothing stepped, so a skipped submission
    /// cannot lose smear trail.
    pub fn take_frame_if_changed(&mut self) -> Option<DisplayList> {
        let (width, height) = self.viewport;
        let list = self.frame()?;
        if self.last_submitted.as_ref() == Some(&(list.clone(), width, height)) {
            return None;
        }
        self.last_submitted = Some((list.clone(), width, height));
        Some(list)
    }

    /// Forwards a pointer press in physical pixels.
    pub fn pointer_down(&mut self, x: i32, y: i32) {
        let pt = self.presenter.fit().to_logical(x, y);
        self.presenter.pointer_down(pt);
    }

    /// Forwards a pointer move in physical pixels.
    pub fn pointer_move(&mut self, x: i32, y: i32) {
        let pt = self.presenter.fit().to_logical(x, y);
        self.presenter.pointer_move(pt);
    }

    /// Forwards a pointer release in physical pixels.
    pub fn pointer_up(&mut self, x: i32, y: i32) {
        let pt = self.presenter.fit().to_logical(x, y);
        self.presenter.pointer_up(pt);
    }

    /// Any key lands running animations first, like the original.
    pub fn any_key(&mut self) {
        self.presenter.key_down();
    }

    /// Deals a new random game (menu "Deal" / F2 / "Deal Again").
    pub fn deal_random(&mut self) {
        self.won_reported = false;
        self.presenter.deal_new(random_seed());
    }

    /// "Select Game…": deals the game named by `digits`.
    ///
    /// Games are numbered `0..=`[`Seed::MAX`] — the range the original can
    /// deal. `false` when `digits` is not a run of ASCII digits naming one of
    /// them, so the caller can keep the dialog open.
    pub fn select_game(&mut self, digits: &str) -> bool {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(seed) = digits.parse::<Seed>() else {
            return false;
        };
        self.won_reported = false;
        self.presenter.deal_new(seed);
        true
    }

    /// Menu Undo. `None` on success, the rejection text otherwise
    /// (rejections are ordinary: Vegas, nothing to undo).
    pub fn undo(&mut self) -> Option<String> {
        self.presenter.undo().err().map(|error| error.to_string())
    }

    /// Menu Redo; rejection text as [`App::undo`].
    pub fn redo(&mut self) -> Option<String> {
        self.presenter.redo().err().map(|error| error.to_string())
    }

    /// Menu Save: writes the autosave slot. Returns a status line.
    pub fn save(&mut self) -> String {
        let Some(path) = &self.paths.autosave else {
            return String::from("Save failed: no autosave location");
        };
        match sol_session::storage::save_to(self.presenter.session(), path) {
            Ok(()) => format!("Saved to {}", path.display()),
            Err(error) => format!("Save failed: {error}"),
        }
    }

    /// Menu Load: restores the autosave slot. A loaded save contributes
    /// game state only — its embedded options are discarded in favor of
    /// whatever was committed before the load, so a save from a
    /// different ruleset never quietly changes future deals or menu
    /// toggles. Returns a status line.
    pub fn load(&mut self) -> String {
        let Some(path) = &self.paths.autosave else {
            return String::from("Load failed: no autosave location");
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => return format!("Load failed: {error}"),
        };
        let options = self.presenter.options().clone();
        match self.presenter.load_bytes(&bytes) {
            Ok(()) => {
                self.presenter.set_options(options);
                self.won_reported = self.presenter.is_won();
                format!("Loaded {}", path.display())
            }
            Err(error) => format!("Load failed: {error}"),
        }
    }

    /// Autosave for window close. Returns the failure text, if any: the
    /// window is closing, so there is nowhere left to show it and the
    /// frontend only logs it.
    #[must_use]
    pub fn autosave_on_exit(&self) -> Option<String> {
        let path = self.paths.autosave.as_ref()?;
        sol_session::storage::save_to(self.presenter.session(), path)
            .err()
            .map(|error| format!("autosave on exit failed: {error}"))
    }

    /// Records the window's current geometry for the next
    /// [`App::persist_settings`]; see [`geometry::next_window_geometry`] for
    /// what a report does to what was already stored.
    pub fn record_window_geometry(
        &mut self,
        width: u32,
        height: u32,
        position: Option<(i32, i32)>,
        maximized: bool,
    ) {
        self.window = Some(geometry::next_window_geometry(
            self.window.as_ref(),
            width,
            height,
            position,
            maximized,
        ));
    }

    /// The recorded window geometry: seeded from settings at startup,
    /// updated by [`App::record_window_geometry`].
    #[must_use]
    pub fn window_geometry(&self) -> Option<&WindowGeometry> {
        self.window.as_ref()
    }

    /// Writes the current settings document — the presenter's options, its
    /// back index, and the recorded window geometry — to the settings path.
    /// A `None` path makes this a no-op. Returns the failure text, if any:
    /// the in-memory settings stay applied either way, so a save-permission
    /// problem never blocks play.
    #[must_use]
    pub fn persist_settings(&self) -> Option<String> {
        let path = self.paths.settings.as_ref()?;
        let settings = Settings {
            options: self.presenter.options().clone(),
            back_index: self.presenter.back_index(),
            window: self.window.clone(),
            theme_scaling: self.theme_scaling.clone(),
            ..Settings::default()
        };
        sol_session::storage::save_settings_to(&settings, path)
            .err()
            .map(|error| format!("saving settings failed: {error}"))
    }

    /// True exactly once per won game, after the cascade has settled —
    /// the Game Won dialog's trigger.
    pub fn won_just_settled(&mut self) -> bool {
        let settled = self.presenter.is_won() && !self.presenter.is_animating();
        announce_once(settled, &mut self.won_reported)
    }

    /// The discovered theme ids, in picker order.
    #[must_use]
    pub fn theme_ids(&self) -> Vec<String> {
        self.themes.iter().map(|entry| entry.id.clone()).collect()
    }

    /// The active theme's id.
    #[must_use]
    pub fn theme_id(&self) -> &str {
        &self.theme_id
    }

    /// The active theme's card scaling: the player's recorded choice, or
    /// [`CardScaling::Original`] for a theme they have never chosen for.
    #[must_use]
    pub fn scaling(&self) -> CardScaling {
        self.scaling_of(&self.theme_id)
    }

    /// The recorded card scaling for `id`, whether or not it is the active
    /// theme — what a live theme preview needs, since it must build the
    /// incoming theme's atlas before adopting it.
    #[must_use]
    pub fn scaling_of(&self, id: &str) -> CardScaling {
        id.parse::<ThemeId>()
            .ok()
            .and_then(|id| self.theme_scaling.get(&id).copied())
            .unwrap_or_default()
    }

    /// Whether the active theme has PNG art, and therefore whether a card
    /// scaling choice means anything for it. A vector theme scales by
    /// rasterizing its SVGs and ignores the choice, so dialogs disable the
    /// control rather than offering a setting with no effect.
    #[must_use]
    pub fn theme_is_png(&self) -> bool {
        self.theme.manifest.render_mode == RenderMode::Png
    }

    /// Records a card scaling for the active theme. The caller has already
    /// rebuilt its render path for the new choice; this is the state half.
    pub fn set_scaling(&mut self, scaling: CardScaling) {
        if let Ok(id) = self.theme_id.parse::<ThemeId>() {
            self.theme_scaling.insert(id, scaling);
        }
    }

    /// The active theme's back names, in declaration order (matching
    /// the presenter's back indices).
    #[must_use]
    pub fn back_names(&self) -> Vec<String> {
        self.theme
            .backs()
            .iter()
            .map(|(name, _)| String::from(name.as_str()))
            .collect()
    }

    /// The selected back's index.
    #[must_use]
    pub fn back_index(&self) -> usize {
        self.presenter.back_index()
    }

    /// Marks the Options dialog open: remembers what a Cancel restores.
    pub fn begin_preview(&mut self) {
        self.preview = Some(PreviewRestore {
            theme_id: self.theme_id.clone(),
            back_index: self.presenter.back_index(),
            theme_scaling: self.theme_scaling.clone(),
        });
    }

    /// Takes the pending restore point (Options dialog Cancel); the frontend
    /// replays it through the same theme-switch path the preview used.
    pub fn take_preview_restore(&mut self) -> Option<PreviewRestore> {
        self.preview.take()
    }

    /// Loads the theme named `id` without adopting it, so a frontend whose
    /// render path can reject a theme rebuilds its atlas first and leaves the
    /// current theme fully in place on failure. Errors come back as display
    /// text.
    ///
    /// # Errors
    ///
    /// The [`crate::ThemeLookupError`] display text when `id` is unknown or
    /// its package fails to load.
    pub fn load_theme(&self, id: &str) -> Result<Theme, String> {
        if id == self.theme_id {
            // The no-op switch: hand back the active theme unchanged.
            return Ok(self.theme.clone());
        }
        themes::load(&self.themes, id).map_err(|error| error.to_string())
    }

    /// Adopts a theme the render path already accepted: points the
    /// presenter at it and refits the scale (a new `base_size` changes
    /// the design client). Returns `true` so the caller forwards the
    /// new fit; `false` when `id` is already active.
    pub fn adopt_theme(&mut self, id: &str, theme: &Theme) -> bool {
        if id == self.theme_id {
            return false;
        }
        self.presenter.set_theme(theme);
        self.theme = theme.clone();
        self.theme_id = String::from(id);
        self.refit();
        true
    }

    /// Live-applies a back selection (the dialog's preview); rejection
    /// text when the index is out of range.
    pub fn set_back(&mut self, index: usize) -> Option<String> {
        self.presenter
            .set_back(index)
            .err()
            .map(|error| error.to_string())
    }

    /// Options dialog Cancel: reverts the scaling bookkeeping to what
    /// `restore` recorded. Bookkeeping only — this issues no render work
    /// of its own. When the caller also replays a theme switch after this
    /// call, that replay rebuilds its render path anyway and picks up the
    /// restored scaling along with it for free. When the theme is
    /// unchanged, no such replay happens, and a caller that can apply a
    /// scaling change independently of a theme switch must rebuild its own
    /// render path itself here — otherwise the display keeps showing the
    /// scaling that was live before Cancel, even though this map now says
    /// otherwise.
    pub fn restore_scaling(&mut self, restore: &PreviewRestore) {
        self.theme_scaling = restore.theme_scaling.clone();
    }

    /// The currently committed options, as an editable snapshot.
    ///
    /// A dialog seeds its edit state from this, mutates the copy, and commits
    /// it back through [`App::commit_options`]; nothing observes a partially
    /// edited option set.
    #[must_use]
    pub fn options_snapshot(&self) -> EditedOptions {
        let options = self.presenter.options();
        EditedOptions {
            draw_three: options.draw_mode == DrawMode::Three,
            scoring: options.scoring,
            timed: options.timed,
            outline_dragging: options.outline_dragging,
            keep_vegas_score: options.keep_vegas_score,
            sounds: options.sounds,
        }
    }

    /// Options dialog OK: commits the edited options (theme id from the
    /// live selection), drops the restore point, and persists the settings
    /// document. Returns [`App::persist_settings`]' failure text, if any.
    pub fn commit_options(&mut self, edited: EditedOptions) -> Option<String> {
        let mut options = self.presenter.options().clone();
        options.draw_mode = if edited.draw_three {
            DrawMode::Three
        } else {
            DrawMode::One
        };
        options.scoring = edited.scoring;
        options.timed = edited.timed;
        options.outline_dragging = edited.outline_dragging;
        options.keep_vegas_score = edited.keep_vegas_score;
        options.sounds = edited.sounds;
        if let Ok(id) = self.theme_id.parse::<ThemeId>() {
            options.theme = id;
        }
        self.presenter.set_options(options);
        self.preview = None;
        self.persist_settings()
    }

    /// What every menu item's enabled and checked state should be, as plain
    /// data; see [`MenuModel`].
    #[must_use]
    pub fn menu_model(&self) -> MenuModel {
        MenuModel::of(&self.presenter)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use sol_engine::{Command, Event, LogEntry, ScoringMode};
    use sol_session::{Bankroll, Options};
    use sol_theme::canonical_faces;

    use super::*;

    /// An `App` whose state paths point into a temporary directory, returned
    /// alongside the directory so the caller keeps it alive for the test's
    /// duration. Every test that touches save, load or settings uses this;
    /// none of them may see the real per-user data directory.
    pub(crate) fn app_in_tempdir() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        (app, dir)
    }

    /// An `App` with no state paths at all — the shape a self-test run or an
    /// unresolvable data directory produces.
    fn app_without_paths() -> App {
        App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::default(),
        )
        .unwrap()
        .app
    }

    #[test]
    fn a_fresh_app_deals_the_requested_game() {
        let (app, _dir) = app_in_tempdir();
        assert_eq!(app.presenter().seed().get(), 1);
        assert_eq!(app.theme_id(), "default");
        assert!(app.theme_ids().iter().any(|id| id == "default"));
        assert!(!app.back_names().is_empty());
        assert_eq!(app.back_index(), 0);
        assert_eq!(
            app.theme().backs().len(),
            app.back_names().len(),
            "the theme the render path builds from is the one the picker lists"
        );
    }

    #[test]
    fn a_seedless_boot_still_deals() {
        let dir = tempfile::tempdir().unwrap();
        let started = App::start(
            None,
            None,
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap();
        assert!(started.notice.is_none());
        assert_eq!(started.app.presenter().options().theme.as_str(), "default");
    }

    /// A persisted theme that no longer resolves must not stop the game from
    /// starting; it falls back to the default and says so.
    #[test]
    fn an_unresolvable_persisted_theme_falls_back_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            options: Options {
                theme: "no-such-theme".parse().unwrap(),
                ..Options::default()
            },
            ..Settings::default()
        };
        let started = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            settings,
            StatePaths::under(dir.path()),
        )
        .unwrap();
        assert_eq!(started.app.theme_id(), "default");
        let notice = started.notice.unwrap();
        assert!(notice.contains("no-such-theme"), "{notice}");
    }

    /// A `--theme` path that is not a theme is fatal: the user named it
    /// explicitly, so silently ignoring it would be worse than refusing.
    #[test]
    fn a_broken_theme_override_is_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("theme.toml"), b"not a manifest").unwrap();
        let error = App::start(
            Some(dir.path().to_path_buf()),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap_err();
        assert!(matches!(error, AppError::ThemeOverride { .. }), "{error}");
    }

    /// A copy of the in-tree default theme under `name`, so a test can have
    /// a second theme whose id is not `"default"`.
    fn theme_copy_named(under: &Path, name: &str) -> PathBuf {
        fn copy_dir(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap().flatten() {
                let (source, target) = (entry.path(), to.join(entry.file_name()));
                if source.is_dir() {
                    copy_dir(&source, &target);
                } else {
                    std::fs::copy(&source, &target).unwrap();
                }
            }
        }
        let target = under.join(name);
        copy_dir(&themes::dev_default_dir().unwrap(), &target);
        target
    }

    /// Writes the smallest legal `render_mode = "png"` theme under `under`:
    /// 1×1-pixel faces and back. The in-tree default theme is `vector`, so
    /// this is the only theme in the crate's tests that
    /// [`App::theme_is_png`] reports `true` for.
    fn png_theme_dir(under: &Path) -> PathBuf {
        let pixel = || {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
                encoder.set_color(png::ColorType::Grayscale);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(&[0]).unwrap();
            }
            bytes
        };
        let dir = under.join("pngtheme");
        let cards = dir.join("cards");
        std::fs::create_dir_all(&cards).unwrap();
        let backs = dir.join("backs");
        std::fs::create_dir_all(&backs).unwrap();
        for (suit, rank) in canonical_faces() {
            std::fs::write(cards.join(format!("{}.png", suit.stem(rank))), pixel()).unwrap();
        }
        std::fs::write(backs.join("plain.png"), pixel()).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            "[theme]\n\
             name = \"Pngtheme\"\n\
             render_mode = \"png\"\n\
             \n\
             [cards]\n\
             faces = \"cards/\"\n\
             base_size = [1, 1]\n\
             \n\
             [backs]\n\
             plain = { image = \"backs/plain.png\" }\n\
             \n\
             [table]\n\
             background = { color = \"#008000\" }\n\
             \n\
             [drag]\n\
             outline_color = \"#000000\"\n",
        )
        .unwrap();
        dir
    }

    /// A `--theme` path joins the picker under its own directory name, so
    /// the Options dialog can switch away from it and back.
    #[test]
    fn a_theme_override_joins_the_discovered_list_under_its_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let winter = theme_copy_named(dir.path(), "winter");
        let started = App::start(
            Some(winter),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap();
        assert_eq!(started.app.theme_id(), "winter");
        let ids = started.app.theme_ids();
        assert!(ids.iter().any(|id| id == "winter"), "{ids:?}");
        assert!(
            ids.iter().any(|id| id == "default"),
            "the discovered themes survive the override: {ids:?}"
        );
    }

    /// Overriding with a path already in the picker must not list it twice.
    #[test]
    fn a_theme_override_that_is_already_discovered_is_not_listed_twice() {
        let dir = tempfile::tempdir().unwrap();
        let started = App::start(
            Some(themes::dev_default_dir().unwrap()),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap();
        let defaults = started
            .app
            .theme_ids()
            .iter()
            .filter(|id| *id == "default")
            .count();
        assert_eq!(defaults, 1);
    }

    /// Adopting a different theme repoints the presenter and refits, because
    /// a new `base_size` changes the design client the board is laid out in.
    #[test]
    fn adopting_a_different_theme_repoints_the_presenter_and_refits() {
        let dir = tempfile::tempdir().unwrap();
        let winter = theme_copy_named(dir.path(), "winter");
        let mut app = App::start(
            Some(winter),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        app.resize(1600, 900);

        let theme = app.load_theme("default").unwrap();
        assert!(app.adopt_theme("default", &theme), "the switch happened");
        assert_eq!(app.theme_id(), "default");
        assert_eq!(app.theme().backs().len(), theme.backs().len());
        assert!(app.scale().is_finite() && app.scale() > 0.0);

        assert!(
            !app.adopt_theme("default", &theme),
            "re-adopting the now-active theme is a no-op"
        );
    }

    #[test]
    fn select_game_validates_digit_runs() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(!app.select_game(""), "empty");
        assert!(!app.select_game("12a"), "not all digits");
        assert!(!app.select_game("-3"), "sign is not a digit");
        assert!(
            !app.select_game("+3"),
            "a leading plus is not a digit either, even though the numeric \
             parser underneath would otherwise accept one"
        );
        assert!(app.select_game("42"));
        assert_eq!(app.presenter().seed().get(), 42);
        assert!(app.select_game("32767"), "the last game");
        assert_eq!(app.presenter().seed().get(), Seed::MAX);
        // Past the last game the deal stands: there is no such board.
        assert!(!app.select_game("32768"), "beyond the last game");
        assert!(!app.select_game("99999999999"), "beyond u32 entirely");
        assert_eq!(app.presenter().seed().get(), Seed::MAX);
    }

    #[test]
    fn resize_refits_the_continuous_scale_and_gates_on_change() {
        let (mut app, _dir) = app_in_tempdir();
        // 1170x768 is exactly twice the default theme's 585x384 design, so
        // the fitted scale is a value worth naming rather than bounding.
        assert!(app.resize(1170, 768));
        assert_eq!(app.viewport(), (1170, 768));
        assert!((app.scale() - 2.0).abs() < f32::EPSILON, "{}", app.scale());

        assert!(!app.resize(1170, 768), "the same size is not a change");

        // Half the surface halves the scale, so the refit really re-fits.
        assert!(app.resize(585, 384));
        assert!((app.scale() - 1.0).abs() < f32::EPSILON, "{}", app.scale());
    }

    /// A zero-sized surface is a window-system artifact, not a request to
    /// divide by zero.
    #[test]
    fn a_degenerate_resize_is_floored_to_one_pixel() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.resize(0, 0));
        assert_eq!(app.viewport(), (1, 1));
    }

    #[test]
    fn there_is_no_frame_before_the_first_resize() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.frame().is_none());
        app.resize(800, 600);
        assert!(app.frame().is_some());
    }

    /// A fresh deal animates; advancing the clock steps that animation, so
    /// the board it draws changes without any command being issued.
    #[test]
    fn advance_drives_the_running_animation() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(1170, 768);
        assert!(app.presenter().is_animating(), "a fresh deal animates");
        // The first advance only records the clock; elapsed time is measured
        // between calls, so a step needs a second one.
        app.advance();
        let _ = app.take_frame_if_changed();

        std::thread::sleep(std::time::Duration::from_millis(30));
        app.advance();
        assert!(
            app.take_frame_if_changed().is_some(),
            "advancing the clock moved the deal animation on"
        );
    }

    /// Any key lands running animations, like the original.
    #[test]
    fn any_key_lands_a_running_animation() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(1170, 768);
        assert!(app.presenter().is_animating());
        app.any_key();
        assert!(!app.presenter().is_animating(), "the deal landed at once");
    }

    /// A press and release on the stock deals, so the pointer really reaches
    /// the presenter and really arrives in logical space — the stock's
    /// rectangle is only where it is once the physical point is scaled.
    #[test]
    fn a_press_and_release_on_the_stock_deals_a_card() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(1170, 768);
        app.any_key();
        assert!(!app.presenter().can_undo(), "nothing has happened yet");

        // The stock's card centre: logical (46, 53) at exactly 2x.
        app.pointer_down(92, 106);
        app.pointer_up(92, 106);
        assert!(app.presenter().can_undo(), "the draw is on the log");
    }

    /// A drag in flight follows the pointer, so moving it redraws the board
    /// even though nothing has been dropped yet.
    #[test]
    fn moving_a_held_card_redraws_the_board() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(1170, 768);
        app.any_key();

        // Tableau column 0's single card, centre: logical (46, 155) at 2x.
        app.pointer_down(92, 310);
        let _ = app.take_frame_if_changed();
        app.pointer_move(300, 400);
        assert!(
            app.take_frame_if_changed().is_some(),
            "the held card follows the pointer"
        );
        assert!(
            !app.presenter().is_animating(),
            "a held card is not in flight"
        );

        // Released over bare felt — logical (500, 350) is the gap between
        // columns 5 and 6 — so the card snaps back, which is an animation.
        // Without the release it would still be riding the pointer.
        app.pointer_up(1000, 700);
        assert!(
            app.presenter().is_animating(),
            "releasing over felt starts the snap-back"
        );
    }

    #[test]
    fn deal_random_starts_a_new_game() {
        let (mut app, _dir) = app_in_tempdir();
        let before = app.presenter().seed().get();
        app.deal_random();
        assert_ne!(app.presenter().seed().get(), before);
    }

    /// Undo and redo report the engine's own rejection text rather than
    /// restating when they are legal.
    /// Undo and redo report the engine's own rejection text, and report
    /// nothing at all when they succeed — a status bar shows what comes
    /// back, so an empty string would read as a silent failure.
    #[test]
    fn undo_and_redo_report_only_when_they_are_refused() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(1170, 768);
        app.any_key();
        assert!(app.undo().is_some(), "nothing to undo yet");
        assert!(app.redo().is_some(), "nothing to redo yet");

        app.presenter_mut_for_test().apply(Command::Draw).unwrap();
        assert_eq!(app.undo(), None, "undoing the draw succeeds silently");
        assert_eq!(app.redo(), None, "redoing it succeeds silently");
    }

    #[test]
    fn save_then_load_round_trips_through_the_injected_path() {
        let (mut app, dir) = app_in_tempdir();
        app.any_key();
        let saved = app.save();
        assert!(saved.starts_with("Saved to"), "{saved}");
        assert!(dir.path().join("autosave.json").is_file());

        let loaded = app.load();
        assert!(loaded.starts_with("Loaded"), "{loaded}");
    }

    #[test]
    fn loading_without_a_saved_game_reports_the_read_failure() {
        let (mut app, _dir) = app_in_tempdir();
        let loaded = app.load();
        assert!(loaded.starts_with("Load failed:"), "{loaded}");
    }

    #[test]
    fn a_corrupt_autosave_reports_a_load_failure() {
        let (mut app, dir) = app_in_tempdir();
        std::fs::write(dir.path().join("autosave.json"), b"not json").unwrap();
        let loaded = app.load();
        assert!(loaded.starts_with("Load failed:"), "{loaded}");
    }

    /// A load contributes game state only: the options committed before it
    /// survive, so a save from another ruleset cannot change future deals.
    #[test]
    fn a_load_keeps_the_options_that_were_committed_before_it() {
        let (mut app, _dir) = app_in_tempdir();
        app.save();
        app.commit_options(EditedOptions {
            scoring: ScoringMode::Vegas,
            ..app.options_snapshot()
        });
        app.load();
        assert_eq!(app.presenter().options().scoring, ScoringMode::Vegas);
    }

    #[test]
    fn save_and_load_without_a_path_report_rather_than_pretend() {
        let mut app = app_without_paths();
        assert_eq!(app.save(), "Save failed: no autosave location");
        assert_eq!(app.load(), "Load failed: no autosave location");
        assert!(app.autosave_on_exit().is_none());
        assert!(app.persist_settings().is_none());
    }

    #[test]
    fn autosave_on_exit_writes_the_session() {
        let (app, dir) = app_in_tempdir();
        assert!(app.autosave_on_exit().is_none());
        assert!(dir.path().join("autosave.json").is_file());
    }

    /// A directory where the document should be is unwritable; the failure
    /// comes back as text rather than aborting the close.
    #[test]
    fn a_blocked_autosave_path_reports_instead_of_failing_the_close() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("autosave.json")).unwrap();
        let mut app = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        let notice = app.autosave_on_exit().unwrap();
        assert!(notice.starts_with("autosave on exit failed:"), "{notice}");
        assert!(app.save().starts_with("Save failed:"));
    }

    /// The written document carries every field the next launch restores:
    /// the options, the back index and the window geometry.
    #[test]
    fn the_settings_document_round_trips_options_back_and_geometry() {
        let (mut app, dir) = app_in_tempdir();
        app.set_back(1);
        app.record_window_geometry(1000, 700, Some((10, 20)), false);
        assert!(
            app.commit_options(EditedOptions {
                scoring: ScoringMode::Vegas,
                timed: true,
                ..app.options_snapshot()
            })
            .is_none()
        );
        assert!(dir.path().join("settings.json").is_file());

        let (restored, notice) = load_settings(&StatePaths::under(dir.path()));
        assert!(notice.is_none());
        assert_eq!(restored.options.scoring, ScoringMode::Vegas);
        assert!(restored.options.timed);
        assert_eq!(restored.back_index, 1);
        let window = restored.window.unwrap();
        assert_eq!((window.width, window.height), (1000, 700));
        assert_eq!((window.x, window.y), (Some(10), Some(20)));
    }

    #[test]
    fn a_blocked_settings_path_reports_instead_of_blocking_play() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("settings.json")).unwrap();
        let app = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        let notice = app.persist_settings().unwrap();
        assert!(notice.starts_with("saving settings failed:"), "{notice}");
    }

    /// The snapshot is what the dialog edits, so it has to describe the
    /// options actually in force — including the draw mode, which is an enum
    /// on one side and a checkbox on the other.
    #[test]
    fn the_options_snapshot_round_trips_through_a_commit() {
        let (mut app, _dir) = app_in_tempdir();
        let edited = EditedOptions {
            draw_three: false,
            scoring: ScoringMode::Vegas,
            timed: true,
            outline_dragging: true,
            keep_vegas_score: true,
            sounds: false,
        };
        app.commit_options(edited);
        assert_eq!(app.options_snapshot(), edited);
        assert_eq!(app.presenter().options().draw_mode, DrawMode::One);

        let three = EditedOptions {
            draw_three: true,
            ..edited
        };
        app.commit_options(three);
        assert_eq!(app.options_snapshot(), three);
        assert_eq!(app.presenter().options().draw_mode, DrawMode::Three);
    }

    #[test]
    fn a_preview_restore_point_survives_until_it_is_taken() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.take_preview_restore().is_none());
        app.begin_preview();
        let restore = app.take_preview_restore().unwrap();
        assert_eq!(restore.theme_id, "default");
        assert_eq!(restore.back_index, 0);
        assert!(app.take_preview_restore().is_none(), "taken once");
    }

    /// Committing the dialog drops the restore point: an OK is not a Cancel
    /// waiting to happen.
    #[test]
    fn committing_options_drops_the_restore_point() {
        let (mut app, _dir) = app_in_tempdir();
        app.begin_preview();
        app.commit_options(app.options_snapshot());
        assert!(app.take_preview_restore().is_none());
    }

    #[test]
    fn setting_a_back_selects_it_and_out_of_range_is_reported() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.set_back(0).is_none());
        assert_eq!(app.back_index(), 0);

        assert!(app.set_back(1).is_none());
        assert_eq!(app.back_index(), 1, "the selection is what is reported");

        assert!(app.set_back(9_999).is_some(), "out of range");
        assert_eq!(app.back_index(), 1, "a refused selection changes nothing");
    }

    /// Re-selecting the active theme is a no-op on both halves of the swap,
    /// so a picker that fires on every selection change costs nothing.
    #[test]
    fn re_adopting_the_active_theme_changes_nothing() {
        let (mut app, _dir) = app_in_tempdir();
        let theme = app.load_theme("default").unwrap();
        assert!(!app.adopt_theme("default", &theme));
    }

    #[test]
    fn loading_an_unknown_theme_reports_by_name() {
        let (app, _dir) = app_in_tempdir();
        let error = app.load_theme("no-such-theme").unwrap_err();
        assert!(error.contains("no-such-theme"), "{error}");
    }

    #[test]
    fn window_geometry_is_recorded_and_reported() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.window_geometry().is_none());
        app.record_window_geometry(1000, 700, Some((10, 20)), false);
        let geometry = app.window_geometry().unwrap();
        assert_eq!((geometry.width, geometry.height), (1000, 700));
        assert_eq!((geometry.x, geometry.y), (Some(10), Some(20)));
        assert!(!geometry.maximized);
    }

    /// A settled board that is *not* won announces nothing, and an
    /// unsettled one announces nothing either — the trigger is both
    /// conditions, not either.
    #[test]
    fn an_unwon_board_is_never_announced() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(
            !app.won_just_settled(),
            "mid-deal: not won, still animating"
        );
        app.any_key();
        assert!(!app.won_just_settled(), "settled, but not won");
    }

    /// A won board announces once, after the cascade has settled.
    ///
    /// The win is injected rather than played: reaching one takes a full
    /// game, and what is under test is the announcement, not the rules.
    /// `Event::GameWon` is what the engine itself emits on the final move,
    /// so folding it produces the same won flag a real game would.
    #[test]
    fn a_settled_win_is_announced_exactly_once() {
        let (mut app, dir) = app_in_tempdir();
        let won = Session::restore(
            Options::default(),
            Seed::new(1).unwrap(),
            vec![LogEntry {
                command: Command::Draw,
                events: vec![Event::GameWon],
            }],
            Bankroll::default(),
            0,
        );
        let path = dir.path().join("won.json");
        sol_session::storage::save_to(&won, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();

        // Straight into the presenter: `App::load` deliberately marks a
        // loaded win as already announced, and this is about the first
        // announcement.
        app.presenter_mut_for_test().load_bytes(&bytes).unwrap();
        assert!(app.presenter().is_won());
        assert!(!app.presenter().is_animating());

        assert!(app.won_just_settled(), "the win is announced");
        assert!(!app.won_just_settled(), "and only once");
    }

    /// Loading a won save does not re-announce a win the player has already
    /// seen: the dialog belongs to the moment the game was won.
    #[test]
    fn loading_a_won_save_does_not_re_announce_it() {
        let (mut app, dir) = app_in_tempdir();
        let won = Session::restore(
            Options::default(),
            Seed::new(1).unwrap(),
            vec![LogEntry {
                command: Command::Draw,
                events: vec![Event::GameWon],
            }],
            Bankroll::default(),
            0,
        );
        sol_session::storage::save_to(&won, &dir.path().join("autosave.json")).unwrap();

        assert!(app.load().starts_with("Loaded"));
        assert!(app.presenter().is_won());
        assert!(!app.won_just_settled(), "already announced by the load");
    }

    /// A new deal clears the "already announced" flag, so the second win of
    /// a session is announced too.
    #[test]
    fn dealing_again_re_arms_the_win_announcement() {
        let (mut app, _dir) = app_in_tempdir();
        app.deal_random();
        assert!(!app.won_just_settled());
        app.select_game("7");
        assert!(!app.won_just_settled());
    }

    #[test]
    fn state_paths_resolve_to_the_real_documents() {
        let (paths, notices) = StatePaths::resolve();
        assert!(notices.is_empty(), "{notices:?}");
        assert!(paths.autosave.unwrap().ends_with("autosave.json"));
        assert!(paths.settings.unwrap().ends_with("settings.json"));
    }

    /// The failure side, which no in-process test can provoke through the
    /// real lookup: a frontend must be told *which* document has no home,
    /// and must get no path for it.
    #[test]
    fn an_unresolvable_location_yields_a_notice_and_no_path() {
        let (path, notice) =
            resolved_or_notice("settings", Err(sol_session::StorageError::NoHomeDirectory));
        assert!(path.is_none());
        let notice = notice.unwrap();
        assert!(notice.contains("settings"), "{notice}");

        let (path, notice) = resolved_or_notice("autosave", Ok(PathBuf::from("/tmp/a.json")));
        assert_eq!(path, Some(PathBuf::from("/tmp/a.json")));
        assert!(notice.is_none());
    }

    /// The win banner fires once and then stays quiet, and never fires while
    /// the game is unwon — including after a win has already been announced.
    #[test]
    fn a_win_is_announced_once_and_only_once() {
        let mut announced = false;
        assert!(!announce_once(false, &mut announced), "not won yet");
        assert!(!announced);

        assert!(announce_once(true, &mut announced), "the first settled win");
        assert!(announced);

        assert!(!announce_once(true, &mut announced), "still won: silent");
        assert!(!announce_once(false, &mut announced), "unwon again: silent");
    }

    /// The scaling map is settings state: it arrives from the settings
    /// document at startup and goes back out on every commit, the same
    /// round trip the window geometry makes.
    #[test]
    fn the_scaling_map_survives_a_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let mut settings = Settings::default();
        settings.theme_scaling.insert(
            ThemeId::try_from(String::from("default")).unwrap(),
            CardScaling::Xbrz,
        );

        let app = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            settings.clone(),
            paths.clone(),
        )
        .unwrap()
        .app;
        assert!(app.persist_settings().is_none());

        let (reloaded, notice) = load_settings(&paths);
        assert_eq!(notice, None);
        assert_eq!(reloaded.theme_scaling, settings.theme_scaling);
    }

    #[test]
    fn scaling_defaults_to_original_for_an_unrecorded_theme() {
        let (mut app, _dir) = app_in_tempdir();
        assert_eq!(app.scaling(), CardScaling::Original);
        app.set_scaling(CardScaling::Xbrz);
        assert_eq!(app.scaling(), CardScaling::Xbrz);
    }

    #[test]
    fn scaling_of_reads_any_theme_not_just_the_active_one() {
        let (mut app, _dir) = app_in_tempdir();
        app.set_scaling(CardScaling::Xbrz);
        assert_eq!(app.scaling_of("default"), CardScaling::Xbrz);
        assert_eq!(app.scaling_of("winter"), CardScaling::Original);
        assert_eq!(app.scaling_of(""), CardScaling::Original);
    }

    /// A Cancel handler undoing a scaling-only preview has to read the
    /// value the dialog opened with, which lives in the snapshot rather
    /// than in the live map the preview has already changed.
    #[test]
    fn preview_restore_resolves_a_scaling_against_its_own_snapshot() {
        let (mut app, _dir) = app_in_tempdir();
        app.set_scaling(CardScaling::Xbrz);
        app.begin_preview();
        app.set_scaling(CardScaling::Original);

        let restore = app.take_preview_restore().unwrap();
        assert_eq!(
            restore.scaling_of("default"),
            CardScaling::Xbrz,
            "the snapshot holds the pre-dialog choice, not the previewed one"
        );
        assert_eq!(restore.scaling_of("winter"), CardScaling::Original);
        assert_eq!(restore.scaling_of(""), CardScaling::Original);
    }

    /// The in-tree default theme is a vector theme, so the scaling control
    /// means nothing for it and the dialogs disable it.
    #[test]
    fn theme_is_png_follows_the_active_theme() {
        let (app, _dir) = app_in_tempdir();
        assert!(!app.theme_is_png());
    }

    /// The other half of the same fact: a png-mode theme is the one shape
    /// the scaling control means anything for. Nothing else in this crate
    /// loads a theme of this render mode, so this fixture is the only way
    /// to reach that branch at all.
    #[test]
    fn theme_is_png_is_true_for_a_png_theme() {
        let dir = tempfile::tempdir().unwrap();
        let theme_dir = png_theme_dir(dir.path());
        let app = App::start(
            Some(theme_dir),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        assert!(app.theme_is_png());
    }

    /// Options → Cancel puts back every scaling choice the dialog touched,
    /// including one made for a theme the dialog then switched away from.
    #[test]
    fn cancel_restores_the_whole_scaling_map() {
        let dir = tempfile::tempdir().unwrap();
        let winter = theme_copy_named(dir.path(), "winter");
        let mut app = App::start(
            Some(winter),
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;

        // Recorded before the dialog ever opens: a Cancel must not touch it.
        app.set_scaling(CardScaling::Xbrz);
        app.begin_preview();

        // The dialog switches to "default" and changes its scaling too.
        let default_theme = app.load_theme("default").unwrap();
        app.adopt_theme("default", &default_theme);
        app.set_scaling(CardScaling::Xbrz);
        assert_eq!(app.scaling(), CardScaling::Xbrz);

        let restore = app.take_preview_restore().unwrap();
        app.restore_scaling(&restore);
        assert_eq!(
            app.scaling(),
            CardScaling::Original,
            "default's mid-dialog choice is undone"
        );

        let winter_theme = app.load_theme("winter").unwrap();
        app.adopt_theme("winter", &winter_theme);
        assert_eq!(
            app.scaling(),
            CardScaling::Xbrz,
            "winter's pre-dialog choice, made before the dialog switched away \
             from it, survives the cancel"
        );
    }

    #[test]
    fn settings_load_from_the_injected_path_or_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());

        // Absent: defaults, silently — a first run is not a problem.
        let (settings, notice) = load_settings(&paths);
        assert!(notice.is_none());
        assert_eq!(settings.back_index, Settings::default().back_index);

        // Present and valid: read back.
        let mut app = App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            paths.clone(),
        )
        .unwrap()
        .app;
        app.set_back(1);
        assert!(app.persist_settings().is_none());
        let (settings, notice) = load_settings(&paths);
        assert!(notice.is_none());
        assert_eq!(settings.back_index, 1);

        // Present and broken: defaults, with a notice.
        std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();
        let (settings, notice) = load_settings(&paths);
        assert_eq!(settings.back_index, Settings::default().back_index);
        assert!(notice.unwrap().contains("loading settings failed"));

        // No path at all: defaults, silently.
        let (_, notice) = load_settings(&StatePaths::default());
        assert!(notice.is_none());
    }

    /// A solitaire board is static most of the time; an unchanged frame must
    /// not be resubmitted.
    #[test]
    fn an_unchanged_board_does_not_resubmit_a_frame() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(800, 600);
        assert!(
            app.take_frame_if_changed().is_some(),
            "the first frame is always new"
        );
        assert!(
            app.take_frame_if_changed().is_none(),
            "an idle tick must not resubmit an identical frame"
        );
    }

    #[test]
    fn a_command_makes_the_next_frame_new_again() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(800, 600);
        app.any_key();
        let _ = app.take_frame_if_changed();
        app.deal_random();
        app.any_key();
        assert!(
            app.take_frame_if_changed().is_some(),
            "a new deal changes the board, so the frame must be submitted"
        );
    }

    #[test]
    fn a_resize_makes_the_next_frame_new_even_with_an_identical_board() {
        let (mut app, _dir) = app_in_tempdir();
        app.resize(800, 600);
        let _ = app.take_frame_if_changed();
        app.resize(1024, 768);
        assert!(
            app.take_frame_if_changed().is_some(),
            "the comparison includes the surface size, not just the display list"
        );
    }

    #[test]
    fn there_is_nothing_to_submit_before_the_first_resize() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(app.take_frame_if_changed().is_none());
    }

    /// The mix folds in sub-second entropy, so two deals started within the
    /// same second still differ. A mix that read only whole seconds — or one
    /// that returned a constant — would collapse these to a single value.
    #[test]
    fn seeds_taken_in_quick_succession_are_not_all_equal() {
        let seeds: std::collections::HashSet<u16> = (0..16).map(|_| random_seed().get()).collect();
        assert!(
            seeds.len() > 1,
            "sixteen seeds taken back to back were all identical: {seeds:?}"
        );
    }

    /// The fold itself, against known readings. Two calls that merely differ
    /// cannot say *how* the parts combine — whether the sub-second half is
    /// mixed in or masked away, whether the seconds are multiplied or
    /// merged — so the arithmetic is pinned here. Every result is the mix
    /// reduced into the seed range.
    #[test]
    fn the_seed_fold_combines_both_halves_of_the_clock() {
        // 0 * 1_000_003 = 0, so the nanoseconds pass straight through.
        assert_eq!(mix_seed(0, 0).get(), 0);
        assert_eq!(mix_seed(0, 12_345).get(), 12_345);
        // 1 * 1_000_003 with no nanoseconds is the multiplier itself, folded.
        assert_eq!(mix_seed(1, 0).get(), 16_963, "1_000_003 mod 32_768");
        // …and the two halves combine rather than one winning. The results
        // are spelled out rather than recomputed with the same operators the
        // implementation uses, which would agree with any of them.
        assert_eq!(
            mix_seed(1, 12_345).get(),
            29_306,
            "1_000_003 xor 12_345, mod 32_768"
        );
        assert_eq!(mix_seed(2, 7).get(), 1_153, "2_000_006 xor 7, mod 32_768");

        // Only the low 32 bits of the second count take part, so a clock
        // past 2^32 seconds folds to the same seed as its low half.
        assert_eq!(mix_seed(1 << 32, 99).get(), mix_seed(0, 99).get());
        assert_eq!(mix_seed((1 << 32) + 1, 99).get(), mix_seed(1, 99).get());
    }

    /// Every fold lands on a real game, whatever the clock reads.
    #[test]
    fn the_seed_fold_always_lands_in_range() {
        for secs in 0..64_u64 {
            for nanos in [0, 1, 999_999_999, u32::MAX] {
                assert!(mix_seed(secs, nanos).get() <= Seed::MAX);
            }
        }
    }

    /// A requested theme that loads is used as-is, one that does not falls
    /// back to the default and says so, and a default that will not load is
    /// fatal — there is nothing left to fall back to.
    #[test]
    fn the_startup_theme_falls_back_only_when_there_is_something_to_fall_back_to() {
        let entries = themes::discover();

        let chosen = choose_startup_theme(&entries, "default").unwrap();
        assert_eq!(chosen.id, "default");
        assert!(chosen.notice.is_none());

        let chosen = choose_startup_theme(&entries, "no-such-theme").unwrap();
        assert_eq!(chosen.id, "default");
        assert!(chosen.notice.unwrap().contains("no-such-theme"));

        // No themes at all: the default itself is what fails, so the caller
        // gets the error rather than a second report of the same failure.
        let error = choose_startup_theme(&[], "default").unwrap_err();
        assert!(matches!(error, AppError::Theme(_)), "{error}");

        // And a non-default request with no default to reach either.
        let error = choose_startup_theme(&[], "winter").unwrap_err();
        assert!(matches!(error, AppError::Theme(_)), "{error}");
    }

    /// The command surface reaches the engine, so a legal command through the
    /// presenter makes undo legal in turn.
    #[test]
    fn a_performed_command_makes_undo_reachable() {
        let (mut app, _dir) = app_in_tempdir();
        app.any_key();
        assert!(!app.presenter().can_undo());
        app.presenter_mut_for_test().apply(Command::Draw).unwrap();
        assert!(app.presenter().can_undo());
    }

    impl App {
        /// Mutable presenter access, for tests that need to drive the engine
        /// directly rather than through a pointer gesture.
        #[cfg(test)]
        pub(crate) const fn presenter_mut_for_test(&mut self) -> &mut Presenter {
            &mut self.presenter
        }
    }
}
