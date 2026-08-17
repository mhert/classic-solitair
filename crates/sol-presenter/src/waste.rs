//! The waste fan fold: how many top waste cards are currently fanned.
//!
//! The engine's [`sol_engine::GameState`] holds the waste as a plain card
//! list; which of its top cards still belong to the most recent draw's
//! Draw-Three fan is presentation state. The original tracked it
//! statefully; here it is a pure fold over the engine's event log, so
//! undo, redo, and save/load reproduce the fan exactly.

use sol_engine::{Event, LogEntry, PileId};

/// Folds the current fan length from a game's log.
///
/// A draw fans the cards it turned; a card played off the waste shrinks
/// the fan; recycling the waste (and anything before it) leaves no fan.
pub(crate) fn fan_len(log: &[LogEntry]) -> usize {
    let mut fan = 0_usize;
    for entry in log {
        for event in &entry.events {
            match *event {
                Event::CardsMoved {
                    from: PileId::Stock,
                    to: PileId::Waste,
                    count,
                } => fan = usize::from(count),
                Event::CardsMoved {
                    from: PileId::Waste,
                    count,
                    ..
                } => fan = fan.saturating_sub(usize::from(count)),
                Event::WastePassCompleted => fan = 0,
                Event::CardsMoved { .. }
                | Event::CardFlipped { .. }
                | Event::ScoreChanged { .. }
                | Event::TimeAdvanced { .. }
                | Event::GameWon => {}
            }
        }
    }
    fan
}

#[cfg(test)]
mod tests {
    use sol_engine::Command;

    use super::*;

    fn entry(events: Vec<Event>) -> LogEntry {
        LogEntry {
            command: Command::Draw,
            events,
        }
    }

    fn draw(count: u8) -> LogEntry {
        entry(vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count,
        }])
    }

    fn play_from_waste() -> LogEntry {
        entry(vec![Event::CardsMoved {
            from: PileId::Waste,
            to: PileId::Tableau(0),
            count: 1,
        }])
    }

    #[test]
    fn empty_log_has_no_fan() {
        assert_eq!(fan_len(&[]), 0);
    }

    #[test]
    fn a_draw_fans_what_it_turned() {
        assert_eq!(fan_len(&[draw(3)]), 3);
        assert_eq!(fan_len(&[draw(3), draw(2)]), 2);
        assert_eq!(fan_len(&[draw(1)]), 1);
    }

    #[test]
    fn playing_off_the_waste_shrinks_the_fan_to_zero_at_most() {
        assert_eq!(fan_len(&[draw(3), play_from_waste()]), 2);
        let log = [draw(1), play_from_waste(), play_from_waste()];
        assert_eq!(fan_len(&log), 0);
    }

    #[test]
    fn a_recycle_clears_the_fan() {
        let log = [draw(3), entry(vec![Event::WastePassCompleted])];
        assert_eq!(fan_len(&log), 0);
    }

    #[test]
    fn unrelated_events_leave_the_fan_alone() {
        let log = [
            draw(3),
            entry(vec![
                Event::CardsMoved {
                    from: PileId::Tableau(1),
                    to: PileId::Tableau(2),
                    count: 2,
                },
                Event::CardFlipped {
                    pile: PileId::Tableau(1),
                },
                Event::ScoreChanged { delta: 5 },
                Event::TimeAdvanced {
                    total_elapsed_secs: 3,
                },
                Event::GameWon,
            ]),
        ];
        assert_eq!(fan_len(&log), 3);
    }
}
