//! Undo/redo semantics on the [`Game`] aggregate: log pop + replay from the
//! seed, redo stacks cleared by new player commands, Vegas rejection, and
//! tick transparency.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The deal these tests share with `rules_moves.rs`: an ace face-up on
/// tableau 0, so a game can open with a foundation move.
const SEED: u16 = 8622;

mod common;

use common::config;
use sol_engine::game::{Game, LogEntry};
use sol_engine::{Command, DrawMode, Event, PileId, RuleError, ScoringMode, Seed};

fn standard_game() -> Game {
    Game::new(
        Seed::new(SEED).unwrap(),
        config(DrawMode::One, ScoringMode::Standard, false),
    )
}

fn timed_game() -> Game {
    Game::new(
        Seed::new(SEED).unwrap(),
        config(DrawMode::One, ScoringMode::Standard, true),
    )
}

/// Seed 1: the clubs ace of tableau 0 onto foundation 0.
const FIRST_MOVE: Command = Command::MoveCards {
    from: PileId::Tableau(0),
    to: PileId::Foundation(0),
    count: 1,
};

/// Seed 1: the hearts ace of tableau 5 onto foundation 1.
const SECOND_MOVE: Command = Command::MoveCards {
    from: PileId::Tableau(5),
    to: PileId::Foundation(1),
    count: 1,
};

/// Seed 1: the queen of spades onto the emptied tableau 0 — illegal.
const ILLEGAL_MOVE: Command = Command::MoveCards {
    from: PileId::Tableau(3),
    to: PileId::Tableau(0),
    count: 1,
};

#[test]
fn accepted_commands_are_logged_and_the_log_replays_to_the_same_state() {
    let mut game = standard_game();
    let events = game.apply(FIRST_MOVE).unwrap().to_vec();
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
    game.apply(Command::Draw).unwrap();
    game.apply(SECOND_MOVE).unwrap();
    assert_eq!(game.log().len(), 3);
    assert_eq!(game.log()[0].command, FIRST_MOVE);
    let replayed = Game::from_log(game.seed(), game.state().config(), game.log().to_vec());
    assert_eq!(replayed.state(), game.state());
    assert_eq!(replayed.log(), game.log());
}

#[test]
fn rejected_commands_change_nothing_and_are_not_logged() {
    let mut game = standard_game();
    game.apply(FIRST_MOVE).unwrap();
    let before = game.clone();
    assert_eq!(
        game.apply(ILLEGAL_MOVE).unwrap_err(),
        RuleError::EmptyTableauNeedsKing
    );
    assert_eq!(game, before);
}

#[test]
fn undo_pops_the_last_move_and_equals_never_having_made_it() {
    let mut game = standard_game();
    game.apply(FIRST_MOVE).unwrap();
    game.apply(SECOND_MOVE).unwrap();
    assert!(game.can_undo());
    game.undo().unwrap();

    let mut only_first = standard_game();
    only_first.apply(FIRST_MOVE).unwrap();
    assert_eq!(game.state(), only_first.state());
    assert_eq!(game.log(), only_first.log());
    assert_eq!(game.state().score(), 10, "the second move's score reverted");
    assert!(game.can_redo());
}

#[test]
fn redo_reapplies_the_taken_back_command_exactly() {
    let mut game = standard_game();
    game.apply(FIRST_MOVE).unwrap();
    let original_events = game.apply(SECOND_MOVE).unwrap().to_vec();
    let full_state = game.state().clone();
    let full_log = game.log().to_vec();
    game.undo().unwrap();
    let redone_events = game.redo().unwrap().to_vec();
    assert_eq!(redone_events, original_events);
    assert_eq!(game.state(), &full_state);
    assert_eq!(game.log(), full_log);
    assert!(!game.can_redo());
    assert_eq!(game.redo().unwrap_err(), RuleError::NothingToRedo);
}

#[test]
fn a_new_player_command_clears_the_redo_stack() {
    let mut game = standard_game();
    game.apply(FIRST_MOVE).unwrap();
    game.apply(SECOND_MOVE).unwrap();
    game.undo().unwrap();
    assert!(game.can_redo());
    game.apply(Command::Draw).unwrap();
    assert!(!game.can_redo());
    assert_eq!(game.redo().unwrap_err(), RuleError::NothingToRedo);
}

#[test]
fn undoing_everything_then_undoing_again_is_rejected() {
    let mut game = standard_game();
    assert!(!game.can_undo());
    assert_eq!(game.undo().unwrap_err(), RuleError::NothingToUndo);
    game.apply(FIRST_MOVE).unwrap();
    game.undo().unwrap();
    assert_eq!(game.state(), standard_game().state());
    assert_eq!(game.undo().unwrap_err(), RuleError::NothingToUndo);
}

#[test]
fn vegas_rejects_undo_and_redo_but_still_logs() {
    let mut game = Game::new(
        Seed::new(SEED).unwrap(),
        config(DrawMode::One, ScoringMode::Vegas, false),
    );
    game.apply(FIRST_MOVE).unwrap();
    assert_eq!(game.log().len(), 1, "Vegas still logs — saving must work");
    assert!(!game.can_undo());
    assert!(!game.can_redo());
    assert_eq!(game.undo().unwrap_err(), RuleError::UndoNotAllowed);
    assert_eq!(game.redo().unwrap_err(), RuleError::UndoNotAllowed);
    assert_eq!(game.log().len(), 1);
}

#[test]
fn idle_ticks_are_accepted_but_never_logged() {
    let mut game = standard_game();
    let events = game.apply(Command::Tick {
        total_elapsed_secs: 50,
    });
    assert_eq!(
        events.unwrap(),
        &[] as &[Event],
        "untimed games ignore time"
    );
    assert!(game.log().is_empty());
}

#[test]
fn undo_skips_over_trailing_tick_entries() {
    let mut game = timed_game();
    game.apply(FIRST_MOVE).unwrap();
    game.apply(Command::Tick {
        total_elapsed_secs: 5,
    })
    .unwrap();
    game.apply(Command::Tick {
        total_elapsed_secs: 12,
    })
    .unwrap();
    assert_eq!(game.log().len(), 3);
    assert_eq!(game.state().score(), 8, "one decay boundary crossed");
    assert!(game.can_undo());
    game.undo().unwrap();
    assert_eq!(
        game.state(),
        timed_game().state(),
        "the move and the newer ticks are gone"
    );
    assert!(game.log().is_empty());
    assert!(game.can_redo(), "the player move waits on the redo stack");
    let events = game.redo().unwrap().to_vec();
    assert_eq!(
        events,
        vec![
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            },
            Event::ScoreChanged { delta: 10 },
        ],
        "re-deciding against the rewound clock gives the original events"
    );
    assert_eq!(game.state().score(), 10);
}

#[test]
fn ticks_between_undo_and_redo_do_not_clear_the_redo_stack() {
    let mut game = timed_game();
    game.apply(FIRST_MOVE).unwrap();
    game.undo().unwrap();
    game.apply(Command::Tick {
        total_elapsed_secs: 5,
    })
    .unwrap();
    assert_eq!(game.log().len(), 1, "the tick was logged");
    assert!(game.can_redo());
    game.redo().unwrap();
    assert_eq!(game.state().score(), 10);
    assert_eq!(game.log().len(), 2);
    assert_eq!(game.state().elapsed_secs(), 5);
}

#[test]
fn a_log_of_only_ticks_leaves_nothing_to_undo() {
    let mut game = timed_game();
    game.apply(Command::Tick {
        total_elapsed_secs: 3,
    })
    .unwrap();
    assert_eq!(game.log().len(), 1);
    assert!(!game.can_undo());
    assert_eq!(game.undo().unwrap_err(), RuleError::NothingToUndo);
    assert_eq!(game.log().len(), 1, "the failed undo removed nothing");
}

#[test]
fn undo_reopens_a_won_game() {
    // Stage 51 foundation cards through a synthetic log entry, then win
    // legally through the aggregate.
    let mut staging = vec![Event::CardsMoved {
        from: PileId::Stock,
        to: PileId::Foundation(0),
        count: 24,
    }];
    let probe = standard_game();
    for index in 1..7_u8 {
        let pile = probe.state().tableau(index).unwrap();
        let mut remaining_down = pile.face_down().len();
        staging.push(Event::CardsMoved {
            from: PileId::Tableau(index),
            to: PileId::Foundation(0),
            count: 1,
        });
        while remaining_down > 0 {
            staging.push(Event::CardFlipped {
                pile: PileId::Tableau(index),
            });
            staging.push(Event::CardsMoved {
                from: PileId::Tableau(index),
                to: PileId::Foundation(0),
                count: 1,
            });
            remaining_down -= 1;
        }
    }
    let mut game = Game::from_log(
        Seed::new(SEED).unwrap(),
        config(DrawMode::One, ScoringMode::Standard, false),
        vec![LogEntry {
            command: Command::Draw,
            events: staging,
        }],
    );
    assert_eq!(game.state().foundation_card_count(), 51);
    let win = Command::MoveCards {
        from: PileId::Tableau(0),
        to: PileId::Foundation(1),
        count: 1,
    };
    game.apply(win).unwrap();
    assert!(game.state().is_won());
    game.undo().unwrap();
    assert!(!game.state().is_won());
    assert_eq!(game.state().foundation_card_count(), 51);
    assert_eq!(game.log().len(), 1, "only the staging entry remains");
    game.apply(win).unwrap();
    assert!(game.state().is_won());
}
