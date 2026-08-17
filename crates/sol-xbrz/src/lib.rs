//! Safe wrapper over **xBRZ 1.8**, the pixel-art upscaler, via the pure-Rust
//! `xbrz-rs` port (GPLv3 — which is why this crate alone is `GPL-3.0-only`,
//! unlike the rest of the workspace).
//!
//! The crate's entire public surface is one function, [`scale_rgba`]. Its
//! validation is load-bearing rather than cosmetic: `xbrz-rs` panics on
//! malformed input, so every rejection here is what keeps the workspace's
//! no-panic rule true.
//!
//! One consumer: the renderer's xBRZ pipeline. `soltool` does not link this
//! crate at all.
//!
//! # Pixel format
//!
//! [`scale_rgba`] speaks **`RGBA8`** (PNG byte order `[R, G, B, A]`, row-major,
//! tightly packed) on both sides. The bytes pass through unpacked: this
//! wrapper never assembles them into an integer. `xbrz-rs` reinterprets the
//! byte slice as a slice of its own pixel type — a `#[repr(C)]` struct
//! wrapping `[u8; 4]` — using `align_to`. Because that layout is a byte
//! array rather than a multi-byte integer, channel order is positional, not
//! numeric, so the reinterpretation does not depend on host endianness.
//! Because that type's alignment is 1, every byte offset already satisfies
//! it, which is what makes the reinterpretation total: `align_to` can never
//! hand back a misaligned, non-empty remainder.
//!
//! # Thread safety
//!
//! [`scale_rgba`] is safe to call concurrently from multiple threads with no
//! external synchronization. `xbrz-rs` lazily builds one process-global
//! colour-distance table behind a `Once`, and the table is only ever read
//! afterwards. That first build is a one-time cost (a 64 MiB table, on the
//! order of 100 ms) paid by whichever call reaches it first; every later call
//! is unaffected, so callers on a latency-sensitive path should warm the table
//! with one throwaway call up front. The `large_lut` feature that selects this
//! table is required for fidelity: the smaller default table is faster to
//! build but does not reproduce upstream's output.

/// xBRZ's maximum scale factor (upstream `SCALE_FACTOR_MAX`; `xbrz-rs` accepts up to 6).
/// [`scale_rgba`] accepts factors `2..=SCALE_FACTOR_MAX`.
pub const SCALE_FACTOR_MAX: u8 = 6;

/// Errors from [`scale_rgba`]. Every variant rejects caller input *before*
/// `xbrz-rs` is called, so a returned error always means xBRZ never ran.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum XbrzError {
    /// `factor` was outside the supported `2..=SCALE_FACTOR_MAX` range.
    #[error("scale factor {factor} out of range: xBRZ supports 2..={max}", max = SCALE_FACTOR_MAX)]
    InvalidFactor {
        /// The rejected factor.
        factor: u8,
    },
    /// `width` or `height` was zero.
    #[error("image dimensions must be non-zero, got {width}x{height}")]
    ZeroDimension {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
    },
    /// `src` was not exactly `width * height * 4` bytes (`RGBA8`).
    #[error("source buffer is {actual} bytes, but {width}x{height} RGBA8 needs {expected}")]
    BufferSize {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
        /// The required byte length (`width * height * 4`).
        expected: u64,
        /// The actual `src.len()`.
        actual: usize,
    },
    /// A dimension or buffer-size computation overflowed. The scaled target
    /// must be allocatable, and `xbrz-rs` computes its own buffer sizes with
    /// unchecked arithmetic and asserts on them, so both the source and scaled
    /// dimensions (and the buffer byte lengths) are bounded here before it is
    /// ever called.
    #[error("dimensions {width}x{height} at factor {factor} overflow xBRZ's size limits")]
    Overflow {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
        /// The requested factor.
        factor: u8,
    },
    /// The scaled image's total pixel count is beyond what can be addressed
    /// and allocated. The bound is on the product of the scaled dimensions,
    /// not on either one alone.
    #[error("scaled target of {width}x{height} exceeds the addressable pixel count")]
    TargetTooLarge {
        /// The scaled width that was requested.
        width: u32,
        /// The scaled height that was requested.
        height: u32,
    },
}

/// Upscales a tightly-packed **`RGBA8`** image by an integer `factor` using
/// upstream xBRZ, returning a fresh `RGBA8` buffer of `(width*factor)` by
/// `(height*factor)` pixels.
///
/// `src` must be exactly `width * height` pixels in row-major `[R, G, B, A]`
/// order.
///
/// # Errors
///
/// Returns [`XbrzError::InvalidFactor`] if `factor` is not in
/// `2..=SCALE_FACTOR_MAX`; [`XbrzError::ZeroDimension`] if `width` or `height`
/// is zero; [`XbrzError::BufferSize`] if `src.len() != width * height * 4`; and
/// [`XbrzError::Overflow`] if the source or scaled dimensions, or the buffer
/// byte lengths, exceed the bounds allocation requires.
/// On any error, xBRZ is not invoked.
///
/// # Examples
///
/// ```
/// # use sol_xbrz::scale_rgba;
/// // One opaque pixel (R, G, B, A) upscaled 3x -> a 3x3 RGBA image.
/// let src = [0x11, 0x22, 0x33, 0xFF];
/// let out = scale_rgba(&src, 1, 1, 3)?;
/// assert_eq!(out.len(), 3 * 3 * 4);
/// # Ok::<(), sol_xbrz::XbrzError>(())
/// ```
pub fn scale_rgba(src: &[u8], width: u32, height: u32, factor: u8) -> Result<Vec<u8>, XbrzError> {
    if !(2..=SCALE_FACTOR_MAX).contains(&factor) {
        return Err(XbrzError::InvalidFactor { factor });
    }
    if width == 0 || height == 0 {
        return Err(XbrzError::ZeroDimension { width, height });
    }

    let overflow = || XbrzError::Overflow {
        width,
        height,
        factor,
    };

    // One fully transparent column is prepended before scaling (below), so
    // every bound below is computed on the padded width while the errors keep
    // reporting the dimensions the caller actually asked for.
    let padded_width = width.checked_add(1).ok_or_else(overflow)?;
    i32::try_from(padded_width).map_err(|_| overflow())?;
    i32::try_from(height).map_err(|_| overflow())?;

    let scaled_w = width.checked_mul(u32::from(factor)).ok_or_else(overflow)?;
    let scaled_h = height.checked_mul(u32::from(factor)).ok_or_else(overflow)?;
    let padded_scaled_w = padded_width
        .checked_mul(u32::from(factor))
        .ok_or_else(overflow)?;
    i32::try_from(padded_scaled_w).map_err(|_| overflow())?;
    i32::try_from(scaled_h).map_err(|_| overflow())?;

    // The bound is on the product, not on either dimension alone: 8000x8000 at
    // 6x gives scaled dimensions that each fit i32 comfortably and a product
    // that does not, and a target that large cannot be allocated regardless.
    if i32::try_from(u64::from(padded_scaled_w) * u64::from(scaled_h)).is_err() {
        return Err(XbrzError::TargetTooLarge {
            width: scaled_w,
            height: scaled_h,
        });
    }

    // Source byte length must equal width*height*4. width and height each fit
    // i32 here, so the u64 product cannot overflow.
    let expected = u64::from(width) * u64::from(height) * 4;
    if !usize::try_from(expected).is_ok_and(|want| want == src.len()) {
        return Err(XbrzError::BufferSize {
            width,
            height,
            expected,
            actual: src.len(),
        });
    }

    let src_w = usize::try_from(width).map_err(|_| overflow())?;
    let src_h = usize::try_from(height).map_err(|_| overflow())?;
    let pad_w = usize::try_from(padded_width).map_err(|_| overflow())?;
    let scale = usize::from(factor);

    // xbrz-rs treats everything outside the image as fully transparent, so
    // prepending one transparent column hands the real column 0 exactly the
    // neighbourhood its out-of-bounds reader would have synthesised for it.
    // Making that region explicit, then cropping its expansion back off,
    // sidesteps a defect in xbrz-rs 0.1.0 whose leftmost-column corner
    // blending disagrees with upstream xBRZ 1.8. Output is byte-identical to
    // upstream both with and without that defect present, so this stays
    // correct if it is ever fixed.
    let row_bytes = src_w.checked_mul(4).ok_or_else(overflow)?;
    let padded_len = pad_w
        .checked_mul(src_h)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(overflow)?;
    let mut padded = Vec::with_capacity(padded_len);
    for row in src.chunks_exact(row_bytes) {
        padded.extend_from_slice(&[0, 0, 0, 0]);
        padded.extend_from_slice(row);
    }

    let scaled = xbrz::scale_rgba(&padded, pad_w, src_h, scale);

    let pad_bytes = scale.checked_mul(4).ok_or_else(overflow)?;
    let out_row = row_bytes.checked_mul(scale).ok_or_else(overflow)?;
    let crop_end = pad_bytes.checked_add(out_row).ok_or_else(overflow)?;
    let scaled_row = pad_w
        .checked_mul(scale)
        .and_then(|w| w.checked_mul(4))
        .ok_or_else(overflow)?;
    let out_rows = src_h.checked_mul(scale).ok_or_else(overflow)?;
    let mut out = Vec::with_capacity(out_row.checked_mul(out_rows).ok_or_else(overflow)?);
    for row in scaled.chunks_exact(scaled_row) {
        // Sizes are guaranteed by construction; the fallible form is what
        // keeps this total under the crate's no-indexing rule.
        if let Some(cropped) = row.get(pad_bytes..crop_end) {
            out.extend_from_slice(cropped);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    /// The total target pixel count — not just each dimension — bounds what
    /// can be addressed and allocated. Dimensions that each fit individually
    /// can still overflow their product.
    #[test]
    fn rejects_a_target_whose_total_pixel_count_is_unallocatable() {
        // 8000 x 8000 at 6x: each scaled dimension (48_000) fits i32
        // comfortably, but the product is 2.3e9, past i32::MAX.
        let error = scale_rgba(&[], 8000, 8000, 6).unwrap_err();
        assert!(matches!(
            error,
            XbrzError::TargetTooLarge {
                width: 48_000,
                height: 48_000
            }
        ));
    }

    /// The bound is on the product alone, not on the image's shape: a very
    /// wide, comparatively short target trips it just the same.
    #[test]
    fn the_bound_is_on_the_product_not_the_shape() {
        // Scaled to 200_000 x 40_000: both fit i32, their product is 8e9.
        assert!(matches!(
            scale_rgba(&[], 100_000, 20_000, 2).unwrap_err(),
            XbrzError::TargetTooLarge { .. }
        ));
    }

    /// Opaque colour with four distinct channel values (catches channel swaps).
    const OPAQUE: [u8; 4] = [0x11, 0x22, 0x33, 0xFF];

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut buf = Vec::new();
        for _ in 0..width * height {
            buf.extend_from_slice(&color);
        }
        buf
    }

    fn pixel(buf: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = usize::try_from((y * width + x) * 4).unwrap();
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn checkerboard_4x4() -> Vec<u8> {
        let red = [0xFF, 0x00, 0x00, 0xFF];
        let blue = [0x00, 0x00, 0xFF, 0xFF];
        let mut buf = Vec::new();
        for y in 0..4u32 {
            for x in 0..4u32 {
                buf.extend_from_slice(if (x + y) % 2 == 1 { &red } else { &blue });
            }
        }
        buf
    }

    // Locked output of `scale_rgba(&checkerboard_4x4(), 4, 4, 2)`: an 8x8 RGBA8
    // image (256 bytes). A mismatch means xbrz-rs, its scaler configuration,
    // the large_lut feature, or the transparent-pad-and-crop path above
    // drifted — regenerate DELIBERATELY only after confirming the new bytes
    // are correct. This fixture is also the in-tree regression lock for the
    // left-column defect noted above: its first and last rows carry the
    // alpha-201 corners, and column 0 is exactly where xbrz-rs 0.1.0's
    // leftmost-column corner blending disagrees with upstream xBRZ 1.8 once
    // that padding is absent, so a casual regeneration could silently relock
    // onto the wrong bytes. Note the four corners carry alpha 201 (xBRZ
    // rounds image corners toward the transparent out-of-bounds).
    #[rustfmt::skip]
    const CHECKER_2X_EXPECTED: [u8; 256] = [
        0,0,255,201, 0,0,255,255, 127,0,127,255, 255,0,0,255, 0,0,255,255, 127,0,127,255, 255,0,0,255, 255,0,0,201,
        0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255,
        127,0,127,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 127,0,127,255,
        255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255,
        0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255,
        127,0,127,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 127,0,127,255,
        255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255, 255,0,0,255, 255,0,0,255, 0,0,255,255, 0,0,255,255,
        255,0,0,201, 255,0,0,255, 127,0,127,255, 0,0,255,255, 255,0,0,255, 127,0,127,255, 0,0,255,255, 0,0,255,201,
    ];

    #[test]
    fn solid_color_scales_uniformly() {
        // xBRZ maps a uniform opaque image to one whose RGB equals the source
        // RGB on EVERY pixel; only the four corners lose alpha (blended toward
        // the transparent out-of-bounds region — verified against upstream).
        // We pin dims, RGB-everywhere (R/G/B mapping + xbrz-rs
        // plumbing), and the interior's full colour (A mapping) via the centre.
        let (w, h) = (5u32, 5u32);
        for factor in 2..=SCALE_FACTOR_MAX {
            let src = solid(w, h, OPAQUE);
            let out = scale_rgba(&src, w, h, factor).unwrap();
            let f = u32::from(factor);
            let (tw, th) = (w * f, h * f);
            assert_eq!(
                out.len(),
                usize::try_from(tw * th).unwrap() * 4,
                "dims f{factor}"
            );
            for y in 0..th {
                for x in 0..tw {
                    assert_eq!(
                        pixel(&out, tw, x, y)[0..3],
                        OPAQUE[0..3],
                        "RGB preserved @({x},{y}) f{factor}"
                    );
                }
            }
            assert_eq!(
                pixel(&out, tw, tw / 2, th / 2),
                OPAQUE,
                "interior alpha preserved f{factor}"
            );
        }
    }

    #[test]
    fn fully_transparent_input_stays_transparent() {
        let transparent = [0x22, 0x33, 0x44, 0x00];
        let (w, h) = (4u32, 4u32);
        let out = scale_rgba(&solid(w, h, transparent), w, h, 3).unwrap();
        assert_eq!(out.len(), usize::try_from(w * 3 * h * 3).unwrap() * 4);
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk[3], 0x00, "alpha stays fully transparent");
        }
    }

    #[test]
    fn checkerboard_scale_is_deterministic_and_locked() {
        let src = checkerboard_4x4();
        let first = scale_rgba(&src, 4, 4, 2).unwrap();
        let second = scale_rgba(&src, 4, 4, 2).unwrap();
        assert_eq!(first, second, "same input scales identically");
        assert_eq!(
            first.as_slice(),
            CHECKER_2X_EXPECTED.as_slice(),
            "output drifted from locked fixture"
        );
    }

    #[test]
    fn checkerboard_blends_only_derivable_colors() {
        let out = scale_rgba(&checkerboard_4x4(), 4, 4, 2).unwrap();
        assert_eq!(out.len(), 8 * 8 * 4);
        let pixels: Vec<[u8; 4]> = out
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let red = [0xFF, 0x00, 0x00, 0xFF];
        let blue = [0x00, 0x00, 0xFF, 0xFF];
        assert!(pixels.contains(&red), "red survives");
        assert!(pixels.contains(&blue), "blue survives");
        let mut distinct = pixels.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() >= 2, "at least two colours");
        // Both inputs have G=0; xBRZ blends channelwise, so every output pixel
        // keeps G=0 — i.e. only colours derivable from the inputs appear.
        for p in &pixels {
            assert_eq!(p[1], 0, "green stays 0 (only red/blue blends)");
        }
    }

    #[test]
    fn rejects_factor_below_range() {
        let err = scale_rgba(&[0, 0, 0, 0], 1, 1, 1).unwrap_err();
        assert!(matches!(err, XbrzError::InvalidFactor { factor: 1 }));
    }

    #[test]
    fn rejects_factor_above_range() {
        let err = scale_rgba(&[0, 0, 0, 0], 1, 1, 7).unwrap_err();
        assert!(matches!(err, XbrzError::InvalidFactor { factor: 7 }));
    }

    #[test]
    fn rejects_zero_width() {
        let err = scale_rgba(&[], 0, 4, 2).unwrap_err();
        assert!(matches!(
            err,
            XbrzError::ZeroDimension {
                width: 0,
                height: 4
            }
        ));
    }

    #[test]
    fn rejects_zero_height() {
        let err = scale_rgba(&[], 4, 0, 2).unwrap_err();
        assert!(matches!(
            err,
            XbrzError::ZeroDimension {
                width: 4,
                height: 0
            }
        ));
    }

    #[test]
    fn rejects_wrong_buffer_length() {
        // 2x2 RGBA needs 16 bytes; give 15.
        let err = scale_rgba(&[0u8; 15], 2, 2, 2).unwrap_err();
        assert!(matches!(
            err,
            XbrzError::BufferSize {
                width: 2,
                height: 2,
                expected: 16,
                actual: 15
            }
        ));
    }

    #[test]
    fn rejects_overflowing_source_width() {
        // u32::MAX cannot fit i32 -> Overflow, and it wins over the buffer check.
        let err = scale_rgba(&[0u8; 4], u32::MAX, 1, 2).unwrap_err();
        assert!(matches!(
            err,
            XbrzError::Overflow {
                width,
                height: 1,
                factor: 2
            } if width == u32::MAX
        ));
    }

    #[test]
    fn rejects_overflowing_scaled_width() {
        // Fits i32, but width*factor does not (1.5e9 * 2 > i32::MAX).
        let err = scale_rgba(&[], 1_500_000_000, 1, 2).unwrap_err();
        assert!(matches!(err, XbrzError::Overflow { factor: 2, .. }));
    }

    #[test]
    fn invalid_factor_message_cites_the_supported_range() {
        let message = XbrzError::InvalidFactor { factor: 9 }.to_string();
        assert!(message.contains('9'), "{message}");
        assert!(message.contains("2..=6"), "{message}");
    }

    #[test]
    fn zero_dimension_message_cites_the_offending_dimensions() {
        let message = XbrzError::ZeroDimension {
            width: 0,
            height: 4,
        }
        .to_string();
        assert!(message.contains("0x4"), "{message}");
    }

    #[test]
    fn buffer_size_message_cites_expected_and_actual_lengths() {
        let message = XbrzError::BufferSize {
            width: 2,
            height: 2,
            expected: 16,
            actual: 15,
        }
        .to_string();
        assert!(message.contains("is 15 bytes"), "{message}");
        assert!(message.contains("2x2"), "{message}");
        assert!(message.contains("needs 16"), "{message}");
    }

    #[test]
    fn overflow_message_cites_the_dimensions_and_factor() {
        let message = XbrzError::Overflow {
            width: 1_500_000_000,
            height: 1,
            factor: 2,
        }
        .to_string();
        assert!(message.contains("1500000000x1"), "{message}");
        assert!(message.contains("factor 2"), "{message}");
    }

    #[test]
    fn target_too_large_message_cites_the_scaled_dimensions() {
        let message = XbrzError::TargetTooLarge {
            width: 48_000,
            height: 48_000,
        }
        .to_string();
        assert!(message.contains("48000x48000"), "{message}");
    }

    #[test]
    fn scales_concurrently_without_external_sync() {
        use std::thread;
        let (w, h) = (5u32, 5u32);
        let src = solid(w, h, OPAQUE);
        let expected = scale_rgba(&src, w, h, 4).unwrap();
        let src_a = src.clone();
        let src_b = src;
        let a = thread::spawn(move || scale_rgba(&src_a, w, h, 4));
        let b = thread::spawn(move || scale_rgba(&src_b, w, h, 4));
        assert_eq!(a.join().unwrap().unwrap(), expected);
        assert_eq!(b.join().unwrap().unwrap(), expected);
    }
}
