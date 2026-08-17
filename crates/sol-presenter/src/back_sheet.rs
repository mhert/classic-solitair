//! [`BackSheet`]: every card back's every frame packed into one contact
//! sheet, for the Options dialog's card-back picker to draw once and cut
//! apart into per-back, per-frame thumbnails.
//!
//! A grid of live thumbnails needs two things this crate already owns:
//! where each thumbnail sits, and which frame an animated back is
//! currently showing. [`BackSheet::build`] (through
//! [`crate::Presenter::back_sheet`]) answers the first; the second is
//! [`crate::Presenter::back_frame`], the very clock law the board itself
//! draws by, so a thumbnail and the card on the table are never a frame
//! apart.

use crate::backs;
use crate::display::{DisplayList, Rgba, TextureId};
use crate::geometry::{Rect, Size, index_to_i32, saturate};
use crate::profile::BackMeta;

/// The gap, in logical pixels, between neighbouring sheet cells.
///
/// A renderer places a sprite's quad through a float transform, so a
/// cell's outer edge can round onto its neighbour's first column of
/// pixels. The gutter guarantees a whole column of background between
/// cells so the caller — the frontend that draws the sheet once and cuts
/// it apart on the CPU — never slices a sliver of the wrong thumbnail.
pub const GUTTER: i32 = 2;

/// One thumbnail's place in a [`BackSheet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetCell {
    /// Index into the theme's `[backs]`, declaration order.
    pub back: usize,
    /// Which frame of that back this cell shows.
    pub frame: u32,
    /// Where the cell sits in the sheet, in logical pixels.
    pub rect: Rect,
}

/// A contact sheet of every back's every frame: draw [`BackSheet::list`]
/// once, then cut it apart at each cell's [`SheetCell::rect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackSheet {
    /// The whole sheet's logical size.
    pub size: Size,
    /// One cell's logical size: the theme's base (1×) card size.
    pub cell: Size,
    /// Every cell, in `(back, frame)` order.
    pub cells: Vec<SheetCell>,
    /// The display list that draws the sheet.
    pub list: DisplayList,
}

impl BackSheet {
    /// Builds the contact sheet for `backs`: one cell per `(back,
    /// frame)` pair in declaration order, frames ascending — a back with
    /// zero frames contributes none. Every cell is `cell` logical
    /// pixels, packed left to right and wrapped to a new row whenever
    /// the next cell would not fit within `max_side`, [`GUTTER`] pixels
    /// from its neighbours on both axes and from nothing else: no gutter
    /// before the first cell, after the last, or around the sheet. The
    /// sheet is exactly as wide as its fullest row and as tall as its
    /// rows stacked. `list` clears to `background` and draws one
    /// untinted sprite per cell, in cell order, sampling
    /// [`TextureId::Back`] with that frame's source rectangle — the same
    /// rectangle [`backs::frame_source`] gives the board.
    ///
    /// `pub(crate)` rather than public: `backs` is `&[BackMeta]`, and
    /// `BackMeta` is crate-private, so a public function could not take
    /// it. [`crate::Presenter::back_sheet`] is the public entry point,
    /// over the theme profile and base card size the presenter already
    /// holds.
    ///
    /// `None` when no sheet exists: `cell` has no extent on either axis
    /// or does not fit within `max_side` on either axis, a full row of
    /// cells does not fit `max_side` vertically, or there are no cells
    /// to place at all.
    pub(crate) fn build(
        backs: &[BackMeta],
        cell: Size,
        max_side: u32,
        background: Rgba,
    ) -> Option<Self> {
        let (size, cells) = pack(backs, cell, max_side)?;
        let mut list = DisplayList {
            clear: Some(background),
            sprites: Vec::new(),
        };
        for sheet_cell in &cells {
            // Every cell's back index was read from this very `backs`
            // slice by `pack`, so the lookup always succeeds.
            if let Some(meta) = backs.get(sheet_cell.back) {
                let (asset, src) = backs::frame_source(meta, sheet_cell.frame, cell);
                list.push(
                    TextureId::Back {
                        back: sheet_cell.back,
                        asset,
                    },
                    src,
                    sheet_cell.rect,
                    Rgba::WHITE,
                );
            }
        }
        Some(Self {
            size,
            cell,
            cells,
            list,
        })
    }
}

/// Computes every `(back, frame)` cell's rectangle and the sheet's
/// overall size, packed and wrapped as [`BackSheet::build`] documents.
/// `None` under the same conditions.
fn pack(backs: &[BackMeta], cell: Size, max_side: u32) -> Option<(Size, Vec<SheetCell>)> {
    let order: Vec<(usize, u32)> = backs
        .iter()
        .enumerate()
        .flat_map(|(back, meta)| (0..meta.frames).map(move |frame| (back, frame)))
        .collect();
    if order.is_empty() {
        return None;
    }

    let max_side = i64::from(max_side);
    let (w, h) = (i64::from(cell.w), i64::from(cell.h));
    // A cell with no extent on either axis has no sheet: nothing could
    // be seen in it. Rejecting it here also keeps `stride_w` below
    // strictly positive — at `cell.w == -GUTTER` it would otherwise be
    // zero, and dividing by it would panic.
    if w <= 0 || h <= 0 {
        return None;
    }
    if w > max_side || h > max_side {
        return None;
    }

    // `max_side - w` is non-negative (checked above), so this is always
    // at least 1: a cell that fits `max_side` always fits its own row.
    let stride_w = w + i64::from(GUTTER);
    let columns = usize::try_from(1 + (max_side - w) / stride_w).unwrap_or(usize::MAX);
    let row_len = columns.min(order.len());
    let rows = order.len().div_ceil(columns);

    // A row's (or the sheet's) extent is n cells at `stride` apart minus
    // the one trailing gutter that does not exist, so `n·stride − GUTTER`
    // throughout rather than `n·cell + (n−1)·GUTTER`.
    let stride_h = h + i64::from(GUTTER);
    let sheet_h = i64::from(index_to_i32(rows)) * stride_h - i64::from(GUTTER);
    if sheet_h > max_side {
        return None;
    }
    let sheet_w = i64::from(index_to_i32(row_len)) * stride_w - i64::from(GUTTER);

    let cells = order
        .into_iter()
        .enumerate()
        .map(|(index, (back, frame))| SheetCell {
            back,
            frame,
            rect: Rect::new(
                saturate(i64::from(index_to_i32(index % columns)) * stride_w),
                saturate(i64::from(index_to_i32(index / columns)) * stride_h),
                cell.w,
                cell.h,
            ),
        })
        .collect();

    Some((Size::new(saturate(sheet_w), saturate(sheet_h)), cells))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_theme::{BackLayout, BackTiming};

    use super::*;
    use crate::profile::ThemeProfile;
    use crate::testkit::test_theme;

    /// The fixture theme's base card size — every cell's own size too.
    fn cell_size() -> Size {
        Size::new(71, 96)
    }

    /// The fixture theme's four backs (`plain`, `strip`, `steps`,
    /// `tall`), extracted the same way the presenter itself does.
    fn backs() -> Vec<BackMeta> {
        ThemeProfile::from_theme(&test_theme()).backs
    }

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
    fn cells_are_ordered_and_sized_and_a_generous_sheet_is_one_row() {
        let sheet = BackSheet::build(&backs(), cell_size(), 1000, Rgba::opaque(1, 2, 3)).unwrap();
        assert_eq!(sheet.cell, cell_size());
        let order: Vec<(usize, u32)> = sheet.cells.iter().map(|c| (c.back, c.frame)).collect();
        assert_eq!(
            order,
            vec![(0, 0), (1, 0), (1, 1), (2, 0), (2, 1), (3, 0), (3, 1)]
        );
        assert_eq!(sheet.size, Size::new(7 * 73 - 2, 96));
        assert_eq!(sheet.list.clear, Some(Rgba::opaque(1, 2, 3)));
        assert_eq!(sheet.list.sprites.len(), 7);
    }

    #[test]
    fn each_cell_draws_its_own_frame_from_its_own_asset() {
        let sheet = BackSheet::build(&backs(), cell_size(), 1000, Rgba::WHITE).unwrap();
        assert!(sheet.list.sprites.iter().all(|s| s.tint == Rgba::WHITE));

        // (2, 1): the list-form `steps` back's second frame — the whole
        // second asset, at the fifth cell.
        let steps = &sheet.list.sprites[4];
        assert_eq!(steps.texture, TextureId::Back { back: 2, asset: 1 });
        assert_eq!(steps.src, Rect::new(0, 0, 71, 96));
        assert_eq!(steps.dst, Rect::new(4 * 73, 0, 71, 96));

        // (1, 1): the horizontal `strip` back's second frame — one card
        // to the right within its single asset, at the third cell.
        let strip = &sheet.list.sprites[2];
        assert_eq!(strip.texture, TextureId::Back { back: 1, asset: 0 });
        assert_eq!(strip.src, Rect::new(71, 0, 71, 96));
        assert_eq!(strip.dst, Rect::new(2 * 73, 0, 71, 96));

        // (3, 1): the vertical `tall` back's second frame — one card
        // down within its single asset, at the seventh (last) cell.
        let tall = &sheet.list.sprites[6];
        assert_eq!(tall.texture, TextureId::Back { back: 3, asset: 0 });
        assert_eq!(tall.src, Rect::new(0, 96, 71, 96));
        assert_eq!(tall.dst, Rect::new(6 * 73, 0, 71, 96));
    }

    #[test]
    fn a_narrow_limit_wraps_the_sheet_to_five_columns() {
        let sheet = BackSheet::build(&backs(), cell_size(), 400, Rgba::WHITE).unwrap();
        assert_eq!(sheet.size, Size::new(5 * 73 - 2, 2 * 98 - 2));
        assert_eq!(sheet.cells[5].rect, Rect::new(0, 98, 71, 96));
        assert_eq!(sheet.cells[6].rect, Rect::new(73, 98, 71, 96));
    }

    #[test]
    fn limits_too_small_on_either_axis_refuse_the_sheet() {
        // Narrower than one cell.
        assert!(BackSheet::build(&backs(), cell_size(), 60, Rgba::WHITE).is_none());
        // Two columns, four rows: 4·98 − 2 = 390 taller than the limit.
        assert!(BackSheet::build(&backs(), cell_size(), 200, Rgba::WHITE).is_none());
    }

    /// The fixture card is taller than it is wide, so on its own every
    /// limit that starves the width also starves the height — the two
    /// halves of "wider or taller than `max_side`" are never
    /// independently exercised. A wide-short cell separates them: a
    /// limit between the two axes starves width alone.
    #[test]
    fn a_wide_cell_can_be_refused_by_width_alone() {
        let wide = Size::new(100, 50);
        let single = vec![meta(1, None, BackLayout::Horizontal, 1)];
        assert!(BackSheet::build(&single, wide, 80, Rgba::WHITE).is_none());
    }

    /// A cell exactly `max_side` on the constraining axis fits — "wider
    /// or taller than" is a strict inequality, not "at least as", on
    /// both the per-cell check and the packed row's own vertical fit.
    #[test]
    fn a_cell_exactly_at_the_limit_on_either_axis_is_accepted() {
        let single = vec![meta(1, None, BackLayout::Horizontal, 1)];
        // Width exactly at the limit (wide cell: isolates the width
        // check the way `cell_size()` alone cannot).
        let wide = Size::new(100, 50);
        let sheet = BackSheet::build(&single, wide, 100, Rgba::WHITE).unwrap();
        assert_eq!(sheet.size, wide);
        // Height exactly at the limit — also the packed row's own
        // vertical extent, since a single cell's row height is the
        // cell's own height.
        let sheet = BackSheet::build(&single, cell_size(), 96, Rgba::WHITE).unwrap();
        assert_eq!(sheet.size, cell_size());
    }

    /// A cell with no extent on an axis has no sheet — and the negative
    /// case is load-bearing beyond taste: at exactly `-GUTTER` the
    /// column stride would be zero and the column count would divide by
    /// it. No theme produces such a card size today, so this guard is
    /// what keeps that from being a panic waiting for one that does.
    #[test]
    fn a_cell_with_no_extent_on_either_axis_is_refused() {
        let single = vec![meta(1, None, BackLayout::Horizontal, 1)];
        let refused = |cell| BackSheet::build(&single, cell, 1000, Rgba::WHITE).is_none();
        assert!(refused(Size::new(0, 96)), "no width");
        assert!(refused(Size::new(71, 0)), "no height");
        assert!(refused(Size::new(-GUTTER, 96)), "a zero column stride");
        assert!(refused(Size::new(71, -GUTTER)));
    }

    #[test]
    fn a_zero_frame_back_contributes_no_cell_and_an_all_zero_sheet_is_none() {
        let mixed = vec![
            meta(0, None, BackLayout::Horizontal, 1),
            meta(1, None, BackLayout::Horizontal, 1),
        ];
        let sheet = BackSheet::build(&mixed, cell_size(), 1000, Rgba::WHITE).unwrap();
        assert_eq!(sheet.size, cell_size());
        assert_eq!(
            sheet.cells,
            vec![SheetCell {
                back: 1,
                frame: 0,
                rect: Rect::new(0, 0, 71, 96),
            }]
        );

        let all_zero = vec![meta(0, None, BackLayout::Horizontal, 1)];
        assert!(BackSheet::build(&all_zero, cell_size(), 1000, Rgba::WHITE).is_none());
        assert!(BackSheet::build(&[], cell_size(), 1000, Rgba::WHITE).is_none());
    }
}
