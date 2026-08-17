//! [`Fit`]: how a physical playfield surface maps onto logical pixels.
//!
//! The board fills the window: cards scale uniformly to fit the window
//! height and the columns spread to fill the width (the layout's job);
//! for windows narrower than the minimum design aspect the width is
//! filled instead and felt shows below. `scale` is the single
//! logical→physical factor; everything the presenter computes stays in
//! logical pixels and the renderer multiplies by `scale` once.

use sol_theme::CardSize;

use crate::geometry::{Pt, Size};
use crate::layout::Layout;

/// A continuous fit of the board to a physical surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Logical→physical pixel factor; always finite and positive.
    pub scale: f32,
    /// The surface size in logical pixels (`floor(physical / scale)`,
    /// never below the minimum design on the fitted axis).
    pub logical: Size,
}

impl Fit {
    /// Computes the fit of `card`'s board to a `width`×`height` physical
    /// surface (each axis clamped to at least 1).
    ///
    /// The scale is `min(width / minDesignW, height / minDesignH)`,
    /// unclamped: below 1.0 the board shrinks to fit; there is no upper
    /// cap.
    #[must_use]
    pub fn compute(card: CardSize, width: u32, height: u32) -> Self {
        let min = Layout::min_design(card);
        let pw = f64::from(width.max(1));
        let ph = f64::from(height.max(1));
        let by_width = pw / f64::from(min.w.max(1));
        let by_height = ph / f64::from(min.h.max(1));
        let (scale, logical) = if by_width <= by_height {
            // Width-limited: the board spans the width; felt below.
            let h = floor_ratio(ph, by_width).max(1);
            (by_width, Size::new(min.w, h))
        } else {
            // Height-limited: the board spans the height; columns spread.
            let w = floor_ratio(pw, by_height).max(min.w);
            (by_height, Size::new(w, min.h))
        };
        #[allow(clippy::cast_possible_truncation)] // ratios of window sizes are tiny for f32
        Self {
            scale: scale as f32,
            logical,
        }
    }

    /// Maps a physical pixel coordinate into logical pixels (floor
    /// division, correct for negative coordinates during drag capture).
    #[must_use]
    pub fn to_logical(&self, x: i32, y: i32) -> Pt {
        let s = f64::from(self.scale.max(f32::MIN_POSITIVE));
        Pt::new(floor_ratio(f64::from(x), s), floor_ratio(f64::from(y), s))
    }
}

/// `floor(value / scale)` saturated into `i32`.
fn floor_ratio(value: f64, scale: f64) -> i32 {
    let ratio = (value / scale).floor();
    if ratio >= f64::from(i32::MAX) {
        i32::MAX
    } else if ratio <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        #[allow(clippy::cast_possible_truncation)] // range-checked above
        {
            ratio as i32
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    fn card() -> CardSize {
        CardSize {
            width: 71,
            height: 96,
        }
    }

    #[test]
    fn exact_aspect_fits_both_axes() {
        let fit = Fit::compute(card(), 1170, 768);
        assert_eq!(fit.scale, 2.0);
        assert_eq!(fit.logical, Size::new(585, 384));
    }

    #[test]
    fn wide_windows_fill_the_height_and_widen_the_logical_viewport() {
        let fit = Fit::compute(card(), 1600, 768);
        assert_eq!(fit.scale, 2.0);
        assert_eq!(fit.logical, Size::new(800, 384));
    }

    #[test]
    fn narrow_windows_fill_the_width_and_deepen_the_logical_viewport() {
        let fit = Fit::compute(card(), 585, 800);
        assert_eq!(fit.scale, 1.0);
        assert_eq!(fit.logical, Size::new(585, 800));
    }

    #[test]
    fn small_windows_scale_below_one() {
        // 292/585 ≈ 0.499 < 192/384 = 0.5: width-limited.
        let fit = Fit::compute(card(), 292, 192);
        assert!(fit.scale < 0.51, "{}", fit.scale);
        assert!(fit.scale > 0.49, "{}", fit.scale);
        assert_eq!(fit.logical.w, 585, "width-limited: spans the min design");
    }

    #[test]
    fn degenerate_surfaces_clamp_and_stay_finite() {
        let fit = Fit::compute(card(), 0, 0);
        assert!(fit.scale > 0.0);
        assert!(fit.logical.w >= 585);
        assert!(fit.logical.h >= 1);
    }

    #[test]
    fn logical_never_undershoots_the_min_design_on_the_fitted_axis() {
        // Sweep odd height-limited widths (aspect wider than 1170:768)
        // to catch float-rounding undershoot on the spread axis.
        for w in [1171, 1355, 1600, 2000, 3841] {
            let fit = Fit::compute(card(), w, 768);
            assert!(fit.logical.w >= 585, "w={w} -> {:?}", fit.logical);
            assert_eq!(fit.logical.h, 384, "w={w}");
        }
    }

    #[test]
    fn to_logical_saturates_extreme_coordinates() {
        // A 1×1 surface: scale ≈ 1/585, so dividing large physical
        // coordinates overflows i32 and saturates.
        let fit = Fit::compute(card(), 1, 1);
        assert_eq!(
            fit.to_logical(i32::MAX, i32::MAX),
            Pt::new(i32::MAX, i32::MAX)
        );
        assert_eq!(
            fit.to_logical(i32::MIN, i32::MIN),
            Pt::new(i32::MIN, i32::MIN)
        );
    }

    #[test]
    fn to_logical_divides_and_floors_including_negatives() {
        let fit = Fit::compute(card(), 1170, 768); // scale 2.0
        assert_eq!(fit.to_logical(200, 240), Pt::new(100, 120));
        assert_eq!(fit.to_logical(201, 241), Pt::new(100, 120));
        assert_eq!(fit.to_logical(-3, -1), Pt::new(-2, -1));
        let identity = Fit::compute(card(), 585, 384); // scale 1.0
        assert_eq!(identity.to_logical(37, 41), Pt::new(37, 41));
    }
}
