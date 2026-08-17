//! The frontends' shared, platform-free core: theme discovery, the
//! application state machine, the plain-data snapshots the chrome renders,
//! and cutting a rendered card-back sheet into the PNG thumbnails a picker
//! shows.
//!
//! Every frontend — Qt on Linux, `native-windows-gui` on Windows, and
//! `AppKit` on macOS — differs in its chrome and its render path, and agrees
//! on everything else. This crate is that everything else. It depends on no
//! windowing toolkit and on no renderer, so it can be tested without a
//! display and gated like the domain crates below it.
//!
//! Cargo gives binary crates no way to share a module, so a library is the
//! only way three binaries can hold one copy of this code.

pub mod app;
pub mod error;
pub mod geometry;
pub mod menu;
pub mod options;
pub mod previews;
pub mod status;
pub mod themes;

pub use crate::app::{App, Startup, StatePaths, random_seed};
pub use crate::error::AppError;
pub use crate::geometry::clamp_window_size;
pub use crate::menu::MenuModel;
pub use crate::options::EditedOptions;
pub use crate::themes::{ThemeEntry, ThemeLookupError};
