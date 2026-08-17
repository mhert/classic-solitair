//! Property tests: fold-of-log determinism, undo/redo
//! round-trips, undo-N ≡ never-done, and evolve totality over decide
//! output — plus the card-conservation and score-floor invariants.
//!
//! Sequences are *arbitrary attempted* commands: the valid subset is
//! accepted and logged, rejections must change nothing. Every accepted
//! event stream exercises `evolve`, so a panic anywhere fails the property.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::collections::BTreeSet;

use common::{config, total_cards};
use proptest::prelude::*;
use sol_engine::{Command, DrawMode, Game, GameState, PileId, ScoringMode, Seed};

fn arb_pile() -> impl Strategy<Value = PileId> {
    prop_oneof![
        1 => Just(PileId::Stock),
        2 => Just(PileId::Waste),
        3 => (0..5_u8).prop_map(PileId::Foundation),
        6 => (0..8_u8).prop_map(PileId::Tableau),
    ]
}

fn arb_player_command() -> impl Strategy<Value = Command> {
    prop_oneof![
        4 => Just(Command::Draw),
        5 => (arb_pile(), arb_pile(), 0..4_u8)
            .prop_map(|(from, to, count)| Command::MoveCards { from, to, count }),
        3 => arb_pile().prop_map(|pile| Command::AutoToFoundation { pile }),
    ]
}

fn arb_command() -> impl Strategy<Value = Command> {
    prop_oneof![
        9 => arb_player_command(),
        1 => (0..120_u32).prop_map(|total_elapsed_secs| Command::Tick { total_elapsed_secs }),
    ]
}

fn arb_config() -> impl Strategy<Value = (DrawMode, ScoringMode, bool)> {
    (
        prop_oneof![Just(DrawMode::One), Just(DrawMode::Three)],
        prop_oneof![
            Just(ScoringMode::Standard),
            Just(ScoringMode::Vegas),
            Just(ScoringMode::None),
        ],
        any::<bool>(),
    )
}

fn all_52_distinct(state: &GameState) -> bool {
    let mut seen = BTreeSet::new();
    for pile in state.tableaus() {
        for card in pile.face_down().iter().chain(pile.face_up()) {
            seen.insert(card.to_string());
        }
    }
    for card in state.stock().iter().chain(state.waste()) {
        seen.insert(card.to_string());
    }
    for foundation in state.foundations() {
        for card in foundation {
            seen.insert(card.to_string());
        }
    }
    seen.len() == 52
}

proptest! {
    // Explicit rather than implicit: the default case count is a property of
    // whichever proptest version resolves, and this suite's cost is worth
    // stating where it can be read and changed.
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// `state == fold(seed, log)` after any attempted command sequence,
    /// cards are conserved, and the Standard score never drops below 0.
    #[test]
    fn the_log_is_always_the_whole_truth(
        seed in 0..=Seed::MAX,
        (draw_mode, scoring, timed) in arb_config(),
        commands in prop::collection::vec(arb_command(), 0..60),
    ) {
        let game_config = config(draw_mode, scoring, timed);
        let mut game = Game::new(Seed::new(seed).unwrap(), game_config);
        for command in commands {
            let before_log_len = game.log().len();
            if game.apply(command).is_err() {
                prop_assert_eq!(game.log().len(), before_log_len,
                    "a rejected command must not be logged");
            }
            prop_assert_eq!(total_cards(game.state()), 52);
            prop_assert!(all_52_distinct(game.state()));
            if scoring == ScoringMode::Standard {
                prop_assert!(game.state().score() >= 0,
                    "Standard score fell to {}", game.state().score());
            }
        }
        let replayed = Game::from_log(game.seed(), game_config, game.log().to_vec());
        prop_assert_eq!(replayed.state(), game.state());
    }

    /// Undo followed immediately by redo restores identical state, score,
    /// and log (tick-free sequences: time cannot be re-reported).
    #[test]
    fn undo_then_redo_is_an_identity(
        seed in 0..=Seed::MAX,
        draw_mode in prop_oneof![Just(DrawMode::One), Just(DrawMode::Three)],
        scoring in prop_oneof![Just(ScoringMode::Standard), Just(ScoringMode::None)],
        commands in prop::collection::vec(arb_player_command(), 0..50),
    ) {
        let game_config = config(draw_mode, scoring, false);
        let mut game = Game::new(Seed::new(seed).unwrap(), game_config);
        for command in commands {
            let _ = game.apply(command);
        }
        if game.can_undo() {
            let state_before = game.state().clone();
            let score_before = game.state().score();
            let log_before = game.log().to_vec();
            game.undo().unwrap();
            game.redo().unwrap();
            prop_assert_eq!(game.state(), &state_before);
            prop_assert_eq!(game.state().score(), score_before);
            prop_assert_eq!(game.log(), log_before);
        }
    }

    /// Undoing the last N player commands leaves exactly the game that
    /// never made them.
    #[test]
    fn undoing_n_commands_equals_never_having_made_them(
        seed in 0..=Seed::MAX,
        draw_mode in prop_oneof![Just(DrawMode::One), Just(DrawMode::Three)],
        commands in prop::collection::vec(arb_player_command(), 0..50),
        undos in 1..8_usize,
    ) {
        let game_config = config(draw_mode, ScoringMode::Standard, false);
        let mut game = Game::new(Seed::new(seed).unwrap(), game_config);
        for command in commands {
            let _ = game.apply(command);
        }
        let accepted: Vec<Command> = game.log().iter().map(|entry| entry.command).collect();
        let mut undone = 0_usize;
        for _ in 0..undos {
            if game.undo().is_err() {
                break;
            }
            undone += 1;
        }
        let mut expected = Game::new(Seed::new(seed).unwrap(), game_config);
        for command in &accepted[..accepted.len() - undone] {
            expected.apply(*command).unwrap();
        }
        prop_assert_eq!(expected.state(), game.state());
        prop_assert_eq!(expected.log(), game.log());
    }
}

#[test]
fn the_log_serde_round_trips_for_saving() {
    // This seed deals an ace face-up to tableau 0, so the log opens with a
    // foundation move rather than a draw.
    let mut game = Game::new(
        Seed::new(Seed::MAX).unwrap(),
        config(DrawMode::One, ScoringMode::Standard, true),
    );
    game.apply(Command::MoveCards {
        from: PileId::Tableau(0),
        to: PileId::Foundation(0),
        count: 1,
    })
    .unwrap();
    game.apply(Command::Draw).unwrap();
    game.apply(Command::Tick {
        total_elapsed_secs: 11,
    })
    .unwrap();
    let json = serde_json::to_string(game.log()).unwrap();
    let restored: Vec<sol_engine::LogEntry> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, game.log());
    let replayed = Game::from_log(game.seed(), game.state().config(), restored);
    assert_eq!(replayed.state(), game.state());
}
