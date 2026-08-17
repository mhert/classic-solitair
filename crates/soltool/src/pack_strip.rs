//! `soltool pack-strip <frames…> -o <strip.png> --fps <n>`: packs
//! loose frame images into one horizontal strip PNG, side by side in input
//! order, and prints a ready-to-paste `[backs]` snippet for `theme.toml`.

use std::path::{Path, PathBuf};

use crate::raster::{self, RasterImage};

/// Packs `frames` (at least 2 paths, left to right) into one horizontal
/// strip PNG written to `output`, then returns a ready-to-paste `[backs]`
/// snippet naming `fps` — the strip file itself carries no fps metadata,
/// so the snippet is how that number reaches `theme.toml`. The back name is
/// derived from `output`'s file stem, sanitized into a valid back name (see
/// [`snippet`]).
///
/// ```
/// use std::path::Path;
///
/// use soltool::pack_strip;
///
/// let error = pack_strip::run(&[], Path::new("out.png"), 2).unwrap_err();
/// assert!(error.to_string().contains("at least 2"));
/// ```
///
/// # Errors
///
/// Returns [`PackStripError::TooFewFrames`] if `frames` has fewer than 2
/// entries, [`PackStripError::Decode`] if a frame cannot be read or
/// decoded, [`PackStripError::DimensionMismatch`] if a frame's dimensions
/// differ from the first frame's, or [`PackStripError::Write`] if the
/// strip cannot be encoded or written to `output`.
pub fn run(frames: &[PathBuf], output: &Path, fps: u8) -> Result<String, PackStripError> {
    if frames.len() < 2 {
        return Err(PackStripError::TooFewFrames {
            count: frames.len(),
        });
    }

    let mut decoded = Vec::with_capacity(frames.len());
    for path in frames {
        let image = decode_frame(path)?;
        if let Some(reference) = decoded.first() {
            check_dimensions_match(path, reference, &image)?;
        }
        decoded.push(image);
    }

    let strip = build_strip(&decoded);
    write_strip(&strip, output)?;

    Ok(snippet(output, decoded.len(), fps))
}

fn decode_frame(path: &Path) -> Result<RasterImage, PackStripError> {
    let decode_error = |message: String| PackStripError::Decode {
        path: path.to_owned(),
        message,
    };
    let bytes = std::fs::read(path).map_err(|source| decode_error(source.to_string()))?;
    raster::decode(&bytes).map_err(|source| decode_error(source.to_string()))
}

fn check_dimensions_match(
    path: &Path,
    reference: &RasterImage,
    image: &RasterImage,
) -> Result<(), PackStripError> {
    if image.width == reference.width && image.height == reference.height {
        Ok(())
    } else {
        Err(PackStripError::DimensionMismatch {
            path: path.to_owned(),
            expected_width: reference.width,
            expected_height: reference.height,
            found_width: image.width,
            found_height: image.height,
        })
    }
}

/// Concatenates `frames` (already confirmed same-sized and non-empty) into
/// one horizontal strip: for each row, every frame's same row, in input
/// order, left to right. Shared with `extract`'s loose-frame back packing.
pub(crate) fn build_strip(frames: &[RasterImage]) -> RasterImage {
    let (frame_width, frame_height) = frames
        .first()
        .map_or((0, 0), |image| (image.width, image.height));
    let row_bytes = (frame_width as usize) * 4;

    let mut pixels = Vec::with_capacity(row_bytes * frames.len() * (frame_height as usize));
    for row in 0..frame_height {
        for frame in frames {
            let start = (row as usize) * row_bytes;
            let row_pixels = frame.pixels.get(start..start + row_bytes).unwrap_or(&[]);
            pixels.extend_from_slice(row_pixels);
        }
    }

    let strip_width = frame_width.saturating_mul(u32::try_from(frames.len()).unwrap_or(u32::MAX));
    RasterImage {
        width: strip_width,
        height: frame_height,
        pixels,
    }
}

fn write_strip(strip: &RasterImage, output: &Path) -> Result<(), PackStripError> {
    let write_error = |message: String| PackStripError::Write {
        path: output.to_owned(),
        message,
    };
    let bytes = raster::encode(strip).map_err(|source| write_error(source.to_string()))?;
    std::fs::write(output, bytes).map_err(|source| write_error(source.to_string()))
}

/// A ready-to-paste `[backs]` entry, e.g. for `output = "robot.png"`,
/// `frame_count = 4`, `fps = 2`: `` robot = { image = "robot.png", frames =
/// 4, fps = 2 } ``. The back name is `output`'s file stem, sanitized by
/// [`sanitize_back_name`] so the snippet is always both valid TOML and a
/// valid `sol_theme::BackName` — not just when the stem happens to already
/// fit that shape. `image` is `output` exactly as given, TOML-escaped via
/// [`crate::manifest_writer::toml_string`] so a quote or backslash in the
/// path cannot break the snippet.
fn snippet(output: &Path, frame_count: usize, fps: u8) -> String {
    let stem = output
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("");
    let back_name = sanitize_back_name(stem);
    let image = crate::manifest_writer::toml_string(&output.display().to_string());
    format!("{back_name} = {{ image = {image}, frames = {frame_count}, fps = {fps} }}")
}

/// Sanitizes `stem` into the alphabet `sol_theme::BackName` requires —
/// non-empty ASCII `[a-z0-9_-]+` — so a snippet built from it is guaranteed
/// to be both a valid TOML bare key and a valid back name: ASCII uppercase
/// letters are lowercased, and every other character (spaces, dots, other
/// punctuation, non-ASCII) becomes `_`. Falls back to `"strip"` when `stem`
/// is empty, since sanitization alone can never turn an empty stem into a
/// non-empty name. Shared with `extract`'s loose-back naming.
pub(crate) fn sanitize_back_name(stem: &str) -> String {
    if stem.is_empty() {
        return "strip".to_owned();
    }
    stem.chars()
        .map(|ch| match ch.to_ascii_lowercase() {
            lower
                if lower.is_ascii_lowercase()
                    || lower.is_ascii_digit()
                    || lower == '_'
                    || lower == '-' =>
            {
                lower
            }
            _ => '_',
        })
        .collect()
}

/// Every way [`run`] can fail to pack a strip.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackStripError {
    /// Fewer than 2 frame files were given.
    #[error("pack-strip requires at least 2 frames, got {count}")]
    TooFewFrames {
        /// The number of frames actually given.
        count: usize,
    },
    /// A frame's dimensions differ from the first frame's.
    #[error(
        "frame {path}: {found_width}x{found_height} does not match the first frame's {expected_width}x{expected_height}"
    )]
    DimensionMismatch {
        /// The first frame whose dimensions did not match.
        path: PathBuf,
        /// The first frame's width, every other frame must match it.
        expected_width: u32,
        /// The first frame's height, every other frame must match it.
        expected_height: u32,
        /// The offending frame's actual width.
        found_width: u32,
        /// The offending frame's actual height.
        found_height: u32,
    },
    /// A frame file could not be read or decoded.
    #[error("failed to decode frame {path}: {message}")]
    Decode {
        /// The offending frame file.
        path: PathBuf,
        /// The underlying read or decode failure, rendered to text (a
        /// mix of `std::io::Error` and
        /// [`crate::raster::RasterDecodeError`], so rendered here rather
        /// than crossing this crate's public API — mirrors
        /// `sol_theme::SourceError::Io`).
        message: String,
    },
    /// The strip could not be encoded or written to the output path.
    #[error("failed to write {path}: {message}")]
    Write {
        /// The output path that could not be written.
        path: PathBuf,
        /// The underlying encode or write failure, rendered to text (see
        /// [`PackStripError::Decode`]).
        message: String,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    /// A `width`x`height` image filled with `color` (RGBA8).
    fn solid(width: u32, height: u32, color: [u8; 4]) -> RasterImage {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&color);
        }
        RasterImage {
            width,
            height,
            pixels,
        }
    }

    fn write_png(dir: &Path, name: &str, image: &RasterImage) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, raster::encode(image).unwrap()).unwrap();
        path
    }

    // -- build_strip: pixel layout --

    #[test]
    fn two_distinct_2x2_frames_pack_into_a_4x2_strip_with_matching_halves() {
        let red = solid(2, 2, [255, 0, 0, 255]);
        let blue = solid(2, 2, [0, 0, 255, 255]);
        let strip = build_strip(&[red.clone(), blue.clone()]);

        assert_eq!(strip.width, 4);
        assert_eq!(strip.height, 2);

        // Row-major RGBA8: row 0 is [red, red, blue, blue], row 1 the same.
        let row0 = strip.pixels.get(0..16).unwrap();
        assert_eq!(
            row0,
            [
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255
            ]
        );
        let row1 = strip.pixels.get(16..32).unwrap();
        assert_eq!(row1, row0);
    }

    #[test]
    fn build_strip_reads_each_output_row_from_its_own_source_row() {
        // A 1x2 frame whose two rows differ (row 0 red, row 1 blue): a
        // row-indexing bug (e.g. `row + row_bytes` or `row / row_bytes`
        // instead of `row * row_bytes`) would read the wrong row -- but a
        // uniformly-colored frame could never reveal that, since every row
        // would look the same either way.
        let frame = RasterImage {
            width: 1,
            height: 2,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let strip = build_strip(&[frame]);
        assert_eq!(strip.pixels.get(0..4).unwrap(), [255, 0, 0, 255]);
        assert_eq!(strip.pixels.get(4..8).unwrap(), [0, 0, 255, 255]);
    }

    #[test]
    fn build_strip_on_an_empty_slice_is_an_empty_zero_sized_image() {
        let strip = build_strip(&[]);
        assert_eq!(strip.width, 0);
        assert_eq!(strip.height, 0);
        assert!(strip.pixels.is_empty());
    }

    // -- run: typed errors --

    #[test]
    fn fewer_than_two_frames_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let one = write_png(dir.path(), "a.png", &solid(2, 2, [1, 2, 3, 4]));
        let error = run(&[one], &dir.path().join("out.png"), 2).unwrap_err();
        assert!(matches!(error, PackStripError::TooFewFrames { count: 1 }));
    }

    #[test]
    fn zero_frames_is_rejected() {
        let error = run(&[], Path::new("out.png"), 2).unwrap_err();
        assert!(matches!(error, PackStripError::TooFewFrames { count: 0 }));
    }

    #[test]
    fn mismatched_frame_dimensions_name_the_offender() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_png(dir.path(), "a.png", &solid(2, 2, [1, 1, 1, 255]));
        let second = write_png(dir.path(), "b.png", &solid(3, 2, [2, 2, 2, 255]));

        let error = run(&[first, second.clone()], &dir.path().join("out.png"), 2).unwrap_err();
        assert!(matches!(
            error,
            PackStripError::DimensionMismatch {
                path,
                expected_width: 2,
                expected_height: 2,
                found_width: 3,
                found_height: 2,
            } if path == second
        ));
    }

    #[test]
    fn an_unreadable_frame_path_is_a_decode_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.png");
        let present = write_png(dir.path(), "present.png", &solid(2, 2, [1, 2, 3, 4]));

        let error = run(&[missing.clone(), present], &dir.path().join("out.png"), 2).unwrap_err();
        assert!(matches!(error, PackStripError::Decode { path, .. } if path == missing));
    }

    #[test]
    fn a_frame_that_is_not_a_valid_png_is_a_decode_error() {
        let dir = tempfile::tempdir().unwrap();
        let garbage = dir.path().join("garbage.png");
        std::fs::write(&garbage, b"not a png").unwrap();
        let present = write_png(dir.path(), "present.png", &solid(2, 2, [1, 2, 3, 4]));

        let error = run(&[garbage.clone(), present], &dir.path().join("out.png"), 2).unwrap_err();
        assert!(matches!(error, PackStripError::Decode { path, .. } if path == garbage));
    }

    #[test]
    fn an_unwritable_output_path_is_a_write_error() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_png(dir.path(), "a.png", &solid(2, 2, [1, 1, 1, 255]));
        let second = write_png(dir.path(), "b.png", &solid(2, 2, [2, 2, 2, 255]));
        let output = dir.path().join("no-such-directory").join("out.png");

        let error = run(&[first, second], &output, 2).unwrap_err();
        assert!(matches!(error, PackStripError::Write { path, .. } if path == output));
    }

    #[test]
    fn write_strip_reports_an_encode_failure_as_a_write_error() {
        // `run` itself can never reach this: `build_strip` always hands
        // `write_strip` a pixel buffer of exactly `width * height * 4`
        // bytes (non-zero, since `run` already rejected fewer than 2
        // decoded frames), so `raster::encode` can't fail on that path.
        // `write_strip` is private, so this calls it directly with a
        // hand-built `RasterImage` whose pixel buffer is the wrong length
        // for its declared dimensions, exercising the encode-failure arm
        // of `PackStripError::Write` that `run` alone cannot reach.
        let dir = tempfile::tempdir().unwrap();
        let malformed = RasterImage {
            width: 2,
            height: 2,
            pixels: vec![0; 3],
        };
        let output = dir.path().join("out.png");

        let error = write_strip(&malformed, &output).unwrap_err();

        assert!(matches!(error, PackStripError::Write { path, .. } if path == output));
    }

    #[test]
    fn typed_error_messages_are_human_readable() {
        for error in [
            PackStripError::TooFewFrames { count: 1 },
            PackStripError::DimensionMismatch {
                path: PathBuf::from("b.png"),
                expected_width: 2,
                expected_height: 2,
                found_width: 3,
                found_height: 2,
            },
            PackStripError::Decode {
                path: PathBuf::from("a.png"),
                message: "boom".to_owned(),
            },
            PackStripError::Write {
                path: PathBuf::from("out.png"),
                message: "boom".to_owned(),
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }

    // -- run: end to end + snippet --

    #[test]
    fn packs_two_frames_writes_a_valid_strip_and_returns_the_snippet() {
        let dir = tempfile::tempdir().unwrap();
        let red = solid(2, 2, [255, 0, 0, 255]);
        let blue = solid(2, 2, [0, 0, 255, 255]);
        let first = write_png(dir.path(), "f0.png", &red);
        let second = write_png(dir.path(), "f1.png", &blue);
        let output = dir.path().join("robot.png");

        let snippet = run(&[first, second], &output, 2).unwrap();

        let bytes = std::fs::read(&output).unwrap();
        let decoded = raster::decode(&bytes).unwrap();
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 2);
        // Row 0 is [red pixel, red pixel, blue pixel, blue pixel] (4 bytes
        // per pixel): the left half (this frame's own 2 columns) is red,
        // the right half is blue.
        assert_eq!(
            decoded.pixels.get(0..4).unwrap(),
            red.pixels.get(0..4).unwrap()
        );
        assert_eq!(
            decoded.pixels.get(8..12).unwrap(),
            blue.pixels.get(0..4).unwrap()
        );

        assert_eq!(
            snippet,
            format!(
                "robot = {{ image = \"{}\", frames = 2, fps = 2 }}",
                output.display()
            )
        );
    }

    #[test]
    fn snippet_falls_back_to_a_default_back_name_when_the_output_has_no_stem() {
        // `Path::file_stem` returns `None` for a path ending in `..`.
        assert_eq!(
            snippet(Path::new(".."), 2, 3),
            "strip = { image = \"..\", frames = 2, fps = 3 }"
        );
    }

    #[test]
    fn snippet_derives_the_back_name_from_the_file_stem_not_the_full_path() {
        let text = snippet(Path::new("backs/robot.png"), 4, 2);
        assert_eq!(
            text,
            "robot = { image = \"backs/robot.png\", frames = 4, fps = 2 }"
        );
    }

    // -- snippet: back name sanitization (an arbitrary file stem is not
    // necessarily a valid TOML bare key or a valid sol_theme::BackName) --

    #[test]
    fn snippet_lowercases_an_uppercase_stem() {
        let text = snippet(Path::new("ROBOT.png"), 2, 3);
        assert_eq!(
            text,
            "robot = { image = \"ROBOT.png\", frames = 2, fps = 3 }"
        );
    }

    #[test]
    fn snippet_replaces_spaces_in_the_stem_with_underscores() {
        let text = snippet(Path::new("card back.png"), 2, 3);
        assert_eq!(
            text,
            "card_back = { image = \"card back.png\", frames = 2, fps = 3 }"
        );
    }

    #[test]
    fn snippet_replaces_dots_in_the_stem_with_underscores() {
        let text = snippet(Path::new("v1.2.png"), 2, 3);
        assert_eq!(text, "v1_2 = { image = \"v1.2.png\", frames = 2, fps = 3 }");
    }

    #[test]
    fn snippet_keeps_a_hyphen_in_the_stem_unchanged() {
        // A hyphen is one of the four allowed characters, alongside
        // lowercase letters, digits, and underscore -- it must survive, not
        // get replaced like a genuinely disallowed character would.
        let text = snippet(Path::new("card-back.png"), 2, 3);
        assert_eq!(
            text,
            "card-back = { image = \"card-back.png\", frames = 2, fps = 3 }"
        );
    }

    /// The smallest valid `theme.toml` shape (mirrors
    /// `sol_theme::manifest`'s own minimal test fixture) with `backs`
    /// substituted verbatim as the `[backs]` section's sole entry — lets a
    /// test prove a snippet is not just syntactically plausible but
    /// actually loads.
    fn manifest_with_backs_section(backs: &str) -> String {
        format!(
            "[theme]\n\
             name = \"Example\"\n\
             render_mode = \"png\"\n\
             \n\
             [cards]\n\
             faces = \"cards/\"\n\
             base_size = [71, 96]\n\
             \n\
             [backs]\n\
             {backs}\n\
             \n\
             [table]\n\
             background = {{ color = \"#008000\" }}\n\
             \n\
             [drag]\n\
             outline_color = \"#000000\"\n"
        )
    }

    #[test]
    fn snippet_from_a_sanitized_stem_parses_and_loads_as_a_back() {
        let text = snippet(Path::new("card back.png"), 4, 2);

        let manifest =
            sol_theme::Manifest::from_toml_str(&manifest_with_backs_section(&text)).unwrap();

        assert_eq!(manifest.backs.len(), 1);
        let (name, _) = manifest.backs.first().unwrap();
        assert_eq!(name.as_str(), "card_back");
    }

    #[test]
    fn snippet_escapes_a_quote_in_the_output_path_and_still_parses_as_valid_toml() {
        // A literal `"` in the output path, left unescaped, would break out
        // of the TOML string and produce a document the strict parser
        // rejects (as opposed to `sanitize_back_name`, which already keeps
        // the back name itself TOML- and BackName-safe).
        let text = snippet(Path::new("robot\"2.png"), 2, 3);

        let manifest =
            sol_theme::Manifest::from_toml_str(&manifest_with_backs_section(&text)).unwrap();
        assert_eq!(manifest.backs.len(), 1);
        let (name, def) = manifest.backs.first().unwrap();
        assert_eq!(name.as_str(), "robot_2");
        assert_eq!(
            *def,
            sol_theme::BackDef::Strip {
                image: asset_path("robot\"2.png"),
                frames: 2,
                fps: 3,
                layout: sol_theme::BackLayout::Horizontal,
            }
        );
    }
}
