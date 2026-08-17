//! Hit-testing: which pile or card a pointer position lands on.
//!
//! The scan replays the original's order — stock, waste, foundations left
//! to right, tableau columns left to right — and within a pile finds the
//! topmost face-up card whose card-sized rectangle contains the point.
//! The stock is hit by its (padded) pile rectangle whether or not it
//! holds cards: clicking the empty stock is how the waste is recycled.

use sol_engine::{Card, FOUNDATION_COUNT, GameState, PileId};

use crate::geometry::{Pt, Rect};
use crate::layout::Layout;

/// What a pointer position lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HitTarget {
    /// The stock's pile rectangle (empty or not).
    Stock,
    /// A face-up card: the waste top, a foundation top, or any face-up
    /// tableau card (`index` counts from the pile's bottom).
    Card {
        /// The pile holding the card.
        pile: PileId,
        /// The card's index from the bottom of the pile.
        index: usize,
    },
}

/// The card at `index` (from the bottom) of `pile`, if it exists.
pub(crate) fn card_at(state: &GameState, pile: PileId, index: usize) -> Option<Card> {
    match pile {
        PileId::Stock => state.stock().get(index).copied(),
        PileId::Waste => state.waste().get(index).copied(),
        PileId::Foundation(f) => state
            .foundation(f)
            .and_then(|cards| cards.get(index))
            .copied(),
        PileId::Tableau(t) => {
            let pile = state.tableau(t)?;
            let down = pile.face_down().len();
            if index < down {
                pile.face_down().get(index).copied()
            } else {
                pile.face_up().get(index - down).copied()
            }
        }
    }
}

/// The top-left of the card at `index` of `pile`, from the layout.
pub(crate) fn card_pos(
    state: &GameState,
    layout: &Layout,
    fan: usize,
    pile: PileId,
    index: usize,
) -> Option<Pt> {
    match pile {
        PileId::Stock => Some(layout.stock_card_pos(index)),
        PileId::Waste => layout
            .waste_positions(state.waste().len(), fan)
            .get(index)
            .copied(),
        PileId::Foundation(f) => layout.foundation_card_pos(f, index),
        PileId::Tableau(t) => {
            let down = state.tableau(t)?.face_down().len();
            layout.tableau_card_pos(t, down, index)
        }
    }
}

/// The card-sized rectangle of the top card of `pile`, or `None` for an
/// empty (or out-of-range) pile.
pub(crate) fn top_card_rect(
    state: &GameState,
    layout: &Layout,
    fan: usize,
    pile: PileId,
) -> Option<Rect> {
    let len = match pile {
        PileId::Stock => state.stock().len(),
        PileId::Waste => state.waste().len(),
        PileId::Foundation(f) => state.foundation(f)?.len(),
        PileId::Tableau(t) => state.tableau(t)?.len(),
    };
    let top = len.checked_sub(1)?;
    let pos = card_pos(state, layout, fan, pile, top)?;
    Some(Rect::at(pos, layout.card()))
}

/// What `pt` lands on, in the original's scan order, or `None` for bare
/// felt.
pub(crate) fn hit_test(
    state: &GameState,
    layout: &Layout,
    fan: usize,
    pt: Pt,
) -> Option<HitTarget> {
    if layout
        .pile_rect(PileId::Stock)
        .is_some_and(|rect| rect.contains(pt))
    {
        return Some(HitTarget::Stock);
    }

    // Waste: only the top card responds.
    if let Some(rect) = top_card_rect(state, layout, fan, PileId::Waste)
        && rect.contains(pt)
    {
        return Some(HitTarget::Card {
            pile: PileId::Waste,
            index: state.waste().len() - 1,
        });
    }

    for f in 0..FOUNDATION_COUNT {
        let pile = PileId::Foundation(f);
        if let Some(rect) = top_card_rect(state, layout, fan, pile)
            && rect.contains(pt)
            && let Some(len) = state.foundation(f).map(<[Card]>::len)
        {
            return Some(HitTarget::Card {
                pile,
                index: len - 1,
            });
        }
    }

    for (t, tableau) in state.tableaus().enumerate() {
        let t = u8::try_from(t).unwrap_or(u8::MAX);
        let down = tableau.face_down().len();
        let total = tableau.len();
        // Topmost (front-most) face-up card first.
        for index in (down..total).rev() {
            if let Some(pos) = layout.tableau_card_pos(t, down, index)
                && Rect::at(pos, layout.card()).contains(pt)
            {
                return Some(HitTarget::Card {
                    pile: PileId::Tableau(t),
                    index,
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sol_engine::{DrawMode, GameConfig, ScoringMode, Seed, deal};
    use sol_theme::CardSize;

    use super::*;

    fn state() -> GameState {
        deal(
            Seed::new(1).unwrap(),
            GameConfig {
                draw_mode: DrawMode::Three,
                scoring: ScoringMode::Standard,
                timed: false,
            },
        )
    }

    fn layout() -> Layout {
        Layout::new(
            CardSize {
                width: 71,
                height: 96,
            },
            585,
        )
    }

    /// The original's stock rect is ten pixels wider than the card so a click
    /// just past the edge still deals. That padding is absolute, but the pile
    /// spacing scales with the card, so at small card sizes the stock rect
    /// used to reach into the waste's slot — and the stock is scanned first,
    /// so a press on the waste dealt from the stock instead.
    #[test]
    fn the_stock_rect_never_reaches_the_waste_at_a_small_card_size() {
        let card = CardSize {
            width: 32,
            height: 48,
        };
        let state = state();
        let layout = Layout::new(card, Layout::min_design(card).w);
        let waste_origin = layout.pile_origin(PileId::Waste).unwrap();

        assert_ne!(
            hit_test(
                &state,
                &layout,
                0,
                Pt::new(waste_origin.x, waste_origin.y + 1)
            ),
            Some(HitTarget::Stock),
            "the waste origin must not hit the stock"
        );
    }

    /// The clamp only ever narrows the rect, and only where the gap is
    /// narrower than the original's constant: at the original's own card size
    /// the ten-pixel overhang survives intact.
    #[test]
    fn the_stock_overhang_is_unchanged_at_the_original_card_size() {
        let layout = layout();
        let stock = layout.pile_rect(PileId::Stock).unwrap();
        assert_eq!(stock.w, 71 + 10);
    }

    #[test]
    fn the_stock_rect_hits_even_past_the_card() {
        let state = state();
        let layout = layout();
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(11, 5)),
            Some(HitTarget::Stock)
        );
        // The padded region right of the card still counts as the stock.
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(88, 100)),
            Some(HitTarget::Stock)
        );
        assert_eq!(hit_test(&state, &layout, 0, Pt::new(92, 5)), None);
    }

    #[test]
    fn a_dealt_table_hits_tableau_tops_and_felt() {
        let state = state();
        let layout = layout();
        // Column 0: a single face-up card at (11, 107).
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(40, 150)),
            Some(HitTarget::Card {
                pile: PileId::Tableau(0),
                index: 0,
            })
        );
        // Column 6: six face-down cards under the face-up top at y = 125;
        // a point on the fan edge hits nothing (face-down cards are not
        // targets), a point on the top card hits index 6.
        assert_eq!(hit_test(&state, &layout, 0, Pt::new(510, 110)), None);
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(510, 130)),
            Some(HitTarget::Card {
                pile: PileId::Tableau(6),
                index: 6,
            })
        );
        // Bare felt between the rows.
        assert_eq!(hit_test(&state, &layout, 0, Pt::new(300, 104)), None);
        // Empty waste and foundations hit nothing.
        assert_eq!(hit_test(&state, &layout, 0, Pt::new(100, 20)), None);
        assert_eq!(hit_test(&state, &layout, 0, Pt::new(260, 10)), None);
    }

    #[test]
    fn waste_hits_only_the_fanned_top_card() {
        let mut state = state();
        // Fan three cards onto the waste.
        sol_engine::evolve(
            &mut state,
            sol_engine::Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 3,
            },
        );
        let layout = layout();
        // Top card of the fan sits at (121, 7).
        assert_eq!(
            hit_test(&state, &layout, 3, Pt::new(130, 20)),
            Some(HitTarget::Card {
                pile: PileId::Waste,
                index: 2,
            })
        );
        // The exposed sliver of the second fan card is not a target; at
        // x = 100 the point is left of the top card's rect and hits
        // nothing (the waste's lower cards never respond).
        assert_eq!(hit_test(&state, &layout, 3, Pt::new(100, 20)), None);
    }

    #[test]
    fn overlapping_tableau_cards_hit_the_front_most() {
        let mut state = state();
        // Stack a second face-up card onto column 0 (blind mechanics).
        sol_engine::evolve(
            &mut state,
            sol_engine::Event::CardsMoved {
                from: PileId::Tableau(1),
                to: PileId::Tableau(0),
                count: 1,
            },
        );
        let layout = layout();
        // The overlap region belongs to the front-most (newer) card.
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(40, 130)),
            Some(HitTarget::Card {
                pile: PileId::Tableau(0),
                index: 1,
            })
        );
        // The exposed top sliver of the older card still hits it.
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(40, 110)),
            Some(HitTarget::Card {
                pile: PileId::Tableau(0),
                index: 0,
            })
        );
    }

    #[test]
    fn foundation_tops_are_hit_targets() {
        let mut state = state();
        // Put one card onto foundation 2 (blind mechanics).
        sol_engine::evolve(
            &mut state,
            sol_engine::Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(2),
                count: 1,
            },
        );
        let layout = layout();
        assert_eq!(
            hit_test(&state, &layout, 0, Pt::new(425, 20)),
            Some(HitTarget::Card {
                pile: PileId::Foundation(2),
                index: 0,
            })
        );
    }

    #[test]
    fn card_at_reads_every_pile_kind() {
        let state = state();
        assert!(card_at(&state, PileId::Stock, 0).is_some());
        assert!(card_at(&state, PileId::Waste, 0).is_none());
        assert!(card_at(&state, PileId::Foundation(0), 0).is_none());
        assert!(card_at(&state, PileId::Foundation(9), 0).is_none());
        assert!(card_at(&state, PileId::Tableau(9), 0).is_none());
        // Column 6: indices 0..6 face-down, 6 face-up.
        let down = card_at(&state, PileId::Tableau(6), 0);
        let up = card_at(&state, PileId::Tableau(6), 6);
        assert!(down.is_some());
        assert!(up.is_some());
        let top = state.tableau(6).unwrap().face_up().first().copied();
        assert_eq!(up, top);
        assert!(card_at(&state, PileId::Tableau(6), 7).is_none());
    }

    #[test]
    fn card_pos_and_top_rect_cover_every_pile_kind() {
        let state = state();
        let layout = layout();
        assert_eq!(
            card_pos(&state, &layout, 0, PileId::Stock, 0),
            Some(Pt::new(11, 5))
        );
        assert_eq!(card_pos(&state, &layout, 0, PileId::Waste, 0), None);
        assert_eq!(card_pos(&state, &layout, 0, PileId::Tableau(9), 0), None);
        assert_eq!(
            top_card_rect(&state, &layout, 0, PileId::Stock),
            Some(Rect::new(15, 7, 71, 96))
        );
        assert_eq!(top_card_rect(&state, &layout, 0, PileId::Waste), None);
        assert_eq!(
            top_card_rect(&state, &layout, 0, PileId::Foundation(1)),
            None
        );
        assert_eq!(
            top_card_rect(&state, &layout, 0, PileId::Foundation(9)),
            None
        );
        assert_eq!(top_card_rect(&state, &layout, 0, PileId::Tableau(9)), None);
        assert_eq!(
            top_card_rect(&state, &layout, 0, PileId::Tableau(0)),
            Some(Rect::new(11, 107, 71, 96))
        );
    }
}
