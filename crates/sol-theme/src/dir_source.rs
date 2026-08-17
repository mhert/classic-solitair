//! [`DirSource`]: an [`crate::AssetSource`] rooted at a real directory — the
//! only implementation in this crate that touches a filesystem.

use std::path::PathBuf;

use crate::path::RelativeAssetPath;
use crate::source::{AssetSource, SourceError};

/// A theme package stored as a directory on disk.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    /// Roots a source at `root`.
    ///
    /// Does not check that `root` exists: a missing or unreadable root
    /// simply surfaces as a [`SourceError`] from the first
    /// [`AssetSource::read`] call (starting with `theme.toml`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl AssetSource for DirSource {
    fn read(&self, path: &RelativeAssetPath) -> Result<Vec<u8>, SourceError> {
        // The parameter type carries the guarantee this join needs: a
        // `RelativeAssetPath` is package-relative, `/`-separated, and free of
        // traversal and of every platform's absolute-path form, so it cannot
        // escape `root` and cannot drop `root` the way `Path::join` does when
        // given something absolute.
        let full_path = self.root.join(path.as_str());
        std::fs::read(&full_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SourceError::NotFound {
                    path: path.as_str().to_owned(),
                }
            } else {
                SourceError::Io {
                    path: path.as_str().to_owned(),
                    message: error.to_string(),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use super::*;
    use crate::testkit::asset_path;

    #[test]
    fn reads_a_file_that_exists_under_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("theme.toml"), b"hello").unwrap();

        let source = DirSource::new(dir.path());
        assert_eq!(source.read(&asset_path("theme.toml")).unwrap(), b"hello");
    }

    #[test]
    fn reads_a_file_in_a_nested_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cards")).unwrap();
        fs::write(dir.path().join("cards").join("spades_01.png"), b"png").unwrap();

        let source = DirSource::new(dir.path());
        assert_eq!(
            source.read(&asset_path("cards/spades_01.png")).unwrap(),
            b"png"
        );
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let source = DirSource::new(dir.path());

        let error = source.read(&asset_path("nope.png")).unwrap_err();
        assert!(matches!(error, SourceError::NotFound { path } if path == "nope.png"));
    }

    #[test]
    fn a_missing_root_is_not_found() {
        let source = DirSource::new("/no/such/directory/at/all");
        let error = source.read(&asset_path("theme.toml")).unwrap_err();
        assert!(matches!(error, SourceError::NotFound { .. }));
    }

    #[test]
    fn reading_a_directory_as_a_file_is_an_io_error_not_not_found() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cards")).unwrap();

        let source = DirSource::new(dir.path());
        let error = source.read(&asset_path("cards")).unwrap_err();
        assert!(matches!(error, SourceError::Io { .. }));
    }
}
