//! Card-back frame animation: which frame a back shows at a given clock
//! reading, and where that frame lives in the back's assets.
//!
//! Purely a function of the presenter clock — the host drives time through
//! `advance(dt)`, never an internal timer. All cards on the table share
//! one phase, like the original's single animation timer.

use sol_theme::{BackLayout, BackTiming};

use crate::geometry::{Rect, Size, saturate};
use crate::profile::BackMeta;

/// The frame a back shows at `clock_ms`, `0..frames`.
///
/// Uniform `fps` timing maps the clock through `clock · fps / 1000` so no
/// drift accumulates; explicit `durations_ms` timing cycles through the
/// per-frame durations.
pub(crate) fn frame_index(meta: &BackMeta, clock_ms: u64) -> u32 {
    if meta.frames == 0 {
        return 0;
    }
    match &meta.timing {
        None => 0,
        Some(BackTiming::Fps(fps)) => {
            let ticks = clock_ms.wrapping_mul(u64::from(*fps)) / 1000;
            u32::try_from(ticks % u64::from(meta.frames)).unwrap_or(0)
        }
        Some(BackTiming::DurationsMs(durations)) => {
            let total: u64 = durations.iter().map(|d| u64::from(*d)).sum();
            if total == 0 {
                return 0;
            }
            let cycle_pos = clock_ms % total;
            // cycle_pos < total = the durations' sum, so the scan always
            // lands in some frame's window.
            durations
                .iter()
                .scan(0_u64, |elapsed, duration| {
                    *elapsed += u64::from(*duration);
                    Some(*elapsed)
                })
                .position(|frame_end| cycle_pos < frame_end)
                .map_or(0, |index| u32::try_from(index).unwrap_or(0))
        }
    }
}

/// Which asset and source rectangle draw frame `frame` of a back.
///
/// Strip backs sample a card-sized slice of their single asset along the
/// strip axis; list-form backs use one whole asset per frame. The source
/// rectangle is in the asset's own unscaled pixels.
pub(crate) fn frame_source(meta: &BackMeta, frame: u32, card_base: Size) -> (usize, Rect) {
    if meta.assets > 1 {
        let asset = usize::try_from(frame).unwrap_or(0).min(meta.assets - 1);
        return (asset, Rect::new(0, 0, card_base.w, card_base.h));
    }
    let along = i64::from(frame);
    let src = match meta.layout {
        BackLayout::Horizontal => Rect::new(
            saturate(along * i64::from(card_base.w)),
            0,
            card_base.w,
            card_base.h,
        ),
        BackLayout::Vertical => Rect::new(
            0,
            saturate(along * i64::from(card_base.h)),
            card_base.w,
            card_base.h,
        ),
    };
    (0, src)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(
        frames: u32,
        timing: Option<BackTiming>,
        layout: BackLayout,
        assets: usize,
    ) -> BackMeta {
        BackMeta {
            frames,
            timing,
            layout,
            assets,
        }
    }

    #[test]
    fn static_backs_hold_frame_zero() {
        let back = meta(1, None, BackLayout::Horizontal, 1);
        assert_eq!(frame_index(&back, 0), 0);
        assert_eq!(frame_index(&back, 123_456), 0);
    }

    #[test]
    fn fps_timing_advances_without_drift() {
        // 2 fps over 4 frames: a frame every 500 ms.
        let back = meta(4, Some(BackTiming::Fps(2)), BackLayout::Horizontal, 1);
        assert_eq!(frame_index(&back, 0), 0);
        assert_eq!(frame_index(&back, 499), 0);
        assert_eq!(frame_index(&back, 500), 1);
        assert_eq!(frame_index(&back, 1999), 3);
        assert_eq!(frame_index(&back, 2000), 0);
        // 3 fps: frame boundaries at 333.33… ms — exact rational timing,
        // frame 1 starts at the 334th millisecond.
        let back = meta(2, Some(BackTiming::Fps(3)), BackLayout::Horizontal, 1);
        assert_eq!(frame_index(&back, 333), 0);
        assert_eq!(frame_index(&back, 334), 1);
        assert_eq!(frame_index(&back, 666), 1);
        assert_eq!(frame_index(&back, 667), 0);
    }

    #[test]
    fn durations_timing_cycles_the_declared_holds() {
        let timing = BackTiming::DurationsMs(vec![250, 750]);
        let back = meta(2, Some(timing), BackLayout::Horizontal, 2);
        assert_eq!(frame_index(&back, 0), 0);
        assert_eq!(frame_index(&back, 249), 0);
        assert_eq!(frame_index(&back, 250), 1);
        assert_eq!(frame_index(&back, 999), 1);
        assert_eq!(frame_index(&back, 1000), 0);
        assert_eq!(frame_index(&back, 1250), 1);
    }

    #[test]
    fn degenerate_timing_is_total() {
        assert_eq!(frame_index(&meta(0, None, BackLayout::Horizontal, 0), 5), 0);
        let zeroed = meta(
            2,
            Some(BackTiming::DurationsMs(Vec::new())),
            BackLayout::Horizontal,
            2,
        );
        assert_eq!(frame_index(&zeroed, 5), 0);
    }

    #[test]
    fn strip_frames_slice_along_the_axis() {
        let card = Size::new(71, 96);
        let horizontal = meta(4, Some(BackTiming::Fps(2)), BackLayout::Horizontal, 1);
        assert_eq!(
            frame_source(&horizontal, 0, card),
            (0, Rect::new(0, 0, 71, 96))
        );
        assert_eq!(
            frame_source(&horizontal, 2, card),
            (0, Rect::new(142, 0, 71, 96))
        );
        let vertical = meta(2, Some(BackTiming::Fps(2)), BackLayout::Vertical, 1);
        assert_eq!(
            frame_source(&vertical, 1, card),
            (0, Rect::new(0, 96, 71, 96))
        );
    }

    #[test]
    fn list_form_frames_use_one_asset_each() {
        let card = Size::new(71, 96);
        let list = meta(2, Some(BackTiming::Fps(2)), BackLayout::Horizontal, 2);
        assert_eq!(frame_source(&list, 0, card), (0, Rect::new(0, 0, 71, 96)));
        assert_eq!(frame_source(&list, 1, card), (1, Rect::new(0, 0, 71, 96)));
        // An out-of-range frame clamps to the last asset.
        assert_eq!(frame_source(&list, 9, card), (1, Rect::new(0, 0, 71, 96)));
    }
}
