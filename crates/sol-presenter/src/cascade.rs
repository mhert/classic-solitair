//! The win cascade: the bouncing-card celebration, physics-exact to the
//! original.
//!
//! Reverse-engineered from the original game's animation loop:
//!
//! - Cards launch one at a time — kings first, then queens, and so on,
//!   each rank sweeping the foundations left to right; the next card
//!   launches only once the previous one has left the playfield.
//! - Velocities are in **tenths of a pixel per step**. Per card, from the
//!   C runtime `rand()` sequence: `dx = rand() % 110 − 65` (forced to
//!   `−20` when `|dx| < 15`), `dy = rand() % 110 − 75`.
//! - Per step: `x += dx/10; y += dy/10; dy += 3`, all division truncating;
//!   after that, once the card sits below `floor = viewport bottom − card
//!   height` while falling (`dy > 0`), it bounces: `dy = (dy · 8) / −10`
//!   — 80% restitution, position not clamped.
//! - A card is done when its left edge passes either side of the
//!   viewport; there is no bottom exit.
//! - Nothing is erased between steps: every stepped position stays on
//!   screen, painting the smear trail.
//!
//! The original free-ran this loop at machine speed; a fixed
//! [`CASCADE_STEP_MS`] step stands in for that here, and the physics runs
//! in unscaled logical pixels so trajectories are identical at every
//! scale. One deliberate difference: the original seeded `rand()` from
//! the wall clock, which a deterministic presenter cannot; the seed here
//! is the game's own deal seed, so a given game always cascades the same
//! way.

use sol_engine::{Card, FOUNDATION_COUNT, GameState};

use crate::geometry::{Pt, Size};
use crate::layout::Layout;
use crate::msrand::MsRand;

/// Milliseconds per physics step. The original stepped as fast as the
/// machine allowed; 10 ms per step reproduces the era's pace.
pub(crate) const CASCADE_STEP_MS: u32 = 10;

/// Gravity: added to the vertical velocity (tenths of a pixel) each step.
const GRAVITY_TENTHS: i32 = 3;

/// One card mid-flight. Velocities are tenths of a pixel per step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flight {
    card: Card,
    x: i32,
    y: i32,
    dx_tenths: i32,
    dy_tenths: i32,
}

/// The running win cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cascade {
    rng: MsRand,
    launches: Vec<(Card, Pt)>,
    cursor: usize,
    flight: Option<Flight>,
    pending: Vec<(Card, Pt)>,
    started: bool,
    acc_ms: u32,
    floor_y: i32,
    x_max: i32,
    card: Size,
}

impl Cascade {
    /// Builds the cascade for a won game.
    ///
    /// `viewport` is in logical pixels; the floor and side exits freeze
    /// at these bounds for the whole cascade, exactly as the original
    /// captured its client rectangle once at cascade start.
    pub(crate) fn new(state: &GameState, layout: &Layout, viewport: Size, seed: u32) -> Self {
        let card = layout.card_base();
        let mut launches = Vec::with_capacity(52);
        for value in (1..=13_usize).rev() {
            for foundation in 0..FOUNDATION_COUNT {
                let card_there = state
                    .foundation(foundation)
                    .and_then(|cards| cards.get(value - 1))
                    .copied();
                let Some((card, pos)) =
                    card_there.zip(layout.foundation_card_pos(foundation, value - 1))
                else {
                    continue;
                };
                launches.push((card, pos));
            }
        }
        Self {
            rng: MsRand::new(seed & 0x7FFF),
            launches,
            cursor: 0,
            flight: None,
            pending: Vec::new(),
            started: false,
            acc_ms: 0,
            floor_y: viewport.h - card.h,
            x_max: viewport.w,
            card,
        }
    }

    /// Advances the physics by `dt_ms`, replacing the pending trail with
    /// every position stepped since the previous `advance` call. The host
    /// draws each batch exactly once per frame; nothing erases them.
    pub(crate) fn advance(&mut self, dt_ms: u32) {
        self.pending.clear();
        if self.is_done() {
            return;
        }
        self.acc_ms = self.acc_ms.saturating_add(dt_ms);
        while self.acc_ms >= CASCADE_STEP_MS {
            self.acc_ms -= CASCADE_STEP_MS;
            if !self.step() {
                break;
            }
        }
    }

    /// One physics step. Returns `false` once there is nothing left to do.
    fn step(&mut self) -> bool {
        if let Some(flight) = &mut self.flight {
            flight.x = flight.x.saturating_add(flight.dx_tenths / 10);
            flight.y = flight.y.saturating_add(flight.dy_tenths / 10);
            flight.dy_tenths = flight.dy_tenths.saturating_add(GRAVITY_TENTHS);
            if flight.y > self.floor_y && flight.dy_tenths.is_positive() {
                flight.dy_tenths = flight.dy_tenths.saturating_mul(8) / -10;
            }
            if flight.x <= -self.card.w || flight.x >= self.x_max {
                self.flight = None;
            } else {
                self.pending
                    .push((flight.card, Pt::new(flight.x, flight.y)));
            }
            return true;
        }
        let Some((card, start)) = self.launches.get(self.cursor).copied() else {
            return false;
        };
        self.cursor += 1;
        let mut dx = self.rng.next() % 110 - 65;
        if dx.abs() < 15 {
            dx = -20;
        }
        let dy = self.rng.next() % 110 - 75;
        self.flight = Some(Flight {
            card,
            x: start.x,
            y: start.y,
            dx_tenths: dx,
            dy_tenths: dy,
        });
        self.started = true;
        self.pending.push((card, start));
        true
    }

    /// Whether any physics step has run yet — the frame before the first
    /// step still shows the freshly won board (and clears normally).
    pub(crate) const fn is_started(&self) -> bool {
        self.started
    }

    /// Whether every card has left the playfield.
    pub(crate) fn is_done(&self) -> bool {
        self.cursor >= self.launches.len() && self.flight.is_none()
    }

    /// Stops the cascade instantly (any input skips it).
    pub(crate) fn skip(&mut self) {
        self.cursor = self.launches.len();
        self.flight = None;
        self.pending.clear();
    }

    /// The positions stepped since the last `advance` call, in step order
    /// — the smear-trail segments to draw this frame, in unscaled logical
    /// pixels.
    pub(crate) fn pending(&self) -> &[(Card, Pt)] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_engine::{PileId, Rank};
    use sol_theme::CardSize;

    use super::*;
    use crate::geometry::Size;

    /// A won game: every foundation complete, ace to king.
    fn won_state() -> GameState {
        crate::testkit_engine::won_game().state().clone()
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

    fn viewport() -> Size {
        Size::new(585, 384)
    }

    #[test]
    fn launches_run_kings_first_across_the_foundations() {
        let cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        assert_eq!(cascade.launches.len(), 52);
        // The first four launches are the four kings, foundations left to
        // right, from the foundation tops (thickness offset (6, 3)).
        for (index, (card, pos)) in cascade.launches.iter().take(4).enumerate() {
            assert_eq!(card.rank, Rank::King);
            let x = 257 + 82 * i32::try_from(index).unwrap();
            assert_eq!(*pos, Pt::new(x + 6, 5 + 3));
        }
        // The last four are the aces at the foundation bases.
        for (card, pos) in cascade.launches.iter().skip(48) {
            assert_eq!(card.rank, Rank::Ace);
            assert_eq!(pos.y, 5);
        }
        // Foundations hold one suit each; the first two kings differ.
        assert_ne!(cascade.launches[0].0.suit, cascade.launches[1].0.suit);
    }

    #[test]
    fn trajectory_fixture_seed_42() {
        // Locks the physics bit-for-bit. MS rand() from seed 42 rolls
        // 175 then 400: dx = 175 % 110 − 65 = 0, |0| < 15 forces
        // dx = −20; dy = 400 % 110 − 75 = −5. The first king launches
        // from (263, 8) and drifts left 2 px per step while gravity
        // (+3 tenths per step) slowly wins over the upward −5.
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        let mut trail = Vec::new();
        for _ in 0..200 {
            cascade.advance(CASCADE_STEP_MS);
            trail.extend(cascade.pending().iter().map(|(_, pos)| *pos));
        }
        assert_eq!(trail[0], Pt::new(263, 8));
        assert_eq!(
            &trail[1..6],
            &[
                Pt::new(261, 8),
                Pt::new(259, 8),
                Pt::new(257, 8),
                Pt::new(255, 8),
                Pt::new(253, 8),
            ]
        );
        // Step 6 is the first the accumulated dy moves the card down.
        assert_eq!(trail[6], Pt::new(251, 9));
        assert_eq!(trail[7], Pt::new(249, 10));
        // Step 47: the card dips below the floor (288) to y = 290 and
        // bounces — dy 136 flips to 136·8/−10 = −108.
        assert_eq!(trail[47], Pt::new(169, 290));
        assert_eq!(trail[48], Pt::new(167, 280));
        // Step 100, mid second arc.
        assert_eq!(trail[100], Pt::new(63, 140));
        // Step 120: second bounce, again dipping to 290.
        assert_eq!(trail[120], Pt::new(23, 290));
        // The card keeps drawing at negative x until its left edge
        // passes −71; the last drawn position is step 166.
        assert_eq!(trail[150], Pt::new(-37, 170));
        assert_eq!(trail[166], Pt::new(-69, 202));
        // Step 167 exits (x = −71 ≤ −cardW): not drawn; the second king
        // launches on the following step from foundation 1's top.
        assert_eq!(trail[167], Pt::new(339 + 6, 8));
    }

    #[test]
    fn cards_fall_bounce_with_80_percent_restitution_and_exit() {
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        let floor = cascade.floor_y;
        assert_eq!(floor, 384 - 96);
        let mut peak_dy = 0;
        let mut bounced = false;
        let mut exited = false;
        // Drive the first card until it exits: it must dip below the
        // floor, invert dy at −80%, and leave through a side.
        for _ in 0..10_000 {
            cascade.advance(CASCADE_STEP_MS);
            if let Some(flight) = cascade.flight {
                if flight.y > floor && flight.dy_tenths < 0 {
                    bounced = true;
                }
                peak_dy = peak_dy.max(flight.dy_tenths);
            }
            if cascade.cursor == 1 && cascade.flight.is_none() {
                exited = true;
                break;
            }
        }
        assert!(bounced, "the card must bounce off the floor");
        assert!(exited, "the card must exit through a side");
        assert!(peak_dy > 0, "gravity must have accelerated the card");
    }

    #[test]
    fn the_whole_cascade_completes_and_reports_done() {
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 7);
        assert!(!cascade.is_started());
        // Big chunks: the final card's exit lands mid-advance, so the
        // step loop also exercises its nothing-left-to-do exit.
        for _ in 0..6_000 {
            cascade.advance(CASCADE_STEP_MS * 1_000);
            if cascade.is_done() {
                break;
            }
        }
        assert!(cascade.is_done());
        assert!(cascade.is_started());
        // Once done, advancing yields no more trail.
        cascade.advance(CASCADE_STEP_MS * 3);
        assert!(cascade.pending().is_empty());
    }

    #[test]
    fn partial_foundations_launch_only_what_is_there() {
        // A freshly dealt game has empty foundations: nothing launches.
        let dealt = sol_engine::deal(
            sol_engine::Seed::new(1).unwrap(),
            sol_engine::GameConfig {
                draw_mode: sol_engine::DrawMode::One,
                scoring: sol_engine::ScoringMode::None,
                timed: false,
            },
        );
        let empty = Cascade::new(&dealt, &layout(), viewport(), 1);
        assert!(empty.launches.is_empty());
        let mut empty = empty;
        empty.advance(CASCADE_STEP_MS * 5);
        assert!(empty.is_done());
        assert!(!empty.is_started());

        // One card on a foundation: exactly one launch, from its slot.
        let mut one = dealt;
        sol_engine::evolve(
            &mut one,
            sol_engine::Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Foundation(2),
                count: 1,
            },
        );
        let cascade = Cascade::new(&one, &layout(), viewport(), 1);
        assert_eq!(cascade.launches.len(), 1);
        assert_eq!(cascade.launches[0].1, Pt::new(421, 5));
    }

    #[test]
    fn skip_ends_everything_instantly() {
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        cascade.advance(CASCADE_STEP_MS * 3);
        assert!(!cascade.is_done());
        cascade.skip();
        assert!(cascade.is_done());
        assert!(cascade.pending().is_empty());
    }

    #[test]
    fn dx_below_15_is_forced_left() {
        // Search a seed whose first dx roll lands in |dx| < 15 to pin the
        // forced −20; seed 0's first rand() is 38 → dx = 38 − 65 = −27,
        // so probe the sequence until the guard fires.
        let mut found = false;
        for seed in 0..200 {
            let mut probe = MsRand::new(seed & 0x7FFF);
            let dx = probe.next() % 110 - 65;
            if dx.abs() < 15 {
                let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), seed);
                cascade.advance(CASCADE_STEP_MS);
                let flight = cascade.flight.unwrap();
                assert_eq!(flight.dx_tenths, -20);
                found = true;
                break;
            }
        }
        assert!(
            found,
            "some seed below 200 must trigger the |dx| < 15 guard"
        );
    }

    #[test]
    fn accumulator_carries_partial_steps() {
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        cascade.advance(CASCADE_STEP_MS - 1);
        assert!(cascade.pending().is_empty());
        assert!(!cascade.is_started());
        cascade.advance(1);
        assert_eq!(cascade.pending().len(), 1);
        assert!(cascade.is_started());
        // One and a half steps: one step now, the remainder carries.
        cascade.advance(CASCADE_STEP_MS + CASCADE_STEP_MS / 2);
        assert_eq!(cascade.pending().len(), 1);
        // The carried half joins the next half into exactly one step.
        cascade.advance(CASCADE_STEP_MS / 2);
        assert_eq!(cascade.pending().len(), 1);
        // A fresh full step still yields exactly one.
        cascade.advance(CASCADE_STEP_MS);
        assert_eq!(cascade.pending().len(), 1);
    }

    #[test]
    fn a_boundary_dx_of_fifteen_is_kept_not_forced() {
        // Seed 114's first rolls are 410 and 20621: dx = 410 % 110 − 65 =
        // +15 — exactly on the |dx| < 15 boundary, so it is kept and the
        // card drifts right — and dy = 20621 % 110 − 75 = −24.
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 114);
        let mut trail = Vec::new();
        for _ in 0..6 {
            cascade.advance(CASCADE_STEP_MS);
            trail.extend(cascade.pending().iter().map(|(_, pos)| *pos));
        }
        assert_eq!(
            trail,
            vec![
                Pt::new(263, 8),
                Pt::new(264, 6),
                Pt::new(265, 4),
                Pt::new(266, 3),
                Pt::new(267, 2),
                Pt::new(268, 1),
            ]
        );
    }

    #[test]
    fn landing_exactly_on_the_floor_does_not_bounce() {
        // Hand-built flight: the step puts the card at y == floor (288)
        // exactly, still falling. The original bounces only strictly
        // below the floor, so the fall continues through it.
        let mut cascade = Cascade::new(&won_state(), &layout(), viewport(), 42);
        cascade.flight = Some(Flight {
            card: cascade.launches[0].0,
            x: 100,
            y: 280,
            dx_tenths: 0,
            dy_tenths: 80,
        });
        cascade.advance(CASCADE_STEP_MS);
        let after_first = cascade.flight.unwrap();
        assert_eq!(after_first.y, 288, "landed exactly on the floor");
        assert_eq!(after_first.dy_tenths, 83, "no bounce at y == floor");
        cascade.advance(CASCADE_STEP_MS);
        let after_second = cascade.flight.unwrap();
        assert_eq!(after_second.y, 296, "fell through, now below the floor");
        assert!(after_second.dy_tenths < 0, "and only now bounced");
    }
}
