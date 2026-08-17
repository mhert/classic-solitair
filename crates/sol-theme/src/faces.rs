//! [`FacesSource`]: `[cards] faces` — a face directory or a vector sheet.

use crate::error::ManifestError;
use crate::path::RelativeAssetPath;
use crate::render_mode::RenderMode;

/// Where a theme's 52 card faces come from (`[cards] faces`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacesSource {
    /// A directory of 52 individual face images (`faces` ends in `/`),
    /// e.g. `"cards/"`.
    Directory(RelativeAssetPath),
    /// A single SVG sheet, a 13-wide × 4-high grid of faces (`faces` ends
    /// in `.svg`). Only legal when `render_mode = "vector"`.
    SvgSheet(RelativeAssetPath),
}

/// Validates a raw `[cards] faces` value.
///
/// # Errors
///
/// Returns [`ManifestError::InvalidFacesShape`] if `raw` ends in neither
/// `/` nor `.svg`, [`ManifestError::SvgFacesRequireVectorMode`] if it ends
/// in `.svg` but `render_mode` is not [`RenderMode::Vector`], or
/// [`ManifestError::InvalidPath`] if it is not theme-package-relative.
// `raw` is TOML text, not a filesystem path — the format fixes
// the suffix as the exact lowercase literal `.svg`, so a case-insensitive
// `Path::extension()` comparison (clippy's suggested fix) would silently
// accept a spelling the format doesn't define.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub(crate) fn validate(raw: &str, render_mode: RenderMode) -> Result<FacesSource, ManifestError> {
    if raw.ends_with('/') {
        let path = RelativeAssetPath::parse("cards.faces".to_owned(), raw)?;
        Ok(FacesSource::Directory(path))
    } else if raw.ends_with(".svg") {
        if render_mode != RenderMode::Vector {
            return Err(ManifestError::SvgFacesRequireVectorMode {
                value: raw.to_owned(),
            });
        }
        let path = RelativeAssetPath::parse("cards.faces".to_owned(), raw)?;
        Ok(FacesSource::SvgSheet(path))
    } else {
        Err(ManifestError::InvalidFacesShape {
            value: raw.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    #[test]
    fn a_trailing_slash_is_a_directory() {
        assert_eq!(
            validate("cards/", RenderMode::Png).unwrap(),
            FacesSource::Directory(asset_path("cards/"))
        );
    }

    #[test]
    fn a_dot_svg_suffix_is_a_sheet_under_vector_mode() {
        assert_eq!(
            validate("cards/faces.svg", RenderMode::Vector).unwrap(),
            FacesSource::SvgSheet(asset_path("cards/faces.svg"))
        );
    }

    #[test]
    fn a_dot_svg_suffix_is_rejected_outside_vector_mode() {
        let error = validate("cards/faces.svg", RenderMode::Png).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::SvgFacesRequireVectorMode { value } if value == "cards/faces.svg"
        ));
    }

    #[test]
    fn anything_else_is_an_invalid_shape() {
        let error = validate("cards", RenderMode::Png).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidFacesShape { value } if value == "cards"
        ));

        let error = validate("cards.png", RenderMode::Png).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidFacesShape { .. }));
    }

    #[test]
    fn rejects_an_absolute_directory_path() {
        let error = validate("/cards/", RenderMode::Png).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn rejects_a_parent_segment_in_a_sheet_path() {
        let error = validate("../faces.svg", RenderMode::Vector).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }
}
