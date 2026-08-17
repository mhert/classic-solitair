//! Cross-game session state (Session context).
//!
//! Tracks Vegas-mode bankroll, user options, and versioned save/load —
//! everything that outlives a single game and must persist across runs.
//!
//! There is always a deal on the table (Win98-faithful): [`Session`] owns
//! exactly one running [`sol_engine::Game`], and "abandoning" it IS dealing
//! the next one via [`Session::new_game`] — or, at load time, replacing the
//! session wholesale via [`Session::restore`]. There is no separate
//! `abandon` API.
//!
//! `bankroll`, `options`, `session`, `save`, and `settings` are
//! platform-free (no filesystem, no `directories`); [`paths`] and
//! [`storage`] are the only modules that touch the platform — see
//! [`storage`] for the frontend's save/load and autosave contract.

pub mod bankroll;
pub mod options;
pub mod paths;
pub mod save;
pub mod session;
pub mod settings;
pub mod storage;

pub use bankroll::Bankroll;
pub use options::{Options, ThemeId, ThemeIdError};
pub use save::{ENGINE_VERSION, FORMAT_VERSION, SaveError, SaveGame};
pub use session::Session;
pub use settings::{Settings, SettingsError, WindowGeometry};
pub use storage::StorageError;
