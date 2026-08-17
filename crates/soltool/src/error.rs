//! [`ToolError`]: the single error type [`crate::run`] returns, unifying
//! every subcommand's own error type for `main.rs`'s one exit-code mapping.

use crate::extract::ExtractError;
use crate::pack_strip::PackStripError;

/// Every way [`crate::run`] can fail — a thin dispatch over each
/// subcommand's own error type, so `main.rs` has exactly one type to match
/// on. Every variant delegates its `Display` (and therefore stderr output)
/// entirely to the wrapped error, which already carries the full source
/// chain (which theme asset, or which frame, and why).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    /// `soltool extract` failed — see [`ExtractError`].
    #[error(transparent)]
    Extract(#[from] ExtractError),
    /// `soltool validate` failed — see [`sol_theme::ThemeError`].
    #[error(transparent)]
    Validate(#[from] sol_theme::ThemeError),
    /// `soltool pack-strip` failed — see [`PackStripError`].
    #[error(transparent)]
    PackStrip(#[from] PackStripError),
}
