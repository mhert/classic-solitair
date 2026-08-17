//! [`Background`]: `[table] background` — a flat color or a tiled/stretched
//! image.

use core::str::FromStr;

use serde::Deserialize;

use crate::color::Color;
use crate::error::ManifestError;
use crate::path::RelativeAssetPath;

/// The table background (`[table] background`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Background {
    /// A flat fill color.
    Color(Color),
    /// An image, optionally tiled.
    Image {
        /// Validated theme-package-relative path to the image.
        path: RelativeAssetPath,
        /// Whether the image repeats to fill the table, rather than being
        /// stretched. Defaults to `false`.
        tile: bool,
    },
}

/// The permissive, shape-only parse of `[table] background`. [`validate`]
/// applies the "exactly one of `color`/`image`, `tile` only with `image`"
/// rule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBackground {
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    tile: Option<bool>,
}

/// Validates a raw `[table] background` table.
///
/// # Errors
///
/// Returns [`ManifestError::BackgroundNeedsExactlyOneOfColorOrImage`] if
/// `color`/`image` are both absent or both present,
/// [`ManifestError::BackgroundTileWithoutImage`] if `tile` is set without
/// `image`, [`ManifestError::InvalidColor`] if `color` does not parse, or
/// [`ManifestError::InvalidPath`] if `image` is not theme-package-relative.
pub(crate) fn validate(raw: RawBackground) -> Result<Background, ManifestError> {
    match (raw.color, raw.image) {
        (Some(color), None) => {
            if raw.tile.is_some() {
                return Err(ManifestError::BackgroundTileWithoutImage);
            }
            let color = Color::from_str(&color).map_err(|source| ManifestError::InvalidColor {
                field: "table.background.color",
                source,
            })?;
            Ok(Background::Color(color))
        }
        (None, Some(image)) => {
            let path = RelativeAssetPath::parse("table.background.image".to_owned(), &image)?;
            Ok(Background::Image {
                path,
                tile: raw.tile.unwrap_or(false),
            })
        }
        (None, None) | (Some(_), Some(_)) => {
            Err(ManifestError::BackgroundNeedsExactlyOneOfColorOrImage)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    fn raw(color: Option<&str>, image: Option<&str>, tile: Option<bool>) -> RawBackground {
        RawBackground {
            color: color.map(str::to_owned),
            image: image.map(str::to_owned),
            tile,
        }
    }

    #[test]
    fn a_color_only_table_is_a_flat_color() {
        let background = validate(raw(Some("#008000"), None, None)).unwrap();
        assert_eq!(background, Background::Color(Color::new(0x00, 0x80, 0x00)));
    }

    #[test]
    fn an_image_only_table_defaults_tile_to_false() {
        let background = validate(raw(None, Some("table.png"), None)).unwrap();
        assert_eq!(
            background,
            Background::Image {
                path: asset_path("table.png"),
                tile: false
            }
        );
    }

    #[test]
    fn an_image_table_may_set_tile_true() {
        let background = validate(raw(None, Some("table.png"), Some(true))).unwrap();
        assert_eq!(
            background,
            Background::Image {
                path: asset_path("table.png"),
                tile: true
            }
        );
    }

    #[test]
    fn rejects_neither_color_nor_image() {
        let error = validate(raw(None, None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackgroundNeedsExactlyOneOfColorOrImage
        ));
    }

    #[test]
    fn rejects_both_color_and_image() {
        let error = validate(raw(Some("#000000"), Some("table.png"), None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackgroundNeedsExactlyOneOfColorOrImage
        ));
    }

    #[test]
    fn rejects_tile_without_image() {
        let error = validate(raw(Some("#000000"), None, Some(true))).unwrap_err();
        assert!(matches!(error, ManifestError::BackgroundTileWithoutImage));
    }

    #[test]
    fn rejects_an_invalid_color() {
        let error = validate(raw(Some("not-a-color"), None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidColor {
                field: "table.background.color",
                ..
            }
        ));
    }

    #[test]
    fn rejects_a_non_relative_image_path() {
        let error = validate(raw(None, Some("/table.png"), None)).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }
}
