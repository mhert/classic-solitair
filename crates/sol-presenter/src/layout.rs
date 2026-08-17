//! [`Layout`]: the Win98 playfield geometry in logical pixels.
//!
//! Every constant here reproduces the layout arithmetic of the original
//! `sol.exe` (verified by disassembly of a Win95-era build), generalized
//! from its 71×96 card to any theme `base_size`. With the original card
//! size at the design width the numbers come out exactly as the
//! original's design window: stock at (11, 5), waste at (93, 5),
//! foundations at x = 257, 339, 421, 503, tableau columns at x = 11 +
//! 82·i, tableau row at y = 107.
//!
//! The original spread its columns proportionally when the window grew;
//! this layout does the same, fed the logical client width. The
//! continuous window fit lives elsewhere ([`crate::fit::Fit`]): the
//! renderer stretches these logical coordinates by one f32 scale, so
//! the layout itself stays pure truncating integer arithmetic.

use sol_engine::{FOUNDATION_COUNT, PileId, TABLEAU_COUNT};
use sol_theme::CardSize;

use crate::geometry::{Pt, Rect, Size, index_to_i32, saturate};

/// Stock and waste stacks shift by (2, 1) once per this many cards — the
/// original's "thick pile" edge effect.
const STOCK_THICKNESS_DIVISOR: usize = 10;

/// Foundation stacks shift by (2, 1) once per this many cards.
const FOUNDATION_THICKNESS_DIVISOR: usize = 4;

/// The Win98 playfield geometry for one card size and logical client
/// width.
///
/// All coordinates are in logical pixels (the theme's `base_size`
/// space). The layout is anchored at the playfield's top-left; the
/// design size ([`Layout::design_size`]) is the original's startup
/// client area, `7·cardW + 8·xUnitMin` wide and `4·cardH` tall — the
/// minimum the column spread bottoms out at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    card: Size,
    x_unit: i32,
    y_top: i32,
    tableau_y: i32,
    face_up_step: i32,
    face_down_step: i32,
    waste_fan_step: Pt,
    design: Size,
}

impl Layout {
    /// Computes the layout for a theme card size in a logical client
    /// `client_width` pixels wide.
    ///
    /// The formulas, from the original (all division truncating):
    ///
    /// - column unit `xUnit = max((clientW − 7·cardW)/8, cardW/8 + 3)` —
    ///   the original's proportional column spread, bottoming out at the
    ///   design spacing; every horizontal position (stock, waste,
    ///   foundations, tableau) derives from it, so the whole board
    ///   redistributes with the width;
    /// - top row `yTop = (cardH·5 + 50)/100` — `MulDiv(cardH, 5, 100)`,
    ///   rounding to nearest;
    /// - tableau row `tableauY = yTop + cardH + 6`;
    /// - tableau fan steps `4·cardH/25` face-up and `cardH/25` face-down;
    /// - waste fan step `(cardW/5, 1)` per fanned card.
    #[must_use]
    pub fn new(card: CardSize, client_width: i32) -> Self {
        let w = i64::from(card.width);
        let h = i64::from(card.height);
        let x_unit_min = w / 8 + 3;
        let spread = (i64::from(client_width) - 7 * w) / 8;
        let y_top = (h * 5 + 50) / 100;
        Self {
            card: Size::new(saturate(w), saturate(h)),
            x_unit: saturate(spread.max(x_unit_min)),
            y_top: saturate(y_top),
            tableau_y: saturate(y_top + h + 6),
            face_up_step: saturate(4 * h / 25),
            face_down_step: saturate(h / 25),
            waste_fan_step: Pt::new(saturate(w / 5), 1),
            design: Size::new(saturate(7 * w + 8 * x_unit_min), saturate(4 * h)),
        }
    }

    /// The card size in logical pixels.
    #[must_use]
    pub const fn card(&self) -> Size {
        self.card
    }

    /// The card size in the asset's own (unscaled) pixels — the space
    /// display-list source rectangles are expressed in. Identical to
    /// [`Layout::card`]; kept as a separate accessor because source and
    /// destination rectangles mean different things.
    #[must_use]
    pub const fn card_base(&self) -> Size {
        self.card
    }

    /// The playfield design size in logical pixels: the original's
    /// startup client area, independent of the current column spread.
    #[must_use]
    pub const fn design_size(&self) -> Size {
        self.design
    }

    /// The minimum board size in logical pixels: the original's startup
    /// client area, `7·cardW + 8·xUnitMin` wide and `4·cardH` tall —
    /// the size the proportional column spread bottoms out at.
    #[must_use]
    pub fn min_design(card: CardSize) -> Size {
        let w = i64::from(card.width);
        let h = i64::from(card.height);
        let x_unit = w / 8 + 3;
        Size::new(saturate(7 * w + 8 * x_unit), saturate(4 * h))
    }

    /// The vertical fan step below a face-up tableau card.
    #[must_use]
    pub const fn face_up_step(&self) -> i32 {
        self.face_up_step
    }

    /// The vertical fan step below a face-down tableau card.
    #[must_use]
    pub const fn face_down_step(&self) -> i32 {
        self.face_down_step
    }

    /// The (x, y) step between fanned waste cards (Draw Three).
    #[must_use]
    pub const fn waste_fan_step(&self) -> Pt {
        self.waste_fan_step
    }

    /// The top-left of `pile`'s base card slot, or `None` for an
    /// out-of-range pile index.
    #[must_use]
    pub fn pile_origin(&self, pile: PileId) -> Option<Pt> {
        let column = |i: i64| {
            saturate(i64::from(self.x_unit) + i * (i64::from(self.x_unit) + i64::from(self.card.w)))
        };
        match pile {
            PileId::Stock => Some(Pt::new(column(0), self.y_top)),
            PileId::Waste => Some(Pt::new(
                saturate(2 * i64::from(self.x_unit) + i64::from(self.card.w)),
                self.y_top,
            )),
            PileId::Foundation(index) if index < FOUNDATION_COUNT => {
                Some(Pt::new(column(3 + i64::from(index)), self.y_top))
            }
            PileId::Tableau(index) if index < TABLEAU_COUNT => {
                Some(Pt::new(column(i64::from(index)), self.tableau_y))
            }
            PileId::Foundation(_) | PileId::Tableau(_) => None,
        }
    }

    /// `pile`'s full hit rectangle — the original's per-pile target rect,
    /// used for empty-pile hit-testing and empty-pile drop targeting — or
    /// `None` for an out-of-range pile index.
    ///
    /// The original's paddings: stock `cardW+10 × cardH+5`, waste
    /// `7·cardW/5+4 × cardH+5`, foundation `cardW+6 × cardH+5`, tableau
    /// exactly `cardW` wide and `6·(2·upStep + downStep) + cardH` tall
    /// (room for the deepest possible fan).
    #[must_use]
    pub fn pile_rect(&self, pile: PileId) -> Option<Rect> {
        let origin = self.pile_origin(pile)?;
        // The original pads the stock's hit rect ten pixels past the card so
        // a click just outside still deals. That constant assumes the
        // original's 71-pixel card and the column gap it produces; the gap
        // scales with the card, so at smaller sizes an unclamped pad reaches
        // into the waste's slot — which hit-testing, scanning the stock
        // first and returning, would win.
        let stock_pad = i64::from(self.x_unit).saturating_sub(1).min(10);
        let (w, h) = match pile {
            PileId::Stock => (
                i64::from(self.card.w) + stock_pad,
                i64::from(self.card.h) + 5,
            ),
            PileId::Waste => (
                7 * i64::from(self.card.w) / 5 + 4,
                i64::from(self.card.h) + 5,
            ),
            PileId::Foundation(_) => (i64::from(self.card.w) + 6, i64::from(self.card.h) + 5),
            PileId::Tableau(_) => (
                i64::from(self.card.w),
                6 * (2 * i64::from(self.face_up_step) + i64::from(self.face_down_step))
                    + i64::from(self.card.h),
            ),
        };
        Some(Rect::new(origin.x, origin.y, saturate(w), saturate(h)))
    }

    /// The (x, y) thickness offset of card `index` in a stack that shifts
    /// by (2, 1) once per `divisor` cards.
    fn thickness_offset(index: usize, divisor: usize) -> Pt {
        let steps = index_to_i32(index / divisor);
        Pt::new(steps.saturating_mul(2), steps)
    }

    /// The top-left of stock card `index` (0 = bottom).
    #[must_use]
    pub fn stock_card_pos(&self, index: usize) -> Pt {
        let offset = Self::thickness_offset(index, STOCK_THICKNESS_DIVISOR);
        // Origin exists for every non-indexed pile; fall back to it defensively.
        let origin = self.pile_origin(PileId::Stock).unwrap_or_default();
        origin.translated(offset.x, offset.y)
    }

    /// The top-left of foundation `foundation`'s card `index` (0 = the
    /// ace), or `None` for an out-of-range foundation.
    #[must_use]
    pub fn foundation_card_pos(&self, foundation: u8, index: usize) -> Option<Pt> {
        let origin = self.pile_origin(PileId::Foundation(foundation))?;
        let offset = Self::thickness_offset(index, FOUNDATION_THICKNESS_DIVISOR);
        Some(origin.translated(offset.x, offset.y))
    }

    /// The top-lefts of all `total` waste cards (index 0 = bottom), the
    /// top `fanned` of them fanned Draw-Three style.
    ///
    /// Faithful to the original: the flat portion stacks with the same
    /// thickness offsets as the stock; the fan starts **on** the flat
    /// top's position (a new draw first collapses the previous fan, and
    /// its first card lands exactly where the old top lay) and each
    /// further fanned card steps by [`Layout::waste_fan_step`]. Cards
    /// played off the fan do not re-slide the remainder.
    #[must_use]
    pub fn waste_positions(&self, total: usize, fanned: usize) -> Vec<Pt> {
        let origin = self.pile_origin(PileId::Waste).unwrap_or_default();
        let fanned = fanned.min(total);
        let flat = total - fanned;
        let fan_base = Self::thickness_offset(flat.saturating_sub(1), STOCK_THICKNESS_DIVISOR);
        (0..total)
            .map(|index| {
                if index < flat {
                    let offset = Self::thickness_offset(index, STOCK_THICKNESS_DIVISOR);
                    origin.translated(offset.x, offset.y)
                } else {
                    let along = index_to_i32(index - flat);
                    origin.translated(
                        fan_base
                            .x
                            .saturating_add(self.waste_fan_step.x.saturating_mul(along)),
                        fan_base
                            .y
                            .saturating_add(self.waste_fan_step.y.saturating_mul(along)),
                    )
                }
            })
            .collect()
    }

    /// The top-left of tableau `column`'s card `index` (0 = bottom) when
    /// the pile's bottom `face_down` cards lie face-down, or `None` for an
    /// out-of-range column.
    ///
    /// Each face-down card advances the fan by the face-down step, each
    /// face-up card by the face-up step.
    #[must_use]
    pub fn tableau_card_pos(&self, column: u8, face_down: usize, index: usize) -> Option<Pt> {
        let origin = self.pile_origin(PileId::Tableau(column))?;
        let down_cards = index.min(face_down);
        let up_cards = index - down_cards;
        let y = i64::from(self.face_down_step) * i64::from(index_to_i32(down_cards))
            + i64::from(self.face_up_step) * i64::from(index_to_i32(up_cards));
        Some(origin.translated(0, saturate(y)))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn win98_card() -> CardSize {
        CardSize {
            width: 71,
            height: 96,
        }
    }

    fn win98_layout() -> Layout {
        Layout::new(win98_card(), 585)
    }

    #[test]
    fn original_card_size_reproduces_the_win98_design_layout() {
        let layout = win98_layout();
        assert_eq!(layout.card(), Size::new(71, 96));
        assert_eq!(layout.design_size(), Size::new(585, 384));
        assert_eq!(layout.pile_origin(PileId::Stock), Some(Pt::new(11, 5)));
        assert_eq!(layout.pile_origin(PileId::Waste), Some(Pt::new(93, 5)));
        for (index, x) in [257, 339, 421, 503].into_iter().enumerate() {
            let pile = PileId::Foundation(u8::try_from(index).unwrap());
            assert_eq!(layout.pile_origin(pile), Some(Pt::new(x, 5)));
        }
        for column in 0..TABLEAU_COUNT {
            let expected = Pt::new(11 + 82 * i32::from(column), 107);
            assert_eq!(layout.pile_origin(PileId::Tableau(column)), Some(expected));
        }
    }

    #[test]
    fn fan_steps_match_the_original() {
        let layout = win98_layout();
        assert_eq!(layout.face_up_step(), 15);
        assert_eq!(layout.face_down_step(), 3);
        assert_eq!(layout.waste_fan_step(), Pt::new(14, 1));
    }

    #[test]
    fn narrow_clients_clamp_to_the_design_layout() {
        // The spread bottoms out at the design spacing: any width at or
        // below the minimum produces the identical layout.
        assert_eq!(win98_layout(), Layout::new(win98_card(), 400));
        assert_eq!(win98_layout(), Layout::new(win98_card(), 0));
        assert_eq!(win98_layout(), Layout::new(win98_card(), i32::MIN));
    }

    #[test]
    fn wider_clients_spread_the_columns_with_the_original_formula() {
        // xUnit = max((800 − 7·71)/8, 71/8 + 3) = max(37, 11) = 37.
        let layout = Layout::new(win98_card(), 800);
        assert_eq!(layout.pile_origin(PileId::Stock), Some(Pt::new(37, 5)));
        assert_eq!(layout.pile_origin(PileId::Waste), Some(Pt::new(145, 5)));
        for (index, x) in [361, 469, 577, 685].into_iter().enumerate() {
            let pile = PileId::Foundation(u8::try_from(index).unwrap());
            assert_eq!(layout.pile_origin(pile), Some(Pt::new(x, 5)));
        }
        for column in 0..TABLEAU_COUNT {
            let expected = Pt::new(37 + 108 * i32::from(column), 107);
            assert_eq!(layout.pile_origin(PileId::Tableau(column)), Some(expected));
        }
        // Vertical geometry, card size, and the design box are untouched
        // by the spread.
        assert_eq!(layout.card(), Size::new(71, 96));
        assert_eq!(layout.face_up_step(), 15);
        assert_eq!(layout.design_size(), Size::new(585, 384));
    }

    #[test]
    fn the_spread_truncates_like_the_original() {
        // (807 − 497)/8 truncates to 38: remainder felt stays at the right.
        let layout = Layout::new(win98_card(), 807);
        assert_eq!(layout.pile_origin(PileId::Stock), Some(Pt::new(38, 5)));
    }

    #[test]
    fn the_spread_saturates_absurd_widths() {
        let layout = Layout::new(win98_card(), i32::MAX);
        assert_eq!(
            layout.pile_origin(PileId::Tableau(6)).map(|pt| pt.y),
            Some(107)
        );
        assert!(layout.pile_origin(PileId::Tableau(6)).unwrap().x > 0);
    }

    #[test]
    fn min_design_is_the_original_startup_client() {
        assert_eq!(Layout::min_design(win98_card()), Size::new(585, 384));
        let square = CardSize {
            width: 100,
            height: 100,
        };
        assert_eq!(Layout::min_design(square), Size::new(820, 400));
        let absurd = CardSize {
            width: u32::MAX,
            height: u32::MAX,
        };
        assert_eq!(Layout::min_design(absurd), Size::new(i32::MAX, i32::MAX));
    }

    #[test]
    fn out_of_range_piles_have_no_geometry() {
        let layout = win98_layout();
        assert_eq!(
            layout.pile_origin(PileId::Foundation(FOUNDATION_COUNT)),
            None
        );
        assert_eq!(layout.pile_origin(PileId::Tableau(TABLEAU_COUNT)), None);
        assert_eq!(layout.pile_rect(PileId::Foundation(FOUNDATION_COUNT)), None);
        assert_eq!(layout.tableau_card_pos(TABLEAU_COUNT, 0, 0), None);
        assert_eq!(layout.foundation_card_pos(FOUNDATION_COUNT, 0), None);
    }

    #[test]
    fn pile_rects_match_the_original_paddings() {
        let layout = win98_layout();
        assert_eq!(
            layout.pile_rect(PileId::Stock),
            Some(Rect::new(11, 5, 81, 101))
        );
        assert_eq!(
            layout.pile_rect(PileId::Waste),
            Some(Rect::new(93, 5, 103, 101))
        );
        assert_eq!(
            layout.pile_rect(PileId::Foundation(0)),
            Some(Rect::new(257, 5, 77, 101))
        );
        assert_eq!(
            layout.pile_rect(PileId::Tableau(0)),
            Some(Rect::new(11, 107, 71, 294))
        );
    }

    #[test]
    fn stock_and_foundation_stacks_thicken_stepwise() {
        let layout = win98_layout();
        assert_eq!(layout.stock_card_pos(0), Pt::new(11, 5));
        assert_eq!(layout.stock_card_pos(9), Pt::new(11, 5));
        assert_eq!(layout.stock_card_pos(10), Pt::new(13, 6));
        assert_eq!(layout.stock_card_pos(23), Pt::new(15, 7));
        assert_eq!(layout.foundation_card_pos(1, 0), Some(Pt::new(339, 5)));
        assert_eq!(layout.foundation_card_pos(1, 3), Some(Pt::new(339, 5)));
        assert_eq!(layout.foundation_card_pos(1, 4), Some(Pt::new(341, 6)));
        assert_eq!(layout.foundation_card_pos(1, 12), Some(Pt::new(345, 8)));
    }

    #[test]
    fn waste_positions_fan_the_top_cards_from_the_flat_top() {
        let layout = win98_layout();
        // Twelve cards, top three fanned: nine flat, fan based on card 8's
        // slot (still in the first thickness decade).
        let positions = layout.waste_positions(12, 3);
        assert_eq!(positions.len(), 12);
        assert_eq!(positions[0], Pt::new(93, 5));
        assert_eq!(positions[8], Pt::new(93, 5));
        assert_eq!(positions[9], Pt::new(93, 5));
        assert_eq!(positions[10], Pt::new(107, 6));
        assert_eq!(positions[11], Pt::new(121, 7));
        // Exactly ten flat cards: the flat top (card 9) still sits at the
        // origin — the decade step belongs to card 10, which here is the
        // first fan card and lands on the flat top instead.
        let positions = layout.waste_positions(13, 3);
        assert_eq!(positions[9], Pt::new(93, 5));
        assert_eq!(positions[10], Pt::new(93, 5));
        assert_eq!(positions[11], Pt::new(107, 6));
        // Eleven flat cards: the flat top has stepped once; the fan tracks it.
        let positions = layout.waste_positions(14, 3);
        assert_eq!(positions[10], Pt::new(95, 6));
        assert_eq!(positions[11], Pt::new(95, 6));
        assert_eq!(positions[12], Pt::new(109, 7));
        assert_eq!(positions[13], Pt::new(123, 8));
    }

    #[test]
    fn waste_positions_handle_small_and_empty_piles() {
        let layout = win98_layout();
        assert!(layout.waste_positions(0, 0).is_empty());
        // A fresh Draw Three onto an empty waste: fan based at the origin.
        let positions = layout.waste_positions(3, 3);
        assert_eq!(positions[0], Pt::new(93, 5));
        assert_eq!(positions[1], Pt::new(107, 6));
        assert_eq!(positions[2], Pt::new(121, 7));
        // Fanned count larger than the pile is clamped.
        let positions = layout.waste_positions(2, 3);
        assert_eq!(positions[0], Pt::new(93, 5));
        assert_eq!(positions[1], Pt::new(107, 6));
    }

    #[test]
    fn tableau_positions_step_by_facing() {
        let layout = win98_layout();
        // Column 6 as dealt: six face-down, one face-up on top.
        assert_eq!(layout.tableau_card_pos(6, 6, 0), Some(Pt::new(503, 107)));
        assert_eq!(layout.tableau_card_pos(6, 6, 5), Some(Pt::new(503, 122)));
        assert_eq!(layout.tableau_card_pos(6, 6, 6), Some(Pt::new(503, 125)));
        // A face-up run growing below: face-up steps from there on.
        assert_eq!(layout.tableau_card_pos(6, 6, 7), Some(Pt::new(503, 140)));
        assert_eq!(layout.tableau_card_pos(6, 6, 8), Some(Pt::new(503, 155)));
        // A fully face-up pile steps 15 from the start.
        assert_eq!(layout.tableau_card_pos(0, 0, 2), Some(Pt::new(11, 137)));
    }

    #[test]
    fn a_different_card_size_follows_the_formulas() {
        let layout = Layout::new(
            CardSize {
                width: 100,
                height: 100,
            },
            0,
        );
        // xUnit = 100/8 + 3 = 15; yTop = (500 + 50)/100 = 5; tableauY = 111.
        assert_eq!(layout.pile_origin(PileId::Stock), Some(Pt::new(15, 5)));
        assert_eq!(
            layout.pile_origin(PileId::Tableau(0)),
            Some(Pt::new(15, 111))
        );
        assert_eq!(layout.face_up_step(), 16);
        assert_eq!(layout.face_down_step(), 4);
        assert_eq!(layout.waste_fan_step(), Pt::new(20, 1));
        assert_eq!(layout.design_size(), Size::new(820, 400));
    }

    #[test]
    fn absurd_card_sizes_saturate_instead_of_overflowing() {
        let layout = Layout::new(
            CardSize {
                width: u32::MAX,
                height: u32::MAX,
            },
            0,
        );
        assert_eq!(layout.card().w, i32::MAX);
        assert_eq!(layout.design_size().w, i32::MAX);
        assert_eq!(
            layout.pile_origin(PileId::Tableau(6)).map(|pt| pt.x),
            Some(i32::MAX)
        );
    }
}
