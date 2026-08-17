//! This frontend's residue on top of the shared application core: the
//! render worker it owns, and the theme swap and per-tick render that
//! ownership implies.
//!
//! Everything platform-free — the presenter, the theme list, save/load,
//! settings persistence, the menu snapshot — lives in [`sol_frontend`] and
//! is reached through [`App::core`] / [`App::core_mut`]. What is left here is
//! what genuinely differs: this frontend renders offscreen on a worker
//! thread, so a theme switch is transactional against that worker and a tick
//! feeds it a frame.

use sol_engine::Seed;
use sol_frontend::app::{App as Core, StatePaths};
use sol_frontend::{AppError as CoreError, Startup, previews};
use sol_presenter::{BackSheet, Rgba, Size};
use sol_session::Settings;
use sol_theme::CardScaling;

use crate::offscreen::{Offscreen, OffscreenError};
use crate::worker::{WorkerEvent, WorkerHandle};

/// Errors that abort frontend startup.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// The shared application core could not start.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// The offscreen render path could not start.
    #[error(transparent)]
    Offscreen(#[from] OffscreenError),
    /// The render worker thread failed to start.
    #[error("starting the render worker thread")]
    Worker(#[source] std::io::Error),
}

/// The one-time status text the first tick that observes the render worker
/// gone should surface; `None` while it is alive or once already reported.
fn gone_status_once(gone: bool, reported: &mut bool) -> Option<String> {
    if gone && !*reported {
        *reported = true;
        return Some(String::from("Render failed: the render worker is gone"));
    }
    None
}

/// Logs a notice the platform-free core handed back. The core never prints —
/// so that its behaviour stays observable to a test — and each frontend
/// labels what it logs with its own name.
pub(crate) fn report(notice: Option<String>) {
    if let Some(notice) = notice {
        eprintln!("sol-qt: {notice}");
    }
}

/// The settings this build should boot from and the paths it reads and
/// writes back through, with anything that went wrong already logged.
pub(crate) fn startup_state() -> (Settings, StatePaths) {
    let (paths, notices) = StatePaths::resolve();
    let (settings, notice) = sol_frontend::app::load_settings(&paths);
    for notice in notices.into_iter().chain(notice) {
        report(Some(notice));
    }
    (settings, paths)
}

/// The shared application core plus this frontend's render worker.
pub struct App {
    core: Core,
    worker: WorkerHandle,
    /// Whether the render worker's loss has already been surfaced, so
    /// [`Self::tick`] reports it exactly once.
    worker_gone_reported: bool,
}

/// One [`App::back_previews`] rebuild: every back's frames as PNG bytes,
/// the logical size of one grid cell, and the integer scale the sheet
/// rendered at.
///
/// `frames` is indexed by the theme's own declared back order, exactly as
/// [`previews::png_frames`] returns it — `frames[back]` is empty for a
/// back the theme declares with no frames. `cell` is the theme's own base
/// card size: themes size their own cards, so it is never a caller-side
/// constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackPreviews {
    /// `frames[back][frame]` is that frame's PNG-encoded bytes.
    pub frames: Vec<Vec<Vec<u8>>>,
    /// One grid cell's logical size.
    pub cell: Size,
    /// The integer scale the sheet was rendered at
    /// ([`previews::sheet_scale`]) — part of the cache key a caller keys
    /// its own rebuilds against, alongside the active theme and its card
    /// scaling.
    pub scale: u32,
}

impl App {
    /// Boots the shared core and starts the render worker on top of it.
    /// `deliver` is called from the worker thread for every
    /// [`WorkerEvent`]; marshalling it to the GUI thread is the caller's
    /// job.
    ///
    /// # Errors
    ///
    /// [`AppError`] when the theme fails to load, no graphics adapter is
    /// available, or the render worker thread fails to start.
    pub fn start(
        theme_override: Option<std::path::PathBuf>,
        seed: Option<Seed>,
        settings: Settings,
        paths: StatePaths,
        deliver: impl FnMut(WorkerEvent) + Send + 'static,
    ) -> Result<Self, AppError> {
        let Startup { app: core, notice } = Core::start(theme_override, seed, settings, paths)?;
        report(notice);
        let offscreen = Offscreen::new(core.theme().clone(), core.scaling(), 1.0)?;
        let worker = WorkerHandle::start(offscreen, deliver).map_err(AppError::Worker)?;
        Ok(Self {
            core,
            worker,
            worker_gone_reported: false,
        })
    }

    /// The shared core: everything platform-free this frontend drives.
    #[must_use]
    pub const fn core(&self) -> &Core {
        &self.core
    }

    /// The shared core, mutably.
    pub const fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Adopts a new viewport size in physical pixels: refits the continuous
    /// scale through the worker and sends an immediate render of the current
    /// presenter frame at the new size — without advancing the clock (that
    /// stays [`Self::tick`]'s job; a resize must not double-charge game
    /// time).
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.core.resize(width, height) {
            self.refit_and_render();
        }
    }

    /// Tells the worker the current scale and sends it an immediate render
    /// of the current presenter frame — used after a resize or a live theme
    /// swap so new geometry or artwork appears without waiting for the next
    /// tick. Never advances the clock; that stays [`Self::tick`]'s job.
    fn refit_and_render(&mut self) {
        self.worker.adopt_scale(self.core.scale());
        let (width, height) = self.core.viewport();
        if (width, height) == (0, 0) {
            return;
        }
        // Unconditional, unlike the per-tick path: the surface or the
        // artwork just changed, so the last frame the worker holds no longer
        // describes what should be on screen even if the display list is
        // byte-identical.
        if let Some(list) = self.core.frame() {
            self.worker.render(list, width, height);
        }
    }

    /// Advances animations/clock by the real elapsed time and, once a
    /// viewport is known, the worker is still alive, and the board has
    /// actually changed, sends the current presenter frame to the render
    /// worker. No GPU work happens here and this never blocks — rendered
    /// frames arrive back asynchronously through the worker's `deliver`
    /// callback.
    ///
    /// Returns the one-time status text the first tick that observes the
    /// render worker [gone](WorkerHandle::is_gone); every later tick returns
    /// `None`. Once the worker is gone, render/scale messages stop being
    /// sent, but the clock keeps advancing here and every non-render path
    /// (menus, autosave, the Options dialog) keeps working.
    pub fn tick(&mut self) -> Option<String> {
        self.core.advance();

        // Stop feeding a gone worker; the clock above keeps running
        // regardless. A live send here may itself discover the worker is
        // gone, which the report below then picks up in this same tick.
        if !self.worker.is_gone() {
            let (width, height) = self.core.viewport();
            if let Some(list) = self.core.take_frame_if_changed() {
                self.worker.render(list, width, height);
            }
        }
        gone_status_once(self.worker.is_gone(), &mut self.worker_gone_reported)
    }

    /// Live-applies a theme selection (the dialog's preview). The worker
    /// rebuilds its atlas first, so on failure the previous theme stays
    /// active — and its atlas is restored — before the error text comes
    /// back.
    pub fn select_theme_live(&mut self, id: &str) -> Option<String> {
        if id == self.core.theme_id() {
            return None;
        }
        let theme = match self.core.load_theme(id) {
            Ok(theme) => theme,
            Err(error) => return Some(error),
        };
        let scale = self.core.scale();
        let scaling = self.core.scaling_of(id);
        if let Err(error) = self.worker.set_theme(theme.clone(), scaling, scale) {
            // The render worker keeps needing a working atlas; rebuild
            // it for the previous theme before reporting.
            let _ = self
                .worker
                .set_theme(self.core.theme().clone(), self.core.scaling(), scale);
            return Some(error);
        }
        self.core.adopt_theme(id, &theme);
        // A new theme may change base_size: refit scale to the viewport,
        // and render immediately so the new artwork appears without
        // waiting for the next tick.
        self.refit_and_render();
        None
    }

    /// Options dialog live scaling preview: rebuilds the atlas for the
    /// active theme under `scaling`. On failure the previous atlas is put
    /// back and the error text returned, so a rejected rebuild leaves the
    /// board intact — the same contract the theme preview has.
    pub fn select_scaling_live(&mut self, scaling: CardScaling) -> Option<String> {
        if scaling == self.core.scaling() {
            return None;
        }
        let theme = self.core.theme().clone();
        let scale = self.core.scale();
        if let Err(error) = self.worker.set_theme(theme, scaling, scale) {
            let _ = self
                .worker
                .set_theme(self.core.theme().clone(), self.core.scaling(), scale);
            return Some(error);
        }
        self.core.set_scaling(scaling);
        // A scaling change cannot alter base_size, so no refit is needed —
        // but the new atlas still has to reach the screen without waiting
        // for the next tick, the same as a theme swap.
        self.refit_and_render();
        None
    }

    /// Options dialog Cancel: restores the theme, back and card scaling
    /// the dialog previewed away from.
    pub fn cancel_preview(&mut self) {
        let Some(restore) = self.core.take_preview_restore() else {
            return;
        };
        if restore.theme_id == self.core.theme_id() {
            // The theme replay below never runs in this branch, so a
            // scaling-only live preview against this same theme (no
            // theme switch involved) rebuilt the worker directly and
            // needs its own undo here. The target has to come from the
            // restore snapshot rather than from `core.scaling()` after
            // the bookkeeping restore below runs: at that point it would
            // already equal the target, defeating `select_scaling_live`'s
            // own no-op guard and leaving the worker's atlas on the
            // previewed value.
            let restored_scaling = restore.scaling_of(&restore.theme_id);
            report(
                self.select_scaling_live(restored_scaling)
                    .map(|error| format!("restoring scaling after cancel failed: {error}")),
            );
            self.core.restore_scaling(&restore);
        } else {
            // Restore the scaling bookkeeping before the replay below
            // reads it, so the theme swap rebuilds under the scaling
            // recorded when the dialog opened rather than one edited
            // mid-session on a theme since switched away from.
            self.core.restore_scaling(&restore);
            // Restoring what was active before cannot introduce a new
            // failure mode worth surfacing on a Cancel; stderr is enough.
            report(
                self.select_theme_live(&restore.theme_id)
                    .map(|error| format!("restoring theme after cancel failed: {error}")),
            );
        }
        report(
            self.core
                .set_back(restore.back_index)
                .map(|error| format!("restoring back after cancel failed: {error}")),
        );
    }

    /// Rebuilds every card back's Options-dialog preview thumbnails: asks
    /// the presenter for one contact sheet over a transparent background
    /// (the dialog composites the thumbnails itself), renders it through
    /// the worker, and cuts it apart into per-back, per-frame PNGs.
    ///
    /// `dpr` is the hosting display's device pixel ratio; see
    /// [`previews::sheet_scale`] for how it becomes the sheet's integer
    /// render scale, which the returned [`BackPreviews::scale`] reports
    /// back.
    ///
    /// # Errors
    ///
    /// Text naming what failed: the active theme's card backs do not fit
    /// one preview image ([`sol_presenter::Presenter::back_sheet`]
    /// returned `None`); the render worker failed or did not answer in
    /// time ([`WorkerHandle::render_sheet`]); or the rendered pixels and
    /// the sheet's own layout disagree ([`previews::png_frames`]).
    pub fn back_previews(&self, dpr: f64) -> Result<BackPreviews, String> {
        let scale = previews::sheet_scale(dpr);
        let max_side = self.worker.max_texture_dim() / scale;
        let transparent = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        let sheet = self
            .core
            .presenter()
            .back_sheet(transparent, max_side)
            .ok_or_else(|| {
                String::from("the active theme's card backs do not fit one preview image")
            })?;
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
        let pixels = self.worker.render_sheet(list, width, height, scale_f32)?;
        let back_count = self.core.presenter().back_count();
        let frames = previews::png_frames(&pixels, (width, height), &cells, scale, back_count)
            .map_err(|error| error.to_string())?;

        Ok(BackPreviews {
            frames,
            cell,
            scale,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    // A lost render worker must announce itself once — the existing
    // "Render failed" status path — and then stay quiet, never spamming
    // the status bar tick after tick.
    #[test]
    fn a_lost_worker_is_reported_exactly_once() {
        let mut reported = false;
        // A live worker: nothing to report, the flag stays down.
        assert_eq!(gone_status_once(false, &mut reported), None);
        assert!(!reported);
        // First observation of the loss: the one-time status, flag latched.
        assert_eq!(
            gone_status_once(true, &mut reported).as_deref(),
            Some("Render failed: the render worker is gone")
        );
        assert!(reported);
        // Still gone on later ticks: silent.
        assert_eq!(gone_status_once(true, &mut reported), None);
    }

    // ---- GPU-gated: exercises the real worker, skipping cleanly
    // without a graphics adapter like offscreen's and worker's own tests ----

    /// Blocks until at least one frame arrives — proving the preceding
    /// action actually reached the worker rather than only bookkeeping —
    /// then drains any stragglers so the next call starts from a quiet
    /// channel. Panics on an `Error` event or a timeout: nothing in this
    /// test's scenario is expected to fail or stall.
    fn expect_a_rebuild(rx: &mpsc::Receiver<WorkerEvent>) {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(WorkerEvent::Frame(_)) => {}
            Ok(WorkerEvent::Error(reason)) => panic!("unexpected worker error: {reason}"),
            Err(error) => panic!("expected the worker to rebuild and render: {error}"),
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(WorkerEvent::Frame(_)) => {}
                Ok(WorkerEvent::Error(reason)) => panic!("unexpected worker error: {reason}"),
                Err(mpsc::RecvTimeoutError::Timeout) => return,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("worker event channel disconnected")
                }
            }
        }
    }

    /// A Cancel that never switches theme still has to undo a
    /// scaling-only live preview: the theme-replay branch only touches
    /// the worker on an actual theme switch, so the same-theme branch has
    /// to rebuild it directly instead. Checking bookkeeping alone
    /// (`core.scaling()`) would not catch a regression here — restoring
    /// just the recorded map already makes that assertion pass — so this
    /// also requires a fresh frame to actually arrive after Cancel,
    /// proving the worker's atlas was rebuilt too, not just the record of
    /// what it should be.
    #[test]
    fn cancel_rebuilds_the_worker_when_only_scaling_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut app = match App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
            move |event| {
                let _ = tx.send(event);
            },
        ) {
            Ok(app) => app,
            Err(AppError::Offscreen(OffscreenError::NoAdapter)) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("app startup failed: {error}"),
        };

        app.resize(200, 200);
        expect_a_rebuild(&rx);

        app.core_mut().begin_preview();
        assert_eq!(app.core().scaling(), CardScaling::Original);
        assert_eq!(app.select_scaling_live(CardScaling::Xbrz), None);
        assert_eq!(app.core().scaling(), CardScaling::Xbrz);
        expect_a_rebuild(&rx);

        app.cancel_preview();
        assert_eq!(
            app.core().scaling(),
            CardScaling::Original,
            "bookkeeping reverts"
        );
        expect_a_rebuild(&rx);
    }

    /// End-to-end: the default theme's card backs render into a preview
    /// grid whose per-back frame counts match its back names, whose cell
    /// size is the theme's own card size, and whose bytes actually
    /// decode as PNGs — checked against the PNG magic header rather than
    /// a full decode, since this crate has no other reason to depend on
    /// a PNG decoder.
    #[test]
    fn back_previews_builds_decodable_thumbnails_for_every_back() {
        const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut app = match App::start(
            None,
            Some(Seed::new(1).unwrap()),
            Settings::default(),
            StatePaths::under(dir.path()),
            move |event| {
                let _ = tx.send(event);
            },
        ) {
            Ok(app) => app,
            Err(AppError::Offscreen(OffscreenError::NoAdapter)) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("app startup failed: {error}"),
        };
        app.resize(200, 200);
        expect_a_rebuild(&rx);

        let built = app.back_previews(1.0).unwrap();
        assert_eq!(built.scale, 1);
        assert_eq!(built.frames.len(), app.core().back_names().len());
        assert!(
            built.frames.iter().any(|frames| !frames.is_empty()),
            "at least one back has a thumbnail"
        );
        for frames in &built.frames {
            for png in frames {
                assert!(png.starts_with(&PNG_MAGIC), "not a PNG: {png:02x?}");
            }
        }

        // Calling again with the same DPR asks the same question and
        // gets the same answer: this method itself is a plain query, not
        // a cache — that layer is the QML bridge's job.
        let built_again = app.back_previews(1.0).unwrap();
        assert_eq!(built_again, built);
    }
}
