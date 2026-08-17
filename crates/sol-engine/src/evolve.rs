//! The apply side: [`evolve`] folds one [`Event`] into a [`GameState`].
//!
//! `evolve` is **total, infallible, and rules-free**: it performs pure pile
//! mechanics with clamped counts and saturating arithmetic, never panics,
//! and never validates legality — all rule knowledge lives in
//! [`crate::decide`], which materializes every consequence as an explicit
//! event. For any event `decide` can emit, `evolve` reproduces the decided
//! outcome exactly; for events it can never emit (a hand-built or corrupted
//! log), it degrades to a harmless deterministic no-op rather than failing.

use crate::card::Card;
use crate::event::Event;
use crate::pile::PileId;
use crate::state::GameState;

/// Folds one event into the state, in place. Conceptually
/// `evolve(state, event) -> state` — the total, rules-free left fold of the
/// event log.
///
/// ```
/// use sol_engine::{Command, DrawMode, GameConfig, ScoringMode, Seed, deal, decide, evolve};
///
/// let config = GameConfig {
///     draw_mode: DrawMode::One,
///     scoring: ScoringMode::None,
///     timed: false,
/// };
/// let mut state = deal(Seed::new(7).unwrap(), config);
/// for event in decide(&state, Command::Draw)? {
///     evolve(&mut state, event);
/// }
/// assert_eq!(state.waste().len(), 1);
/// # Ok::<(), sol_engine::RuleError>(())
/// ```
pub fn evolve(state: &mut GameState, event: Event) {
    match event {
        Event::CardsMoved { from, to, count } => move_cards(state, from, to, count),
        Event::CardFlipped { pile } => {
            if let PileId::Tableau(index) = pile
                && let Some(tableau) = state.tableau_mut(index)
                && let Some(card) = tableau.face_down.pop()
            {
                tableau.face_up.push(card);
            }
        }
        Event::WastePassCompleted => {
            let waste = core::mem::take(&mut state.waste);
            state.stock.extend(waste.into_iter().rev());
            state.passes_completed = state.passes_completed.saturating_add(1);
        }
        Event::ScoreChanged { delta } => {
            state.score = state.score.saturating_add(delta);
        }
        Event::TimeAdvanced { total_elapsed_secs } => {
            state.elapsed_secs = total_elapsed_secs;
        }
        Event::GameWon => {
            state.won = true;
        }
    }
}

/// Pure pile mechanics for [`Event::CardsMoved`]. Counts clamp to what the
/// source holds; unknown piles make the whole move a no-op, so cards are
/// never lost. Tableau-to-tableau moves the run as one block, order
/// preserved; every other combination moves card by card, so stock-to-waste
/// turns the block over (last drawn ends up on top).
fn move_cards(state: &mut GameState, from: PileId, to: PileId, count: u8) {
    if !from.is_valid() || !to.is_valid() {
        return;
    }
    if let (PileId::Tableau(source_index), PileId::Tableau(target_index)) = (from, to)
        && source_index != target_index
    {
        // Both piles exist — the validity gate above already returned
        // otherwise — so the fallbacks here can never lose cards.
        let run = state
            .tableau_mut(source_index)
            .map(|source| {
                let take = usize::from(count).min(source.face_up.len());
                let cut = source.face_up.len().saturating_sub(take);
                source.face_up.split_off(cut)
            })
            .unwrap_or_default();
        if let Some(target) = state.tableau_mut(target_index) {
            target.face_up.extend(run);
        }
        return;
    }
    for _ in 0..count {
        let Some(card) = pop_top(state, from) else {
            break;
        };
        push_top(state, to, card);
    }
}

/// Removes the top card of `pile`, if the pile exists and has one.
/// For a tableau pile the top of the face-up run is taken.
fn pop_top(state: &mut GameState, pile: PileId) -> Option<Card> {
    match pile {
        PileId::Stock => state.stock.pop(),
        PileId::Waste => state.waste.pop(),
        PileId::Foundation(index) => state.foundation_mut(index)?.pop(),
        PileId::Tableau(index) => state.tableau_mut(index)?.face_up.pop(),
    }
}

/// Puts a card on top of `pile`; for a tableau pile, onto the face-up run.
/// Callers verify the pile exists first, so the card cannot be lost.
fn push_top(state: &mut GameState, pile: PileId, card: Card) {
    match pile {
        PileId::Stock => state.stock.push(card),
        PileId::Waste => state.waste.push(card),
        PileId::Foundation(index) => {
            if let Some(foundation) = state.foundation_mut(index) {
                foundation.push(card);
            }
        }
        PileId::Tableau(index) => {
            if let Some(tableau) = state.tableau_mut(index) {
                tableau.face_up.push(card);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use crate::config::{DrawMode, GameConfig, ScoringMode};
    use crate::deal::deal;
    use crate::seed::Seed;

    fn fresh() -> GameState {
        deal(
            Seed::new(1).unwrap(),
            GameConfig {
                draw_mode: DrawMode::Three,
                scoring: ScoringMode::Standard,
                timed: false,
            },
        )
    }

    fn total_cards(state: &GameState) -> usize {
        state.stock().len()
            + state.waste().len()
            + state.foundation_card_count()
            + state
                .tableaus()
                .map(crate::state::TableauPile::len)
                .sum::<usize>()
    }

    #[test]
    fn stock_to_waste_turns_cards_one_by_one_last_drawn_on_top() {
        let mut state = fresh();
        let expected_top_after_three = state.stock()[state.stock().len() - 3];
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 3,
            },
        );
        assert_eq!(state.stock().len(), 21);
        assert_eq!(state.waste().len(), 3);
        assert_eq!(*state.waste().last().unwrap(), expected_top_after_three);
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn tableau_to_tableau_moves_the_run_as_a_block_preserving_order() {
        let mut state = fresh();
        // Mechanically stack three cards onto tableau 0's face-up run (evolve
        // does not validate legality, which is exactly what we exploit here).
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 3,
            },
        );
        for _ in 0..3 {
            evolve(
                &mut state,
                Event::CardsMoved {
                    from: PileId::Waste,
                    to: PileId::Tableau(0),
                    count: 1,
                },
            );
        }
        let run: Vec<Card> = state.tableau(0).unwrap().face_up().to_vec();
        assert_eq!(run.len(), 4);
        let target_before: Vec<Card> = state.tableau(1).unwrap().face_up().to_vec();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Tableau(1),
                count: 3,
            },
        );
        assert_eq!(state.tableau(0).unwrap().face_up(), &run[..1]);
        let mut expected_target = target_before;
        expected_target.extend_from_slice(&run[1..]);
        assert_eq!(state.tableau(1).unwrap().face_up(), expected_target);
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn single_card_moves_between_waste_tableau_and_foundation() {
        let mut state = fresh();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 1,
            },
        );
        let card = *state.waste().last().unwrap();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Waste,
                to: PileId::Foundation(2),
                count: 1,
            },
        );
        assert_eq!(state.foundation(2).unwrap(), &[card]);
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Foundation(2),
                to: PileId::Tableau(3),
                count: 1,
            },
        );
        assert!(state.foundation(2).unwrap().is_empty());
        assert_eq!(*state.tableau(3).unwrap().face_up().last().unwrap(), card);
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(3),
                to: PileId::Foundation(0),
                count: 1,
            },
        );
        assert_eq!(state.foundation(0).unwrap(), &[card]);
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn card_flipped_turns_the_top_face_down_card_face_up() {
        let mut state = fresh();
        // Empty tableau 1's face-up run mechanically, exposing its one
        // face-down card.
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(1),
                to: PileId::Tableau(2),
                count: 1,
            },
        );
        let pile = state.tableau(1).unwrap();
        assert_eq!(pile.face_down().len(), 1);
        assert!(pile.face_up().is_empty());
        let hidden = pile.face_down()[0];
        evolve(
            &mut state,
            Event::CardFlipped {
                pile: PileId::Tableau(1),
            },
        );
        let pile = state.tableau(1).unwrap();
        assert!(pile.face_down().is_empty());
        assert_eq!(pile.face_up(), &[hidden]);
    }

    #[test]
    fn waste_pass_completed_turns_the_waste_over_into_the_stock() {
        let mut state = fresh();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 24,
            },
        );
        assert!(state.stock().is_empty());
        let waste: Vec<Card> = state.waste().to_vec();
        evolve(&mut state, Event::WastePassCompleted);
        assert!(state.waste().is_empty());
        assert_eq!(state.stock().len(), 24);
        assert_eq!(state.passes_completed(), 1);
        // Turning the pile over: the first card drawn in the previous pass
        // (deepest in the waste) is on top of the stock, drawn first again.
        assert_eq!(*state.stock().last().unwrap(), waste[0]);
        assert_eq!(*state.stock().first().unwrap(), *waste.last().unwrap());
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn score_changes_apply_blindly_and_saturate() {
        let mut state = fresh();
        evolve(&mut state, Event::ScoreChanged { delta: 10 });
        assert_eq!(state.score(), 10);
        evolve(&mut state, Event::ScoreChanged { delta: -25 });
        assert_eq!(state.score(), -15, "evolve never floors — decide does");
        evolve(&mut state, Event::ScoreChanged { delta: i32::MAX });
        evolve(&mut state, Event::ScoreChanged { delta: i32::MAX });
        assert_eq!(state.score(), i32::MAX, "saturates instead of overflowing");
        evolve(&mut state, Event::ScoreChanged { delta: i32::MIN });
        evolve(&mut state, Event::ScoreChanged { delta: i32::MIN });
        assert_eq!(state.score(), i32::MIN.saturating_add(1).saturating_sub(1));
    }

    #[test]
    fn time_advanced_sets_the_elapsed_clock() {
        let mut state = fresh();
        evolve(
            &mut state,
            Event::TimeAdvanced {
                total_elapsed_secs: 61,
            },
        );
        assert_eq!(state.elapsed_secs(), 61);
    }

    /// `TimeAdvanced` carries the total, not a step, so `evolve` applies it
    /// verbatim — a backwards value rewinds the clock rather than being
    /// clamped. That is deliberate and mirrors the score: `evolve` is a
    /// total applier of events that already happened, and every policy about
    /// which events may happen lives in `decide`.
    #[test]
    fn a_backwards_time_advance_is_applied_verbatim_not_clamped() {
        let mut state = fresh();
        evolve(
            &mut state,
            Event::TimeAdvanced {
                total_elapsed_secs: 120,
            },
        );
        assert_eq!(state.elapsed_secs(), 120);

        evolve(
            &mut state,
            Event::TimeAdvanced {
                total_elapsed_secs: 30,
            },
        );
        assert_eq!(
            state.elapsed_secs(),
            30,
            "evolve never clamps the clock — decide does"
        );
    }

    #[test]
    fn game_won_sets_the_flag() {
        let mut state = fresh();
        assert!(!state.is_won());
        evolve(&mut state, Event::GameWon);
        assert!(state.is_won());
    }

    #[test]
    fn impossible_events_are_harmless_no_ops() {
        let mut state = fresh();
        let before = state.clone();
        // Sources that are empty or out of range, targets out of range, a
        // move onto the stock, zero counts, flips of non-tableau piles: all
        // events decide can never emit. Evolve must not panic, must not lose
        // cards, and must leave a deterministic state.
        for event in [
            Event::CardsMoved {
                from: PileId::Waste,
                to: PileId::Tableau(0),
                count: 1,
            },
            Event::CardsMoved {
                from: PileId::Tableau(9),
                to: PileId::Tableau(0),
                count: 1,
            },
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(200),
                count: 1,
            },
            Event::CardsMoved {
                from: PileId::Tableau(9),
                to: PileId::Tableau(11),
                count: 3,
            },
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 0,
            },
            // A move onto the pile it came from: the cards end up exactly
            // where they started, so nothing may shift and no face-down
            // card underneath may turn over.
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Tableau(0),
                count: 1,
            },
            Event::CardsMoved {
                from: PileId::Waste,
                to: PileId::Waste,
                count: 1,
            },
            // A zero-delta score change: `saturating_add(0)` is the
            // identity, and it must stay one at every score, including the
            // saturation boundaries.
            Event::ScoreChanged { delta: 0 },
            Event::CardFlipped {
                pile: PileId::Stock,
            },
            Event::CardFlipped {
                pile: PileId::Waste,
            },
            Event::CardFlipped {
                pile: PileId::Foundation(1),
            },
            Event::CardFlipped {
                pile: PileId::Tableau(77),
            },
            Event::CardFlipped {
                pile: PileId::Tableau(0),
            },
        ] {
            evolve(&mut state, event);
        }
        assert_eq!(state, before, "all impossible events left state untouched");
    }

    #[test]
    fn oversized_counts_clamp_to_what_the_pile_holds() {
        let mut state = fresh();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 200,
            },
        );
        assert!(state.stock().is_empty());
        assert_eq!(state.waste().len(), 24);
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(6),
                to: PileId::Tableau(0),
                count: 99,
            },
        );
        // Only the face-up run can move; the six face-down cards stay.
        assert_eq!(state.tableau(6).unwrap().face_down().len(), 6);
        assert!(state.tableau(6).unwrap().face_up().is_empty());
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn moves_involving_the_stock_as_target_still_conserve_cards() {
        let mut state = fresh();
        // decide never emits these; mechanics still move the top card.
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Stock,
                count: 1,
            },
        );
        assert_eq!(state.stock().len(), 25);
        assert!(state.tableau(0).unwrap().face_up().is_empty());
        assert_eq!(total_cards(&state), 52);
    }

    #[test]
    fn waste_pass_completed_with_empty_waste_only_counts_the_pass() {
        let mut state = fresh();
        let stock_before: Vec<Card> = state.stock().to_vec();
        evolve(&mut state, Event::WastePassCompleted);
        assert_eq!(state.stock(), stock_before.as_slice());
        assert_eq!(state.passes_completed(), 1);
    }
}
