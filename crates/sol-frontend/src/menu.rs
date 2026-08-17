//! [`MenuModel`]: what every menu item's enabled and checked state should be,
//! as plain data.
//!
//! Menu state is a consequence of game state — Vegas scoring forbids undo,
//! the scoring radio group follows the committed options — and three
//! frontends would otherwise each restate those rules against their own
//! toolkit's menu API, where they cannot be tested without a display. The
//! core computes the answer; the chrome only applies it.

use sol_engine::{DrawMode, ScoringMode};
use sol_presenter::Presenter;

/// A snapshot of every menu item's state, recomputed after each command.
// Ten bools is what a menu of ten independent items is: there is no state
// here to model, only one answer per item, and grouping them would invent a
// structure the menu does not have.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MenuModel {
    /// Whether Undo may be invoked.
    pub undo_enabled: bool,
    /// Whether Redo may be invoked.
    pub redo_enabled: bool,
    /// Whether the Standard scoring item carries the radio mark.
    pub scoring_standard_checked: bool,
    /// Whether the Vegas scoring item carries the radio mark.
    pub scoring_vegas_checked: bool,
    /// Whether the None scoring item carries the radio mark.
    pub scoring_none_checked: bool,
    /// Whether the Draw Three item is checked.
    pub draw_three_checked: bool,
    /// Whether the Timed Game item is checked.
    pub timed_checked: bool,
    /// Whether the Outline Dragging item is checked.
    pub outline_dragging_checked: bool,
    /// Whether the Keep Vegas Score item is checked.
    pub keep_vegas_score_checked: bool,
    /// Whether the Sounds item is checked.
    pub sounds_checked: bool,
}

impl MenuModel {
    /// The menu state `presenter`'s current game and options imply.
    ///
    /// Undo and redo read the engine's own `can_undo`/`can_redo` rather than
    /// restating the rule that Vegas scoring forbids them — the engine
    /// already reports `false` there, and a second copy of that rule is a
    /// second place for it to go wrong.
    #[must_use]
    pub fn of(presenter: &Presenter) -> Self {
        let options = presenter.options();
        Self {
            undo_enabled: presenter.can_undo(),
            redo_enabled: presenter.can_redo(),
            scoring_standard_checked: options.scoring == ScoringMode::Standard,
            scoring_vegas_checked: options.scoring == ScoringMode::Vegas,
            scoring_none_checked: options.scoring == ScoringMode::None,
            draw_three_checked: options.draw_mode == DrawMode::Three,
            timed_checked: options.timed,
            outline_dragging_checked: options.outline_dragging,
            keep_vegas_score_checked: options.keep_vegas_score,
            sounds_checked: options.sounds,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sol_engine::{Command, Seed};

    use super::*;
    use crate::app::tests::app_in_tempdir;
    use crate::options::EditedOptions;

    /// Vegas scoring forbids undo and redo, even with moves behind you. The
    /// chrome must not restate that rule — it reads the snapshot, and the
    /// snapshot reads the engine.
    ///
    /// Scoring is fixed when a game is dealt, so this boots into Vegas rather
    /// than committing it mid-game: committing options changes what the *next*
    /// deal uses, which is a different rule and not this one.
    #[test]
    fn vegas_scoring_disables_undo_and_redo() {
        let (mut app, _dir) = app_in_vegas();
        app.any_key();
        app.presenter_mut_for_test().apply(Command::Draw).unwrap();

        let model = app.menu_model();
        assert!(!model.undo_enabled, "Vegas: the menu entry stays disabled");
        assert!(!model.redo_enabled);
    }

    /// An `App` dealt under Vegas scoring, alongside its temporary directory.
    fn app_in_vegas() -> (crate::app::App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let settings = sol_session::Settings {
            options: sol_session::Options {
                scoring: ScoringMode::Vegas,
                ..sol_session::Options::default()
            },
            ..sol_session::Settings::default()
        };
        let app = crate::app::App::start(
            None,
            Some(Seed::new(1).unwrap()),
            settings,
            crate::app::StatePaths::under(dir.path()),
        )
        .unwrap()
        .app;
        (app, dir)
    }

    /// A fresh deal under Standard scoring has nothing to undo yet, but the
    /// rule is "no history", not "no undo ever".
    #[test]
    fn a_fresh_standard_deal_has_nothing_to_undo_but_may_undo_later() {
        let (mut app, _dir) = app_in_tempdir();
        assert!(!app.menu_model().undo_enabled);
        app.any_key();
        app.presenter_mut_for_test().apply(Command::Draw).unwrap();
        assert!(app.menu_model().undo_enabled);
        assert_eq!(app.menu_model().undo_enabled, app.presenter().can_undo());
        assert_eq!(app.menu_model().redo_enabled, app.presenter().can_redo());
    }

    #[test]
    fn the_checked_scoring_item_matches_the_committed_options() {
        let (mut app, _dir) = app_in_tempdir();
        for (scoring, expected) in [
            (ScoringMode::None, (false, false, true)),
            (ScoringMode::Standard, (true, false, false)),
            (ScoringMode::Vegas, (false, true, false)),
        ] {
            app.commit_options(EditedOptions {
                scoring,
                ..app.options_snapshot()
            });
            let model = app.menu_model();
            assert_eq!(
                (
                    model.scoring_standard_checked,
                    model.scoring_vegas_checked,
                    model.scoring_none_checked
                ),
                expected,
                "{scoring:?}"
            );
        }
    }

    /// Every toggle is independent, so each has to read its own option
    /// rather than any of them standing in for the others.
    #[test]
    fn each_toggle_follows_its_own_option() {
        let (mut app, _dir) = app_in_tempdir();
        app.commit_options(EditedOptions {
            draw_three: true,
            scoring: ScoringMode::Standard,
            timed: false,
            outline_dragging: true,
            keep_vegas_score: false,
            sounds: true,
        });
        let model = app.menu_model();
        assert!(model.draw_three_checked);
        assert!(!model.timed_checked);
        assert!(model.outline_dragging_checked);
        assert!(!model.keep_vegas_score_checked);
        assert!(model.sounds_checked);

        app.commit_options(EditedOptions {
            draw_three: false,
            scoring: ScoringMode::Standard,
            timed: true,
            outline_dragging: false,
            keep_vegas_score: true,
            sounds: false,
        });
        let model = app.menu_model();
        assert!(!model.draw_three_checked);
        assert!(model.timed_checked);
        assert!(!model.outline_dragging_checked);
        assert!(model.keep_vegas_score_checked);
        assert!(!model.sounds_checked);
    }
}
