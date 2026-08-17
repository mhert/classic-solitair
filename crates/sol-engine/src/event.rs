//! Events: immutable facts emitted by [`crate::decide`] and folded by
//! [`crate::evolve`].
//!
//! Every rule consequence — auto-flips, score changes (already floored),
//! pass completions, time advancement, the win — is materialized here at
//! decision time, so the fold stays rules-free.

use serde::{Deserialize, Serialize};

use crate::pile::PileId;

/// An immutable fact about the game, in ubiquitous language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Event {
    /// `count` cards went from the top of `from` to the top of `to`.
    /// Stock-to-waste turns them over one by one (last drawn on top);
    /// tableau-to-tableau moves the run as a block, order preserved.
    CardsMoved {
        /// Source pile.
        from: PileId,
        /// Target pile.
        to: PileId,
        /// Number of cards moved.
        count: u8,
    },
    /// The exposed top face-down card of a tableau pile turned face-up.
    CardFlipped {
        /// The tableau pile whose card flipped.
        pile: PileId,
    },
    /// The waste was turned over to become the stock again, completing one
    /// pass through the deck.
    WastePassCompleted,
    /// The score changed by `delta` — already floored at decision time, so
    /// the fold applies it blindly.
    ScoreChanged {
        /// Signed score change (points or Vegas dollars).
        delta: i32,
    },
    /// The engine's clock advanced to the given total elapsed seconds.
    TimeAdvanced {
        /// Total elapsed play time in whole seconds.
        total_elapsed_secs: u32,
    },
    /// All 52 cards reached the foundations.
    GameWon,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::command::Command;

    #[test]
    fn events_and_commands_serde_round_trip() {
        let events = [
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 3,
            },
            Event::CardFlipped {
                pile: PileId::Tableau(4),
            },
            Event::WastePassCompleted,
            Event::ScoreChanged { delta: -100 },
            Event::TimeAdvanced {
                total_elapsed_secs: 61,
            },
            Event::GameWon,
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
        }
        let commands = [
            Command::Draw,
            Command::MoveCards {
                from: PileId::Waste,
                to: PileId::Tableau(2),
                count: 1,
            },
            Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            },
            Command::Tick {
                total_elapsed_secs: 5,
            },
        ];
        for command in commands {
            let json = serde_json::to_string(&command).unwrap();
            assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
        }
    }
}
