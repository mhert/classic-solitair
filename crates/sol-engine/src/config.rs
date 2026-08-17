//! Per-game rule configuration: draw mode, scoring mode, timed flag.
//!
//! The configuration is fixed when a game is dealt and never changes during
//! play; changing an option means dealing a new game. It parameterizes the
//! rules only — the card arrangement of a deal depends solely on the
//! [`crate::Seed`].

use serde::{Deserialize, Serialize};

/// How many cards a draw turns from the stock onto the waste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DrawMode {
    /// Draw One: one card per draw, unlimited-ish passes (Vegas: 1 pass).
    One,
    /// Draw Three: three cards per draw, fanned (Vegas: 3 passes).
    Three,
}

impl DrawMode {
    /// Cards turned per draw: 1 or 3.
    #[must_use]
    pub const fn cards_per_draw(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Three => 3,
        }
    }
}

/// The scoring mode of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScoringMode {
    /// Point scoring with optional timed decay and win bonus.
    Standard,
    /// Dollar scoring: −$52 buy-in, ±$5 per foundation card, undo rejected.
    Vegas,
    /// No scoring events at all.
    None,
}

/// A game's fixed rule configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GameConfig {
    /// Draw One or Draw Three.
    pub draw_mode: DrawMode,
    /// Standard, Vegas, or None scoring.
    pub scoring: ScoringMode,
    /// Whether the game is timed (Standard scoring only: decay and win bonus).
    pub timed: bool,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn draw_one_turns_one_card_draw_three_turns_three() {
        assert_eq!(DrawMode::One.cards_per_draw(), 1);
        assert_eq!(DrawMode::Three.cards_per_draw(), 3);
    }

    #[test]
    fn game_config_serde_round_trips() {
        let config = GameConfig {
            draw_mode: DrawMode::Three,
            scoring: ScoringMode::Vegas,
            timed: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<GameConfig>(&json).unwrap(), config);
    }
}
