//! PE (Win32 / PE32+) resource reader, backed by `pelite`.
//!
//! Where [`crate::ne`] hand-rolls the Win16 table, PE binaries are read
//! through `pelite`'s memory-safe, zero-allocation resource walker. Both PE32
//! and PE32+ flow through the same unified [`pelite::PeFile`] API, so one code
//! path serves both. The `RT_BITMAP` (type id 2) resources it yields are the
//! same header-less DIBs the NE path produces, decoded downstream by
//! [`crate::dib`].
//!
//! Only integer-id resources map to cards; string-named ones are counted and
//! skipped (surfaced in the `extract` summary). A `PE` with no `RT_BITMAP`
//! type at all simply yields no bitmaps — the classifier then reports the
//! missing faces, exactly as for `SOL.EXE`.

use core::fmt::Display;

use pelite::PeFile;
use pelite::resources::{Entry, Name, Resources};

use crate::resource::{ContainerBitmaps, ResourceBitmap};

/// PE resource type id for `RT_BITMAP`.
const RT_BITMAP: u16 = 2;

/// Reads every integer-id `RT_BITMAP` resource from the PE binary in `data`.
///
/// # Errors
///
/// Returns [`PeError::Parse`] if `pelite` cannot parse `data` as a PE image,
/// or [`PeError::Walk`] if its resource directory cannot be read.
pub fn extract(data: &[u8]) -> Result<ContainerBitmaps, PeError> {
    // `PeFile::from_bytes` auto-detects PE32 vs PE32+, and the unified
    // `resources()` dispatches over both — one path serves either format.
    let file = PeFile::from_bytes(data).map_err(parse_error)?;
    let resources = file.resources().map_err(walk_error)?;
    walk(resources)
}

/// Walks the `RT_BITMAP` directory of `resources`, collecting integer-id
/// bitmaps and counting string-named ones as skipped.
fn walk(resources: Resources<'_>) -> Result<ContainerBitmaps, PeError> {
    let mut result = ContainerBitmaps::default();
    let root = resources.root().map_err(walk_error)?;
    // No RT_BITMAP type at all: nothing to extract (the classifier then
    // reports the missing faces, as for SOL.EXE).
    let Ok(bitmap_dir) = root.get_dir(Name::Id(u32::from(RT_BITMAP))) else {
        return Ok(result);
    };
    for entry in bitmap_dir.entries() {
        let name = entry.name().map_err(walk_error)?;
        let Name::Id(id) = name else {
            result.string_named_skipped += 1;
            continue;
        };
        let child = entry.entry().map_err(walk_error)?;
        let bytes = leaf_bytes(child)?;
        result.bitmaps.push(ResourceBitmap {
            id,
            data: bytes.to_vec(),
        });
    }
    Ok(result)
}

/// The bytes of the first data entry under `entry`: either the entry itself
/// (a two-level tree) or the first data entry of its language directory (the
/// standard three-level tree).
fn leaf_bytes(entry: Entry<'_>) -> Result<&[u8], PeError> {
    match entry {
        Entry::DataEntry(data) => data.bytes().map_err(walk_error),
        Entry::Directory(dir) => dir
            .first_data()
            .map_err(walk_error)?
            .bytes()
            .map_err(walk_error),
    }
}

/// Wraps any `pelite` parse failure as [`PeError::Parse`].
fn parse_error<E: Display>(error: E) -> PeError {
    PeError::Parse {
        message: error.to_string(),
    }
}

/// Wraps any `pelite` resource-walk failure as [`PeError::Walk`] (`pelite`
/// exposes two distinct error types across the walk, both `Display`).
fn walk_error<E: Display>(error: E) -> PeError {
    PeError::Walk {
        message: error.to_string(),
    }
}

/// Every way [`extract`] can fail to read PE resources.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PeError {
    /// `pelite` could not parse the bytes as a PE image.
    #[error("failed to parse PE image: {message}")]
    Parse {
        /// The underlying `pelite` error, rendered to text (kept out of this
        /// crate's public API — mirrors `sol_theme`'s foreign-error handling).
        message: String,
    },
    /// The PE parsed, but its resource directory could not be walked.
    #[error("failed to read PE resources: {message}")]
    Walk {
        /// The underlying `pelite` error, rendered to text.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::{Rsrc, build_pe, solid_dib};

    #[test]
    fn pelite_parses_the_synthetic_pe_and_extracts_integer_bitmaps() {
        let image = build_pe(&[(
            RT_BITMAP,
            vec![
                Rsrc::Id(1, solid_dib(5, 7, (255, 0, 0))),
                Rsrc::Id(2, solid_dib(5, 7, (0, 255, 0))),
            ],
        )]);
        let result = extract(&image).unwrap();
        assert_eq!(result.bitmaps.len(), 2);
        assert_eq!(result.bitmaps.first().unwrap().id, 1);
        assert_eq!(result.bitmaps.get(1).unwrap().id, 2);
        // The bytes round-trip through the DIB decoder to the solid color.
        let decoded = crate::dib::decode_dib(&result.bitmaps.first().unwrap().data).unwrap();
        assert_eq!(decoded.pixels.get(0..4).unwrap(), [255, 0, 0, 255]);
    }

    #[test]
    fn a_string_named_bitmap_resource_is_skipped_and_counted() {
        let image = build_pe(&[(
            RT_BITMAP,
            vec![
                Rsrc::Id(1, solid_dib(5, 7, (1, 2, 3))),
                Rsrc::Named("SPLASH", solid_dib(5, 7, (4, 5, 6))),
            ],
        )]);
        let result = extract(&image).unwrap();
        assert_eq!(result.bitmaps.len(), 1);
        assert_eq!(result.bitmaps.first().unwrap().id, 1);
        assert_eq!(result.string_named_skipped, 1);
    }

    #[test]
    fn a_two_level_id_resource_pointing_straight_at_data_is_read() {
        // Some PE trees skip the language level; the id entry points directly
        // at a data entry. The reader must handle that as well as the usual
        // three-level tree.
        let image = build_pe(&[(
            RT_BITMAP,
            vec![Rsrc::IdDirect(9, solid_dib(5, 7, (7, 8, 9)))],
        )]);
        let result = extract(&image).unwrap();
        assert_eq!(result.bitmaps.len(), 1);
        assert_eq!(result.bitmaps.first().unwrap().id, 9);
        let decoded = crate::dib::decode_dib(&result.bitmaps.first().unwrap().data).unwrap();
        assert_eq!(decoded.pixels.get(0..4).unwrap(), [7, 8, 9, 255]);
    }

    #[test]
    fn a_pe_with_no_bitmap_type_yields_no_bitmaps() {
        // A resource directory whose only type is not RT_BITMAP (id 16 here):
        // `get_dir(RT_BITMAP)` fails and extraction yields nothing.
        let image = build_pe(&[(16, vec![Rsrc::Id(1, solid_dib(5, 7, (0, 0, 0)))])]);
        let result = extract(&image).unwrap();
        assert!(result.bitmaps.is_empty());
        assert_eq!(result.string_named_skipped, 0);
    }

    #[test]
    fn bytes_pelite_cannot_parse_are_a_parse_error() {
        let error = extract(&[0_u8; 64]).unwrap_err();
        assert!(matches!(error, PeError::Parse { .. }));
    }

    #[test]
    fn a_pe_whose_resource_data_directory_is_absent_is_a_walk_error() {
        // Build a valid PE, then zero its resource data directory entry (index
        // 2, at PE + 4 + 20 + 96 + 16). `from_bytes` still parses the image,
        // but `resources()` then fails — exercising the walk-error path.
        let mut image = build_pe(&[(RT_BITMAP, vec![Rsrc::Id(1, solid_dib(5, 7, (0, 0, 0)))])]);
        let resource_dir = 0x40 + 4 + 20 + 96 + 2 * 8;
        for byte in image.get_mut(resource_dir..resource_dir + 8).unwrap() {
            *byte = 0;
        }
        let error = extract(&image).unwrap_err();
        assert!(matches!(error, PeError::Walk { .. }));
    }

    #[test]
    fn every_error_variant_renders_a_non_empty_message() {
        for error in [
            PeError::Parse {
                message: "boom".to_owned(),
            },
            PeError::Walk {
                message: "boom".to_owned(),
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
