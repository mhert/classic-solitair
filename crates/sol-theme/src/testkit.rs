//! Test-only helpers shared across this crate's unit tests.

#![allow(clippy::expect_used)] // Test fixtures: a broken fixture must abort the suite loudly.

use crate::path::RelativeAssetPath;

/// Parses `raw` as an asset path, panicking if it is not a valid one.
///
/// Fixtures spell out paths as literals; a literal that does not parse is a
/// broken fixture, not a case under test. Tests that exercise the rule
/// itself call [`RelativeAssetPath::parse`] directly and inspect the error.
pub(crate) fn asset_path(raw: &str) -> RelativeAssetPath {
    RelativeAssetPath::parse("test fixture".to_owned(), raw)
        .expect("test fixture path must be package-relative")
}
