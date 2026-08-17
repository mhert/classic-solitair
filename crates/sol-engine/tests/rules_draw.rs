//! Rules tests: Draw One / Draw Three and waste recycling, including
//! Vegas pass limits.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{apply, config, dealt, total_cards};
use sol_engine::{Command, DrawMode, Event, PileId, RuleError, ScoringMode, decide, evolve};

#[test]
fn draw_one_turns_exactly_one_card_onto_the_waste() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::Standard, false));
    let stock_top = *state.stock().last().unwrap();
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count: 1,
        }]
    );
    assert_eq!(state.waste(), &[stock_top]);
    assert_eq!(state.stock().len(), 23);
}

#[test]
fn draw_three_turns_three_cards_fanning_the_last_drawn_on_top() {
    let mut state = dealt(1, config(DrawMode::Three, ScoringMode::Standard, false));
    let stock_before = state.stock().to_vec();
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count: 3,
        }]
    );
    assert_eq!(
        state.waste(),
        &[stock_before[23], stock_before[22], stock_before[21]],
        "cards turn over one by one; the last drawn is the waste top"
    );
    assert_eq!(state.stock().len(), 21);
}

#[test]
fn a_short_stock_draws_only_what_is_left() {
    let mut state = dealt(1, config(DrawMode::Three, ScoringMode::Standard, false));
    // Shrink the stock to 2 with a synthetic mechanics-only event.
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Tableau(0),
            count: 22,
        },
    );
    assert_eq!(state.stock().len(), 2);
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count: 2,
        }]
    );
    assert!(state.stock().is_empty());
    assert_eq!(total_cards(&state), 52);
}

#[test]
fn drawing_on_an_empty_stock_recycles_the_waste_into_a_new_pass() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::Standard, false));
    for _ in 0..24 {
        apply(&mut state, Command::Draw);
    }
    assert!(state.stock().is_empty());
    let first_drawn = state.waste()[0];
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![Event::WastePassCompleted],
        "with a score of 0 the recycle penalty floors away entirely"
    );
    assert_eq!(state.passes_completed(), 1);
    assert!(state.waste().is_empty());
    assert_eq!(state.stock().len(), 24);
    assert_eq!(
        *state.stock().last().unwrap(),
        first_drawn,
        "turning the waste over puts the first-drawn card back on top"
    );
    assert_eq!(total_cards(&state), 52);
}

#[test]
fn draw_with_stock_and_waste_both_empty_is_rejected() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::Standard, false));
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Tableau(0),
            count: 24,
        },
    );
    assert!(state.stock().is_empty() && state.waste().is_empty());
    assert_eq!(
        decide(&state, Command::Draw).unwrap_err(),
        RuleError::NothingToDraw
    );
}

#[test]
fn vegas_draw_one_gets_a_single_pass_and_no_recycle() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::Vegas, false));
    for _ in 0..24 {
        apply(&mut state, Command::Draw);
    }
    assert_eq!(
        decide(&state, Command::Draw).unwrap_err(),
        RuleError::NoMorePasses
    );
    assert_eq!(state.passes_completed(), 0);
    assert_eq!(
        state.waste().len(),
        24,
        "the rejected command changed nothing"
    );
}

#[test]
fn vegas_draw_three_gets_three_passes_then_rejects() {
    let mut state = dealt(1, config(DrawMode::Three, ScoringMode::Vegas, false));
    for recycle in 1..=2_u32 {
        for _ in 0..8 {
            apply(&mut state, Command::Draw);
        }
        let events = apply(&mut state, Command::Draw);
        assert_eq!(events, vec![Event::WastePassCompleted]);
        assert_eq!(state.passes_completed(), recycle);
    }
    for _ in 0..8 {
        apply(&mut state, Command::Draw);
    }
    assert_eq!(
        decide(&state, Command::Draw).unwrap_err(),
        RuleError::NoMorePasses
    );
    assert_eq!(state.passes_completed(), 2);
}

#[test]
fn standard_and_none_scoring_allow_unlimited_passes() {
    for scoring in [ScoringMode::Standard, ScoringMode::None] {
        let mut state = dealt(1, config(DrawMode::One, scoring, false));
        for pass in 1..=3_u32 {
            for _ in 0..24 {
                apply(&mut state, Command::Draw);
            }
            apply(&mut state, Command::Draw);
            assert_eq!(state.passes_completed(), pass, "{scoring:?}");
        }
    }
}

#[test]
fn none_scoring_never_emits_score_events_on_recycle() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::None, false));
    for _ in 0..24 {
        apply(&mut state, Command::Draw);
    }
    let events = apply(&mut state, Command::Draw);
    assert_eq!(events, vec![Event::WastePassCompleted]);
    assert_eq!(state.score(), 0);
}

#[test]
fn draw_is_rejected_once_the_game_is_won() {
    let mut state = dealt(1, config(DrawMode::One, ScoringMode::Standard, false));
    evolve(&mut state, Event::GameWon);
    assert_eq!(
        decide(&state, Command::Draw).unwrap_err(),
        RuleError::GameAlreadyWon
    );
}
