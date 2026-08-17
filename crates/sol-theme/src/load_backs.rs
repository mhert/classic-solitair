//! [`LoadedBack`] and back loading/validation: validation matrix row 5 (and
//! row 2's extension/format checks as they apply to back images).

use crate::asset::{self, Asset, AssetKind};
use crate::back::{BackDef, BackLayout, BackName, BackTiming};
use crate::path::RelativeAssetPath;
use crate::size::CardSize;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;

/// One `[backs]` entry, loaded and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBack {
    /// Number of frames: 1 for a static back, `frames` for a strip, the
    /// number of listed images for the list form.
    pub frame_count: u32,
    /// How the back's frames advance over time — `None` for a static back,
    /// `Some` for an animated one (strip or list), carrying the full timing
    /// (uniform `fps` or per-frame `durations_ms`) forward unchanged from
    /// [`BackDef`].
    pub timing: Option<BackTiming>,
    /// The strip layout axis — `Some` only for the strip shape.
    pub layout: Option<BackLayout>,
    /// The frame image bytes: one asset for static or strip (the whole
    /// strip is one file), `frame_count` assets for the list form.
    pub assets: Vec<Asset>,
}

/// Loads and validates every `[backs]` entry, in declaration order — the
/// first unreadable, malformed, or mis-sized back wins.
///
/// # Errors
///
/// See [`load_one`].
pub(crate) fn load(
    source: &impl AssetSource,
    backs: &[(BackName, BackDef)],
    kind: AssetKind,
    base_size: CardSize,
) -> Result<Vec<(BackName, LoadedBack)>, ThemeError> {
    backs
        .iter()
        .map(|(name, def)| Ok((name.clone(), load_one(source, name, def, kind, base_size)?)))
        .collect()
}

/// Loads and validates one `[backs]` entry.
///
/// # Errors
///
/// Returns [`ThemeError::BackWrongExtension`] if an image path does not end
/// in the extension `kind` requires, [`ThemeError::BackUnreadable`]
/// if an image cannot be read, [`ThemeError::BackInvalidFormat`] if its
/// bytes do not probe as `kind`, or [`ThemeError::BackWrongSize`] if the
/// probed size does not match what the back's shape requires.
fn load_one(
    source: &impl AssetSource,
    name: &BackName,
    def: &BackDef,
    kind: AssetKind,
    base_size: CardSize,
) -> Result<LoadedBack, ThemeError> {
    match def {
        BackDef::Static { image } => {
            let asset = load_frame(source, name, image, kind, base_size)?;
            Ok(LoadedBack {
                frame_count: 1,
                timing: None,
                layout: None,
                assets: vec![asset],
            })
        }
        BackDef::Strip {
            image,
            frames,
            timing,
            layout,
        } => {
            let expected = strip_size(base_size, *frames, *layout);
            let asset = load_frame(source, name, image, kind, expected)?;
            Ok(LoadedBack {
                frame_count: *frames,
                timing: Some(timing.clone()),
                layout: Some(*layout),
                assets: vec![asset],
            })
        }
        BackDef::Frames { images, timing } => {
            let assets = images
                .iter()
                .map(|image| load_frame(source, name, image, kind, base_size))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LoadedBack {
                frame_count: u32::try_from(images.len()).unwrap_or(u32::MAX),
                timing: Some(timing.clone()),
                layout: None,
                assets,
            })
        }
    }
}

/// The required strip image size: `frames ×` the base dimension
/// along `layout`'s axis, plain `base_size` on the other axis (e.g. a
/// 4-frame horizontal 71×96 back is 284×96).
fn strip_size(base_size: CardSize, frames: u32, layout: BackLayout) -> CardSize {
    match layout {
        BackLayout::Horizontal => CardSize {
            width: base_size.width.saturating_mul(frames),
            height: base_size.height,
        },
        BackLayout::Vertical => CardSize {
            width: base_size.width,
            height: base_size.height.saturating_mul(frames),
        },
    }
}

/// Reads, extension-checks, probes, and size-checks one back image (a
/// static back's image, a strip's single image, or one list-form frame).
fn load_frame(
    source: &impl AssetSource,
    name: &BackName,
    path: &RelativeAssetPath,
    kind: AssetKind,
    expected_size: CardSize,
) -> Result<Asset, ThemeError> {
    if !path.as_str().ends_with(kind.extension()) {
        return Err(ThemeError::BackWrongExtension {
            back: name.clone(),
            path: path.as_str().to_owned(),
            expected_ext: kind.extension(),
        });
    }

    let bytes = source
        .read(path)
        .map_err(|source| ThemeError::BackUnreadable {
            back: name.clone(),
            path: path.as_str().to_owned(),
            source,
        })?;
    let size = asset::probe(&bytes, kind).map_err(|reason| ThemeError::BackInvalidFormat {
        back: name.clone(),
        path: path.as_str().to_owned(),
        reason,
    })?;
    if size != expected_size {
        return Err(ThemeError::BackWrongSize {
            back: name.clone(),
            path: path.as_str().to_owned(),
            expected_width: expected_size.width,
            expected_height: expected_size.height,
            found_width: size.width,
            found_height: size.height,
        });
    }

    Ok(Asset {
        path: path.clone(),
        bytes,
        kind,
        size,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::back::{BackLayout, BackTiming};
    use crate::mem_source::MemSource;
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

    const BASE: CardSize = CardSize {
        width: 71,
        height: 96,
    };

    fn name(raw: &str) -> BackName {
        BackName::try_from(raw.to_owned()).unwrap()
    }

    // -- static --

    #[test]
    fn a_correctly_sized_static_back_loads_with_one_frame() {
        let source = MemSource::new().with_file("backs/plain.png", png_bytes(71, 96));
        let def = BackDef::Static {
            image: asset_path("backs/plain.png"),
        };

        let back = load_one(&source, &name("plain"), &def, AssetKind::Png, BASE).unwrap();

        assert_eq!(back.frame_count, 1);
        assert_eq!(back.timing, None);
        assert_eq!(back.layout, None);
        assert_eq!(back.assets.len(), 1);
        assert_eq!(
            back.assets.first().unwrap().path,
            asset_path("backs/plain.png")
        );
    }

    #[test]
    fn a_wrong_sized_static_back_is_rejected() {
        let source = MemSource::new().with_file("backs/plain.png", png_bytes(10, 10));
        let def = BackDef::Static {
            image: asset_path("backs/plain.png"),
        };

        let error = load_one(&source, &name("plain"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackWrongSize {
                back,
                expected_width: 71,
                expected_height: 96,
                found_width: 10,
                found_height: 10,
                ..
            } if back == name("plain")
        ));
    }

    #[test]
    fn a_missing_static_back_is_rejected() {
        let source = MemSource::new();
        let def = BackDef::Static {
            image: asset_path("backs/plain.png"),
        };

        let error = load_one(&source, &name("plain"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackUnreadable { back, .. } if back == name("plain")
        ));
    }

    #[test]
    fn a_static_back_with_bytes_that_do_not_probe_as_png_is_rejected() {
        let source = MemSource::new().with_file("backs/plain.png", b"nope".to_vec());
        let def = BackDef::Static {
            image: asset_path("backs/plain.png"),
        };

        let error = load_one(&source, &name("plain"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackInvalidFormat { back, .. } if back == name("plain")
        ));
    }

    #[test]
    fn a_back_image_with_the_wrong_extension_for_the_render_mode_is_rejected() {
        let source = MemSource::new().with_file("backs/plain.svg", png_bytes(71, 96));
        let def = BackDef::Static {
            image: asset_path("backs/plain.svg"),
        };

        let error = load_one(&source, &name("plain"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackWrongExtension {
                back,
                expected_ext: ".png",
                ..
            } if back == name("plain")
        ));
    }

    // -- strip --

    #[test]
    fn a_horizontal_strip_expects_frames_times_width_by_plain_height() {
        let source = MemSource::new().with_file("backs/robot.png", png_bytes(71 * 4, 96));
        let def = BackDef::Strip {
            image: asset_path("backs/robot.png"),
            frames: 4,
            timing: BackTiming::Fps(2),
            layout: BackLayout::Horizontal,
        };

        let back = load_one(&source, &name("robot"), &def, AssetKind::Png, BASE).unwrap();

        assert_eq!(back.frame_count, 4);
        assert_eq!(back.timing, Some(BackTiming::Fps(2)));
        assert_eq!(back.layout, Some(BackLayout::Horizontal));
        assert_eq!(back.assets.len(), 1);
    }

    #[test]
    fn a_durations_ms_strip_loads_with_durations_timing() {
        let source = MemSource::new().with_file("backs/palm.png", png_bytes(71 * 4, 96));
        let def = BackDef::Strip {
            image: asset_path("backs/palm.png"),
            frames: 4,
            timing: BackTiming::DurationsMs(vec![250, 250, 250, 49_250]),
            layout: BackLayout::Horizontal,
        };

        let back = load_one(&source, &name("palm"), &def, AssetKind::Png, BASE).unwrap();

        assert_eq!(back.frame_count, 4);
        assert_eq!(
            back.timing,
            Some(BackTiming::DurationsMs(vec![250, 250, 250, 49_250]))
        );
        assert_eq!(back.layout, Some(BackLayout::Horizontal));
        assert_eq!(back.assets.len(), 1);
    }

    #[test]
    fn a_vertical_strip_expects_plain_width_by_frames_times_height() {
        let source = MemSource::new().with_file("backs/robot.png", png_bytes(71, 96 * 3));
        let def = BackDef::Strip {
            image: asset_path("backs/robot.png"),
            frames: 3,
            timing: BackTiming::Fps(1),
            layout: BackLayout::Vertical,
        };

        let back = load_one(&source, &name("robot"), &def, AssetKind::Png, BASE).unwrap();

        assert_eq!(back.frame_count, 3);
        assert_eq!(back.layout, Some(BackLayout::Vertical));
        assert_eq!(
            back.assets.first().unwrap().size,
            CardSize {
                width: 71,
                height: 288
            }
        );
    }

    #[test]
    fn a_strip_sized_for_the_wrong_axis_is_rejected() {
        // Vertical strip declared, but the image is horizontally strip-sized.
        let source = MemSource::new().with_file("backs/robot.png", png_bytes(71 * 4, 96));
        let def = BackDef::Strip {
            image: asset_path("backs/robot.png"),
            frames: 4,
            timing: BackTiming::Fps(2),
            layout: BackLayout::Vertical,
        };

        let error = load_one(&source, &name("robot"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackWrongSize {
                expected_width: 71,
                expected_height: 384,
                found_width: 284,
                found_height: 96,
                ..
            }
        ));
    }

    #[test]
    fn a_vector_strip_uses_the_same_math_as_a_pixel_strip() {
        let source = MemSource::new().with_file("backs/robot.svg", svg_bytes(71 * 4, 96));
        let def = BackDef::Strip {
            image: asset_path("backs/robot.svg"),
            frames: 4,
            timing: BackTiming::Fps(2),
            layout: BackLayout::Horizontal,
        };

        let back = load_one(&source, &name("robot"), &def, AssetKind::Svg, BASE).unwrap();
        assert_eq!(back.assets.first().unwrap().kind, AssetKind::Svg);
        assert_eq!(back.frame_count, 4);
    }

    // -- list (Frames) --

    #[test]
    fn a_list_back_loads_every_frame_at_base_size() {
        let source = MemSource::new()
            .with_file("backs/bats_0.png", png_bytes(71, 96))
            .with_file("backs/bats_1.png", png_bytes(71, 96));
        let def = BackDef::Frames {
            images: vec![
                asset_path("backs/bats_0.png"),
                asset_path("backs/bats_1.png"),
            ],
            timing: BackTiming::Fps(3),
        };

        let back = load_one(&source, &name("bats"), &def, AssetKind::Png, BASE).unwrap();

        assert_eq!(back.frame_count, 2);
        assert_eq!(back.timing, Some(BackTiming::Fps(3)));
        assert_eq!(back.layout, None);
        assert_eq!(back.assets.len(), 2);
        assert_eq!(
            back.assets.first().unwrap().path,
            asset_path("backs/bats_0.png")
        );
        assert_eq!(
            back.assets.get(1).unwrap().path,
            asset_path("backs/bats_1.png")
        );
    }

    #[test]
    fn a_list_back_with_one_wrong_sized_frame_names_that_frame() {
        let source = MemSource::new()
            .with_file("backs/bats_0.png", png_bytes(71, 96))
            .with_file("backs/bats_1.png", png_bytes(1, 1));
        let def = BackDef::Frames {
            images: vec![
                asset_path("backs/bats_0.png"),
                asset_path("backs/bats_1.png"),
            ],
            timing: BackTiming::Fps(3),
        };

        let error = load_one(&source, &name("bats"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackWrongSize { path, .. } if path == "backs/bats_1.png"
        ));
    }

    #[test]
    fn a_list_back_with_one_missing_frame_names_that_frame() {
        let source = MemSource::new().with_file("backs/bats_0.png", png_bytes(71, 96));
        let def = BackDef::Frames {
            images: vec![
                asset_path("backs/bats_0.png"),
                asset_path("backs/bats_1.png"),
            ],
            timing: BackTiming::Fps(3),
        };

        let error = load_one(&source, &name("bats"), &def, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackUnreadable { path, .. } if path == "backs/bats_1.png"
        ));
    }

    // -- multi-back orchestration (declaration order, first failure wins) --

    #[test]
    fn load_processes_backs_in_declaration_order_and_returns_the_first_failure() {
        let source = MemSource::new().with_file("backs/second.png", png_bytes(71, 96));
        let backs = vec![
            (
                name("first"),
                BackDef::Static {
                    image: asset_path("backs/missing.png"),
                },
            ),
            (
                name("second"),
                BackDef::Static {
                    image: asset_path("backs/second.png"),
                },
            ),
        ];

        let error = load(&source, &backs, AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackUnreadable { back, .. } if back == name("first")
        ));
    }

    #[test]
    fn load_returns_every_back_in_declaration_order_on_success() {
        let source = MemSource::new()
            .with_file("backs/a.png", png_bytes(71, 96))
            .with_file("backs/b.png", png_bytes(71, 96));
        let backs = vec![
            (
                name("second"),
                BackDef::Static {
                    image: asset_path("backs/b.png"),
                },
            ),
            (
                name("first"),
                BackDef::Static {
                    image: asset_path("backs/a.png"),
                },
            ),
        ];

        let loaded = load(&source, &backs, AssetKind::Png, BASE).unwrap();
        let names: Vec<&str> = loaded.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["second", "first"]);
    }
}
