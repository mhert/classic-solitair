//! Drag-and-drop: pickup, live drop targeting, and the snap-back.
//!
//! Faithful to the original: pickup happens immediately on button-down
//! (no slop distance); the drop target is the first pile — foundations
//! left to right, then tableau columns left to right — whose top card (or
//! whose pile rectangle, when empty) overlaps the dragged card's
//! rectangle *at all*, and which the rules accept; an illegal release
//! slides the run straight back home. Rule knowledge is not duplicated:
//! legality is exactly [`sol_engine::decide`] accepting the move.

use sol_engine::{Command, FOUNDATION_COUNT, GameState, PileId, TABLEAU_COUNT, decide};

use crate::geometry::{Pt, Rect};
use crate::hit::{card_pos, top_card_rect};
use crate::layout::Layout;

/// The snap-back advances this many logical pixels (along the longer
/// axis) per step — the original redrew every 36th `LineDDA` pixel step.
pub(crate) const SNAP_STEP_PX: i32 = 36;

/// Milliseconds per snap-back step. The original's snap-back free-ran at
/// machine speed; this fixed cadence stands in for it.
pub(crate) const SNAP_STEP_MS: u32 = 10;

/// A run of cards being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Drag {
    /// The pile the run was picked up from.
    pub from: PileId,
    /// Index (from the pile's bottom) of the run's first card.
    pub first_index: usize,
    /// How many cards the run holds.
    pub count: u8,
    /// Grab offset: the first card's top-left minus the pointer, so the
    /// run rides the pointer exactly where it was gripped.
    pub grab: Pt,
    /// The current pointer position.
    pub pos: Pt,
    /// The first card's top-left at pickup — where an illegal drop snaps
    /// back to.
    pub home: Pt,
}

impl Drag {
    /// The dragged run's first-card rectangle at the current pointer.
    pub(crate) fn card_rect(&self, layout: &Layout) -> Rect {
        Rect::at(self.pos.translated(self.grab.x, self.grab.y), layout.card())
    }
}

/// Picks up the card at `(pile, index)` — already hit-tested — plus
/// everything above it, if that stack is draggable: the waste top, a
/// foundation top, or a face-up tableau run.
pub(crate) fn pick_up(
    state: &GameState,
    layout: &Layout,
    fan: usize,
    pile: PileId,
    index: usize,
    pt: Pt,
) -> Option<Drag> {
    let count = match pile {
        PileId::Stock => return None,
        // Hit-testing only ever yields the top card for these; the guards
        // keep pickup honest against any caller.
        PileId::Waste => {
            let len = state.waste().len();
            (len > 0 && index == len - 1).then_some(1_u8)?
        }
        PileId::Foundation(f) => {
            let len = state.foundation(f)?.len();
            (len > 0 && index == len - 1).then_some(1_u8)?
        }
        PileId::Tableau(t) => {
            let tableau = state.tableau(t)?;
            let down = tableau.face_down().len();
            let total = tableau.len();
            if index < down || index >= total {
                return None;
            }
            u8::try_from(total - index).ok()?
        }
    };
    let home = card_pos(state, layout, fan, pile, index)?;
    Some(Drag {
        from: pile,
        first_index: index,
        count,
        grab: Pt::new(home.x.saturating_sub(pt.x), home.y.saturating_sub(pt.y)),
        pos: pt,
        home,
    })
}

/// The pile the run would drop onto right now, or `None`.
///
/// The original's rule: scan the piles in table order (stock and waste
/// never accept), take the **first** whose overlap rectangle intersects
/// the dragged card's rectangle at all *and* whose rules accept the move
/// — no largest-overlap comparison.
pub(crate) fn drop_target(
    state: &GameState,
    layout: &Layout,
    fan: usize,
    drag: &Drag,
) -> Option<PileId> {
    let dragged = drag.card_rect(layout);
    let foundations = (0..FOUNDATION_COUNT).map(PileId::Foundation);
    let tableaus = (0..TABLEAU_COUNT).map(PileId::Tableau);
    for pile in foundations.chain(tableaus) {
        if pile == drag.from {
            continue;
        }
        // Every scanned pile id is in range, so `pile_rect` is always
        // `Some`; the default (an empty rectangle that intersects
        // nothing) merely keeps the lookup total.
        let overlap = top_card_rect(state, layout, fan, pile)
            .or_else(|| layout.pile_rect(pile))
            .unwrap_or_default();
        if !dragged.intersects(overlap) {
            continue;
        }
        let command = Command::MoveCards {
            from: drag.from,
            to: pile,
            count: drag.count,
        };
        if decide(state, command).is_ok() {
            return Some(pile);
        }
    }
    None
}

/// The run sliding home after an illegal drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapBack {
    /// The frozen drag (pile, run, indices) being returned.
    pub drag: Drag,
    from: Pt,
    length: i32,
    step_px: i32,
    traveled: i32,
    acc_ms: u32,
}

impl SnapBack {
    /// Starts the slide from the drag's released position back home.
    pub(crate) fn new(drag: Drag) -> Self {
        let from = drag.pos.translated(drag.grab.x, drag.grab.y);
        let length = (from.x.saturating_sub(drag.home.x).abs())
            .max(from.y.saturating_sub(drag.home.y).abs());
        Self {
            drag,
            from,
            length,
            step_px: SNAP_STEP_PX,
            traveled: 0,
            acc_ms: 0,
        }
    }

    /// Advances the slide by `dt_ms`.
    pub(crate) fn advance(&mut self, dt_ms: u32) {
        self.acc_ms = self.acc_ms.saturating_add(dt_ms);
        while self.acc_ms >= SNAP_STEP_MS && !self.is_done() {
            self.acc_ms -= SNAP_STEP_MS;
            self.traveled = self.traveled.saturating_add(self.step_px);
        }
    }

    /// Whether the run has arrived home.
    pub(crate) const fn is_done(&self) -> bool {
        self.traveled >= self.length
    }

    /// Lands the run instantly.
    pub(crate) fn skip(&mut self) {
        self.traveled = self.length;
    }

    /// Re-aims the slide at a moved home — the board re-laid out while
    /// the run was flying. The line restarts from the run's current
    /// position toward the new home.
    pub(crate) fn retarget(&mut self, home: Pt) {
        self.from = self.pos();
        self.drag.home = home;
        self.length = (self.from.x.saturating_sub(home.x).abs())
            .max(self.from.y.saturating_sub(home.y).abs());
        self.traveled = 0;
        self.acc_ms = 0;
    }

    /// The first card's current top-left along the straight line home.
    pub(crate) fn pos(&self) -> Pt {
        if self.is_done() || self.length == 0 {
            return self.drag.home;
        }
        let t = i64::from(self.traveled);
        let len = i64::from(self.length);
        let lerp = |from: i32, to: i32| {
            crate::geometry::saturate(i64::from(from) + (i64::from(to) - i64::from(from)) * t / len)
        };
        Pt::new(
            lerp(self.from.x, self.drag.home.x),
            lerp(self.from.y, self.drag.home.y),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_engine::{DrawMode, Event, GameConfig, ScoringMode, Seed, deal, evolve};
    use sol_theme::CardSize;

    use super::*;

    /// The deal these tests share with the engine's rules suites.
    const SEED: u16 = 8622;

    fn state() -> GameState {
        deal(
            Seed::new(SEED).unwrap(),
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

    #[test]
    fn retarget_restarts_the_slide_toward_the_new_home() {
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        drag.pos = Pt::new(300, 300);
        let mut snap = SnapBack::new(drag);
        snap.advance(SNAP_STEP_MS);
        let mid = snap.pos();
        snap.retarget(Pt::new(600, 50));
        assert_eq!(snap.drag.home, Pt::new(600, 50));
        assert_eq!(snap.pos(), mid, "the run does not jump on retarget");
        snap.skip();
        assert_eq!(snap.pos(), Pt::new(600, 50));
    }

    #[test]
    fn pick_up_grips_a_tableau_run_with_its_offset() {
        let state = state();
        let layout = layout();
        let drag = pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        assert_eq!(drag.count, 1);
        assert_eq!(drag.first_index, 6);
        assert_eq!(drag.home, Pt::new(503, 125));
        assert_eq!(drag.grab, Pt::new(-7, -5));
        assert_eq!(
            drag.card_rect(&layout),
            Rect::new(503, 125, 71, 96),
            "at pickup the card has not moved"
        );
    }

    #[test]
    fn pick_up_rejects_what_the_original_rejected() {
        let state = state();
        let layout = layout();
        // The stock is never dragged.
        assert!(pick_up(&state, &layout, 0, PileId::Stock, 0, Pt::new(11, 5)).is_none());
        // Face-down tableau cards are not draggable.
        assert!(pick_up(&state, &layout, 0, PileId::Tableau(6), 3, Pt::new(510, 116)).is_none());
        // Out-of-range indices and empty piles yield nothing.
        assert!(pick_up(&state, &layout, 0, PileId::Tableau(6), 7, Pt::new(510, 130)).is_none());
        assert!(pick_up(&state, &layout, 0, PileId::Waste, 0, Pt::new(93, 5)).is_none());
        assert!(
            pick_up(
                &state,
                &layout,
                0,
                PileId::Foundation(0),
                0,
                Pt::new(257, 5)
            )
            .is_none()
        );
        assert!(
            pick_up(
                &state,
                &layout,
                0,
                PileId::Foundation(9),
                0,
                Pt::new(257, 5)
            )
            .is_none()
        );
        assert!(pick_up(&state, &layout, 0, PileId::Tableau(9), 0, Pt::new(0, 0)).is_none());
    }

    #[test]
    fn pick_up_takes_the_waste_top_after_a_draw() {
        let mut state = state();
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Stock,
                to: PileId::Waste,
                count: 3,
            },
        );
        let layout = layout();
        let drag = pick_up(&state, &layout, 3, PileId::Waste, 2, Pt::new(130, 20)).unwrap();
        assert_eq!(drag.count, 1);
        assert_eq!(drag.home, Pt::new(121, 7));
        // Only the top card: index 1 is not draggable.
        assert!(pick_up(&state, &layout, 3, PileId::Waste, 1, Pt::new(110, 20)).is_none());
    }

    #[test]
    fn drop_target_takes_the_first_overlapping_legal_pile() {
        // This deal puts S3 on top of column 6 and H4 on top of column 2 —
        // a black three onto a red four is the one legal tableau move.
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        // Hover the run over column 2's top card: any overlap counts.
        drag.pos = Pt::new(190, 130);
        assert_eq!(
            drop_target(&state, &layout, 0, &drag),
            Some(PileId::Tableau(2))
        );
        // Back over its own column: the source never targets itself.
        drag.pos = Pt::new(510, 130);
        assert_eq!(drop_target(&state, &layout, 0, &drag), None);
        // Far away over bare felt: nothing.
        drag.pos = Pt::new(400, 360);
        assert_eq!(drop_target(&state, &layout, 0, &drag), None);
    }

    #[test]
    fn drop_target_mirrors_decide_not_geometry_alone() {
        // Column 3's top (D11) fully overlaps column 6's top (S3) when
        // dragged there, but a red jack on a black three is illegal —
        // decide says no, so there is no target despite the overlap.
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(3), 3, Pt::new(260, 120)).unwrap();
        drag.pos = Pt::new(506, 129);
        assert_eq!(drop_target(&state, &layout, 0, &drag), None);
    }

    #[test]
    fn empty_piles_accept_by_pile_rect_when_legal() {
        // This deal puts the ace of clubs face-up on column 0: dragged over
        // an empty foundation, the foundation's pile rect accepts it.
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(0), 0, Pt::new(20, 120)).unwrap();
        drag.pos = Pt::new(270, 30);
        assert_eq!(
            drop_target(&state, &layout, 0, &drag),
            Some(PileId::Foundation(0))
        );

        // With column 0 emptied, a non-king over its pile rect stays
        // targetless — geometry overlaps, decide refuses.
        let mut state = state;
        evolve(
            &mut state,
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            },
        );
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(1), 1, Pt::new(100, 120)).unwrap();
        drag.pos = Pt::new(20, 130);
        assert_eq!(
            drop_target(&state, &layout, 0, &drag),
            None,
            "H4 may not land on an empty column"
        );
    }

    #[test]
    fn snap_back_slides_straight_home_and_finishes() {
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        drag.pos = Pt::new(510 + 100, 130 + 50);
        let mut snap = SnapBack::new(drag);
        assert_eq!(snap.pos(), Pt::new(603, 175));
        assert!(!snap.is_done());
        // 36 px per 10 ms along the longer axis (100 px): done after
        // ceil(100/36) = 3 steps.
        snap.advance(SNAP_STEP_MS);
        assert_eq!(snap.pos(), Pt::new(603 - 36, 175 - 18));
        snap.advance(SNAP_STEP_MS * 2);
        assert!(snap.is_done());
        assert_eq!(snap.pos(), Pt::new(503, 125));
        // Advancing past the end holds home.
        snap.advance(SNAP_STEP_MS * 5);
        assert_eq!(snap.pos(), Pt::new(503, 125));
    }

    #[test]
    fn zero_distance_snap_back_is_instantly_done() {
        let state = state();
        let layout = layout();
        let drag = pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        let snap = SnapBack::new(drag);
        assert!(snap.is_done());
        assert_eq!(snap.pos(), Pt::new(503, 125));
    }

    #[test]
    fn snap_back_skip_lands_immediately() {
        let state = state();
        let layout = layout();
        let mut drag =
            pick_up(&state, &layout, 0, PileId::Tableau(6), 6, Pt::new(510, 130)).unwrap();
        drag.pos = Pt::new(200, 300);
        let mut snap = SnapBack::new(drag);
        assert!(!snap.is_done());
        snap.skip();
        assert!(snap.is_done());
        assert_eq!(snap.pos(), drag.home);
    }

    mod mirrors_decide {
        //! The core drag property: drop targeting admits exactly the
        //! moves [`decide`] admits, first pile in table order.

        use proptest::prelude::*;
        use sol_engine::{Game, Seed};

        use super::*;
        use crate::hit::top_card_rect;
        use crate::waste::fan_len;

        /// Random valid play: walk `steps` through draws and
        /// auto-to-foundation attempts, keeping whatever the rules accept.
        fn random_game(seed: u16, steps: &[u8]) -> Game {
            let mut game = Game::new(
                Seed::new(seed).unwrap(),
                GameConfig {
                    draw_mode: DrawMode::Three,
                    scoring: ScoringMode::Standard,
                    timed: false,
                },
            );
            for step in steps {
                let command = match step % 9 {
                    0..=2 => sol_engine::Command::Draw,
                    3 => sol_engine::Command::AutoToFoundation {
                        pile: PileId::Waste,
                    },
                    other => sol_engine::Command::AutoToFoundation {
                        pile: PileId::Tableau(other - 4),
                    },
                };
                let _ = game.apply(command);
            }
            game
        }

        /// Every draggable stack in the state: waste top, foundation
        /// tops, and each face-up tableau index.
        fn draggable_stacks(state: &GameState) -> Vec<(PileId, usize)> {
            let mut stacks = Vec::new();
            if let Some(top) = state.waste().len().checked_sub(1) {
                stacks.push((PileId::Waste, top));
            }
            for f in 0..FOUNDATION_COUNT {
                if let Some(top) = state.foundation(f).and_then(|c| c.len().checked_sub(1)) {
                    stacks.push((PileId::Foundation(f), top));
                }
            }
            for (t, pile) in state.tableaus().enumerate() {
                let t = u8::try_from(t).unwrap_or(u8::MAX);
                for index in pile.face_down().len()..pile.len() {
                    stacks.push((PileId::Tableau(t), index));
                }
            }
            stacks
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]

            #[test]
            fn drop_target_admits_exactly_what_decide_admits(
                seed in 0_u16..400,
                steps in proptest::collection::vec(any::<u8>(), 0..24),
                stack_choice in any::<usize>(),
                px in -100_i32..700,
                py in -100_i32..500,
            ) {
                let game = random_game(seed, &steps);
                let state = game.state();
                let layout = super::layout();
                let fan = fan_len(game.log());
                let stacks = draggable_stacks(state);
                prop_assume!(!stacks.is_empty());
                let (pile, index) = stacks[stack_choice % stacks.len()];

                let mut drag =
                    pick_up(state, &layout, fan, pile, index, Pt::new(0, 0)).unwrap();
                drag.pos = Pt::new(px, py);
                let target = drop_target(state, &layout, fan, &drag);

                // Reference: first pile in table order that overlaps and
                // whose move decide accepts.
                let dragged = drag.card_rect(&layout);
                let piles = (0..FOUNDATION_COUNT)
                    .map(PileId::Foundation)
                    .chain((0..TABLEAU_COUNT).map(PileId::Tableau));
                let mut expected = None;
                for candidate in piles {
                    if candidate == drag.from {
                        continue;
                    }
                    let overlap = top_card_rect(state, &layout, fan, candidate)
                        .or_else(|| layout.pile_rect(candidate));
                    let overlaps = overlap.is_some_and(|rect| dragged.intersects(rect));
                    let legal = decide(
                        state,
                        Command::MoveCards {
                            from: drag.from,
                            to: candidate,
                            count: drag.count,
                        },
                    )
                    .is_ok();
                    if overlaps && legal {
                        expected = Some(candidate);
                        break;
                    }
                }
                prop_assert_eq!(target, expected);

                // Soundness on its own terms: a reported target is always
                // a move the rules accept.
                if let Some(to) = target {
                    let command = Command::MoveCards {
                        from: drag.from,
                        to,
                        count: drag.count,
                    };
                    prop_assert!(decide(state, command).is_ok());
                }
            }
        }
    }
}
