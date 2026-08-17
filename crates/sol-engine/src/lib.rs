//! Pure event-sourced Klondike Solitaire rules engine (Gameplay context).
//!
//! No I/O, no clock, no OS RNG: game state is a deterministic function of a
//! sequence of events, so it can be replayed, tested, and driven by any
//! frontend without touching the outside world.
//!
//! The tactical machinery is exactly two functions and a log. A player
//! intent enters as a [`Command`]; [`decide`] validates it against the rules
//! and materializes every consequence (moves, auto-flips, floored score
//! deltas, pass completions, time, the win) as [`Event`]s; [`evolve`] folds
//! events into [`GameState`] with no rule knowledge at all. The [`Game`]
//! aggregate keeps `(seed, log)` — the canonical representation of a game —
//! and implements undo/redo as log surgery plus replay. Wall-clock time
//! enters only through [`Command::Tick`].
//!
//! ```
//! use sol_engine::{Command, DrawMode, Game, GameConfig, ScoringMode, Seed};
//!
//! let config = GameConfig {
//!     draw_mode: DrawMode::Three,
//!     scoring: ScoringMode::Standard,
//!     timed: false,
//! };
//! let mut game = Game::new(Seed::new(1).unwrap(), config);
//!
//! // Turn three cards onto the waste, then take it back.
//! game.apply(Command::Draw)?;
//! assert_eq!(game.state().waste().len(), 3);
//! game.undo()?;
//! assert!(game.state().waste().is_empty());
//! # Ok::<(), sol_engine::RuleError>(())
//! ```

pub mod card;
pub mod command;
pub mod config;
pub mod deal;
pub mod decide;
pub mod event;
pub mod evolve;
pub mod game;
pub mod pile;
mod rng;
mod score;
pub mod seed;
pub mod state;

pub use card::{Card, Color, Rank, Suit};
pub use command::Command;
pub use config::{DrawMode, GameConfig, ScoringMode};
pub use deal::deal;
pub use decide::{RuleError, decide};
pub use event::Event;
pub use evolve::evolve;
pub use game::{Game, LogEntry};
pub use pile::{FOUNDATION_COUNT, PileId, TABLEAU_COUNT};
pub use seed::{Seed, SeedError};
pub use state::{GameState, TableauPile};
