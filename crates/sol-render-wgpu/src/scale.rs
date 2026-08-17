//! The scaling policy: which content factor the atlas is built at for a
//! requested display scale, per theme `render_mode` and the player's
//! [`CardScaling`] choice.
//!
//! Scaling work happens **only when the scale factor changes** — the
//! renderer compares the planned factor against the atlas it already has
//! and rebuilds nothing when they match. Three rows, by `(render_mode,
//! scaling)`:
//!
//! - **png, original**: the atlas always holds native pixels; integer
//!   scaling to the destination happens in sampling (nearest), so the
//!   factor is 1.
//! - **png, xbrz**: the atlas always holds xBRZ's own fixed ceiling —
//!   the factor does not track the requested scale at all.
//! - **vector**: resvg rasterizes at exactly the requested factor,
//!   regardless of scaling (a vector theme has no PNG art for the
//!   choice to apply to).
//!
//! A png theme's factor therefore never depends on the display scale —
//! a resize never rebuilds its atlas, whichever scaling the player
//! chose — and only a vector theme's atlas ever crosses factors. Any
//! residual difference between the content factor and the display scale
//! (a clamp by the device's texture size limit) is absorbed by GPU
//! sampling: nearest for png at original, linear for png at xbrz and for
//! vector themes.

use sol_theme::{CardScaling, RenderMode};

/// The content factor to rasterize the atlas at for `requested` display
/// scale, before any texture-size clamping.
pub(crate) fn content_factor(mode: RenderMode, scaling: CardScaling, requested: u32) -> u32 {
    match (mode, scaling) {
        (RenderMode::Png, CardScaling::Original) => 1,
        (RenderMode::Png, CardScaling::Xbrz) => u32::from(sol_xbrz::SCALE_FACTOR_MAX),
        (RenderMode::Vector, _) => requested.max(1),
    }
}

/// Whether this combination renders through the pixel-art AA fragment
/// entry point (hard, evenly sized texels with a one-screen-pixel blend at
/// texel seams) rather than plain linear sampling.
pub(crate) const fn pixel_aa(mode: RenderMode, scaling: CardScaling) -> bool {
    matches!((mode, scaling), (RenderMode::Png, CardScaling::Original))
}

/// The integer atlas factor that covers a continuous display scale:
/// `ceil(scale)`, clamped to `1..=16` (a sanity bound far above any real
/// window/design ratio; the per-mode policy and the device's texture
/// limits clamp further).
pub(crate) fn ceil_factor(scale: f32) -> u32 {
    let ceiled = scale.ceil();
    if ceiled <= 1.0 {
        1
    } else if ceiled >= 16.0 {
        16
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1.0 < ceiled < 16.0
        {
            ceiled as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_original_always_stays_native() {
        for requested in [1, 2, 3, 17] {
            assert_eq!(
                content_factor(RenderMode::Png, CardScaling::Original, requested),
                1
            );
        }
    }

    /// xBRZ runs at its own ceiling and nothing else: the factor does not
    /// track the window, which is what keeps a resize from rebuilding a
    /// PNG theme's atlas at all.
    #[test]
    fn png_xbrz_is_always_the_xbrz_ceiling() {
        for requested in [0, 1, 2, 6, 17] {
            assert_eq!(
                content_factor(RenderMode::Png, CardScaling::Xbrz, requested),
                u32::from(sol_xbrz::SCALE_FACTOR_MAX)
            );
        }
    }

    #[test]
    fn vector_rasterizes_at_the_requested_factor_under_either_scaling() {
        for scaling in [CardScaling::Original, CardScaling::Xbrz] {
            assert_eq!(content_factor(RenderMode::Vector, scaling, 1), 1);
            assert_eq!(content_factor(RenderMode::Vector, scaling, 4), 4);
            assert_eq!(content_factor(RenderMode::Vector, scaling, 0), 1);
        }
    }

    #[test]
    fn only_png_original_uses_the_aa_entry_point() {
        assert!(pixel_aa(RenderMode::Png, CardScaling::Original));
        assert!(!pixel_aa(RenderMode::Png, CardScaling::Xbrz));
        assert!(!pixel_aa(RenderMode::Vector, CardScaling::Original));
        assert!(!pixel_aa(RenderMode::Vector, CardScaling::Xbrz));
    }

    #[test]
    fn ceil_factor_covers_the_continuous_scale() {
        assert_eq!(ceil_factor(0.4), 1);
        assert_eq!(ceil_factor(1.0), 1);
        assert_eq!(ceil_factor(1.01), 2);
        assert_eq!(ceil_factor(2.0), 2);
        assert_eq!(ceil_factor(5.5), 6);
        assert_eq!(ceil_factor(40.0), 16, "sanity cap");
    }
}
