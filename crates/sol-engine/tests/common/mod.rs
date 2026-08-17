//! Shared helpers for the sol-engine integration tests.

// Each test binary compiles this module separately and uses its own subset
// of the helpers.
#![allow(dead_code)]

use sol_engine::{
    Command, DrawMode, Event, GameConfig, GameState, ScoringMode, Seed, deal, decide, evolve,
};

/// Builds a config in one line.
#[must_use]
pub fn config(draw_mode: DrawMode, scoring: ScoringMode, timed: bool) -> GameConfig {
    GameConfig {
        draw_mode,
        scoring,
        timed,
    }
}

/// A dealt game for the given seed and config.
#[must_use]
pub fn dealt(seed: u16, game_config: GameConfig) -> GameState {
    deal(Seed::new(seed).unwrap(), game_config)
}

/// Decides a command that must be legal, folds its events, and returns them.
///
/// # Panics
///
/// Panics when the command is rejected — integration tests use this only for
/// commands they expect to succeed.
pub fn apply(state: &mut GameState, command: Command) -> Vec<Event> {
    let events = decide(&*state, command).unwrap_or_else(|error| {
        panic!("expected {command:?} to be legal, got {error}");
    });
    for event in &events {
        evolve(state, *event);
    }
    events
}

/// Total number of cards anywhere on the table — must always be 52.
#[must_use]
pub fn total_cards(state: &GameState) -> usize {
    state.stock().len()
        + state.waste().len()
        + state.foundation_card_count()
        + state
            .tableaus()
            .map(sol_engine::TableauPile::len)
            .sum::<usize>()
}
