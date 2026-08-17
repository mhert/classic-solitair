//! Header-less DIB (device-independent bitmap) decoder — a thin wrapper over
//! the `image` crate's BMP codec, the byte-parsing heart of `soltool
//! extract`.
//!
//! Windows stores card bitmaps in `RT_BITMAP` resources (NE and PE alike) as
//! a DIB *without* the 14-byte `BITMAPFILEHEADER` that a standalone `.bmp`
//! file carries: the resource bytes begin directly at the DIB header
//! (`biSize`). [`decode_dib`] decodes that resource form via
//! `image::codecs::bmp::BmpDecoder::new_without_file_header`, which the
//! `image` crate documents for exactly this header-less `CF_DIB` shape.
//! [`decode_bmp`] is a thin wrapper for loose `.bmp` files that verifies the
//! `BM` signature, skips the 14-byte file header, and hands the rest to
//! [`decode_dib`].
//!
//! `BITMAPFILEHEADER.bfOffBits` (a stored pointer to the pixel data) is
//! deliberately **ignored**: [`decode_bmp`] always skips exactly 14 bytes
//! rather than calling `BmpDecoder::new` (which would honor the file
//! header's own `bfOffBits`), so a file with a wrong or padded `bfOffBits`
//! still decodes from the DIB header's own arithmetic — one decoder, one
//! offset computation. Real files are unaffected either way.
//!
//! ## Decode matrix (an accepted expansion over the old hand-rolled decoder)
//!
//! Swapping in `image`'s BMP codec accepts whatever that codec supports,
//! which is a strict superset of the original hand-rolled matrix (header
//! sizes 12/40/108/124; bit depths 1/4/8/24/32; `BI_RGB` only). Real Win98
//! card art never leaves that original matrix — it is always ≤8bpp,
//! uncompressed (see [`crate::extract`]) — so everything below is only ever
//! exercised by unusual or hostile input, never by real files:
//!
//! - Header sizes 52/56 (`BITMAPV2INFOHEADER`/`BITMAPV3INFOHEADER`, between
//!   the already-supported 40 and 108) now decode instead of erroring.
//! - `BI_RLE8`/`BI_RLE4` (run-length-compressed 8/4bpp) now decode.
//! - 16bpp now decodes: `BI_RGB` (uncompressed) as the implicit X1R5G5B5
//!   layout, or `BI_BITFIELDS` with explicit channel masks.
//! - 2bpp paletted `BI_RGB` now decodes (previously only 1/4/8bpp were
//!   accepted paletted depths).
//!
//! `BI_JPEG`/`BI_PNG`/CMYK compression and header sizes outside the set
//! above remain errors.
//!
//! **32bpp alpha — pinned, not chosen:** for uncompressed (`BI_RGB`) 32bpp,
//! `image` classifies the pixel type as opaque RGB and always discards the
//! fourth byte, the same "ignored pad, forced opaque" contract the old
//! hand-rolled decoder had. This was verified against the crate's source
//! (`add_alpha_channel` is only ever set by `BI_BITFIELDS`'s own alpha mask,
//! which `BI_RGB` never reads) and is pinned by
//! `pins_32bpp_bi_rgb_alpha_as_forced_opaque_not_surfaced` below. Real card
//! art is never 32bpp, so this is untested by real files either way; a
//! `BI_BITFIELDS` 32bpp DIB with a nonzero alpha mask *would* surface real
//! alpha, but real art never uses that shape either, so it is not exercised
//! here.
//!
//! Every failure — a truncated header, corrupt RLE data, an unsupported
//! compression/bit-depth/header size, or anything else `image` rejects — is
//! wrapped as [`DibError::Decode`], which stringifies the crate's own
//! message instead of re-typing its whole error matrix (mirrors
//! [`crate::pe::PeError`]'s handling of `pelite`).

use std::io::Cursor;

use image::DynamicImage;
use image::codecs::bmp::BmpDecoder;

use crate::raster::RasterImage;

/// Decodes a header-less DIB (resource form: bytes begin at `biSize`) into
/// an RGBA8 [`RasterImage`], via
/// `image::codecs::bmp::BmpDecoder::new_without_file_header`.
///
/// ```
/// use soltool::dib;
///
/// // A 1x1 24bpp BITMAPINFOHEADER DIB whose single pixel is BGR = (blue,
/// // green, red) = (0x30, 0x20, 0x10); a row is padded to 4 bytes.
/// let mut bytes = Vec::new();
/// bytes.extend_from_slice(&40_u32.to_le_bytes()); // biSize
/// bytes.extend_from_slice(&1_i32.to_le_bytes()); // biWidth
/// bytes.extend_from_slice(&1_i32.to_le_bytes()); // biHeight (bottom-up)
/// bytes.extend_from_slice(&1_u16.to_le_bytes()); // biPlanes
/// bytes.extend_from_slice(&24_u16.to_le_bytes()); // biBitCount
/// bytes.extend_from_slice(&0_u32.to_le_bytes()); // biCompression = BI_RGB
/// bytes.extend_from_slice(&[0; 20]); // remaining BITMAPINFOHEADER fields
/// bytes.extend_from_slice(&[0x30, 0x20, 0x10, 0x00]); // pixel + row padding
///
/// let image = dib::decode_dib(&bytes)?;
/// assert_eq!((image.width, image.height), (1, 1));
/// assert_eq!(image.pixels, vec![0x10, 0x20, 0x30, 0xFF]);
/// # Ok::<(), soltool::dib::DibError>(())
/// ```
///
/// # Errors
///
/// Returns [`DibError::Decode`] if `data` is not a decodable DIB: a
/// truncated or malformed header, an unrecognized header size, or an
/// unsupported compression/bit-depth combination. See the module
/// documentation for the full decode matrix.
pub fn decode_dib(data: &[u8]) -> Result<RasterImage, DibError> {
    let decoder =
        BmpDecoder::new_without_file_header(Cursor::new(data)).map_err(|e| decode_error(&e))?;
    let image = DynamicImage::from_decoder(decoder).map_err(|e| decode_error(&e))?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(RasterImage {
        width,
        height,
        pixels: rgba.into_raw(),
    })
}

/// Wraps any `image` crate decode failure as [`DibError::Decode`].
fn decode_error(error: &image::ImageError) -> DibError {
    DibError::Decode {
        message: error.to_string(),
    }
}

/// Decodes a loose `.bmp` file (`BM` signature + 14-byte `BITMAPFILEHEADER`,
/// then a DIB) by stripping the file header and delegating to
/// [`decode_dib`]. `bfOffBits` is ignored — see the module doc.
///
/// # Errors
///
/// Returns [`DibError::NotBmp`] if `bytes` does not begin with `BM` or is too
/// short to hold a `BITMAPFILEHEADER`, otherwise any [`DibError`]
/// [`decode_dib`] returns for the embedded DIB.
pub fn decode_bmp(bytes: &[u8]) -> Result<RasterImage, DibError> {
    let signature = bytes.get(0..2).ok_or(DibError::NotBmp)?;
    if signature != b"BM" {
        return Err(DibError::NotBmp);
    }
    let dib = bytes.get(14..).ok_or(DibError::NotBmp)?;
    decode_dib(dib)
}

/// Every way [`decode_dib`] (or [`decode_bmp`]) can fail to decode a bitmap.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DibError {
    /// The bytes given to [`decode_bmp`] are not a `BM`-signature BMP file.
    #[error("not a BMP file: missing the 'BM' signature")]
    NotBmp,
    /// The `image` crate could not decode the bytes as a DIB/BMP.
    #[error("failed to decode DIB: {message}")]
    Decode {
        /// The underlying `image` crate failure, rendered to text (kept out
        /// of this crate's public API — mirrors [`crate::pe::PeError`]'s
        /// handling of `pelite`).
        message: String,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::cast_possible_truncation)]

    use super::*;

    /// Builds a 40-byte `BITMAPINFOHEADER`.
    fn info_header(
        width: i32,
        height: i32,
        bit_count: u16,
        compression: u32,
        clr_used: u32,
    ) -> Vec<u8> {
        let mut header = Vec::with_capacity(40);
        header.extend_from_slice(&40_u32.to_le_bytes());
        header.extend_from_slice(&width.to_le_bytes());
        header.extend_from_slice(&height.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes()); // biPlanes
        header.extend_from_slice(&bit_count.to_le_bytes());
        header.extend_from_slice(&compression.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes()); // biSizeImage
        header.extend_from_slice(&0_i32.to_le_bytes()); // biXPelsPerMeter
        header.extend_from_slice(&0_i32.to_le_bytes()); // biYPelsPerMeter
        header.extend_from_slice(&clr_used.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes()); // biClrImportant
        header
    }

    /// An `RGBQUAD` palette entry (blue, green, red, reserved).
    fn rgbquad(red: u8, green: u8, blue: u8) -> [u8; 4] {
        [blue, green, red, 0]
    }

    /// `biCompression` value for an uncompressed DIB.
    const BI_RGB: u32 = 0;

    // -- 24bpp / 32bpp direct color, bottom-up geometry --

    #[test]
    fn decodes_a_2x2_24bpp_bottom_up_image_flipping_rows_to_top_down() {
        // Two rows, each 2 px * 3 bytes = 6, padded to stride 8. File row 0
        // is the image's BOTTOM row (bottom-up); output must be top-down.
        let mut bytes = info_header(2, 2, 24, BI_RGB, 0);
        // bottom row: red, green (BGR order in file)
        bytes.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        // top row: blue, white
        bytes.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);

        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        // Output row 0 is the top image row: blue, white.
        assert_eq!(image.pixels.get(0..4).unwrap(), [0, 0, 255, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [255, 255, 255, 255]);
        // Output row 1 is the bottom image row: red, green.
        assert_eq!(image.pixels.get(8..12).unwrap(), [255, 0, 0, 255]);
        assert_eq!(image.pixels.get(12..16).unwrap(), [0, 255, 0, 255]);
    }

    #[test]
    fn decodes_a_top_down_24bpp_image_without_flipping() {
        let mut bytes = info_header(1, -2, 24, BI_RGB, 0);
        bytes.extend_from_slice(&[0, 0, 255, 0]); // file row 0 = top = red
        bytes.extend_from_slice(&[255, 0, 0, 0]); // file row 1 = bottom = blue

        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1, 2));
        assert_eq!(image.pixels.get(0..4).unwrap(), [255, 0, 0, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [0, 0, 255, 255]);
    }

    #[test]
    fn pins_32bpp_bi_rgb_alpha_as_forced_opaque_not_surfaced() {
        // Verify-and-pin (module doc): observed against the `image` crate,
        // not assumed. For uncompressed 32bpp, `image` classifies the pixel
        // type as opaque RGB (`ImageType::RGB32`, never `RGBA32`) whenever
        // the compression is `BI_RGB` — `add_alpha_channel` is only ever
        // set by `BI_BITFIELDS`'s own alpha mask, which `BI_RGB` never
        // reads. A non-0xFF fourth byte therefore stays discarded, exactly
        // like the old hand-rolled decoder's contract.
        let mut bytes = info_header(1, 1, 32, BI_RGB, 0);
        bytes.extend_from_slice(&[0x33, 0x22, 0x11, 0x7F]); // B,G,R, non-0xFF pad
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels, vec![0x11, 0x22, 0x33, 0xFF]);
    }

    // -- paletted depths: 1 / 2 / 4 / 8, clr_used variants --

    #[test]
    fn decodes_8bpp_with_a_default_full_palette_when_clr_used_is_zero() {
        let mut bytes = info_header(2, 1, 8, BI_RGB, 0);
        for index in 0..256_u32 {
            let value = index as u8;
            bytes.extend_from_slice(&rgbquad(value, value, value));
        }
        bytes.extend_from_slice(&[5, 250, 0, 0]); // two indices + row padding
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [5, 5, 5, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [250, 250, 250, 255]);
    }

    #[test]
    fn decodes_8bpp_with_an_explicit_clr_used_count() {
        let mut bytes = info_header(2, 1, 8, BI_RGB, 2);
        bytes.extend_from_slice(&rgbquad(10, 20, 30));
        bytes.extend_from_slice(&rgbquad(40, 50, 60));
        bytes.extend_from_slice(&[0, 1, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [10, 20, 30, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [40, 50, 60, 255]);
    }

    #[test]
    fn an_out_of_range_palette_index_defaults_to_black() {
        // clr_used = 1 but a pixel references index 1: `image` always pads
        // its internal 256-entry palette table with zeros past the
        // declared count, so this still lands on opaque black rather than
        // panicking or indexing out of bounds.
        let mut bytes = info_header(2, 1, 8, BI_RGB, 1);
        bytes.extend_from_slice(&rgbquad(10, 20, 30));
        bytes.extend_from_slice(&[0, 1, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [10, 20, 30, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [0, 0, 0, 255]);
    }

    #[test]
    fn decodes_4bpp_reading_high_then_low_nibble() {
        let mut bytes = info_header(2, 1, 4, BI_RGB, 0);
        for index in 0..16_u32 {
            let value = (index as u8) * 16;
            bytes.extend_from_slice(&rgbquad(value, 0, 0));
        }
        // one byte = two pixels: high nibble 1, low nibble 2, + padding.
        bytes.extend_from_slice(&[0x12, 0, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [16, 0, 0, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [32, 0, 0, 255]);
    }

    #[test]
    fn decodes_1bpp_reading_bits_most_significant_first() {
        let mut bytes = info_header(2, 1, 1, BI_RGB, 0);
        bytes.extend_from_slice(&rgbquad(0, 0, 0)); // index 0 = black
        bytes.extend_from_slice(&rgbquad(255, 255, 255)); // index 1 = white
        bytes.extend_from_slice(&[0b1000_0000, 0, 0, 0]); // px0=1 white, px1=0 black
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [255, 255, 255, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [0, 0, 0, 255]);
    }

    #[test]
    fn decodes_1bpp_pixel_8_from_the_second_byte_not_a_wrapped_first_byte() {
        // Width 9 spans two row bytes. Byte 0 is all zero (px0..7 black);
        // byte 1's top bit is set (px8 white).
        let mut bytes = info_header(9, 1, 1, BI_RGB, 0);
        bytes.extend_from_slice(&rgbquad(0, 0, 0)); // index 0 = black
        bytes.extend_from_slice(&rgbquad(255, 255, 255)); // index 1 = white
        bytes.extend_from_slice(&[0x00, 0b1000_0000, 0, 0]); // row bytes, padded to stride 4
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(32..36).unwrap(), [255, 255, 255, 255]);
    }

    #[test]
    fn decodes_1bpp_bit_extraction_does_not_leak_higher_bits_into_the_palette_index() {
        // Palette index 0 is a distinctive (non-black) color, so a bit
        // extraction bug that leaks higher bits into the palette index
        // becomes visible instead of coincidentally still rendering black.
        let mut bytes = info_header(2, 1, 1, BI_RGB, 0);
        bytes.extend_from_slice(&rgbquad(10, 20, 30)); // index 0 = distinctive
        bytes.extend_from_slice(&rgbquad(255, 255, 255)); // index 1 = white
        bytes.extend_from_slice(&[0b1000_0000, 0, 0, 0]); // px0=1 white, px1=0 index0
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(4..8).unwrap(), [10, 20, 30, 255]);
    }

    #[test]
    fn two_bpp_now_decodes_instead_of_erroring() {
        // Accepted expansion (module doc): `image` supports 2bpp palettes,
        // unlike the old hand-rolled decoder. One MSB-first byte packs four
        // 2-bit indices: 00, 01, 10, 11.
        let mut bytes = info_header(4, 1, 2, BI_RGB, 4);
        bytes.extend_from_slice(&rgbquad(10, 20, 30));
        bytes.extend_from_slice(&rgbquad(40, 50, 60));
        bytes.extend_from_slice(&rgbquad(70, 80, 90));
        bytes.extend_from_slice(&rgbquad(100, 110, 120));
        bytes.extend_from_slice(&[0b00_01_10_11, 0, 0, 0]); // indices 0,1,2,3 + row padding
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels.get(0..4).unwrap(), [10, 20, 30, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [40, 50, 60, 255]);
        assert_eq!(image.pixels.get(8..12).unwrap(), [70, 80, 90, 255]);
        assert_eq!(image.pixels.get(12..16).unwrap(), [100, 110, 120, 255]);
    }

    // -- BITMAPCOREHEADER (12-byte) --

    #[test]
    fn decodes_a_core_header_8bpp_image_with_rgbtriple_palette() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&12_u32.to_le_bytes()); // bcSize
        bytes.extend_from_slice(&2_u16.to_le_bytes()); // bcWidth
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // bcHeight
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // bcPlanes
        bytes.extend_from_slice(&8_u16.to_le_bytes()); // bcBitCount
        for index in 0..256_u32 {
            let value = index as u8;
            bytes.extend_from_slice(&[value, value, value]); // RGBTRIPLE: B,G,R
        }
        bytes.extend_from_slice(&[7, 200, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels.get(0..4).unwrap(), [7, 7, 7, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [200, 200, 200, 255]);
    }

    #[test]
    fn rejects_a_core_header_with_zero_width() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&12_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes()); // width 0
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&24_u16.to_le_bytes());
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn a_truncated_core_header_is_a_decode_error() {
        let bytes = 12_u32.to_le_bytes();
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    // -- V2 / V3 / V4 / V5 headers (52 / 56 / 108 / 124) --

    #[test]
    fn decodes_a_v2_52_byte_header_by_placing_the_palette_after_bisize() {
        // Accepted expansion (module doc): header size 52
        // (BITMAPV2INFOHEADER) now decodes instead of erroring — mirrors
        // the V4/V5 (108/124) case below at the other new size `image`
        // added (56 = V3 shares the same code path as V2).
        let mut bytes = info_header(1, 1, 8, BI_RGB, 1);
        bytes
            .get_mut(0..4)
            .unwrap()
            .copy_from_slice(&52_u32.to_le_bytes());
        bytes.resize(52, 0);
        bytes.extend_from_slice(&rgbquad(1, 2, 3));
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels, vec![1, 2, 3, 255]);
    }

    #[test]
    fn decodes_a_v5_124_byte_header_by_placing_the_palette_after_bisize() {
        let mut bytes = info_header(1, 1, 8, BI_RGB, 1);
        // Patch biSize to 124 and pad the header out to 124 bytes; the
        // palette must then start at offset 124, not 40.
        bytes
            .get_mut(0..4)
            .unwrap()
            .copy_from_slice(&124_u32.to_le_bytes());
        bytes.resize(124, 0);
        bytes.extend_from_slice(&rgbquad(90, 80, 70));
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(image.pixels, vec![90, 80, 70, 255]);
    }

    // -- new compressed/16bpp capabilities: full pixel assertions --

    #[test]
    fn decodes_rle8_compressed_pixels_with_full_assertions() {
        // Accepted expansion (module doc): BI_RLE8 now decodes. A
        // hand-built 4x2 8bpp RLE8 stream, two palette entries (0=red,
        // 1=green). Encoded mode is (count, index): `[4, 0]` repeats index
        // 0 four times. Each row ends with an End-of-Line escape (0, 0).
        let mut bytes = info_header(4, 2, 8, 1 /* BI_RLE8 */, 2);
        bytes.extend_from_slice(&rgbquad(255, 0, 0)); // index 0 = red
        bytes.extend_from_slice(&rgbquad(0, 255, 0)); // index 1 = green
        // File row 0 (bottom, bottom-up): four reds, then End-of-Line.
        bytes.extend_from_slice(&[4, 0, 0, 0]);
        // File row 1 (top): green, green, red, red, then End-of-Line.
        bytes.extend_from_slice(&[2, 1, 2, 0, 0, 0]);

        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        // Output row 0 is the top image row: green, green, red, red.
        assert_eq!(
            image.pixels.get(0..16).unwrap(),
            [
                0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ]
        );
        // Output row 1 is the bottom image row: red x4.
        assert_eq!(
            image.pixels.get(16..32).unwrap(),
            [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ]
        );
    }

    #[test]
    fn decodes_rle4_compressed_pixels_with_full_assertions() {
        // Accepted expansion (module doc): BI_RLE4 now decodes. Encoded
        // mode is (count, nibble-pair-byte): the byte's high nibble is the
        // index used for even output pixels, the low nibble for odd ones,
        // alternating until `count` pixels are written. Palette: 0 = red,
        // 1 = blue.
        let mut bytes = info_header(4, 2, 4, 2 /* BI_RLE4 */, 2);
        bytes.extend_from_slice(&rgbquad(255, 0, 0)); // index 0 = red
        bytes.extend_from_slice(&rgbquad(0, 0, 255)); // index 1 = blue
        // File row 0 (bottom): four reds (nibble pair 0x00 repeated).
        bytes.extend_from_slice(&[4, 0x00, 0, 0]);
        // File row 1 (top): red, blue, red, blue (nibble pair 0x01 repeated).
        bytes.extend_from_slice(&[4, 0x01, 0, 0]);

        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(
            image.pixels.get(0..16).unwrap(),
            [
                255, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0, 255, 0, 0, 255, 255,
            ]
        );
        assert_eq!(
            image.pixels.get(16..32).unwrap(),
            [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ]
        );
    }

    #[test]
    fn decodes_16bpp_bi_rgb_as_implicit_x1r5g5b5() {
        // Accepted expansion (module doc): 16bpp now decodes. Uncompressed
        // (BI_RGB) 16bpp has no explicit channel masks, so `image` uses the
        // implicit X1R5G5B5 layout: bits 0-4 blue, 5-9 green, 10-14 red,
        // bit 15 unused. Each 5-bit channel is expanded to 8-bit by
        // `image`'s own rounding table (not a plain `<< 3`), so the
        // expected bytes below are that crate's own table values.
        let mut bytes = info_header(2, 1, 16, BI_RGB, 0);
        // pixel 0: R5=16 (0b10000), G5=0, B5=0 -> value 0x4000.
        bytes.extend_from_slice(&0x4000_u16.to_le_bytes());
        // pixel 1: R5=0, G5=0, B5=31 (0b11111, max) -> value 0x001F.
        bytes.extend_from_slice(&0x001F_u16.to_le_bytes());

        let image = decode_dib(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels.get(0..4).unwrap(), [132, 0, 0, 255]);
        assert_eq!(image.pixels.get(4..8).unwrap(), [0, 0, 255, 255]);
    }

    // -- error matrix: still-invalid input collapses onto `Decode` --

    #[test]
    fn an_empty_input_is_a_decode_error() {
        assert!(matches!(
            decode_dib(&[]).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn an_unknown_header_size_is_rejected() {
        let bytes = 64_u32.to_le_bytes();
        let mut padded = bytes.to_vec();
        padded.resize(64, 0);
        assert!(matches!(
            decode_dib(&padded).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn bi_bitfields_without_mask_bytes_is_still_an_error() {
        // BI_BITFIELDS compression is itself now decodable given real mask
        // bytes (an accepted expansion — see module doc), but this fixture
        // supplies none after the 40-byte header, so it still fails: now
        // for a truncated-read reason, not a rejected-compression-type one.
        let bytes = info_header(1, 1, 32, 3, 0);
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn a_negative_width_is_invalid() {
        let bytes = info_header(-1, 1, 24, BI_RGB, 0);
        let error = decode_dib(&bytes).unwrap_err();
        assert!(matches!(error, DibError::Decode { .. }));
        // Pin (finding 5, no RED phase — this asserts existing behavior):
        // every other rejection test here only checks the `Decode` variant,
        // which `decode_error` would also produce for an empty `message`,
        // so none of them actually prove `image`'s real failure text
        // reaches `DibError::Decode::message` through `decode_error`'s
        // `error.to_string()` wiring. This one does, via `Display`
        // (`#[error("failed to decode DIB: {message}")]`). Observed full
        // message from `image` 0.25.10 (captured directly, not guessed):
        // "failed to decode DIB: Format error decoding Bmp: Negative width
        // (-1)". Asserting only the substring below, not the whole string:
        // `image`'s exact wording may shift across patch releases, but this
        // is the stable part that proves the real width value made it
        // through.
        assert!(
            error.to_string().contains("Negative width (-1)"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn a_zero_height_is_invalid() {
        let bytes = info_header(1, 0, 24, BI_RGB, 0);
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn a_zero_width_is_invalid() {
        let bytes = info_header(0, 1, 24, BI_RGB, 0);
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn a_truncated_palette_is_reported() {
        // 8bpp default palette needs 256 RGBQUADs (1024 bytes); supply none.
        let bytes = info_header(1, 1, 8, BI_RGB, 0);
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    #[test]
    fn truncated_pixel_data_is_still_an_error() {
        // Valid header + full palette, but no pixel bytes at all.
        let mut bytes = info_header(4, 1, 8, BI_RGB, 2);
        bytes.extend_from_slice(&rgbquad(0, 0, 0));
        bytes.extend_from_slice(&rgbquad(1, 1, 1));
        assert!(matches!(
            decode_dib(&bytes).unwrap_err(),
            DibError::Decode { .. }
        ));
    }

    // -- loose .bmp wrapper --

    #[test]
    fn decode_bmp_strips_the_file_header_and_ignores_bf_off_bits() {
        let dib = {
            let mut bytes = info_header(1, 1, 24, BI_RGB, 0);
            bytes.extend_from_slice(&[0x33, 0x22, 0x11, 0]);
            bytes
        };
        let mut file = Vec::new();
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&0_u32.to_le_bytes()); // bfSize (ignored)
        file.extend_from_slice(&0_u32.to_le_bytes()); // reserved
        file.extend_from_slice(&9999_u32.to_le_bytes()); // bfOffBits: deliberately wrong
        file.extend_from_slice(&dib);
        let image = decode_bmp(&file).unwrap();
        assert_eq!(image.pixels, vec![0x11, 0x22, 0x33, 0xFF]);
    }

    #[test]
    fn decode_bmp_rejects_a_file_without_the_bm_signature() {
        assert!(matches!(
            decode_bmp(b"ZZ....").unwrap_err(),
            DibError::NotBmp
        ));
    }

    #[test]
    fn decode_bmp_rejects_bytes_too_short_for_a_signature() {
        assert!(matches!(decode_bmp(b"B").unwrap_err(), DibError::NotBmp));
    }

    #[test]
    fn decode_bmp_rejects_a_file_truncated_before_the_dib() {
        // Has 'BM' but fewer than 14 bytes: no room for a BITMAPFILEHEADER.
        assert!(matches!(
            decode_bmp(b"BM123").unwrap_err(),
            DibError::NotBmp
        ));
    }

    #[test]
    fn every_error_variant_renders_a_non_empty_message() {
        for error in [
            DibError::NotBmp,
            DibError::Decode {
                message: "boom".to_owned(),
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
