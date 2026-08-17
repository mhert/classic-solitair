//! PNG dimension probing via the `png` crate's decoder.
//!
//! [`probe`] runs `png::Decoder::read_info`, which reads the signature and
//! every metadata chunk up to (but not including) the first image-data
//! chunk, then reads width/height from the result. It does **not** decode
//! any pixel data — that boundary is deliberate: full decoding is the
//! renderer's job, and theme validation only needs to know presence,
//! format, and declared dimensions.
//!
//! ## Strictness delta (accepted)
//!
//! Replacing the hand-rolled parser with `png::Decoder` makes this probe
//! **stricter**: `read_info` validates the CRC of every chunk it reads
//! (IHDR, and any ancillary chunk that precedes the first image-data
//! chunk), requires IHDR to be the first chunk after the signature,
//! rejects a zero width or height itself, and rejects unrecognized IHDR
//! field values (bit depth, color type, compression/filter/interlace
//! method). The old hand-parser checked none of this beyond the raw byte
//! layout, so a corrupt-but-parseable header that used to probe
//! successfully — most notably one with a mismatched IHDR CRC — is now
//! rejected.
//!
//! One more consequence of that stricter parsing is worth naming: `read_info`
//! itself eagerly reserves a one-row output buffer sized only from the
//! declared **width**, checked against the crate's own ~64 MiB default
//! limit, so an oversized width is already rejected as
//! [`PngProbeError::Invalid`] before this module's own range check ever
//! runs — [`PngProbeError::InvalidDimensions`] for an out-of-range
//! dimension is therefore only reachable via an oversized height, never an
//! oversized width.

use std::io::Cursor;

/// The largest dimension a PNG's IHDR may declare: PNG stores width/height
/// as 4-byte integers restricted to positive `i32` values (the PNG
/// specification: "zero is an invalid value"; the high bit is always
/// clear). The `png` crate
/// itself already rejects a zero width/height while parsing IHDR (see the
/// module doc); this upper bound is sol-theme's own, checked here against
/// the dimensions `png` decodes.
const MAX_DIMENSION: u32 = 0x7FFF_FFFF;

/// Reads `(width, height)` from `bytes` via the `png` crate's decoder. See
/// the module doc for exactly what `read_info` validates before this ever
/// sees a result.
///
/// # Errors
///
/// Returns [`PngProbeError::Invalid`] if the `png` crate rejects `bytes`
/// for any reason — not a PNG, truncated, structurally malformed, a
/// mismatched chunk CRC, and so on — or [`PngProbeError::InvalidDimensions`]
/// if the decoded width or height is 0 or above `2^31 - 1`.
pub(crate) fn probe(bytes: &[u8]) -> Result<(u32, u32), PngProbeError> {
    let reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|source| PngProbeError::Invalid {
            message: source.to_string(),
        })?;
    let width = reader.info().width;
    let height = reader.info().height;
    if (1..=MAX_DIMENSION).contains(&width) && (1..=MAX_DIMENSION).contains(&height) {
        Ok((width, height))
    } else {
        Err(PngProbeError::InvalidDimensions { width, height })
    }
}

/// [`probe`] could not determine PNG dimensions from `bytes`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum PngProbeError {
    /// The `png` crate rejected `bytes` while reading its signature and
    /// leading metadata chunks (see the module doc for what that covers).
    #[error("not a PNG file: {message}")]
    Invalid {
        /// The underlying `png` crate failure, rendered to text (a
        /// foreign error type, kept out of this crate's public API —
        /// mirrors `ManifestError::InvalidToml`'s foreign-error handling).
        message: String,
    },
    /// IHDR declares a width or height above `2^31 - 1` (a zero
    /// width/height never reaches this: the `png` crate rejects that
    /// itself, as [`PngProbeError::Invalid`]).
    #[error(
        "PNG has an invalid declared size {width}x{height}: each dimension must be 1..=2147483647"
    )]
    InvalidDimensions {
        /// The declared width.
        width: u32,
        /// The declared height.
        height: u32,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Builds a real, minimal PNG via `png::Encoder`: `width` x `height`,
    /// 8-bit grayscale, all-zero pixels (their content is never
    /// inspected — only dimensions are). Unlike the old hand-assembled
    /// fixtures this replaces, the result carries a genuine IHDR CRC.
    fn real_png(width: u32, height: u32) -> Vec<u8> {
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

    /// Overwrites `bytes`' IHDR width/height with `width`/`height` and
    /// recomputes IHDR's CRC to match, so the result stays byte-for-byte a
    /// `png::Encoder` output except for the declared dimensions. Needed
    /// for dimension values the encoder itself refuses to write (zero;
    /// above `2^31 - 1`) — mirrors the technique in
    /// `crates/soltool/src/raster.rs`'s interlace-flag fixture, replicated
    /// locally rather than shared across the crates.
    fn with_patched_ihdr_dimensions(mut bytes: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
        bytes
            .get_mut(16..20)
            .unwrap()
            .copy_from_slice(&width.to_be_bytes());
        bytes
            .get_mut(20..24)
            .unwrap()
            .copy_from_slice(&height.to_be_bytes());
        let ihdr_type_and_data = bytes.get(12..29).unwrap().to_vec();
        let crc = crc32(&ihdr_type_and_data).to_be_bytes();
        bytes.get_mut(29..33).unwrap().copy_from_slice(&crc);
        bytes
    }

    /// The standard CRC-32 (ISO-HDLC / zlib / PNG) checksum, computed
    /// bit-by-bit rather than via a lookup table — only ever called on a
    /// handful of bytes in a few tests, so clarity wins over speed.
    /// Needed because [`with_patched_ihdr_dimensions`] hand-edits a chunk
    /// after encoding it: the `png` crate verifies every chunk's CRC by
    /// default, so the edited chunk needs a matching one or `probe` would
    /// reject the file for the wrong reason.
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
    fn a_valid_ihdr_probes_to_its_declared_dimensions() {
        let bytes = real_png(71, 96);
        assert_eq!(probe(&bytes).unwrap(), (71, 96));
    }

    #[test]
    fn a_bad_signature_is_rejected() {
        let mut bytes = real_png(71, 96);
        if let Some(byte) = bytes.get_mut(0) {
            *byte = 0x00;
        }
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn an_empty_input_is_rejected() {
        let error = probe(&[]).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_header_truncated_before_ihdr_is_rejected() {
        let bytes = real_png(71, 96);
        // signature + length + type, no full dimensions
        let truncated = bytes.get(..20).unwrap();
        let error = probe(truncated).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_first_chunk_that_is_not_ihdr_is_rejected() {
        let mut bytes = real_png(71, 96);
        if let Some(chunk_type) = bytes.get_mut(12..16) {
            chunk_type.copy_from_slice(b"IDAT");
        }
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_corrupt_ihdr_crc_is_rejected() {
        // Valid signature+IHDR layout — the exact bytes `png::Encoder`
        // wrote — but the CRC no longer matches after the flip below.
        // This is the strictness this refactor adds over the old
        // hand-parser, which never read chunk CRCs at all (see the
        // now-superseded RED version of this test, which proved the old
        // parser probed these same corrupt-CRC bytes successfully).
        let mut bytes = real_png(71, 96);
        if let Some(byte) = bytes.get_mut(32) {
            *byte ^= 0xFF;
        }
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_zero_width_is_rejected() {
        // The `png` crate itself rejects a zero width while parsing IHDR
        // (see the module doc), so this is `Invalid`, not
        // `InvalidDimensions` — sol-theme's own range check never runs.
        let bytes = with_patched_ihdr_dimensions(real_png(1, 1), 0, 96);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_zero_height_is_rejected() {
        let bytes = with_patched_ihdr_dimensions(real_png(1, 1), 71, 0);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, PngProbeError::Invalid { .. }));
    }

    #[test]
    fn a_dimension_above_the_signed_31_bit_range_is_rejected() {
        // The `png` crate does not enforce PNG's `2^31 - 1` ceiling itself
        // (only that width/height are nonzero) — sol-theme's own range
        // check is what rejects this one, so it stays `InvalidDimensions`,
        // with a real (recomputed) CRC so the CRC check upstream of it
        // isn't what's actually under test here.
        //
        // The oversized value goes on *height*, not width: `read_info`
        // itself eagerly reserves a one-row output buffer sized from the
        // declared width (checked against its own 64 MiB default limit),
        // so an out-of-range width is already rejected as `Invalid`
        // (`LimitsExceeded`) before sol-theme's own check ever runs — see
        // the module doc's strictness delta. Height carries no such
        // eager, width-only reservation, so it reaches this crate's own
        // range check unobstructed.
        let bytes = with_patched_ihdr_dimensions(real_png(1, 1), 71, 0x8000_0000);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(
            error,
            PngProbeError::InvalidDimensions {
                width: 71,
                height: 0x8000_0000
            }
        ));
    }

    #[test]
    fn error_messages_are_human_readable() {
        assert!(
            !PngProbeError::Invalid {
                message: "x".to_owned()
            }
            .to_string()
            .is_empty()
        );
        assert!(
            !PngProbeError::InvalidDimensions {
                width: 0,
                height: 5
            }
            .to_string()
            .is_empty()
        );
    }
}
