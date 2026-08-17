//! Pile identity: [`PileId`] names every pile on the table.

use serde::{Deserialize, Serialize};

/// Number of tableau piles on the table.
pub const TABLEAU_COUNT: u8 = 7;

/// Number of foundation slots on the table.
pub const FOUNDATION_COUNT: u8 = 4;

/// Identifies one pile on the table, in the game's ubiquitous language:
/// stock, waste, foundations, and tableau piles.
///
/// Indices are zero-based, left to right as laid out in the original game.
/// A `PileId` is plain data and may carry an out-of-range index; commands
/// naming such a pile are rejected by [`crate::decide`], never applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PileId {
    /// The face-down draw pile, top left.
    Stock,
    /// The face-up pile next to the stock that drawn cards fan onto.
    Waste,
    /// A foundation slot, index `0..FOUNDATION_COUNT`, built same-suit
    /// ascending from the ace.
    Foundation(u8),
    /// A tableau pile, index `0..TABLEAU_COUNT`, built alternating-color
    /// descending.
    Tableau(u8),
}

impl PileId {
    /// Whether this id names a pile that exists on the table (any index it
    /// carries is in range).
    #[must_use]
    pub const fn is_valid(self) -> bool {
        match self {
            Self::Stock | Self::Waste => true,
            Self::Foundation(index) => index < FOUNDATION_COUNT,
            Self::Tableau(index) => index < TABLEAU_COUNT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_and_waste_are_valid() {
        assert!(PileId::Stock.is_valid());
        assert!(PileId::Waste.is_valid());
    }

    #[test]
    fn foundation_indices_below_four_are_valid() {
        for index in 0..FOUNDATION_COUNT {
            assert!(PileId::Foundation(index).is_valid());
        }
        assert!(!PileId::Foundation(FOUNDATION_COUNT).is_valid());
        assert!(!PileId::Foundation(u8::MAX).is_valid());
    }

    #[test]
    fn tableau_indices_below_seven_are_valid() {
        for index in 0..TABLEAU_COUNT {
            assert!(PileId::Tableau(index).is_valid());
        }
        assert!(!PileId::Tableau(TABLEAU_COUNT).is_valid());
        assert!(!PileId::Tableau(u8::MAX).is_valid());
    }

    #[test]
    fn pile_id_serde_round_trips() {
        #![allow(clippy::unwrap_used)]
        for pile in [
            PileId::Stock,
            PileId::Waste,
            PileId::Foundation(3),
            PileId::Tableau(6),
        ] {
            let json = serde_json::to_string(&pile).unwrap();
            assert_eq!(serde_json::from_str::<PileId>(&json).unwrap(), pile);
        }
    }
}
