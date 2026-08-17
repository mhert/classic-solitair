//! Commands: player intents that may be invalid.
//!
//! A [`Command`] enters [`crate::decide`], which either rejects it with a
//! [`crate::RuleError`] or turns it into events. Undo and redo are not
//! commands into `decide` — they operate on the log itself and live as
//! methods on [`crate::Game`] (rejected there in Vegas scoring).

use serde::{Deserialize, Serialize};

use crate::pile::PileId;

/// A player intent, in ubiquitous language. May be invalid; only
/// [`crate::decide`] knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    /// Turn cards from the stock onto the waste — one or three per the
    /// game's [`crate::DrawMode`]. On an empty stock this asks to recycle
    /// the waste into a new pass.
    Draw,
    /// Move `count` cards from the top of `from` onto `to`: single cards
    /// between waste/tableau/foundation piles, or a face-up run of `count`
    /// cards between tableau piles.
    MoveCards {
        /// Source pile.
        from: PileId,
        /// Target pile.
        to: PileId,
        /// Number of cards taken from the top of `from`.
        count: u8,
    },
    /// Double-click: send the top card of `pile` (waste or tableau) to an
    /// eligible foundation, if any.
    AutoToFoundation {
        /// The double-clicked pile.
        pile: PileId,
    },
    /// Wall-clock report from the host — the only way time enters the
    /// engine. Carries the total elapsed play time in whole seconds.
    Tick {
        /// Seconds since the game started, as measured by the host.
        total_elapsed_secs: u32,
    },
}
