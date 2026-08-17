//! [`RasterImage`]: soltool's one place that touches PNG pixels.
//!
//! [`decode`] turns PNG bytes into a uniform RGBA8 buffer — normalizing
//! 8-bit grayscale, indexed, RGB, and RGBA (with or without alpha) alike —
//! and [`encode`] turns one back into PNG bytes. `extract` and `pack-strip`
//! both build on this module rather than touching the `png` crate directly,
//! so every subcommand that reads pixels normalizes them the same way.
//!
//! 16-bit depth and interlaced PNGs are rejected with typed errors:
//! extraction and packing only ever produce plain 8-bit, non-interlaced
//! PNGs, so a source image outside that shape signals a
//! problem worth surfacing rather than silently downsampling.

use std::io::Cursor;

/// A decoded (or to-be-encoded) raster image: width, height, and RGBA8
/// pixels, row-major, 4 bytes (red, green, blue, alpha) per pixel.
///
/// `pixels.len()` is always `width as usize * height as usize * 4` for any
/// `RasterImage` this module produces via [`decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 pixel bytes, row-major, 4 bytes per pixel.
    pub pixels: Vec<u8>,
}

/// Decodes `bytes` as a PNG, normalizing its pixels to RGBA8 regardless of
/// the source color type.
///
/// ```
/// use soltool::raster::{self, RasterImage};
///
/// let original = RasterImage {
///     width: 2,
///     height: 1,
///     pixels: vec![255, 0, 0, 255, 0, 255, 0, 128],
/// };
/// let png_bytes = raster::encode(&original)?;
/// let decoded = raster::decode(&png_bytes)?;
/// assert_eq!(decoded, original);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Returns [`RasterDecodeError::UnsupportedBitDepth16`] if the PNG has
/// 16-bit sample depth, [`RasterDecodeError::Interlaced`] if it uses Adam7
/// interlacing, or [`RasterDecodeError::Invalid`] if `bytes` is not a
/// well-formed PNG at all.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, RasterDecodeError> {
    let invalid = |source: png::DecodingError| RasterDecodeError::Invalid {
        message: source.to_string(),
    };

    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Only EXPAND (palette -> RGB(A), <8-bit grayscale -> 8-bit, tRNS ->
    // alpha): deliberately not `Transformations::STRIP_16`, which would
    // silently downsample a 16-bit source instead of letting it fail the
    // `bit_depth` check below.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(invalid)?;

    if reader.info().bit_depth == png::BitDepth::Sixteen {
        return Err(RasterDecodeError::UnsupportedBitDepth16);
    }
    if reader.info().interlaced {
        return Err(RasterDecodeError::Interlaced);
    }

    // Sized to hold exactly one full (non-animated) frame; `buffer_size()`
    // on the `OutputInfo` this decodes into always matches `buffer.len()`
    // exactly for the single still images this module ever decodes, so the
    // whole buffer is the frame's pixels with nothing further to trim.
    let buffer_len = reader
        .output_buffer_size()
        .ok_or(RasterDecodeError::Invalid {
            message: "PNG dimensions are too large to decode".to_owned(),
        })?;
    let mut buffer = vec![0_u8; buffer_len];
    let frame = reader.next_frame(&mut buffer).map_err(invalid)?;

    Ok(RasterImage {
        width: frame.width,
        height: frame.height,
        pixels: to_rgba8(&buffer, frame.color_type),
    })
}

/// Expands already-8-bit `data` (in `color_type`'s native channel layout)
/// to RGBA8.
///
/// `color_type` is always one of `Grayscale`, `GrayscaleAlpha`, `Rgb`, or
/// `Rgba` in practice: [`decode`] always requests
/// `Transformations::EXPAND`, which the `png` crate guarantees expands
/// `Indexed` source images into `Rgb` or `Rgba` before this ever runs. The
/// classification below is written as boolean predicates rather than a
/// match on `png::ColorType` so it stays total either way, with no
/// unreachable arm to carry.
fn to_rgba8(data: &[u8], color_type: png::ColorType) -> Vec<u8> {
    let is_color = matches!(color_type, png::ColorType::Rgb | png::ColorType::Rgba);
    let has_alpha = matches!(
        color_type,
        png::ColorType::GrayscaleAlpha | png::ColorType::Rgba
    );
    let samples = color_type.samples();

    let mut pixels = Vec::with_capacity(data.len() / samples * 4);
    for sample in data.chunks_exact(samples) {
        let (r, g, b) = if is_color {
            (
                sample.first().copied().unwrap_or(0),
                sample.get(1).copied().unwrap_or(0),
                sample.get(2).copied().unwrap_or(0),
            )
        } else {
            let gray = sample.first().copied().unwrap_or(0);
            (gray, gray, gray)
        };
        let alpha = if has_alpha {
            sample.get(samples - 1).copied().unwrap_or(0xFF)
        } else {
            0xFF
        };
        pixels.extend_from_slice(&[r, g, b, alpha]);
    }
    pixels
}

/// Encodes `image`'s RGBA8 pixels as PNG bytes (8-bit depth, non-interlaced).
///
/// ```
/// use soltool::raster::{self, RasterImage};
///
/// let image = RasterImage {
///     width: 1,
///     height: 1,
///     pixels: vec![0x00, 0x80, 0x00, 0xFF],
/// };
/// let bytes = raster::encode(&image)?;
/// assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
/// # Ok::<(), soltool::raster::RasterEncodeError>(())
/// ```
///
/// # Errors
///
/// Returns [`RasterEncodeError`] if `image.pixels` does not hold exactly
/// `image.width * image.height * 4` bytes, or `image.width`/`image.height`
/// is zero.
pub fn encode(image: &RasterImage) -> Result<Vec<u8>, RasterEncodeError> {
    let fail = |source: png::EncodingError| RasterEncodeError {
        message: source.to_string(),
    };

    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&image.pixels).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(bytes)
}

/// `image` with the twelve pixels the original's `cdtDrawExt` never paints
/// made fully transparent, in each of its `frames` equal-width frames.
///
/// The original does not carry a card mask. Instead `cdtDrawExt` reads twelve
/// destination pixels with `GetPixel` before blitting a card and writes them
/// back with `SetPixel` afterwards, so the card's own bitmap never reaches
/// them — three pixels at each corner, which is what rounds a card against
/// the table and against the card below it in a stack. Every card-sized image
/// the original draws goes through that same path. Baking the cutout into
/// straight alpha here reproduces it for a renderer that composites rather
/// than round-trips destination pixels.
///
/// In frame-local coordinates of a `w` x `h` frame, the cleared pixels are
/// `(0,0) (1,0) (0,1)`, `(w-1,0) (w-2,0) (w-1,1)`, `(w-1,h-1) (w-1,h-2)
/// (w-2,h-1)` and `(0,h-1) (1,h-1) (0,h-2)`.
///
/// A frame narrower than two pixels, an image shorter than two pixels, and a
/// `frames` of zero all leave the image exactly as it was: there is no corner
/// to round off. Pixels outside the buffer are skipped, so the result always
/// has exactly the input's length.
///
/// ```
/// use soltool::raster::{self, RasterImage};
///
/// // One 2x2 frame: every pixel is a corner pixel, so all four clear.
/// let image = RasterImage {
///     width: 2,
///     height: 2,
///     pixels: vec![9; 16],
/// };
/// assert_eq!(raster::cut_card_corners(&image, 1).pixels, vec![0; 16]);
/// ```
#[must_use]
pub fn cut_card_corners(image: &RasterImage, frames: u32) -> RasterImage {
    let mut cut = image.clone();
    let Some(frame_width) = image.width.checked_div(frames) else {
        return cut;
    };
    if frame_width < 2 || image.height < 2 {
        return cut;
    }

    let right = frame_width - 1;
    let bottom = image.height - 1;
    for frame in 0..frames {
        let left = frame.saturating_mul(frame_width);
        for (x, y) in [
            (0, 0),
            (1, 0),
            (0, 1),
            (right, 0),
            (right - 1, 0),
            (right, 1),
            (right, bottom),
            (right, bottom - 1),
            (right - 1, bottom),
            (0, bottom),
            (1, bottom),
            (0, bottom - 1),
        ] {
            clear_pixel(&mut cut, left.saturating_add(x), y);
        }
    }
    cut
}

/// Makes the pixel at (`x`, `y`) of `image` fully transparent with its color
/// bytes zeroed, so a cleared pixel never carries stale color. A coordinate
/// whose pixel falls outside the buffer is a no-op.
fn clear_pixel(image: &mut RasterImage, x: u32, y: u32) {
    let start = (y as usize)
        .saturating_mul(image.width as usize)
        .saturating_add(x as usize)
        .saturating_mul(4);
    if let Some(pixel) = image.pixels.get_mut(start..start.saturating_add(4)) {
        pixel.copy_from_slice(&[0, 0, 0, 0]);
    }
}

/// [`decode`] could not produce a [`RasterImage`] from its input bytes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RasterDecodeError {
    /// `bytes` is not a well-formed PNG.
    #[error("failed to decode PNG: {message}")]
    Invalid {
        /// The underlying `png` crate failure, rendered to text (a
        /// foreign error type, kept out of this crate's public API —
        /// mirrors `sol_theme::ManifestError::InvalidToml`).
        message: String,
    },
    /// The PNG has 16-bit sample depth.
    #[error("PNG has 16-bit depth: only 8-bit PNGs are supported")]
    UnsupportedBitDepth16,
    /// The PNG uses Adam7 interlacing.
    #[error("PNG is interlaced: only non-interlaced PNGs are supported")]
    Interlaced,
}

/// [`encode`] could not produce PNG bytes from a [`RasterImage`].
#[derive(Debug, thiserror::Error)]
#[error("failed to encode PNG: {message}")]
pub struct RasterEncodeError {
    /// The underlying `png` crate failure, rendered to text (see
    /// [`RasterDecodeError::Invalid`]).
    message: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn image(width: u32, height: u32, pixels: Vec<u8>) -> RasterImage {
        RasterImage {
            width,
            height,
            pixels,
        }
    }

    // -- cut_card_corners --

    /// An opaque red image of `frames` frames, each `width` x `height`.
    fn opaque(frames: u32, width: u32, height: u32) -> RasterImage {
        let count = (frames * width * height) as usize;
        image(
            frames * width,
            height,
            [0xFF, 0x00, 0x00, 0xFF].repeat(count),
        )
    }

    /// The pixel coordinates a cut image reports as fully transparent.
    fn cleared(image: &RasterImage) -> Vec<(u32, u32)> {
        image
            .pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| *pixel == [0, 0, 0, 0])
            .map(|(index, _)| {
                let index = u32::try_from(index).unwrap();
                (index % image.width, index / image.width)
            })
            .collect()
    }

    #[test]
    fn cutting_clears_the_twelve_pixels_the_original_never_paints() {
        let cut = cut_card_corners(&opaque(1, 5, 7), 1);
        assert_eq!(
            cleared(&cut),
            vec![
                (0, 0),
                (1, 0),
                (3, 0),
                (4, 0), // top edge
                (0, 1),
                (4, 1), // second row
                (0, 5),
                (4, 5), // second-last row
                (0, 6),
                (1, 6),
                (3, 6),
                (4, 6), // bottom edge
            ]
        );
        assert_eq!((cut.width, cut.height), (5, 7));
    }

    #[test]
    fn cutting_clears_every_frame_of_a_strip() {
        let cut = cut_card_corners(&opaque(2, 5, 7), 2);
        // The same twelve per frame, the second frame offset by one width.
        assert_eq!(cleared(&cut).len(), 24);
        for (x, y) in [(0, 0), (4, 0), (0, 6), (4, 6)] {
            assert!(cleared(&cut).contains(&(x + 5, y)), "frame 2 ({x}, {y})");
        }
        // The seam between the frames keeps its ink: frame 1's right edge and
        // frame 2's left edge are cut, the columns beside them are not.
        assert!(!cleared(&cut).contains(&(2, 0)));
        assert!(!cleared(&cut).contains(&(7, 0)));
    }

    #[test]
    fn a_two_by_two_frame_is_cut_away_entirely() {
        // The smallest frame the cutout still applies to: all four pixels are
        // corner pixels.
        let cut = cut_card_corners(&opaque(1, 2, 2), 1);
        assert_eq!(cleared(&cut), vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn a_frame_narrower_than_two_pixels_is_left_alone() {
        let source = opaque(1, 1, 7);
        assert_eq!(cut_card_corners(&source, 1), source);
    }

    #[test]
    fn an_image_shorter_than_two_pixels_is_left_alone() {
        let source = opaque(1, 5, 1);
        assert_eq!(cut_card_corners(&source, 1), source);
    }

    #[test]
    fn zero_frames_leaves_the_image_alone() {
        let source = opaque(1, 5, 7);
        assert_eq!(cut_card_corners(&source, 0), source);
    }

    #[test]
    fn cutting_a_buffer_short_of_its_declared_size_keeps_its_bytes() {
        // A 5x7 image whose buffer holds one pixel: the eleven out-of-buffer
        // corners are skipped rather than panicking or growing the buffer.
        let source = image(5, 7, vec![0xFF, 0x00, 0x00, 0xFF]);
        let cut = cut_card_corners(&source, 1);
        assert_eq!(cut.pixels, vec![0, 0, 0, 0]);
    }

    // -- round trip: the normalization table, gray/indexed/RGB/RGBA in, RGBA8 out --

    #[test]
    fn round_trips_rgba_with_partial_alpha() {
        let original = image(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]);
        let bytes = encode(&original).unwrap();
        assert_eq!(decode(&bytes).unwrap(), original);
    }

    #[test]
    fn a_grayscale_png_decodes_to_opaque_rgba() {
        let bytes = raw_png(
            1,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &[0x80],
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.pixels, vec![0x80, 0x80, 0x80, 0xFF]);
    }

    #[test]
    fn a_grayscale_alpha_png_decodes_replicating_gray_into_rgb() {
        let bytes = raw_png(
            1,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[0x40, 0x11],
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.pixels, vec![0x40, 0x40, 0x40, 0x11]);
    }

    #[test]
    fn an_rgb_png_decodes_to_opaque_rgba() {
        let bytes = raw_png(
            1,
            1,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[0x10, 0x20, 0x30],
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.pixels, vec![0x10, 0x20, 0x30, 0xFF]);
    }

    #[test]
    fn an_rgba_png_decodes_unchanged() {
        let bytes = raw_png(
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0x10, 0x20, 0x30, 0x40],
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.pixels, vec![0x10, 0x20, 0x30, 0x40]);
    }

    #[test]
    fn an_indexed_png_decodes_expanded_to_rgb() {
        // A 1x1 8-bit indexed PNG whose sole palette entry is (10, 20, 30);
        // the pixel data is the single index byte 0.
        let mut encoder_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoder_bytes, 1, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![10, 20, 30]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
        }
        let decoded = decode(&encoder_bytes).unwrap();
        assert_eq!(decoded.pixels, vec![10, 20, 30, 0xFF]);
    }

    #[test]
    fn an_indexed_png_with_trns_decodes_expanded_to_rgba_with_the_palette_alpha() {
        // Same 1x1 8-bit indexed PNG as above, but its sole palette entry
        // (10, 20, 30) also gets a tRNS alpha of 0x80 — this module's doc
        // claims tRNS expansion, via `Transformations::EXPAND`, turns that
        // into a real per-pixel alpha channel (indexed decodes to `Rgba`,
        // not `Rgb`, whenever a tRNS chunk is present).
        let mut encoder_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoder_bytes, 1, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![10, 20, 30]);
            encoder.set_trns(vec![0x80]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
        }
        let decoded = decode(&encoder_bytes).unwrap();
        assert_eq!(decoded.pixels, vec![10, 20, 30, 0x80]);
    }

    #[test]
    fn a_1_bit_grayscale_png_decodes_expanded_to_black_and_white_rgba() {
        // 2 pixels, 1 bit each, packed MSB-first into a single byte: the
        // first pixel is 1 (white), the second is 0 (black). This module's
        // doc claims sub-8-bit grayscale expansion, via
        // `Transformations::EXPAND`, scales each 1-bit sample to a full
        // 0x00/0xFF byte rather than leaving it as a bare 0/1 value.
        let bytes = raw_png(
            2,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::One,
            &[0b1000_0000],
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(
            decoded.pixels,
            vec![
                0xFF, 0xFF, 0xFF, 0xFF, // white
                0x00, 0x00, 0x00, 0xFF, // black
            ]
        );
    }

    // -- rejections: exact variants --

    #[test]
    fn a_16_bit_png_is_rejected_with_the_exact_variant() {
        let pixel = [0, 0x10, 0, 0x20, 0, 0x30, 0, 0x40];
        let bytes = raw_png(1, 1, png::ColorType::Rgba, png::BitDepth::Sixteen, &pixel);
        let error = decode(&bytes).unwrap_err();
        assert!(matches!(error, RasterDecodeError::UnsupportedBitDepth16));
    }

    #[test]
    fn an_interlaced_png_is_rejected_with_the_exact_variant() {
        // The `png` crate's encoder has no public "write interlaced" option
        // in this version, so the fixture is built by patching a normally
        // encoded file: flip IHDR's interlace-method byte (the 13th of
        // IHDR's 13 data bytes, at offset 28: 8-byte signature + 4-byte
        // length + 4-byte "IHDR" type + 12 preceding data bytes) from 0 to
        // 1, then recompute IHDR's own CRC (its trailing 4 bytes) to match
        // — the CRC covers only its own chunk, so the untouched IDAT chunk
        // stays valid. `decode` rejects on the IHDR flag before it ever
        // reads pixel data, so the IDAT payload not actually being
        // Adam7-interlaced does not matter. `raw_png(2, 2, ..)` always
        // produces more than the 33 bytes these offsets touch, so the
        // `.unwrap()`s below (allowed in this module, see the file-level
        // `#![allow(clippy::unwrap_used)]`) never fire in practice; using
        // them instead of an `if let Some(..) = ..` guard also keeps the
        // untaken-`None`-arm branch out of this crate's own coverage (it
        // becomes `Option::unwrap`'s, in `core`, not ours to exercise).
        let mut bytes = raw_png(2, 2, png::ColorType::Rgba, png::BitDepth::Eight, &[0; 16]);
        *bytes.get_mut(28).unwrap() = 1;
        let ihdr_type_and_data = bytes.get(12..29).unwrap().to_vec();
        let crc = crc32(&ihdr_type_and_data).to_be_bytes();
        bytes.get_mut(29..33).unwrap().copy_from_slice(&crc);

        let error = decode(&bytes).unwrap_err();
        assert!(matches!(error, RasterDecodeError::Interlaced));
    }

    /// The standard CRC-32 (ISO-HDLC / zlib / PNG) checksum, computed
    /// bit-by-bit rather than via a lookup table — this is only ever
    /// called on a handful of bytes in one test, so clarity wins over
    /// speed. Needed because the fixture above hand-edits a chunk after
    /// encoding it: the `png` crate verifies every chunk's CRC by default,
    /// so the edited chunk needs a matching one or `decode` would reject
    /// the file before ever reaching the check under test.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let masked_polynomial = 0_u32.wrapping_sub(crc & 1) & 0xEDB8_8320;
                crc = (crc >> 1) ^ masked_polynomial;
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    #[test]
    fn invalid_bytes_are_rejected_as_invalid() {
        let error = decode(b"not a png").unwrap_err();
        assert!(matches!(error, RasterDecodeError::Invalid { .. }));
    }

    #[test]
    fn decode_error_messages_are_human_readable() {
        let error = decode(b"not a png").unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(
            !RasterDecodeError::UnsupportedBitDepth16
                .to_string()
                .is_empty()
        );
        assert!(!RasterDecodeError::Interlaced.to_string().is_empty());
    }

    // -- encode failures --

    #[test]
    fn encode_rejects_a_pixel_buffer_of_the_wrong_length() {
        let error = encode(&image(2, 2, vec![0; 3])).unwrap_err();
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn encode_rejects_zero_width() {
        let error = encode(&image(0, 1, vec![])).unwrap_err();
        assert!(!error.to_string().is_empty());
    }

    /// Builds raw (non-interlaced, single-IDAT) PNG bytes for a color type
    /// and bit depth this module's own `encode` cannot produce (16-bit),
    /// or wants to test decoding of directly rather than round-tripping
    /// through `encode` (grayscale, grayscale+alpha, RGB).
    fn raw_png(
        width: u32,
        height: u32,
        color_type: png::ColorType,
        bit_depth: png::BitDepth,
        pixel_bytes: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(color_type);
            encoder.set_depth(bit_depth);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixel_bytes).unwrap();
        }
        bytes
    }
}
