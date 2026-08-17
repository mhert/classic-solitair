//! Exhaustive scoring tests: every scoring rule for Standard (with the
//! floor at 0 and the timed decay/bonus), Vegas, and None.
//!
//! Card positions come from the deal of [`SEED`] (see `rules_moves.rs`
//! for the layout).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{apply, config, dealt, total_cards};
use sol_engine::{
    Command, DrawMode, Event, GameState, PileId, RuleError, ScoringMode, decide, evolve,
};

/// The deal these tests share with `rules_moves.rs`.
const SEED: u16 = 8622;

fn draws(state: &mut GameState, times: usize) {
    for _ in 0..times {
        apply(state, Command::Draw);
    }
}

fn move_cards(from: PileId, to: PileId, count: u8) -> Command {
    Command::MoveCards { from, to, count }
}

fn tick(total_elapsed_secs: u32) -> Command {
    Command::Tick { total_elapsed_secs }
}

/// Stages every card except tableau 0's clubs ace onto foundation 0 with
/// synthetic events (no score side effects), leaving a legal winning move.
fn stage_51_cards_on_foundations(state: &mut GameState) {
    evolve(
        state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Foundation(0),
            count: 24,
        },
    );
    for index in 1..7_u8 {
        loop {
            let pile = state.tableau(index).unwrap();
            let face_up = u8::try_from(pile.face_up().len()).unwrap();
            if face_up > 0 {
                evolve(
                    state,
                    Event::CardsMoved {
                        from: PileId::Tableau(index),
                        to: PileId::Foundation(0),
                        count: face_up,
                    },
                );
            } else if pile.face_down().is_empty() {
                break;
            } else {
                evolve(
                    state,
                    Event::CardFlipped {
                        pile: PileId::Tableau(index),
                    },
                );
            }
        }
    }
    assert_eq!(state.foundation_card_count(), 51);
}

// ---------------------------------------------------------------- Standard

#[test]
fn standard_waste_to_tableau_scores_5() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    draws(&mut state, 1);
    let events = apply(&mut state, move_cards(PileId::Waste, PileId::Tableau(1), 1));
    assert_eq!(events.get(1), Some(&Event::ScoreChanged { delta: 5 }));
    assert_eq!(state.score(), 5);
}

#[test]
fn standard_waste_to_foundation_scores_10() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    draws(&mut state, 2);
    let events = apply(
        &mut state,
        move_cards(PileId::Waste, PileId::Foundation(0), 1),
    );
    assert_eq!(events.get(1), Some(&Event::ScoreChanged { delta: 10 }));
    assert_eq!(state.score(), 20);
}

#[test]
fn standard_tableau_to_foundation_scores_10_and_a_flip_scores_5() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(0), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(5),
                to: PileId::Foundation(0),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
            Event::CardFlipped {
                pile: PileId::Tableau(5),
            },
            Event::ScoreChanged { delta: 5 },
        ]
    );
    assert_eq!(state.score(), 15);
}

#[test]
fn standard_foundation_to_tableau_costs_15() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    // C1 -> F0 (+10), the clubs two off the waste -> F0 (+10), then the
    // spades ace off tableau 5 -> F1 (+10) with its flip (+5). The ace can
    // come straight back down onto the red two it uncovered: -15.
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    draws(&mut state, 2);
    apply(
        &mut state,
        move_cards(PileId::Waste, PileId::Foundation(0), 1),
    );
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(1), 1),
    );
    assert_eq!(state.score(), 35);
    let events = apply(
        &mut state,
        move_cards(PileId::Foundation(1), PileId::Tableau(5), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Foundation(1),
                to: PileId::Tableau(5),
                count: 1,
            },
            Event::ScoreChanged { delta: -15 },
        ]
    );
    assert_eq!(state.score(), 20);
    assert_eq!(total_cards(&state), 52);
}

#[test]
fn standard_score_floors_at_zero_and_floored_deltas_are_not_emitted() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    // Stage the same C2-comes-back-down position purely synthetically, so
    // the score is still 0 when the -15 move happens.
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Tableau(5),
            to: PileId::Foundation(1),
            count: 1,
        },
    );
    evolve(
        &mut state,
        Event::CardFlipped {
            pile: PileId::Tableau(5),
        },
    );
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Foundation(3),
            count: 1,
        },
    );
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Foundation(0),
            count: 1,
        },
    );
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Foundation(3),
            count: 21,
        },
    );
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Tableau(5),
            count: 1,
        },
    );
    assert_eq!(
        state.foundation(0).unwrap().last().unwrap().to_string(),
        "C2"
    );
    assert_eq!(
        state
            .tableau(5)
            .unwrap()
            .face_up()
            .last()
            .unwrap()
            .to_string(),
        "D3"
    );
    assert_eq!(state.score(), 0);
    let events = apply(
        &mut state,
        move_cards(PileId::Foundation(0), PileId::Tableau(5), 1),
    );
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Foundation(0),
            to: PileId::Tableau(5),
            count: 1,
        }],
        "a delta floored to nothing emits no ScoreChanged at all"
    );
    assert_eq!(state.score(), 0);
}

#[test]
fn standard_draw_one_recycles_cost_100_from_the_first_recycle() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false));
    evolve(&mut state, Event::ScoreChanged { delta: 150 });
    draws(&mut state, 24);
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![
            Event::WastePassCompleted,
            Event::ScoreChanged { delta: -100 },
        ]
    );
    assert_eq!(state.score(), 50);
    // The next recycle floors: max(-100, -50) = -50.
    draws(&mut state, 24);
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![
            Event::WastePassCompleted,
            Event::ScoreChanged { delta: -50 }
        ]
    );
    assert_eq!(state.score(), 0);
}

#[test]
fn standard_draw_three_recycles_cost_20_only_after_the_fourth_pass() {
    let mut state = dealt(SEED, config(DrawMode::Three, ScoringMode::Standard, false));
    evolve(&mut state, Event::ScoreChanged { delta: 100 });
    for free_recycle in 1..=3_u32 {
        draws(&mut state, 8);
        let events = apply(&mut state, Command::Draw);
        assert_eq!(
            events,
            vec![Event::WastePassCompleted],
            "recycle {free_recycle} into passes 2..=4 is free"
        );
    }
    assert_eq!(state.score(), 100);
    draws(&mut state, 8);
    let events = apply(&mut state, Command::Draw);
    assert_eq!(
        events,
        vec![
            Event::WastePassCompleted,
            Event::ScoreChanged { delta: -20 }
        ]
    );
    assert_eq!(state.score(), 80);
}

// ------------------------------------------------------------ timed decay

#[test]
fn timed_standard_ticks_advance_the_clock_and_decay_2_per_10_seconds() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert_eq!(state.score(), 10);
    let events = apply(&mut state, tick(5));
    assert_eq!(
        events,
        vec![Event::TimeAdvanced {
            total_elapsed_secs: 5,
        }]
    );
    let events = apply(&mut state, tick(9));
    assert_eq!(
        events,
        vec![Event::TimeAdvanced {
            total_elapsed_secs: 9,
        }]
    );
    let events = apply(&mut state, tick(10));
    assert_eq!(
        events,
        vec![
            Event::TimeAdvanced {
                total_elapsed_secs: 10,
            },
            Event::ScoreChanged { delta: -2 },
        ]
    );
    assert_eq!(state.score(), 8);
    // A lagging host may skip boundaries; they all decay at once.
    let events = apply(&mut state, tick(35));
    assert_eq!(
        events,
        vec![
            Event::TimeAdvanced {
                total_elapsed_secs: 35,
            },
            Event::ScoreChanged { delta: -4 },
        ]
    );
    assert_eq!(state.score(), 4);
    assert_eq!(state.elapsed_secs(), 35);
}

#[test]
fn timed_decay_floors_at_zero_without_emitting_empty_deltas() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    let events = apply(&mut state, tick(60));
    assert_eq!(
        events,
        vec![
            Event::TimeAdvanced {
                total_elapsed_secs: 60,
            },
            Event::ScoreChanged { delta: -10 },
        ],
        "six boundaries would cost 12; the floor stops at the score of 10"
    );
    assert_eq!(state.score(), 0);
    let events = apply(&mut state, tick(70));
    assert_eq!(
        events,
        vec![Event::TimeAdvanced {
            total_elapsed_secs: 70,
        }],
        "decay on a zero score emits no ScoreChanged"
    );
}

#[test]
fn a_repeated_tick_is_a_silent_no_op_and_a_backwards_tick_is_rejected() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    apply(&mut state, tick(35));
    assert_eq!(decide(&state, tick(35)).unwrap(), vec![]);
    assert_eq!(
        decide(&state, tick(34)).unwrap_err(),
        RuleError::TickInPast {
            reported: 34,
            current: 35,
        }
    );
}

#[test]
fn untimed_vegas_and_none_games_ignore_ticks_entirely() {
    let configs = [
        config(DrawMode::One, ScoringMode::Standard, false),
        config(DrawMode::One, ScoringMode::Vegas, true),
        config(DrawMode::One, ScoringMode::None, true),
    ];
    for game_config in configs {
        let mut state = dealt(SEED, game_config);
        assert_eq!(decide(&state, tick(50)).unwrap(), vec![], "{game_config:?}");
        apply(&mut state, tick(50));
        assert_eq!(state.elapsed_secs(), 0, "{game_config:?}");
        // With no clock tracked, an "earlier" tick is not in the past.
        assert_eq!(decide(&state, tick(3)).unwrap(), vec![], "{game_config:?}");
    }
}

#[test]
fn ticks_after_the_win_are_silently_ignored() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    evolve(&mut state, Event::GameWon);
    assert_eq!(decide(&state, tick(100)).unwrap(), vec![]);
}

// -------------------------------------------------------------- win bonus

#[test]
fn timed_wins_over_30_seconds_earn_700_000_over_seconds() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    stage_51_cards_on_foundations(&mut state);
    apply(&mut state, tick(45));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
            Event::ScoreChanged { delta: 15_555 },
            Event::GameWon,
        ],
        "bonus is the integer quotient 700000 / 45"
    );
    assert_eq!(state.score(), 15_565);
    assert!(state.is_won());
}

/// The first second that earns a bonus. Tested beside the 30-second case
/// because together they pin the boundary itself, and the exact delta pins
/// the numerator: 700000/31 is 22580, a value no other numerator produces.
#[test]
fn a_win_at_31_seconds_earns_the_largest_bonus_there_is() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    stage_51_cards_on_foundations(&mut state);
    apply(&mut state, tick(31));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
            Event::ScoreChanged { delta: 22_580 },
            Event::GameWon,
        ],
        "bonus is the integer quotient 700000 / 31"
    );
    assert!(state.is_won());
}

#[test]
fn wins_at_30_seconds_or_faster_get_no_bonus() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    stage_51_cards_on_foundations(&mut state);
    apply(&mut state, tick(30));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
            Event::GameWon,
        ]
    );
}

#[test]
fn extremely_long_wins_round_the_bonus_down_to_nothing() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Standard, true));
    stage_51_cards_on_foundations(&mut state);
    apply(&mut state, tick(700_001));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
            Event::GameWon,
        ]
    );
}

// ------------------------------------------------------------------ Vegas

#[test]
fn vegas_pays_5_per_foundation_card_and_refunds_5_when_one_leaves() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Vegas, false));
    assert_eq!(state.score(), -52, "the buy-in is charged at the deal");
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert_eq!(events.get(1), Some(&Event::ScoreChanged { delta: 5 }));
    assert_eq!(state.score(), -47);
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(5),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 5 },
            Event::CardFlipped {
                pile: PileId::Tableau(5),
            },
        ],
        "flips pay nothing in Vegas"
    );
    assert_eq!(state.score(), -42);
    draws(&mut state, 2);
    apply(
        &mut state,
        move_cards(PileId::Waste, PileId::Foundation(0), 1),
    );
    assert_eq!(state.score(), -37);
    let events = apply(&mut state, move_cards(PileId::Waste, PileId::Tableau(1), 1));
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Waste,
            to: PileId::Tableau(1),
            count: 1,
        }],
        "waste to tableau pays nothing in Vegas"
    );
    let events = apply(
        &mut state,
        move_cards(PileId::Foundation(1), PileId::Tableau(5), 1),
    );
    assert_eq!(events.get(1), Some(&Event::ScoreChanged { delta: -5 }));
    assert_eq!(state.score(), -42);
}

#[test]
fn a_vegas_win_pays_the_last_5_and_no_bonus() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::Vegas, true));
    stage_51_cards_on_foundations(&mut state);
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::ScoreChanged { delta: 5 },
            Event::GameWon,
        ]
    );
    assert_eq!(state.score(), -47);
}

// ------------------------------------------------------------------- None

#[test]
fn none_scoring_emits_no_score_events_for_anything() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::None, true));
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert_eq!(
        events,
        vec![Event::CardsMoved {
            from: PileId::Tableau(0),
            to: PileId::Foundation(0),
            count: 1,
        }]
    );
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(5),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::CardFlipped {
                pile: PileId::Tableau(5),
            },
        ]
    );
    draws(&mut state, 24);
    apply(&mut state, Command::Draw);
    assert_eq!(state.score(), 0);
    assert_eq!(state.passes_completed(), 1);
}

#[test]
fn a_none_scoring_win_emits_only_the_move_and_the_win() {
    let mut state = dealt(SEED, config(DrawMode::One, ScoringMode::None, true));
    stage_51_cards_on_foundations(&mut state);
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(1), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(1),
                count: 1,
            },
            Event::GameWon,
        ]
    );
    assert_eq!(state.score(), 0);
}
