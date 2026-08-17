//! Face loading and validation: directory form (validation matrix row 3)
//! and SVG sheet form (row 4).

use crate::asset::{self, Asset, AssetKind};
use crate::face::canonical_faces;
use crate::faces::FacesSource;
use crate::path::RelativeAssetPath;
use crate::size::CardSize;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;

/// Loads and validates every face image in canonical order (spades, hearts,
/// diamonds, clubs; rank ascending) — the first missing, malformed, or
/// mis-sized face wins, so a theme author gets one deterministic error.
///
/// Always returns exactly 52 assets, indexed by canonical order (index `i`
/// is `crate::face::canonical_faces().nth(i)`'s asset). For the SVG sheet
/// form, every entry is a clone of the same single-sheet
/// [`Asset`]: the sheet is one file holding all 52 faces, and slicing out
/// an individual face's sub-region is pixel/vector decoding work — out of
/// this crate's scope (the module doc's PNG/SVG probing boundary) and left
/// to the renderer. A theme package is small, so the duplication this
/// costs is an acceptable, deliberate simplicity trade: every caller gets
/// the same per-face lookup API regardless of which form a theme uses.
///
/// # Errors
///
/// Returns [`ThemeError::FaceUnreadable`] / [`ThemeError::FaceSheetUnreadable`]
/// if a face (or the sheet) cannot be read from `source`,
/// [`ThemeError::FaceInvalidFormat`] / [`ThemeError::FaceSheetInvalidFormat`]
/// if its bytes do not probe as `kind`, or [`ThemeError::FaceWrongSize`] /
/// [`ThemeError::FaceSheetWrongSize`] if the probed size is wrong.
pub(crate) fn load(
    source: &impl AssetSource,
    faces: &FacesSource,
    kind: AssetKind,
    base_size: CardSize,
) -> Result<Vec<Asset>, ThemeError> {
    match faces {
        FacesSource::Directory(dir) => load_directory(source, dir, kind, base_size),
        FacesSource::SvgSheet(path) => load_sheet(source, path, base_size),
    }
}

fn load_directory(
    source: &impl AssetSource,
    dir: &RelativeAssetPath,
    kind: AssetKind,
    base_size: CardSize,
) -> Result<Vec<Asset>, ThemeError> {
    let mut assets = Vec::with_capacity(52);
    for (suit, rank) in canonical_faces() {
        let name = suit.stem(rank);
        // The directory is a parsed path and the stem comes from the
        // canonical 52, so the join needs no second validation.
        let path = dir.join_generated(&format!("{name}{}", kind.extension()));

        let bytes = source
            .read(&path)
            .map_err(|source| ThemeError::FaceUnreadable {
                name: name.clone(),
                source,
            })?;
        let size = asset::probe(&bytes, kind).map_err(|reason| ThemeError::FaceInvalidFormat {
            name: name.clone(),
            path: path.as_str().to_owned(),
            reason,
        })?;
        if size != base_size {
            return Err(ThemeError::FaceWrongSize {
                name,
                expected_width: base_size.width,
                expected_height: base_size.height,
                found_width: size.width,
                found_height: size.height,
            });
        }

        assets.push(Asset {
            path,
            bytes,
            kind,
            size,
        });
    }
    Ok(assets)
}

fn load_sheet(
    source: &impl AssetSource,
    path: &RelativeAssetPath,
    base_size: CardSize,
) -> Result<Vec<Asset>, ThemeError> {
    let bytes = source
        .read(path)
        .map_err(|source| ThemeError::FaceSheetUnreadable {
            path: path.as_str().to_owned(),
            source,
        })?;
    let size = asset::probe(&bytes, AssetKind::Svg).map_err(|reason| {
        ThemeError::FaceSheetInvalidFormat {
            path: path.as_str().to_owned(),
            reason,
        }
    })?;

    let expected = CardSize {
        width: base_size.width.saturating_mul(13),
        height: base_size.height.saturating_mul(4),
    };
    if size != expected {
        return Err(ThemeError::FaceSheetWrongSize {
            path: path.as_str().to_owned(),
            expected_width: expected.width,
            expected_height: expected.height,
            found_width: size.width,
            found_height: size.height,
        });
    }

    let sheet = Asset {
        path: path.clone(),
        bytes,
        kind: AssetKind::Svg,
        size,
    };
    Ok(std::iter::repeat_n(sheet, 52).collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
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

    /// `(path, bytes)` for all 52 canonical faces at `cards/<suit>_<NN>.png`,
    /// each `size` — the raw pairs, so missing-face tests can filter one out
    /// before collecting into a [`MemSource`] (which has no "remove" API).
    fn face_pairs(size: CardSize) -> Vec<(String, Vec<u8>)> {
        canonical_faces()
            .map(|(suit, rank)| {
                (
                    format!("cards/{}.png", suit.stem(rank)),
                    png_bytes(size.width, size.height),
                )
            })
            .collect()
    }

    /// A `MemSource` with all 52 canonical faces present at
    /// `cards/<suit>_<NN>.png`, each `size`.
    fn full_directory(size: CardSize) -> MemSource {
        face_pairs(size).into_iter().collect()
    }

    // -- directory form --

    #[test]
    fn a_complete_directory_loads_all_52_in_canonical_order() {
        let source = full_directory(BASE);
        let faces = load_directory(&source, &asset_path("cards/"), AssetKind::Png, BASE).unwrap();

        assert_eq!(faces.len(), 52);
        assert_eq!(
            faces.first().unwrap().path,
            asset_path("cards/spades_01.png")
        );
        assert_eq!(faces.last().unwrap().path, asset_path("cards/clubs_13.png"));
        assert!(faces.iter().all(|asset| asset.kind == AssetKind::Png));
        assert!(faces.iter().all(|asset| asset.size == BASE));
    }

    #[test]
    fn a_missing_face_is_reported_by_canonical_name() {
        let source: MemSource = face_pairs(BASE)
            .into_iter()
            .filter(|(path, _)| path != "cards/hearts_07.png")
            .collect();

        let error =
            load_directory(&source, &asset_path("cards/"), AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceUnreadable { name, .. } if name == "hearts_07"
        ));
    }

    #[test]
    fn the_first_missing_face_in_canonical_order_wins_over_a_later_one() {
        let source: MemSource = face_pairs(BASE)
            .into_iter()
            .filter(|(path, _)| path != "cards/clubs_01.png" && path != "cards/spades_05.png")
            .collect();

        let error =
            load_directory(&source, &asset_path("cards/"), AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceUnreadable { name, .. } if name == "spades_05"
        ));
    }

    #[test]
    fn a_wrong_sized_face_is_reported_by_name() {
        let mut source = full_directory(BASE);
        source = source.with_file("cards/diamonds_10.png", png_bytes(10, 10));

        let error =
            load_directory(&source, &asset_path("cards/"), AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceWrongSize {
                name,
                expected_width: 71,
                expected_height: 96,
                found_width: 10,
                found_height: 10,
            } if name == "diamonds_10"
        ));
    }

    #[test]
    fn a_face_whose_bytes_do_not_probe_as_png_is_reported_by_name() {
        let mut source = full_directory(BASE);
        source = source.with_file("cards/clubs_04.png", b"not a png".to_vec());

        let error =
            load_directory(&source, &asset_path("cards/"), AssetKind::Png, BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceInvalidFormat { name, .. } if name == "clubs_04"
        ));
    }

    // -- sheet form --

    #[test]
    fn a_correctly_sized_sheet_loads_as_52_identical_assets() {
        let source = MemSource::new().with_file("cards/sheet.svg", svg_bytes(71 * 13, 96 * 4));

        let faces = load_sheet(&source, &asset_path("cards/sheet.svg"), BASE).unwrap();

        assert_eq!(faces.len(), 52);
        assert!(
            faces
                .iter()
                .all(|asset| asset.path == asset_path("cards/sheet.svg"))
        );
        assert!(faces.iter().all(|asset| asset.kind == AssetKind::Svg));
        assert!(faces.iter().all(|asset| asset.size
            == CardSize {
                width: 923,
                height: 384
            }));
    }

    #[test]
    fn a_missing_sheet_is_a_typed_error() {
        let source = MemSource::new();
        let error = load_sheet(&source, &asset_path("cards/sheet.svg"), BASE).unwrap_err();
        assert!(
            matches!(error, ThemeError::FaceSheetUnreadable { path, .. } if path == "cards/sheet.svg")
        );
    }

    #[test]
    fn a_wrong_sized_sheet_is_a_typed_error() {
        let source = MemSource::new().with_file("cards/sheet.svg", svg_bytes(100, 100));
        let error = load_sheet(&source, &asset_path("cards/sheet.svg"), BASE).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceSheetWrongSize {
                expected_width: 923,
                expected_height: 384,
                found_width: 100,
                found_height: 100,
                ..
            }
        ));
    }

    #[test]
    fn a_sheet_that_does_not_probe_as_svg_is_a_typed_error() {
        let source = MemSource::new().with_file("cards/sheet.svg", b"not svg".to_vec());
        let error = load_sheet(&source, &asset_path("cards/sheet.svg"), BASE).unwrap_err();
        assert!(matches!(error, ThemeError::FaceSheetInvalidFormat { .. }));
    }

    #[test]
    fn load_dispatches_directory_and_sheet_forms() {
        let dir_source = full_directory(BASE);
        let directory = FacesSource::Directory(asset_path("cards/"));
        assert_eq!(
            load(&dir_source, &directory, AssetKind::Png, BASE)
                .unwrap()
                .len(),
            52
        );

        let sheet_source =
            MemSource::new().with_file("cards/sheet.svg", svg_bytes(71 * 13, 96 * 4));
        let sheet = FacesSource::SvgSheet(asset_path("cards/sheet.svg"));
        assert_eq!(
            load(&sheet_source, &sheet, AssetKind::Svg, BASE)
                .unwrap()
                .len(),
            52
        );
    }
}
