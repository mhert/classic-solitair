//! The state-to-display-list projection: [`Presenter::frame`] and the
//! `push_*` family it delegates to.
//!
//! Split from its parent module, which holds the command surface — the
//! pointer/key handling, the menu commands, the save/load facade. These
//! two touch the presenter only through `&self`: everything here reads
//! settled state and appends sprites, and nothing here mutates. Keeping
//! them apart means a change to how the board is *drawn* never sits in the
//! same file as a change to what the board *does*.

use sol_engine::{Card, Command, PileId};

use super::{Phase, Presenter};
use crate::backs;
use crate::deal_anim::{DEAL_FLIGHT_MS, lerp};
use crate::display::{DisplayList, PlaceholderSlot, Rgba, TextureId};
use crate::drag::drop_target;
use crate::geometry::{Pt, Rect, index_to_i32, saturate};
use crate::hit::{card_at, top_card_rect};
use crate::profile::ProfileBackground;

impl Presenter {
    /// Builds this frame's display list.
    ///
    /// Ordinarily the full board over a cleared table; while a started
    /// cascade is running it is only the newly stepped card positions
    /// with `clear: None`, so the renderer paints the smear trail over
    /// the previous frame. The frames right after `fit_viewport` (while
    /// the `resize_repaint` countdown is nonzero) are the exception: the
    /// host's render target just changed size, so they repaint the
    /// ordinary full board, with the cascade's pending faces on top,
    /// before the smear resumes.
    #[must_use]
    pub fn frame(&self) -> DisplayList {
        if let Phase::Cascade(cascade) = &self.phase
            && cascade.is_started()
            && self.resize_repaint == 0
        {
            let mut list = DisplayList {
                clear: None,
                sprites: Vec::new(),
            };
            for (card, pos) in cascade.pending() {
                self.push_face(&mut list, *card, *pos);
            }
            return list;
        }

        let mut list = DisplayList {
            clear: Some(match self.profile.background {
                ProfileBackground::Color(color) => color,
                ProfileBackground::Image { .. } => Rgba::opaque(0, 0, 0),
            }),
            sprites: Vec::new(),
        };
        self.push_background(&mut list);
        self.push_placeholders(&mut list);
        self.push_board(&mut list);
        self.push_deal_flight(&mut list);
        self.push_dragged_run(&mut list);
        // Falls through to here either because no cascade has started
        // stepping yet (the ordinary pre-cascade frame, unchanged) or
        // because the resize repaint countdown is still nonzero and just
        // forced a self-contained repaint; either way, a running cascade
        // still draws its current pending faces on top, so a flying card
        // never disappears for a frame.
        if let Phase::Cascade(cascade) = &self.phase {
            for (card, pos) in cascade.pending() {
                self.push_face(&mut list, *card, *pos);
            }
        }
        list
    }

    /// Background image sprites (tiled or stretched), if the theme has
    /// an image background.
    fn push_background(&self, list: &mut DisplayList) {
        let ProfileBackground::Image { size, tile } = self.profile.background else {
            return;
        };
        let src = Rect::new(0, 0, size.w, size.h);
        if !tile {
            list.push(
                TextureId::Background,
                src,
                Rect::new(0, 0, self.viewport.w, self.viewport.h),
                Rgba::WHITE,
            );
            return;
        }
        let tile_w = size.w.max(1);
        let tile_h = size.h.max(1);
        let mut y = 0;
        while y < self.viewport.h {
            let mut x = 0;
            while x < self.viewport.w {
                list.push(
                    TextureId::Background,
                    src,
                    Rect::new(x, y, tile_w, tile_h),
                    Rgba::WHITE,
                );
                x = x.saturating_add(tile_w);
            }
            y = y.saturating_add(tile_h);
        }
    }

    /// The pile-slot placeholders, drawn under the cards: the ghost on
    /// every visually empty foundation, and the stock's own indicator when
    /// the stock is empty.
    ///
    /// "Visually empty" counts the cards a frame actually draws, so a
    /// foundation emptied by picking its card up reveals its ghost while
    /// the card is in hand, matching what the player sees rather than what
    /// the game state holds. Only the foundations are ghosted — the
    /// original leaves the waste and an emptied tableau column as bare
    /// table.
    fn push_placeholders(&self, list: &mut DisplayList) {
        let state = self.session.game().state();

        if state.stock().is_empty() {
            // Recycling is the engine's rule, not a rule to restate here:
            // on an empty stock a draw succeeds exactly when a pass
            // remains, so the ring means "this click does something".
            let slot = if sol_engine::decide(state, Command::Draw).is_ok() {
                PlaceholderSlot::StockRecycle
            } else {
                PlaceholderSlot::StockBlocked
            };
            self.push_placeholder(list, slot, PileId::Stock);
        }

        for (index, cards) in state.foundations().enumerate() {
            let pile = PileId::Foundation(u8::try_from(index).unwrap_or(u8::MAX));
            if self.draws_nothing(pile, cards.len()) {
                self.push_placeholder(list, PlaceholderSlot::EmptyPile, pile);
            }
        }
    }

    /// Whether this frame draws none of `pile`'s `len` cards, because the
    /// pile is empty or a drag is holding every card it has elsewhere.
    fn draws_nothing(&self, pile: PileId, len: usize) -> bool {
        (0..len).all(|index| self.is_hidden(pile, index))
    }

    /// One placeholder sprite filling `pile`'s card slot, skipped when the
    /// theme does not supply that slot.
    fn push_placeholder(&self, list: &mut DisplayList, slot: PlaceholderSlot, pile: PileId) {
        if !self.profile.placeholders.has(slot) {
            return;
        }
        // Every pile this is called for is in range, so the origin always
        // exists; the empty default merely keeps the lookup total.
        let pos = self.layout.pile_origin(pile).unwrap_or_default();
        let card = self.layout.card();
        let base = self.layout.card_base();
        list.push(
            TextureId::Placeholder { slot },
            Rect::new(0, 0, base.w, base.h),
            Rect::new(pos.x, pos.y, card.w, card.h),
            Rgba::WHITE,
        );
    }

    /// Every pile's cards. During the deal animation, tableau cards that
    /// have not arrived yet are withheld; during a drag or snap-back, the
    /// dragged run is withheld from its source pile.
    fn push_board(&self, list: &mut DisplayList) {
        let state = self.session.game().state();
        let dealing = match &self.phase {
            Phase::Dealing(deal) => Some(deal),
            _ => None,
        };

        for index in 0..state.stock().len() {
            self.push_back_sprite(list, self.layout.stock_card_pos(index));
        }

        let positions = self.layout.waste_positions(state.waste().len(), self.fan);
        for (index, card) in state.waste().iter().enumerate() {
            if self.is_hidden(PileId::Waste, index) {
                continue;
            }
            if let Some(pos) = positions.get(index) {
                self.push_face(list, *card, *pos);
            }
        }

        for (f, cards) in state.foundations().enumerate() {
            let f = u8::try_from(f).unwrap_or(u8::MAX);
            for (index, card) in cards.iter().enumerate() {
                if self.is_hidden(PileId::Foundation(f), index) {
                    continue;
                }
                if let Some(pos) = self.layout.foundation_card_pos(f, index) {
                    self.push_face(list, *card, pos);
                }
            }
        }

        for (t, pile) in state.tableaus().enumerate() {
            let t = u8::try_from(t).unwrap_or(u8::MAX);
            let down = pile.face_down().len();
            let shown = dealing.map_or(pile.len(), |deal| deal.arrived_rows(t));
            let cards = pile
                .face_down()
                .iter()
                .map(|card| (false, *card))
                .chain(pile.face_up().iter().map(|card| (true, *card)));
            for (index, (face_up, card)) in cards.take(shown.min(pile.len())).enumerate() {
                if self.is_hidden(PileId::Tableau(t), index) {
                    continue;
                }
                // In-range columns always have positions; the empty
                // default merely keeps the lookup total.
                let pos = self
                    .layout
                    .tableau_card_pos(t, down, index)
                    .unwrap_or_default();
                if face_up {
                    self.push_face(list, card, pos);
                } else {
                    self.push_back_sprite(list, pos);
                }
            }
        }
    }

    /// Whether the card at `(pile, index)` is withheld from its pile
    /// because a drag or snap-back is showing it elsewhere.
    fn is_hidden(&self, pile: PileId, index: usize) -> bool {
        let held = match (&self.drag, &self.phase) {
            (Some(drag), _) => Some(drag),
            (None, Phase::SnapBack(snap)) => Some(&snap.drag),
            _ => None,
        };
        held.is_some_and(|drag| drag.from == pile && index >= drag.first_index)
    }

    /// The card currently flying in the deal animation.
    ///
    /// The stock origin and the target slot always exist for a flight's
    /// in-range column; the `if let` chain merely keeps the lookups
    /// total. The chain's first condition is the phase itself, so every
    /// non-dealing frame walks the same exit.
    fn push_deal_flight(&self, list: &mut DisplayList) {
        if let Phase::Dealing(deal) = &self.phase
            && let Some((flight, elapsed)) = deal.current_flight()
            && let Some(from) = self.layout.pile_origin(PileId::Stock)
            && let Some(to) =
                self.layout
                    .tableau_card_pos(flight.column, usize::from(flight.column), flight.row)
        {
            let pos = lerp(from, to, elapsed, DEAL_FLIGHT_MS);
            let state = self.session.game().state();
            let face_card = state
                .tableau(flight.column)
                .and_then(|pile| pile.face_up().first())
                .copied();
            match (flight.face_up, face_card) {
                (true, Some(card)) => self.push_face(list, card, pos),
                _ => self.push_back_sprite(list, pos),
            }
        }
    }

    /// The dragged (or snapping-back) run: full card images, or the
    /// outline rectangle when outline dragging is on.
    fn push_dragged_run(&self, list: &mut DisplayList) {
        let (drag, pos, live) = match (&self.drag, &self.phase) {
            (Some(drag), _) => (drag, drag.pos.translated(drag.grab.x, drag.grab.y), true),
            (None, Phase::SnapBack(snap)) => (&snap.drag, snap.pos(), false),
            _ => return,
        };
        let outline = self.options().outline_dragging;
        if outline && live {
            // The hovered legal target is highlighted in outline mode
            // only, standing in for the original's InvertRect flash.
            let state = self.session.game().state();
            if let Some(target) = drop_target(state, &self.layout, self.fan, drag) {
                let rect = top_card_rect(state, &self.layout, self.fan, target).or_else(|| {
                    self.layout
                        .pile_origin(target)
                        .map(|origin| Rect::at(origin, self.layout.card()))
                });
                if let Some(rect) = rect {
                    self.push_outline_rect(list, rect);
                }
            }
        }
        let step = self.layout.face_up_step();
        if outline {
            let count = i32::from(drag.count);
            let height =
                saturate(i64::from(count - 1) * i64::from(step) + i64::from(self.layout.card().h));
            let run = Rect::new(pos.x, pos.y, self.layout.card().w, height);
            self.push_outline_rect(list, run);
            // A separator line at every card boundary inside the run.
            for k in 1..count {
                let y = pos.y.saturating_add(step.saturating_mul(k));
                list.push(
                    TextureId::White,
                    Rect::new(0, 0, 1, 1),
                    Rect::new(run.x, y, run.w, 1),
                    self.profile.outline,
                );
            }
            return;
        }
        let state = self.session.game().state();
        for k in 0..usize::from(drag.count) {
            // Dragged cards are still in their source pile (hiding them
            // is purely visual), so the lookup always succeeds.
            if let Some(card) = card_at(state, drag.from, drag.first_index + k) {
                let y = pos.y.saturating_add(step.saturating_mul(index_to_i32(k)));
                self.push_face(list, card, Pt::new(pos.x, y));
            }
        }
    }

    /// A one-logical-pixel rectangle outline in the theme's outline
    /// color; the scene transform thickens it on screen. The side edges
    /// are inset by the one-pixel top and bottom edges.
    fn push_outline_rect(&self, list: &mut DisplayList, rect: Rect) {
        let src = Rect::new(0, 0, 1, 1);
        let color = self.profile.outline;
        list.push(
            TextureId::White,
            src,
            Rect::new(rect.x, rect.y, rect.w, 1),
            color,
        );
        list.push(
            TextureId::White,
            src,
            Rect::new(rect.x, rect.bottom().saturating_sub(1), rect.w, 1),
            color,
        );
        list.push(
            TextureId::White,
            src,
            Rect::new(
                rect.x,
                rect.y.saturating_add(1),
                1,
                rect.h.saturating_sub(2),
            ),
            color,
        );
        list.push(
            TextureId::White,
            src,
            Rect::new(
                rect.right().saturating_sub(1),
                rect.y.saturating_add(1),
                1,
                rect.h.saturating_sub(2),
            ),
            color,
        );
    }

    /// A face-up card sprite at `pos`.
    fn push_face(&self, list: &mut DisplayList, card: Card, pos: Pt) {
        let base = self.layout.card_base();
        list.push(
            TextureId::Face {
                suit: card.suit,
                rank: card.rank,
            },
            Rect::new(0, 0, base.w, base.h),
            Rect::at(pos, self.layout.card()),
            Rgba::WHITE,
        );
    }

    /// A card-back sprite at `pos`, showing the selected back's current
    /// animation frame.
    ///
    /// The selected index is validated on entry ([`Presenter::set_back`])
    /// and re-clamped on theme switches, so the lookup always succeeds.
    fn push_back_sprite(&self, list: &mut DisplayList, pos: Pt) {
        if let Some(meta) = self.profile.backs.get(self.back_index) {
            let frame = backs::frame_index(meta, self.clock_ms);
            let (asset, src) = backs::frame_source(meta, frame, self.layout.card_base());
            list.push(
                TextureId::Back {
                    back: self.back_index,
                    asset,
                },
                src,
                Rect::at(pos, self.layout.card()),
                Rgba::WHITE,
            );
        }
    }
}
