//! [`AssetSource`]: the byte-oriented boundary between a theme package's
//! storage (directory, zip archive, or in-memory map) and the
//! format-agnostic loading and validation core in [`crate::theme`].
//!
//! Every asset this crate reads — `theme.toml`, face images, back images,
//! the background image, sound files — goes through this trait, so the
//! loading core never touches a filesystem or zip archive directly (the
//! architecture constraint: byte-oriented core, thin fs/zip layer).

use crate::path::RelativeAssetPath;

/// A theme package's byte-read boundary: a validated package-relative path
/// in, owned bytes out.
///
/// The parameter is a [`RelativeAssetPath`] rather than a `&str` because
/// [`crate::DirSource`] resolves it against a real directory. Demanding the
/// parsed type here is what stops an unvalidated manifest string from
/// reaching that join.
///
/// Implementations: [`crate::MemSource`] (in-memory, no I/O),
/// [`crate::ZipSource`] (a zip archive's bytes, no filesystem), and
/// [`crate::DirSource`] (the one implementation that touches a filesystem).
pub trait AssetSource {
    /// Reads the bytes stored at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::NotFound`] if nothing exists at `path` in this
    /// source. Returns [`SourceError::Io`] if `path` exists (or existence
    /// could not be determined) but reading it failed for another reason.
    fn read(&self, path: &RelativeAssetPath) -> Result<Vec<u8>, SourceError>;
}

/// Errors from [`AssetSource::read`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// Nothing exists at `path` in this source.
    #[error("{path} not found in theme package")]
    NotFound {
        /// The path that was requested.
        path: String,
    },
    /// `path` exists (or its existence could not be determined), but
    /// reading it failed for another reason — a filesystem I/O error, or a
    /// zip archive read/decompression failure.
    #[error("failed to read {path}: {message}")]
    Io {
        /// The path that was requested.
        path: String,
        /// The underlying failure, rendered to text: the concrete error
        /// type differs by implementation (`std::io::Error` for
        /// [`crate::DirSource`], `zip::result::ZipError` for
        /// [`crate::ZipSource`]), so it is rendered here rather than
        /// crossing this crate's public API (mirrors
        /// `ManifestError::InvalidToml`).
        message: String,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn not_found_message_names_the_path() {
        let error = SourceError::NotFound {
            path: "cards/spades_01.png".to_owned(),
        };
        assert!(error.to_string().contains("cards/spades_01.png"));
    }

    #[test]
    fn io_message_names_the_path_and_the_underlying_failure() {
        let error = SourceError::Io {
            path: "theme.toml".to_owned(),
            message: "permission denied".to_owned(),
        };
        let text = error.to_string();
        assert!(text.contains("theme.toml"));
        assert!(text.contains("permission denied"));
    }
}
