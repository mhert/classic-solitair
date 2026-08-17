//! Rules tests: tableau and foundation legality, run moves, auto-flip,
//! `AutoToFoundation`, and win detection.
//!
//! Concrete cards come from the deal of [`SEED`], picked for a board that
//! shows an ace on tableau 0, a second ace on tableau 5, and the clubs two
//! near the front of the stock:
//!
//! ```text
//! tableau0 down= up=C1
//! tableau1 down=S2 up=D7
//! tableau2 down=S13,D1 up=H4
//! tableau3 down=C10,D9,C11 up=D11
//! tableau4 down=D4,H3,C12,D6 up=S7
//! tableau5 down=C8,H2,H10,S8,D2 up=S1
//! tableau6 down=H1,C5,C7,H8,H5,S10 up=S3
//! stock=C6,C2,S11,S12,D12,D13,H13,C9,C13,C4,D10,H9,D5,C3,H7,S4,H11,H12,H6,D8,S9,S5,S6,D3
//! ```

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

/// The deal these tests are written against; see the module documentation
/// for the board it lays out.
const SEED: u16 = 8622;

fn standard_one() -> GameState {
    dealt(SEED, config(DrawMode::One, ScoringMode::Standard, false))
}

fn draws(state: &mut GameState, times: usize) {
    for _ in 0..times {
        apply(state, Command::Draw);
    }
}

fn move_cards(from: PileId, to: PileId, count: u8) -> Command {
    Command::MoveCards { from, to, count }
}

fn top_name(state: &GameState, pile: PileId) -> String {
    match pile {
        PileId::Waste => state.waste().last().unwrap().to_string(),
        PileId::Foundation(i) => state.foundation(i).unwrap().last().unwrap().to_string(),
        PileId::Tableau(i) => state
            .tableau(i)
            .unwrap()
            .face_up()
            .last()
            .unwrap()
            .to_string(),
        PileId::Stock => state.stock().last().unwrap().to_string(),
    }
}

/// Mechanically dumps every card except tableau 0's lone ace onto foundation
/// 0 via synthetic events — evolve validates nothing, which lets tests stage
/// a near-won table without playing a full game.
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

#[test]
fn an_ace_moves_onto_any_empty_foundation() {
    let mut state = standard_one();
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
        ]
    );
    assert_eq!(top_name(&state, PileId::Foundation(0)), "C1");
    assert!(state.tableau(0).unwrap().is_empty());

    // The spades ace is equally welcome on any other empty foundation slot.
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(3), 1),
    );
    assert_eq!(
        events.first().unwrap(),
        &Event::CardsMoved {
            from: PileId::Tableau(5),
            to: PileId::Foundation(3),
            count: 1,
        }
    );
    assert_eq!(top_name(&state, PileId::Foundation(3)), "S1");
}

#[test]
fn a_non_ace_may_not_start_a_foundation() {
    let state = standard_one();
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(2), PileId::Foundation(3), 1)
        )
        .unwrap_err(),
        RuleError::IllegalFoundationMove
    );
}

#[test]
fn foundations_build_same_suit_ascending() {
    let mut state = standard_one();
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    draws(&mut state, 2);
    assert_eq!(top_name(&state, PileId::Waste), "C2");
    let events = apply(
        &mut state,
        move_cards(PileId::Waste, PileId::Foundation(0), 1),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Waste,
                to: PileId::Foundation(0),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
        ]
    );
    // A spade on the clubs foundation: wrong suit.
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(5), PileId::Foundation(0), 1)
        )
        .unwrap_err(),
        RuleError::IllegalFoundationMove
    );
}

#[test]
fn moving_the_last_face_up_card_flips_the_exposed_card_in_the_same_command() {
    let mut state = standard_one();
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
    assert_eq!(top_name(&state, PileId::Tableau(5)), "D2");
    assert_eq!(state.tableau(5).unwrap().face_down().len(), 4);
    assert_eq!(state.score(), 15);
}

#[test]
fn a_foundation_needs_matching_suit_and_next_rank_together() {
    let mut state = standard_one();
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    // Mechanically dig the spades two out of tableau 1: the right rank for
    // the clubs foundation, but the wrong suit.
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Tableau(1),
            to: PileId::Tableau(3),
            count: 1,
        },
    );
    evolve(
        &mut state,
        Event::CardFlipped {
            pile: PileId::Tableau(1),
        },
    );
    assert_eq!(top_name(&state, PileId::Tableau(1)), "S2");
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(1), PileId::Foundation(0), 1)
        )
        .unwrap_err(),
        RuleError::IllegalFoundationMove,
        "right rank, wrong suit"
    );
    // And the clubs queen: the right suit, but not the next rank.
    for _ in 0..2 {
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(4),
                to: PileId::Tableau(2),
                count: 1,
            },
        );
        evolve(
            &mut state,
            Event::CardFlipped {
                pile: PileId::Tableau(4),
            },
        );
    }
    assert_eq!(top_name(&state, PileId::Tableau(4)), "C12");
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(4), PileId::Foundation(0), 1)
        )
        .unwrap_err(),
        RuleError::IllegalFoundationMove,
        "right suit, wrong rank"
    );
}

#[test]
fn a_tableau_showing_only_face_down_cards_accepts_nothing() {
    let mut state = standard_one();
    // Strip tableau 1's face-up card without flipping — a position legal
    // play can never reach, but decide must still answer deterministically.
    evolve(
        &mut state,
        Event::CardsMoved {
            from: PileId::Tableau(1),
            to: PileId::Tableau(2),
            count: 1,
        },
    );
    let pile = state.tableau(1).unwrap();
    assert!(pile.face_up().is_empty());
    assert_eq!(pile.face_down().len(), 1);
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(2), PileId::Tableau(1), 1)
        )
        .unwrap_err(),
        RuleError::IllegalTableauMove
    );
}

#[test]
fn tableau_builds_alternate_colors_descending() {
    let mut state = standard_one();
    // Expose D2 on tableau 5, then lay it on the black three of tableau 6.
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(0), 1),
    );
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Tableau(6), 1),
    );
    assert_eq!(
        events.first().unwrap(),
        &Event::CardsMoved {
            from: PileId::Tableau(5),
            to: PileId::Tableau(6),
            count: 1,
        }
    );
    assert!(
        events.contains(&Event::CardFlipped {
            pile: PileId::Tableau(5),
        }),
        "taking D2 exposes the next face-down card"
    );
    assert_eq!(top_name(&state, PileId::Tableau(6)), "D2");
}

#[test]
fn same_color_or_wrong_rank_tableau_moves_are_rejected() {
    let state = standard_one();
    // H4 onto D5: rank fits, both red.
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(1), PileId::Tableau(2), 1)
        )
        .unwrap_err(),
        RuleError::IllegalTableauMove
    );
    // D5 onto S12: colors alternate, rank gap.
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(2), PileId::Tableau(3), 1)
        )
        .unwrap_err(),
        RuleError::IllegalTableauMove
    );
}

#[test]
fn only_a_king_may_take_an_emptied_tableau_pile() {
    let mut state = standard_one();
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert!(state.tableau(0).unwrap().is_empty());
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(3), PileId::Tableau(0), 1)
        )
        .unwrap_err(),
        RuleError::EmptyTableauNeedsKing
    );
    draws(&mut state, 6);
    assert_eq!(top_name(&state, PileId::Waste), "D13");
    let events = apply(&mut state, move_cards(PileId::Waste, PileId::Tableau(0), 1));
    assert_eq!(
        events.first().unwrap(),
        &Event::CardsMoved {
            from: PileId::Waste,
            to: PileId::Tableau(0),
            count: 1,
        }
    );
    assert_eq!(top_name(&state, PileId::Tableau(0)), "D13");
}

#[test]
fn a_face_up_run_moves_as_one_unit_in_order() {
    let mut state = standard_one();
    // Build the run S3,D2 on tableau 6.
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(0), 1),
    );
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Tableau(6), 1),
    );
    // Both cards travel to the red four of tableau 2 as one unit.
    let events = apply(
        &mut state,
        move_cards(PileId::Tableau(6), PileId::Tableau(2), 2),
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(6),
                to: PileId::Tableau(2),
                count: 2,
            },
            Event::CardFlipped {
                pile: PileId::Tableau(6),
            },
            Event::ScoreChanged { delta: 5 },
        ],
        "a tableau-to-tableau move scores nothing; the flip scores 5"
    );
    let run: Vec<String> = state
        .tableau(2)
        .unwrap()
        .face_up()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(run, ["H4", "S3", "D2"], "run arrives in order");
    assert_eq!(total_cards(&state), 52);
}

#[test]
fn a_run_is_judged_by_its_deepest_card() {
    let mut state = standard_one();
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Foundation(0), 1),
    );
    apply(
        &mut state,
        move_cards(PileId::Tableau(5), PileId::Tableau(6), 1),
    );
    // The run on tableau 6 is now S3,D2. The red four of tableau 2 takes the
    // black three at the run's bottom, so the pair travels — but the two on
    // top does not belong on a four and cannot go alone.
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(6), PileId::Tableau(2), 1)
        )
        .unwrap_err(),
        RuleError::IllegalTableauMove,
        "the top card alone does not fit"
    );
    assert!(
        decide(
            &state,
            move_cards(PileId::Tableau(6), PileId::Tableau(2), 2)
        )
        .is_ok(),
        "the whole run does, judged by its deepest card"
    );
}

#[test]
fn a_run_larger_than_the_face_up_stack_cannot_move() {
    let state = standard_one();
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(6), PileId::Tableau(2), 2)
        )
        .unwrap_err(),
        RuleError::NothingToMove
    );
}

#[test]
fn only_single_cards_move_except_between_tableaus() {
    let state = standard_one();
    for (from, to) in [
        (PileId::Tableau(5), PileId::Foundation(0)),
        (PileId::Waste, PileId::Tableau(1)),
        (PileId::Waste, PileId::Foundation(0)),
        (PileId::Foundation(0), PileId::Tableau(1)),
    ] {
        assert_eq!(
            decide(&state, move_cards(from, to, 2)).unwrap_err(),
            RuleError::TooManyCards { from, to },
            "{from:?} -> {to:?}"
        );
    }
}

#[test]
fn cards_never_move_between_forbidden_pile_combinations() {
    let state = standard_one();
    for (from, to) in [
        (PileId::Stock, PileId::Tableau(0)),
        (PileId::Stock, PileId::Foundation(0)),
        (PileId::Tableau(0), PileId::Stock),
        (PileId::Tableau(0), PileId::Waste),
        (PileId::Waste, PileId::Stock),
        (PileId::Foundation(0), PileId::Foundation(1)),
        (PileId::Foundation(0), PileId::Waste),
    ] {
        assert_eq!(
            decide(&state, move_cards(from, to, 1)).unwrap_err(),
            RuleError::MoveNotAllowed { from, to },
            "{from:?} -> {to:?}"
        );
    }
}

#[test]
fn moves_naming_piles_off_the_table_are_rejected() {
    let state = standard_one();
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(7), PileId::Tableau(0), 1)
        )
        .unwrap_err(),
        RuleError::UnknownPile {
            pile: PileId::Tableau(7),
        }
    );
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(0), PileId::Foundation(4), 1)
        )
        .unwrap_err(),
        RuleError::UnknownPile {
            pile: PileId::Foundation(4),
        }
    );
}

#[test]
fn empty_sources_and_zero_counts_have_nothing_to_move() {
    let state = standard_one();
    assert_eq!(
        decide(&state, move_cards(PileId::Waste, PileId::Tableau(1), 1)).unwrap_err(),
        RuleError::NothingToMove,
        "the waste starts empty"
    );
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Foundation(2), PileId::Tableau(1), 1)
        )
        .unwrap_err(),
        RuleError::NothingToMove,
        "foundations start empty"
    );
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Tableau(1), PileId::Tableau(2), 0)
        )
        .unwrap_err(),
        RuleError::NothingToMove
    );
}

#[test]
fn double_click_sends_an_eligible_card_to_its_foundation() {
    let mut state = standard_one();
    // The clubs ace goes to the first empty slot.
    let events = apply(
        &mut state,
        Command::AutoToFoundation {
            pile: PileId::Tableau(0),
        },
    );
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
        ]
    );
    // The spades ace skips the occupied slot 0.
    let events = apply(
        &mut state,
        Command::AutoToFoundation {
            pile: PileId::Tableau(5),
        },
    );
    assert_eq!(
        events.first().unwrap(),
        &Event::CardsMoved {
            from: PileId::Tableau(5),
            to: PileId::Foundation(1),
            count: 1,
        }
    );
    // The clubs two from the waste finds the clubs foundation.
    draws(&mut state, 2);
    assert_eq!(top_name(&state, PileId::Waste), "C2");
    let events = apply(
        &mut state,
        Command::AutoToFoundation {
            pile: PileId::Waste,
        },
    );
    assert_eq!(
        events.first().unwrap(),
        &Event::CardsMoved {
            from: PileId::Waste,
            to: PileId::Foundation(0),
            count: 1,
        }
    );
    assert_eq!(top_name(&state, PileId::Foundation(0)), "C2");
}

#[test]
fn double_click_with_no_eligible_foundation_does_nothing() {
    let mut state = standard_one();
    draws(&mut state, 1);
    assert_eq!(top_name(&state, PileId::Waste), "C6");
    assert_eq!(
        decide(
            &state,
            Command::AutoToFoundation {
                pile: PileId::Waste,
            }
        )
        .unwrap_err(),
        RuleError::NoEligibleFoundation
    );
}

#[test]
fn double_click_on_unusable_piles_is_rejected() {
    let mut state = standard_one();
    for pile in [PileId::Stock, PileId::Foundation(0)] {
        assert_eq!(
            decide(&state, Command::AutoToFoundation { pile }).unwrap_err(),
            RuleError::NoEligibleFoundation,
            "{pile:?}"
        );
    }
    assert_eq!(
        decide(
            &state,
            Command::AutoToFoundation {
                pile: PileId::Tableau(12),
            }
        )
        .unwrap_err(),
        RuleError::UnknownPile {
            pile: PileId::Tableau(12),
        }
    );
    // An emptied tableau pile has nothing to send.
    apply(
        &mut state,
        move_cards(PileId::Tableau(0), PileId::Foundation(0), 1),
    );
    assert_eq!(
        decide(
            &state,
            Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            }
        )
        .unwrap_err(),
        RuleError::NoEligibleFoundation
    );
}

#[test]
fn the_52nd_foundation_card_wins_the_game() {
    let mut state = standard_one();
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
            Event::ScoreChanged { delta: 10 },
            Event::GameWon,
        ]
    );
    assert!(state.is_won());
    assert_eq!(state.foundation_card_count(), 52);
    // Nothing but undo may happen now.
    assert_eq!(
        decide(
            &state,
            move_cards(PileId::Foundation(1), PileId::Tableau(0), 1)
        )
        .unwrap_err(),
        RuleError::GameAlreadyWon
    );
    assert_eq!(
        decide(
            &state,
            Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            }
        )
        .unwrap_err(),
        RuleError::GameAlreadyWon
    );
}
