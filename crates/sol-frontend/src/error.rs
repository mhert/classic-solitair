//! [`AppError`]: the ways starting or re-theming the shared application core
//! can fail.

use std::path::PathBuf;

use crate::themes::ThemeLookupError;

/// Every way the shared application core can fail to reach a running board.
///
/// A frontend's own startup can fail in ways this does not describe — a
/// windowing toolkit that will not initialize, a render path that will not
/// start — and each frontend wraps this in an error of its own for those.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// The startup theme failed to resolve or load.
    #[error(transparent)]
    Theme(#[from] ThemeLookupError),
    /// A `--theme` override path failed to load.
    #[error("loading theme override {path}")]
    ThemeOverride {
        /// The override path that could not be loaded.
        path: PathBuf,
        // Boxed for the same slim-Err reason as ThemeLookupError::Load.
        /// The underlying loader failure.
        #[source]
        source: Box<sol_theme::ThemeError>,
    },
}
