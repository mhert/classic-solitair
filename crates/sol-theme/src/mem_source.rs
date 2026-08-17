//! [`MemSource`]: an in-memory [`crate::AssetSource`] — no filesystem, no
//! zip archive, just a path→bytes map. Used by this crate's own tests, and
//! public so other crates (a future wasm shell with no filesystem access,
//! `soltool`'s tests) can build a theme package purely in memory too.

use std::collections::HashMap;

use crate::path::RelativeAssetPath;
use crate::source::{AssetSource, SourceError};

/// An in-memory theme package: a map from package-relative path to bytes.
#[derive(Debug, Clone, Default)]
pub struct MemSource {
    files: HashMap<String, Vec<u8>>,
}

impl MemSource {
    /// An empty source with no files.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one file, overwriting any existing bytes already at `path`.
    /// Consumes and returns `self` for chaining.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.insert(path.into(), bytes.into());
        self
    }

    /// Whether anything is stored under the exact key `raw`.
    ///
    /// Test-only, and deliberately so: it is the one way to observe a key
    /// that [`AssetSource::read`] cannot express, which is what
    /// [`crate::ZipSource`]'s entry-name filtering needs to assert.
    #[cfg(test)]
    pub(crate) fn contains_raw_key(&self, raw: &str) -> bool {
        self.files.contains_key(raw)
    }
}

impl FromIterator<(String, Vec<u8>)> for MemSource {
    fn from_iter<T: IntoIterator<Item = (String, Vec<u8>)>>(iter: T) -> Self {
        Self {
            files: iter.into_iter().collect(),
        }
    }
}

impl AssetSource for MemSource {
    fn read(&self, path: &RelativeAssetPath) -> Result<Vec<u8>, SourceError> {
        self.files
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| SourceError::NotFound {
                path: path.as_str().to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    #[test]
    fn reads_back_a_file_that_was_added() {
        let source = MemSource::new().with_file("theme.toml", b"hello".to_vec());
        assert_eq!(source.read(&asset_path("theme.toml")).unwrap(), b"hello");
    }

    #[test]
    fn reading_a_missing_path_is_not_found() {
        let source = MemSource::new();
        let error = source.read(&asset_path("nope.png")).unwrap_err();
        assert!(matches!(error, SourceError::NotFound { path } if path == "nope.png"));
    }

    #[test]
    fn with_file_overwrites_a_previous_value_at_the_same_path() {
        let source = MemSource::new()
            .with_file("a.png", b"first".to_vec())
            .with_file("a.png", b"second".to_vec());
        assert_eq!(source.read(&asset_path("a.png")).unwrap(), b"second");
    }

    #[test]
    fn from_iter_collects_pairs_into_a_source() {
        let pairs = vec![
            ("a.png".to_owned(), b"1".to_vec()),
            ("b.png".to_owned(), b"2".to_vec()),
        ];
        let source: MemSource = pairs.into_iter().collect();
        assert_eq!(source.read(&asset_path("a.png")).unwrap(), b"1");
        assert_eq!(source.read(&asset_path("b.png")).unwrap(), b"2");
    }

    #[test]
    fn default_is_empty() {
        let source = MemSource::default();
        assert!(source.read(&asset_path("anything")).is_err());
    }
}
