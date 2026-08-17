//! Game state: [`GameState`] and [`TableauPile`], the folded state of one
//! game.
//!
//! State is produced by [`crate::deal`] and advanced exclusively by
//! [`crate::evolve`]; nothing here contains rule knowledge. All queries are
//! read-only — mutation happens only through events.

use crate::card::Card;
use crate::config::GameConfig;
use crate::pile::{FOUNDATION_COUNT, TABLEAU_COUNT};

/// One tableau pile: face-down cards underneath, face-up cards on top.
///
/// Within each list, index 0 is the bottom-most card and the last element is
/// the top (the card nearest the player, farthest down the fan on screen).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableauPile {
    pub(crate) face_down: Vec<Card>,
    pub(crate) face_up: Vec<Card>,
}

impl TableauPile {
    /// The face-down cards, bottom first.
    #[must_use]
    pub fn face_down(&self) -> &[Card] {
        &self.face_down
    }

    /// The face-up run, deepest card first.
    #[must_use]
    pub fn face_up(&self) -> &[Card] {
        &self.face_up
    }

    /// Total number of cards in the pile.
    #[must_use]
    pub fn len(&self) -> usize {
        self.face_down.len() + self.face_up.len()
    }

    /// Whether the pile holds no cards at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.face_down.is_empty() && self.face_up.is_empty()
    }
}

/// The complete state of one game: every pile plus score, elapsed time,
/// completed waste passes, and the won flag.
///
/// For the stock and the waste, the **last** element of the slice is the top
/// card — the next card drawn, and the card a move takes, respectively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameState {
    pub(crate) config: GameConfig,
    pub(crate) stock: Vec<Card>,
    pub(crate) waste: Vec<Card>,
    pub(crate) foundations: [Vec<Card>; FOUNDATION_COUNT as usize],
    pub(crate) tableaus: [TableauPile; TABLEAU_COUNT as usize],
    pub(crate) score: i32,
    pub(crate) elapsed_secs: u32,
    pub(crate) passes_completed: u32,
    pub(crate) won: bool,
}

impl GameState {
    /// The game's fixed rule configuration.
    #[must_use]
    pub const fn config(&self) -> GameConfig {
        self.config
    }

    /// The face-down stock; the last element is drawn next.
    #[must_use]
    pub fn stock(&self) -> &[Card] {
        &self.stock
    }

    /// The face-up waste; the last element is the playable top card.
    #[must_use]
    pub fn waste(&self) -> &[Card] {
        &self.waste
    }

    /// The foundation at `index` (`0..FOUNDATION_COUNT`), ace first, or
    /// `None` for an out-of-range index.
    #[must_use]
    pub fn foundation(&self, index: u8) -> Option<&[Card]> {
        self.foundations.get(usize::from(index)).map(Vec::as_slice)
    }

    /// All four foundations, left to right.
    pub fn foundations(&self) -> impl Iterator<Item = &[Card]> {
        self.foundations.iter().map(Vec::as_slice)
    }

    /// The tableau pile at `index` (`0..TABLEAU_COUNT`), or `None` for an
    /// out-of-range index.
    #[must_use]
    pub fn tableau(&self, index: u8) -> Option<&TableauPile> {
        self.tableaus.get(usize::from(index))
    }

    /// All seven tableau piles, left to right.
    pub fn tableaus(&self) -> impl Iterator<Item = &TableauPile> {
        self.tableaus.iter()
    }

    /// The current score: points in Standard scoring, dollars in Vegas,
    /// always 0 in None scoring.
    #[must_use]
    pub const fn score(&self) -> i32 {
        self.score
    }

    /// Elapsed play time in whole seconds, as last reported via
    /// `Command::Tick`. Only timed Standard games track it.
    #[must_use]
    pub const fn elapsed_secs(&self) -> u32 {
        self.elapsed_secs
    }

    /// Completed passes through the stock (number of waste recycles so far).
    #[must_use]
    pub const fn passes_completed(&self) -> u32 {
        self.passes_completed
    }

    /// Whether the game has been won (all 52 cards on the foundations).
    #[must_use]
    pub const fn is_won(&self) -> bool {
        self.won
    }

    /// Number of cards currently on all foundations together.
    #[must_use]
    pub fn foundation_card_count(&self) -> usize {
        self.foundations.iter().map(Vec::len).sum()
    }

    pub(crate) fn tableau_mut(&mut self, index: u8) -> Option<&mut TableauPile> {
        self.tableaus.get_mut(usize::from(index))
    }

    pub(crate) fn foundation_mut(&mut self, index: u8) -> Option<&mut Vec<Card>> {
        self.foundations.get_mut(usize::from(index))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crate::config::{DrawMode, GameConfig, ScoringMode};
    use crate::deal::deal;
    use crate::pile::{FOUNDATION_COUNT, TABLEAU_COUNT};
    use crate::seed::Seed;

    fn any_config() -> GameConfig {
        GameConfig {
            draw_mode: DrawMode::One,
            scoring: ScoringMode::Standard,
            timed: false,
        }
    }

    #[test]
    fn out_of_range_pile_queries_return_none() {
        let state = deal(Seed::new(1).unwrap(), any_config());
        assert!(state.tableau(TABLEAU_COUNT).is_none());
        assert!(state.foundation(FOUNDATION_COUNT).is_none());
        assert!(state.tableau(0).is_some());
        assert!(state.foundation(0).is_some());
    }

    #[test]
    fn tableau_pile_len_counts_both_halves() {
        let state = deal(Seed::new(1).unwrap(), any_config());
        let last = state.tableau(6).map(super::TableauPile::len);
        assert_eq!(last, Some(7));
        let has_empty = state.tableaus().any(super::TableauPile::is_empty);
        assert!(!has_empty, "every dealt tableau holds cards");
    }
}
