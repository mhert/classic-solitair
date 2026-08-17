//! The status-bar strings every frontend shows, formatted once.
//!
//! The seed appears in two forms, deliberately. A status bar that shows the
//! seed among other fields wants it labelled (`Game 42`) so the number reads
//! as a game number rather than a stray integer; a control whose entire
//! contents are copied to the clipboard wants the bare digits, because the
//! label would be copied with them. Both are real requirements, so both are
//! named here rather than one frontend quietly disagreeing with the other.

use sol_engine::ScoringMode;
use sol_presenter::Presenter;

/// The seed as bare digits, for a control whose whole contents are copied.
#[must_use]
pub fn seed_digits(presenter: &Presenter) -> String {
    presenter.seed().get().to_string()
}

/// The seed labelled for a status bar that shows it among other fields.
#[must_use]
pub fn seed_label(presenter: &Presenter) -> String {
    format!("Game {}", presenter.seed().get())
}

/// The status bar's score field: points, Vegas dollars, or empty under
/// `None` scoring.
#[must_use]
pub fn score_text(presenter: &Presenter) -> String {
    format_score(presenter.options().scoring, presenter.score())
}

/// [`score_text`]'s rule, as a function of the two things it depends on.
///
/// Separate so every score can be exercised: a Vegas bankroll only climbs
/// above the buy-in after eleven foundation moves, which is a game to play,
/// not a formatting rule to check.
#[must_use]
fn format_score(scoring: ScoringMode, score: i32) -> String {
    match scoring {
        ScoringMode::Standard => format!("Score: {score}"),
        // The sign goes outside the symbol: `-$52`, not `$-52`.
        ScoringMode::Vegas if score < 0 => format!("-${}", score.unsigned_abs()),
        ScoringMode::Vegas => format!("${score}"),
        ScoringMode::None => String::new(),
    }
}

/// The status bar's time field: `Time: M:SS` while the game is timed,
/// empty otherwise.
#[must_use]
pub fn time_text(presenter: &Presenter) -> String {
    if !presenter.options().timed {
        return String::new();
    }
    let elapsed = presenter.elapsed_secs();
    format!("Time: {}:{:02}", elapsed / 60, elapsed % 60)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sol_engine::Seed;
    use sol_session::{Options, Session};

    use super::*;
    use crate::themes;

    fn presenter(options: Options) -> Presenter {
        let theme = themes::load(&themes::discover(), "default").unwrap();
        Presenter::new(Session::new(options, Seed::new(1).unwrap()), &theme)
    }

    /// The two seed forms differ by exactly the label, and each matches
    /// what its control does with the text.
    #[test]
    fn the_seed_reads_labelled_or_bare() {
        let presenter = presenter(Options::default());
        assert_eq!(seed_digits(&presenter), "1");
        assert_eq!(seed_label(&presenter), "Game 1");
    }

    #[test]
    fn standard_scoring_shows_points() {
        let presenter = presenter(Options::default());
        assert_eq!(score_text(&presenter), "Score: 0");
    }

    #[test]
    fn vegas_shows_dollars_with_the_sign_outside_the_symbol() {
        let presenter = presenter(Options {
            scoring: ScoringMode::Vegas,
            ..Options::default()
        });
        assert_eq!(score_text(&presenter), "-$52", "the Vegas buy-in");
    }

    /// Every branch of the rule, at the values a game would take minutes to
    /// reach — including zero, which is neither a loss nor a win and must
    /// not grow a sign.
    #[test]
    fn the_score_rule_covers_every_mode_and_sign() {
        assert_eq!(format_score(ScoringMode::Standard, 0), "Score: 0");
        assert_eq!(format_score(ScoringMode::Standard, 1_250), "Score: 1250");
        assert_eq!(format_score(ScoringMode::Standard, -30), "Score: -30");

        assert_eq!(format_score(ScoringMode::Vegas, -52), "-$52");
        assert_eq!(format_score(ScoringMode::Vegas, 0), "$0");
        assert_eq!(format_score(ScoringMode::Vegas, 75), "$75");

        assert_eq!(format_score(ScoringMode::None, 999), "");
    }

    #[test]
    fn no_scoring_shows_nothing() {
        let presenter = presenter(Options {
            scoring: ScoringMode::None,
            ..Options::default()
        });
        assert_eq!(score_text(&presenter), "");
    }

    #[test]
    fn the_clock_shows_only_in_a_timed_game() {
        let timed = presenter(Options {
            timed: true,
            ..Options::default()
        });
        assert_eq!(time_text(&timed), "Time: 0:00");

        let untimed = presenter(Options {
            timed: false,
            ..Options::default()
        });
        assert_eq!(time_text(&untimed), "");
    }

    /// Seconds are zero-padded and minutes are not, so the clock reads as a
    /// clock rather than as a raw second count.
    #[test]
    fn the_clock_pads_seconds_and_rolls_into_minutes() {
        let mut presenter = presenter(Options {
            timed: true,
            ..Options::default()
        });
        presenter.advance(65_000);
        assert_eq!(time_text(&presenter), "Time: 1:05");
    }
}
