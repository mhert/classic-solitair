//! The aggregate root: [`Game`] — one seed, one config, one command log.
//!
//! `(seed, log)` is the canonical representation of a game; the cached
//! [`GameState`] is always its left fold. Undo pops the log and replays from
//! the seed; redo re-decides the taken-back command; both are rejected in
//! Vegas scoring. Time entries (`Command::Tick`) are reality reports, not
//! player actions: undo skips over them (discarding trailing ones) and they
//! never clear the redo stack, so a running clock cannot starve undo/redo.

use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::config::{GameConfig, ScoringMode};
use crate::deal::deal;
use crate::decide::{RuleError, decide};
use crate::event::Event;
use crate::evolve::evolve;
use crate::seed::Seed;
use crate::state::GameState;

/// One accepted command and the events it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The accepted command.
    pub command: Command,
    /// The events `decide` materialized for it, in order.
    pub events: Vec<Event>,
}

/// A running game: seed, config, the log of accepted commands, and the
/// folded state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    seed: Seed,
    state: GameState,
    log: Vec<LogEntry>,
    redo_stack: Vec<Command>,
}

impl Game {
    /// Deals a new game from a seed and a fixed rule configuration.
    #[must_use]
    pub fn new(seed: Seed, config: GameConfig) -> Self {
        Self {
            seed,
            state: deal(seed, config),
            log: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Rebuilds a game from its canonical representation: re-deals the seed
    /// and blindly folds the logged events (loading a save). Infallible —
    /// [`crate::evolve`] is total — though a hand-built log yields whatever
    /// state its events describe.
    ///
    /// ```
    /// use sol_engine::{Command, DrawMode, Game, GameConfig, ScoringMode, Seed};
    ///
    /// let config = GameConfig {
    ///     draw_mode: DrawMode::One,
    ///     scoring: ScoringMode::Vegas,
    ///     timed: false,
    /// };
    /// let mut game = Game::new(Seed::new(3).unwrap(), config);
    /// game.apply(Command::Draw)?;
    /// let restored = Game::from_log(game.seed(), config, game.log().to_vec());
    /// assert_eq!(restored.state(), game.state());
    /// # Ok::<(), sol_engine::RuleError>(())
    /// ```
    #[must_use]
    pub fn from_log(seed: Seed, config: GameConfig, log: Vec<LogEntry>) -> Self {
        let state = replay(seed, config, &log);
        Self {
            seed,
            state,
            log,
            redo_stack: Vec::new(),
        }
    }

    /// The seed this game was dealt from.
    #[must_use]
    pub const fn seed(&self) -> Seed {
        self.seed
    }

    /// The current folded state.
    #[must_use]
    pub const fn state(&self) -> &GameState {
        &self.state
    }

    /// The log of accepted commands with their events — everything a save
    /// needs besides the seed and the config.
    #[must_use]
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// Whether undo is currently possible: never in Vegas scoring, and only
    /// when a player command (not a tick) is left in the log.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.state.config().scoring != ScoringMode::Vegas
            && self.log.iter().any(|entry| !is_tick(entry.command))
    }

    /// Whether redo is currently possible: never in Vegas scoring, and only
    /// while an undone command waits and no new player command intervened.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.state.config().scoring != ScoringMode::Vegas && !self.redo_stack.is_empty()
    }

    /// Runs a command through [`crate::decide`], folds its events, and logs
    /// it. Accepted commands with no events (idle ticks) are not logged.
    /// A new player command clears the redo stack; ticks do not.
    ///
    /// # Errors
    ///
    /// Returns the [`RuleError`] from `decide`; the game is unchanged.
    pub fn apply(&mut self, command: Command) -> Result<&[Event], RuleError> {
        let events = decide(&self.state, command)?;
        if events.is_empty() {
            return Ok(&[]);
        }
        for event in &events {
            evolve(&mut self.state, *event);
        }
        if !is_tick(command) {
            self.redo_stack.clear();
        }
        self.log.push(LogEntry { command, events });
        Ok(self.log.last().map_or(&[], |entry| entry.events.as_slice()))
    }

    /// Takes back the most recent player command: discards any newer tick
    /// entries, pops the command's log entry onto the redo stack, and
    /// replays the remaining log from the seed.
    ///
    /// # Errors
    ///
    /// [`RuleError::UndoNotAllowed`] in Vegas scoring;
    /// [`RuleError::NothingToUndo`] when no player command is logged.
    pub fn undo(&mut self) -> Result<(), RuleError> {
        if self.state.config().scoring == ScoringMode::Vegas {
            return Err(RuleError::UndoNotAllowed);
        }
        let Some(index) = self.log.iter().rposition(|entry| !is_tick(entry.command)) else {
            return Err(RuleError::NothingToUndo);
        };
        // Everything from the player entry on: the entry itself plus only
        // tick entries (rposition guarantees that). The command goes to the
        // redo stack; stale time reports are discarded.
        let removed = self.log.split_off(index);
        if let Some(entry) = removed.into_iter().next() {
            self.redo_stack.push(entry.command);
        }
        self.state = replay(self.seed, self.state.config(), &self.log);
        Ok(())
    }

    /// Re-applies the most recently undone command by deciding it against
    /// the current state — identical to re-folding its original events
    /// whenever no time passed in between.
    ///
    /// # Errors
    ///
    /// [`RuleError::UndoNotAllowed`] in Vegas scoring;
    /// [`RuleError::NothingToRedo`] when nothing was undone; any
    /// [`RuleError`] from re-deciding the command (unreachable in practice —
    /// ticks never change card positions).
    pub fn redo(&mut self) -> Result<&[Event], RuleError> {
        if self.state.config().scoring == ScoringMode::Vegas {
            return Err(RuleError::UndoNotAllowed);
        }
        let Some(&command) = self.redo_stack.last() else {
            return Err(RuleError::NothingToRedo);
        };
        let events = decide(&self.state, command)?;
        self.redo_stack.pop();
        for event in &events {
            evolve(&mut self.state, *event);
        }
        self.log.push(LogEntry { command, events });
        Ok(self.log.last().map_or(&[], |entry| entry.events.as_slice()))
    }
}

/// Whether a command is a time report rather than a player action.
const fn is_tick(command: Command) -> bool {
    matches!(command, Command::Tick { .. })
}

/// The left fold: re-deal the seed, then blindly evolve every logged event.
fn replay(seed: Seed, config: GameConfig, log: &[LogEntry]) -> GameState {
    let mut state = deal(seed, config);
    for entry in log {
        for event in &entry.events {
            evolve(&mut state, *event);
        }
    }
    state
}
