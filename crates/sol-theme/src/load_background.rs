//! [`LoadedBackground`] and background loading/validation: validation
//! matrix row 6 (and row 2's extension/format checks as they apply to the
//! background image).
//!
//! A stretched background may be any size the probers accept. A *tiled* one
//! may not: it is drawn once per tile, so the tile size is the only thing a
//! theme controls that scales the per-frame sprite count without bound —
//! every other count on the board is fixed by the 52-card deck. Tiles below
//! [`MIN_TILE_EDGE`] are refused.

use crate::asset::{self, Asset, AssetKind};
use crate::background::Background;
use crate::color::Color;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;

/// [`Background`], loaded: the flat fill color unchanged, or the background
/// image's bytes read and probed.
///
/// Unlike faces and backs, a stretched background has no fixed expected size
/// to check against — any successfully probed size is accepted (matrix row
/// 6). A tiled one must be at least [`MIN_TILE_EDGE`] on both axes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedBackground {
    /// A flat fill color.
    Color(Color),
    /// A loaded, probed background image.
    Image {
        /// The loaded image asset.
        asset: Asset,
        /// Whether the image repeats to fill the table, rather than being
        /// stretched.
        tile: bool,
    },
}

/// Smallest image a theme may tile. The sprite count a tiled background
/// costs is the viewport area over the tile area, so a tiny tile is the one
/// place a theme can drive an unbounded per-frame sprite count. A tile below
/// this is also indistinguishable in effect from `background = { color = … }`,
/// which the manifest supports directly.
const MIN_TILE_EDGE: u32 = 16;

/// Loads and validates `[table] background`.
///
/// # Errors
///
/// Returns [`ThemeError::BackgroundWrongExtension`] if the image path does
/// not end in the extension `kind` requires,
/// [`ThemeError::BackgroundUnreadable`] if the image cannot be read,
/// [`ThemeError::BackgroundInvalidFormat`] if its bytes do not probe as
/// `kind`, or probe to a zero-valued dimension, or
/// [`ThemeError::TiledBackgroundTooSmall`] if `tile` is set and the probed
/// image is under [`MIN_TILE_EDGE`] on either axis.
pub(crate) fn load(
    source: &impl AssetSource,
    background: &Background,
    kind: AssetKind,
) -> Result<LoadedBackground, ThemeError> {
    let (path, tile) = match background {
        Background::Color(color) => return Ok(LoadedBackground::Color(*color)),
        Background::Image { path, tile } => (path, tile),
    };

    if !path.as_str().ends_with(kind.extension()) {
        return Err(ThemeError::BackgroundWrongExtension {
            path: path.as_str().to_owned(),
            expected_ext: kind.extension(),
        });
    }

    let bytes = source
        .read(path)
        .map_err(|source| ThemeError::BackgroundUnreadable {
            path: path.as_str().to_owned(),
            source,
        })?;
    let size =
        asset::probe(&bytes, kind).map_err(|reason| ThemeError::BackgroundInvalidFormat {
            path: path.as_str().to_owned(),
            reason,
        })?;
    // Matrix row 6: a stretched background is "any size >= 1x1" — both
    // probers now guarantee this themselves as of the SVG prober's usvg swap
    // (png.rs already rejected a zero dimension; sol-theme/src/svg.rs's
    // usvg-backed probe now rejects a non-positive resolved size too —
    // usvg's own `Size` type cannot be zero by construction), so no extra
    // check is needed for it.
    //
    // A tiled background is different: the renderer emits one sprite per
    // tile, so the tile size — not the viewport — decides how many sprites a
    // frame costs. Bounding it here bounds that count by construction,
    // rather than leaving the renderer to discover a 1x1 tile at draw time.
    if *tile && (size.width < MIN_TILE_EDGE || size.height < MIN_TILE_EDGE) {
        return Err(ThemeError::TiledBackgroundTooSmall {
            path: path.as_str().to_owned(),
            width: size.width,
            height: size.height,
            minimum: MIN_TILE_EDGE,
        });
    }

    Ok(LoadedBackground::Image {
        asset: Asset {
            path: path.clone(),
            bytes,
            kind,
            size,
        },
        tile: *tile,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::mem_source::MemSource;
    use crate::size::CardSize;
    use crate::testkit::asset_path;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0_u8; (width * height) as usize])
                .unwrap();
        }
        bytes
    }

    fn svg_bytes(width: u32, height: u32) -> Vec<u8> {
        format!(r#"<svg width="{width}" height="{height}"></svg>"#).into_bytes()
    }

    /// A tiled background emits one sprite per tile, so a tiny tile is the
    /// one thing a theme controls that scales the per-frame sprite count
    /// without bound. The constraint is on tiling, not on the image: the
    /// same file stretched is fine.
    #[test]
    fn a_tile_below_the_minimum_edge_is_refused_but_stretches_fine() {
        let source = MemSource::new().with_file("table.png", png_bytes(4, 4));

        let tiled = Background::Image {
            path: asset_path("table.png"),
            tile: true,
        };
        let error = load(&source, &tiled, AssetKind::Png).unwrap_err();
        assert_eq!(
            error.to_string(),
            "[table] background: table.png is 4x4, but a tiled background must be at least 16x16"
        );

        let stretched = Background::Image {
            path: asset_path("table.png"),
            tile: false,
        };
        assert!(load(&source, &stretched, AssetKind::Png).is_ok());
    }

    /// The bound is per-axis: a tile wide enough but not tall enough is
    /// still unbounded in one direction.
    #[test]
    fn a_tile_short_on_only_one_axis_is_refused() {
        let source = MemSource::new().with_file("table.png", png_bytes(64, 8));
        let tiled = Background::Image {
            path: asset_path("table.png"),
            tile: true,
        };
        assert!(matches!(
            load(&source, &tiled, AssetKind::Png).unwrap_err(),
            ThemeError::TiledBackgroundTooSmall { .. }
        ));
    }

    /// A tile exactly at the minimum is legal — the rule is a floor, not a
    /// gap.
    #[test]
    fn a_tile_exactly_at_the_minimum_edge_is_accepted() {
        let source = MemSource::new().with_file("table.png", png_bytes(16, 16));
        let tiled = Background::Image {
            path: asset_path("table.png"),
            tile: true,
        };
        assert!(load(&source, &tiled, AssetKind::Png).is_ok());
    }

    #[test]
    fn a_color_background_loads_unchanged() {
        let source = MemSource::new();
        let background = Background::Color(Color::new(0x00, 0x80, 0x00));

        let loaded = load(&source, &background, AssetKind::Png).unwrap();
        assert_eq!(
            loaded,
            LoadedBackground::Color(Color::new(0x00, 0x80, 0x00))
        );
    }

    #[test]
    fn an_image_background_loads_with_tile_false() {
        let source = MemSource::new().with_file("table.png", png_bytes(200, 150));
        let background = Background::Image {
            path: asset_path("table.png"),
            tile: false,
        };

        let loaded = load(&source, &background, AssetKind::Png).unwrap();
        assert_eq!(
            loaded,
            LoadedBackground::Image {
                asset: Asset {
                    path: asset_path("table.png"),
                    bytes: png_bytes(200, 150),
                    kind: AssetKind::Png,
                    size: CardSize {
                        width: 200,
                        height: 150
                    },
                },
                tile: false,
            }
        );
    }

    #[test]
    fn an_image_background_passes_tile_true_through() {
        let source = MemSource::new().with_file("table.png", png_bytes(50, 50));
        let background = Background::Image {
            path: asset_path("table.png"),
            tile: true,
        };

        let loaded = load(&source, &background, AssetKind::Png).unwrap();
        assert_eq!(
            loaded,
            LoadedBackground::Image {
                asset: Asset {
                    path: asset_path("table.png"),
                    bytes: png_bytes(50, 50),
                    kind: AssetKind::Png,
                    size: CardSize {
                        width: 50,
                        height: 50
                    },
                },
                tile: true,
            }
        );
    }

    #[test]
    fn any_size_at_least_one_by_one_is_accepted_no_fixed_expected_size() {
        // Unlike faces/backs, background has no `base_size`-style equality
        // check — an unusual, non-card-shaped size must still pass.
        let source = MemSource::new().with_file("table.png", png_bytes(1920, 47));
        let background = Background::Image {
            path: asset_path("table.png"),
            tile: false,
        };
        assert!(load(&source, &background, AssetKind::Png).is_ok());
    }

    #[test]
    fn wrong_extension_for_the_render_mode_is_rejected() {
        let source = MemSource::new().with_file("table.svg", png_bytes(200, 150));
        let background = Background::Image {
            path: asset_path("table.svg"),
            tile: false,
        };

        let error = load(&source, &background, AssetKind::Png).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackgroundWrongExtension {
                expected_ext: ".png",
                ..
            }
        ));
    }

    #[test]
    fn an_unreadable_image_is_rejected() {
        let source = MemSource::new();
        let background = Background::Image {
            path: asset_path("table.png"),
            tile: false,
        };

        let error = load(&source, &background, AssetKind::Png).unwrap_err();
        assert!(matches!(error, ThemeError::BackgroundUnreadable { .. }));
    }

    #[test]
    fn bytes_that_do_not_probe_as_the_expected_format_are_rejected() {
        let source = MemSource::new().with_file("table.png", b"not a png".to_vec());
        let background = Background::Image {
            path: asset_path("table.png"),
            tile: false,
        };

        let error = load(&source, &background, AssetKind::Png).unwrap_err();
        assert!(matches!(error, ThemeError::BackgroundInvalidFormat { .. }));
    }

    #[test]
    fn a_vector_background_probes_via_svg() {
        let source = MemSource::new().with_file("table.svg", svg_bytes(300, 200));
        let background = Background::Image {
            path: asset_path("table.svg"),
            tile: false,
        };

        let loaded = load(&source, &background, AssetKind::Svg).unwrap();
        assert_eq!(
            loaded,
            LoadedBackground::Image {
                asset: Asset {
                    path: asset_path("table.svg"),
                    bytes: svg_bytes(300, 200),
                    kind: AssetKind::Svg,
                    size: CardSize {
                        width: 300,
                        height: 200
                    },
                },
                tile: false,
            }
        );
    }

    #[test]
    fn a_zero_sized_svg_background_is_rejected() {
        // Matrix row 6: "any size >= 1x1". Both probers reject a zero
        // dimension themselves (png.rs's IHDR check; svg.rs's usvg-backed
        // probe, since usvg's own `Size` type cannot be zero by
        // construction) — `load` has no separate manual check of its own,
        // it just maps that probe failure onto `BackgroundInvalidFormat`
        // (see `load`'s doc comment above).
        let source = MemSource::new().with_file("table.svg", svg_bytes(0, 0));
        let background = Background::Image {
            path: asset_path("table.svg"),
            tile: false,
        };

        let error = load(&source, &background, AssetKind::Svg).unwrap_err();
        assert!(matches!(error, ThemeError::BackgroundInvalidFormat { .. }));
    }

    #[test]
    fn a_zero_width_svg_background_is_rejected_even_with_a_nonzero_height() {
        // Exactly one dimension zero, not both: proves the SVG probe
        // rejects a zero on either axis alone, not only an all-zero size —
        // guards against a hypothetical "both dimensions zero" check that
        // this crate does not have (see the comment on the test above).
        let source = MemSource::new().with_file("table.svg", svg_bytes(0, 5));
        let background = Background::Image {
            path: asset_path("table.svg"),
            tile: false,
        };

        let error = load(&source, &background, AssetKind::Svg).unwrap_err();
        assert!(matches!(error, ThemeError::BackgroundInvalidFormat { .. }));
    }
}
