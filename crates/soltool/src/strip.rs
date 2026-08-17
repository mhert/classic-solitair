//! Strip frame geometry: join equal frames into one strip, along either
//! layout axis.
//!
//! [`join`] backs `--animate`'s recipe-composed backs
//! ([`crate::animate::compose_strip`]); the horizontal case is exactly
//! [`crate::pack_strip::build_strip`], reused rather than re-derived.

use sol_theme::BackLayout;

use crate::pack_strip;
use crate::raster::RasterImage;

/// Joins equal `frames` into one strip along `layout`'s axis. Horizontal
/// joining is [`pack_strip::build_strip`].
pub(crate) fn join(frames: &[RasterImage], layout: BackLayout) -> RasterImage {
    match layout {
        BackLayout::Horizontal => pack_strip::build_strip(frames),
        BackLayout::Vertical => join_vertical(frames),
    }
}

/// Stacks equal-width frames top to bottom: their row-major buffers simply
/// concatenate into one taller image.
fn join_vertical(frames: &[RasterImage]) -> RasterImage {
    let width = frames.first().map_or(0, |frame| frame.width);
    let height = frames
        .iter()
        .fold(0_u32, |total, frame| total.saturating_add(frame.height));
    let mut pixels = Vec::new();
    for frame in frames {
        pixels.extend_from_slice(&frame.pixels);
    }
    RasterImage {
        width,
        height,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    /// A `width`×`height` image filled with one RGBA color.
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

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn vertical_join_stacks_frames_top_to_bottom() {
        let top = solid(2, 1, RED);
        let bottom = solid(2, 1, BLUE);
        let joined = join(&[top, bottom], BackLayout::Vertical);
        assert_eq!((joined.width, joined.height), (2, 2));
        // Row 0 is the red frame, row 1 the blue frame.
        assert_eq!(
            joined.pixels.get(0..8).unwrap(),
            [255, 0, 0, 255, 255, 0, 0, 255]
        );
        assert_eq!(
            joined.pixels.get(8..16).unwrap(),
            [0, 0, 255, 255, 0, 0, 255, 255]
        );
    }
}
