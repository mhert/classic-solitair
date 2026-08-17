//! The decision side: [`decide`] validates a [`Command`] against the rules
//! and materializes every consequence — moves, flips, score changes, pass
//! completions, time, the win — as an ordered list of events.
//!
//! All rule knowledge lives here and nowhere else. Score deltas are floored
//! at decision time (Standard scoring never drops below 0), so
//! [`crate::evolve`] can apply them blindly.

use crate::card::{Card, Rank};
use crate::command::Command;
use crate::config::{DrawMode, ScoringMode};
use crate::event::Event;
use crate::pile::{FOUNDATION_COUNT, PileId};
use crate::score::{
    FOUNDATION_TO_TABLEAU, FREE_PASSES_DRAW_ONE, FREE_PASSES_DRAW_THREE, RECYCLE_PENALTY_DRAW_ONE,
    RECYCLE_PENALTY_DRAW_THREE, TABLEAU_FLIP, TABLEAU_TO_FOUNDATION, TIME_DECAY_DELTA,
    TIME_DECAY_INTERVAL_SECS, VEGAS_CARD_OFF_FOUNDATION, VEGAS_CARD_TO_FOUNDATION,
    VEGAS_PASS_LIMIT_DRAW_ONE, VEGAS_PASS_LIMIT_DRAW_THREE, WASTE_TO_FOUNDATION, WASTE_TO_TABLEAU,
    WIN_BONUS_MIN_ELAPSED_SECS, WIN_BONUS_NUMERATOR,
};
use crate::state::GameState;

/// A rejected command: why the rules refuse it, in player terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RuleError {
    /// The command names a pile that does not exist on the table.
    #[error("no such pile: {pile:?}")]
    UnknownPile {
        /// The unknown pile id.
        pile: PileId,
    },
    /// Draw with both the stock and the waste empty.
    #[error("nothing to draw: the stock and the waste are both empty")]
    NothingToDraw,
    /// Vegas pass limit reached; the waste may not be recycled again.
    #[error("no more passes through the stock are allowed")]
    NoMorePasses,
    /// Cards never move between this combination of piles.
    #[error("cards cannot move from {from:?} to {to:?}")]
    MoveNotAllowed {
        /// Source pile.
        from: PileId,
        /// Target pile.
        to: PileId,
    },
    /// The source pile does not hold the requested cards face-up.
    #[error("the source pile does not hold that many movable cards")]
    NothingToMove,
    /// More than one card where only single cards may move.
    #[error("only a single card may move from {from:?} to {to:?}")]
    TooManyCards {
        /// Source pile.
        from: PileId,
        /// Target pile.
        to: PileId,
    },
    /// Tableau builds go alternating-color, descending by one.
    #[error("a tableau card must go on an alternating-color card one rank higher")]
    IllegalTableauMove,
    /// Only kings may occupy an emptied tableau pile.
    #[error("only a king may move onto an empty tableau pile")]
    EmptyTableauNeedsKing,
    /// Foundations build same-suit, ascending from the ace.
    #[error("a foundation builds up in suit from the ace")]
    IllegalFoundationMove,
    /// Double-clicked card has no foundation to go to.
    #[error("no foundation can take that card")]
    NoEligibleFoundation,
    /// Undo/redo are rejected in Vegas scoring.
    #[error("undo and redo are not allowed in Vegas scoring")]
    UndoNotAllowed,
    /// Undo with no player command left to take back.
    #[error("nothing to undo")]
    NothingToUndo,
    /// Redo with no undone command waiting.
    #[error("nothing to redo")]
    NothingToRedo,
    /// The game is over; only undo can reopen it.
    #[error("the game is already won")]
    GameAlreadyWon,
    /// The host reported an elapsed time earlier than one already reported.
    #[error("time cannot run backwards: tick {reported}s is before {current}s")]
    TickInPast {
        /// The elapsed seconds the tick reported.
        reported: u32,
        /// The elapsed seconds the engine had already seen.
        current: u32,
    },
}

/// Decides one command against the current state.
///
/// On success returns the ordered events materializing every consequence of
/// the command; folding them with [`crate::evolve`] yields the next state.
/// The state is never modified here.
///
/// ```
/// use sol_engine::{Command, DrawMode, GameConfig, PileId, ScoringMode, Seed, deal, decide};
///
/// let config = GameConfig {
///     draw_mode: DrawMode::One,
///     scoring: ScoringMode::Standard,
///     timed: false,
/// };
/// let state = deal(Seed::new(1).unwrap(), config);
/// let events = decide(&state, Command::Draw)?;
/// assert_eq!(events.len(), 1);
///
/// // The waste is still empty, so nothing can move off it.
/// let refused = decide(
///     &state,
///     Command::MoveCards {
///         from: PileId::Waste,
///         to: PileId::Tableau(0),
///         count: 1,
///     },
/// );
/// assert!(refused.is_err());
/// # Ok::<(), sol_engine::RuleError>(())
/// ```
///
/// # Errors
///
/// Returns the [`RuleError`] naming the violated rule; the command must not
/// be logged and the state must not change.
pub fn decide(state: &GameState, command: Command) -> Result<Vec<Event>, RuleError> {
    match command {
        Command::Draw => decide_draw(state),
        Command::MoveCards { from, to, count } => decide_move(state, from, to, count),
        Command::AutoToFoundation { pile } => decide_auto(state, pile),
        Command::Tick { total_elapsed_secs } => decide_tick(state, total_elapsed_secs),
    }
}

/// Tick: the host reports total elapsed play time. Only timed Standard
/// games track it; there it advances the engine clock and materializes the
/// −2-per-10-seconds decay, floored at 0. Everywhere else — untimed games,
/// Vegas, None, and finished games — ticks are silent no-ops that produce
/// no events (and are therefore never logged).
fn decide_tick(state: &GameState, total_elapsed_secs: u32) -> Result<Vec<Event>, RuleError> {
    let config = state.config();
    if state.is_won() || !config.timed || config.scoring != ScoringMode::Standard {
        return Ok(Vec::new());
    }
    let current = state.elapsed_secs();
    match total_elapsed_secs.cmp(&current) {
        core::cmp::Ordering::Less => Err(RuleError::TickInPast {
            reported: total_elapsed_secs,
            current,
        }),
        core::cmp::Ordering::Equal => Ok(Vec::new()),
        core::cmp::Ordering::Greater => {
            let mut events = vec![Event::TimeAdvanced { total_elapsed_secs }];
            let crossed =
                total_elapsed_secs / TIME_DECAY_INTERVAL_SECS - current / TIME_DECAY_INTERVAL_SECS;
            // Infallible: crossed is at most u32::MAX / 10. With no boundary
            // crossed the delta is 0 and nothing is emitted.
            let steps = i32::try_from(crossed).unwrap_or(i32::MAX);
            let delta = floored(state.score(), TIME_DECAY_DELTA.saturating_mul(steps));
            if delta != 0 {
                events.push(Event::ScoreChanged { delta });
            }
            Ok(events)
        }
    }
}

/// The per-move score deltas of one source/target pile combination.
#[derive(Clone, Copy)]
struct MoveScore {
    standard: i32,
    vegas: i32,
}

/// Validates a card move and materializes its full consequence: the move,
/// its score change, a possible auto-flip (with its score change), and a
/// possible win.
fn decide_move(
    state: &GameState,
    from: PileId,
    to: PileId,
    count: u8,
) -> Result<Vec<Event>, RuleError> {
    if state.is_won() {
        return Err(RuleError::GameAlreadyWon);
    }
    for pile in [from, to] {
        if !pile.is_valid() {
            return Err(RuleError::UnknownPile { pile });
        }
    }
    if count == 0 {
        return Err(RuleError::NothingToMove);
    }
    let score = match (from, to) {
        (PileId::Waste, PileId::Tableau(target)) => {
            single_card_only(from, to, count)?;
            let card = top_card(state, from)?;
            tableau_accepts(state, target, card)?;
            MoveScore {
                standard: WASTE_TO_TABLEAU,
                vegas: 0,
            }
        }
        (PileId::Waste, PileId::Foundation(target)) => {
            single_card_only(from, to, count)?;
            let card = top_card(state, from)?;
            foundation_accepts(state, target, card)?;
            MoveScore {
                standard: WASTE_TO_FOUNDATION,
                vegas: VEGAS_CARD_TO_FOUNDATION,
            }
        }
        (PileId::Tableau(source), PileId::Tableau(target)) => {
            let deepest = run_deepest_card(state, source, count)?;
            tableau_accepts(state, target, deepest)?;
            MoveScore {
                standard: 0,
                vegas: 0,
            }
        }
        (PileId::Tableau(_), PileId::Foundation(target)) => {
            single_card_only(from, to, count)?;
            let card = top_card(state, from)?;
            foundation_accepts(state, target, card)?;
            MoveScore {
                standard: TABLEAU_TO_FOUNDATION,
                vegas: VEGAS_CARD_TO_FOUNDATION,
            }
        }
        (PileId::Foundation(_), PileId::Tableau(target)) => {
            single_card_only(from, to, count)?;
            let card = top_card(state, from)?;
            tableau_accepts(state, target, card)?;
            MoveScore {
                standard: FOUNDATION_TO_TABLEAU,
                vegas: VEGAS_CARD_OFF_FOUNDATION,
            }
        }
        _ => return Err(RuleError::MoveNotAllowed { from, to }),
    };

    let mut events = vec![Event::CardsMoved { from, to, count }];
    let mut projected = state.score();
    push_score(state, &mut events, &mut projected, score);
    if let PileId::Tableau(source) = from
        && let Some(pile) = state.tableau(source)
        && pile.face_up().len() == usize::from(count)
        && !pile.face_down().is_empty()
    {
        events.push(Event::CardFlipped { pile: from });
        push_score(
            state,
            &mut events,
            &mut projected,
            MoveScore {
                standard: TABLEAU_FLIP,
                vegas: 0,
            },
        );
    }
    if matches!(to, PileId::Foundation(_)) && state.foundation_card_count() + 1 == 52 {
        let config = state.config();
        let elapsed = state.elapsed_secs();
        if config.timed
            && config.scoring == ScoringMode::Standard
            && elapsed > WIN_BONUS_MIN_ELAPSED_SECS
        {
            // Infallible: the quotient is at most 700_000 / 31.
            let bonus = i32::try_from(WIN_BONUS_NUMERATOR / elapsed).unwrap_or(0);
            if bonus > 0 {
                events.push(Event::ScoreChanged { delta: bonus });
            }
        }
        events.push(Event::GameWon);
    }
    Ok(events)
}

/// Double-click: the top card of `pile` goes to the eligible foundation, if
/// one exists — aces to the lowest-index empty slot, other ranks to their
/// suit's growing pile.
fn decide_auto(state: &GameState, pile: PileId) -> Result<Vec<Event>, RuleError> {
    if state.is_won() {
        return Err(RuleError::GameAlreadyWon);
    }
    if !pile.is_valid() {
        return Err(RuleError::UnknownPile { pile });
    }
    let card = match pile {
        // `top_card` yields nothing for the stock, matching the rule that
        // face-down stock cards never auto-move.
        PileId::Waste | PileId::Stock | PileId::Tableau(_) => top_card(state, pile).ok(),
        PileId::Foundation(_) => None,
    };
    let Some(card) = card else {
        return Err(RuleError::NoEligibleFoundation);
    };
    let target = (0..FOUNDATION_COUNT)
        .find(|&index| foundation_accepts(state, index, card).is_ok())
        .ok_or(RuleError::NoEligibleFoundation)?;
    decide_move(state, pile, PileId::Foundation(target), 1)
}

/// Appends the mode-appropriate `ScoreChanged` event, flooring Standard
/// deltas against the running projected score of this command's event list.
fn push_score(state: &GameState, events: &mut Vec<Event>, projected: &mut i32, score: MoveScore) {
    let delta = match state.config().scoring {
        ScoringMode::Standard => floored(*projected, score.standard),
        ScoringMode::Vegas => score.vegas,
        ScoringMode::None => 0,
    };
    if delta != 0 {
        events.push(Event::ScoreChanged { delta });
        *projected = projected.saturating_add(delta);
    }
}

/// Multi-card moves exist only between tableau piles.
fn single_card_only(from: PileId, to: PileId, count: u8) -> Result<(), RuleError> {
    if count > 1 {
        return Err(RuleError::TooManyCards { from, to });
    }
    Ok(())
}

/// The single movable top card of the waste or a foundation.
fn top_card(state: &GameState, pile: PileId) -> Result<Card, RuleError> {
    let card = match pile {
        PileId::Waste => state.waste().last(),
        PileId::Foundation(index) => state.foundation(index).and_then(<[Card]>::last),
        PileId::Tableau(index) => state.tableau(index).and_then(|t| t.face_up().last()),
        PileId::Stock => None,
    };
    card.copied().ok_or(RuleError::NothingToMove)
}

/// The deepest card of the `count`-card face-up run about to leave a tableau
/// pile — the card the target must accept.
fn run_deepest_card(state: &GameState, source: u8, count: u8) -> Result<Card, RuleError> {
    let face_up = state
        .tableau(source)
        .map(crate::state::TableauPile::face_up)
        .unwrap_or_default();
    let cut = face_up
        .len()
        .checked_sub(usize::from(count))
        .ok_or(RuleError::NothingToMove)?;
    face_up.get(cut).copied().ok_or(RuleError::NothingToMove)
}

/// Tableau legality: alternating colors, descending by exactly one; only
/// kings on a fully empty pile; a pile with only face-down cards accepts
/// nothing (unreachable in play — the auto-flip always turns one card up).
fn tableau_accepts(state: &GameState, target: u8, card: Card) -> Result<(), RuleError> {
    let pile = state.tableau(target).ok_or(RuleError::UnknownPile {
        pile: PileId::Tableau(target),
    })?;
    match pile.face_up().last() {
        Some(top) => {
            if card.color() != top.color() && card.rank.successor() == Some(top.rank) {
                Ok(())
            } else {
                Err(RuleError::IllegalTableauMove)
            }
        }
        None if pile.face_down().is_empty() => {
            if card.rank == Rank::King {
                Ok(())
            } else {
                Err(RuleError::EmptyTableauNeedsKing)
            }
        }
        None => Err(RuleError::IllegalTableauMove),
    }
}

/// Foundation legality: an ace starts an empty slot; otherwise same suit,
/// ascending by exactly one.
fn foundation_accepts(state: &GameState, target: u8, card: Card) -> Result<(), RuleError> {
    let foundation = state.foundation(target).ok_or(RuleError::UnknownPile {
        pile: PileId::Foundation(target),
    })?;
    match foundation.last() {
        Some(top) => {
            if top.suit == card.suit && top.rank.successor() == Some(card.rank) {
                Ok(())
            } else {
                Err(RuleError::IllegalFoundationMove)
            }
        }
        None => {
            if card.rank == Rank::Ace {
                Ok(())
            } else {
                Err(RuleError::IllegalFoundationMove)
            }
        }
    }
}

/// Draw: turn cards from the stock, or — on an empty stock — recycle the
/// waste into a new pass, subject to the Vegas pass limits and the Standard
/// recycle penalties.
fn decide_draw(state: &GameState) -> Result<Vec<Event>, RuleError> {
    if state.is_won() {
        return Err(RuleError::GameAlreadyWon);
    }
    let config = state.config();
    let stock_len = state.stock().len();
    if stock_len > 0 {
        let turned = stock_len.min(usize::from(config.draw_mode.cards_per_draw()));
        // Infallible: turned is at most 3.
        let count = u8::try_from(turned).unwrap_or(1);
        return Ok(vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count,
        }]);
    }
    if state.waste().is_empty() {
        return Err(RuleError::NothingToDraw);
    }
    let completed = state.passes_completed();
    if config.scoring == ScoringMode::Vegas {
        let limit = match config.draw_mode {
            DrawMode::One => VEGAS_PASS_LIMIT_DRAW_ONE,
            DrawMode::Three => VEGAS_PASS_LIMIT_DRAW_THREE,
        };
        if completed.saturating_add(1) >= limit {
            return Err(RuleError::NoMorePasses);
        }
    }
    let mut events = vec![Event::WastePassCompleted];
    if config.scoring == ScoringMode::Standard {
        let (free_passes, penalty) = match config.draw_mode {
            DrawMode::One => (FREE_PASSES_DRAW_ONE, RECYCLE_PENALTY_DRAW_ONE),
            DrawMode::Three => (FREE_PASSES_DRAW_THREE, RECYCLE_PENALTY_DRAW_THREE),
        };
        let entering_pass = completed.saturating_add(2);
        if entering_pass > free_passes {
            let delta = floored(state.score(), penalty);
            if delta != 0 {
                events.push(Event::ScoreChanged { delta });
            }
        }
    }
    Ok(events)
}

/// Floors a Standard-scoring delta so the score never drops below 0. The
/// running score is never negative in Standard scoring, so the clamp is
/// simply `max(delta, -score)`.
fn floored(score: i32, delta: i32) -> i32 {
    delta.max(score.saturating_neg())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_error_renders_a_message() {
        let errors = [
            RuleError::UnknownPile {
                pile: PileId::Tableau(9),
            },
            RuleError::NothingToDraw,
            RuleError::NoMorePasses,
            RuleError::MoveNotAllowed {
                from: PileId::Stock,
                to: PileId::Waste,
            },
            RuleError::NothingToMove,
            RuleError::TooManyCards {
                from: PileId::Waste,
                to: PileId::Foundation(0),
            },
            RuleError::IllegalTableauMove,
            RuleError::EmptyTableauNeedsKing,
            RuleError::IllegalFoundationMove,
            RuleError::NoEligibleFoundation,
            RuleError::UndoNotAllowed,
            RuleError::NothingToUndo,
            RuleError::NothingToRedo,
            RuleError::GameAlreadyWon,
            RuleError::TickInPast {
                reported: 3,
                current: 8,
            },
        ];
        for error in errors {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }

    /// The four variants that interpolate a field have to actually name it.
    /// These messages reach players through both frontends' status bars, so
    /// a rejection that dropped the pile or the second it disagreed about
    /// would leave nothing to act on.
    ///
    /// The interpolated values are asserted, not the prose around them: the
    /// wording is free to change, the facts are not.
    #[test]
    fn every_interpolated_field_reaches_the_message() {
        let message = RuleError::UnknownPile {
            pile: PileId::Tableau(9),
        }
        .to_string();
        assert!(message.contains("Tableau(9)"), "{message}");

        let message = RuleError::MoveNotAllowed {
            from: PileId::Stock,
            to: PileId::Foundation(2),
        }
        .to_string();
        assert!(message.contains("Stock"), "{message}");
        assert!(message.contains("Foundation(2)"), "{message}");

        let message = RuleError::TooManyCards {
            from: PileId::Waste,
            to: PileId::Foundation(0),
        }
        .to_string();
        assert!(message.contains("Waste"), "{message}");
        assert!(message.contains("Foundation(0)"), "{message}");

        let message = RuleError::TickInPast {
            reported: 3,
            current: 8,
        }
        .to_string();
        assert!(message.contains('3'), "{message}");
        assert!(message.contains('8'), "{message}");
    }

    /// Two rejections that differ only in their fields must read
    /// differently, or the message is not carrying the field at all.
    #[test]
    fn rejections_differing_only_in_their_fields_read_differently() {
        let one = RuleError::MoveNotAllowed {
            from: PileId::Stock,
            to: PileId::Foundation(2),
        };
        let other = RuleError::MoveNotAllowed {
            from: PileId::Waste,
            to: PileId::Tableau(3),
        };
        assert_ne!(one.to_string(), other.to_string());
    }
}
