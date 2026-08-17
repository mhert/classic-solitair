//! The deal animation: cards flying from the stock to the tableau.
//!
//! The original placed its deal instantly; the flying deal is this
//! project's deliberate modern addition. What *is* faithful is the deal
//! order it replays: seven rounds, round `r` dealing one card to each of
//! columns `r..7` left to right, the first card of each round landing
//! face-up — exactly how the original filled the table.

use crate::geometry::{Pt, saturate};

/// How long one card is in flight. The whole 28-card deal takes just over
/// a second, matching the brisk feel of the era's animated deals.
pub(crate) const DEAL_FLIGHT_MS: u32 = 40;

/// One card's flight: target column and row (index within the pile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Flight {
    /// Target tableau column, `0..7`.
    pub column: u8,
    /// Target row in that column, `0..=column` (the row equals the round
    /// it was dealt in).
    pub row: usize,
    /// Whether the card lands (and flies) face-up — true exactly for each
    /// column's last card, `row == column`.
    pub face_up: bool,
}

/// The running deal animation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DealAnimation {
    flights: Vec<Flight>,
    current: usize,
    in_flight_ms: u32,
}

impl DealAnimation {
    /// Starts the deal: 28 flights in the original's order.
    pub(crate) fn new() -> Self {
        let mut flights = Vec::with_capacity(28);
        for round in 0..7_u8 {
            for column in round..7 {
                flights.push(Flight {
                    column,
                    row: usize::from(round),
                    face_up: column == round,
                });
            }
        }
        Self {
            flights,
            current: 0,
            in_flight_ms: 0,
        }
    }

    /// Advances the animation by `dt_ms`.
    pub(crate) fn advance(&mut self, dt_ms: u32) {
        self.in_flight_ms = self.in_flight_ms.saturating_add(dt_ms);
        while !self.is_done() && self.in_flight_ms >= DEAL_FLIGHT_MS {
            self.in_flight_ms -= DEAL_FLIGHT_MS;
            self.current += 1;
        }
        if self.is_done() {
            self.in_flight_ms = 0;
        }
    }

    /// Whether every card has landed.
    pub(crate) fn is_done(&self) -> bool {
        self.current >= self.flights.len()
    }

    /// Lands everything instantly (any input skips the deal).
    pub(crate) fn skip(&mut self) {
        self.current = self.flights.len();
        self.in_flight_ms = 0;
    }

    /// The card currently in flight and how far along it is, or `None`
    /// when the deal is done.
    pub(crate) fn current_flight(&self) -> Option<(Flight, u32)> {
        self.flights
            .get(self.current)
            .map(|flight| (*flight, self.in_flight_ms.min(DEAL_FLIGHT_MS)))
    }

    /// How many cards have already landed in `column`.
    pub(crate) fn arrived_rows(&self, column: u8) -> usize {
        self.flights
            .iter()
            .take(self.current)
            .filter(|flight| flight.column == column)
            .count()
    }
}

/// Linear interpolation between two points at `elapsed / duration`.
pub(crate) fn lerp(from: Pt, to: Pt, elapsed_ms: u32, duration_ms: u32) -> Pt {
    if duration_ms == 0 {
        return to;
    }
    let t = i64::from(elapsed_ms.min(duration_ms));
    let d = i64::from(duration_ms);
    Pt::new(
        saturate(i64::from(from.x) + (i64::from(to.x) - i64::from(from.x)) * t / d),
        saturate(i64::from(from.y) + (i64::from(to.y) - i64::from(from.y)) * t / d),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_deal_replays_the_original_order() {
        let deal = DealAnimation::new();
        assert_eq!(deal.flights.len(), 28);
        // Round 0: columns 0..7, the first face-up.
        assert_eq!(
            deal.flights.first(),
            Some(&Flight {
                column: 0,
                row: 0,
                face_up: true
            })
        );
        assert_eq!(
            deal.flights.get(1),
            Some(&Flight {
                column: 1,
                row: 0,
                face_up: false
            })
        );
        // Round 1 starts at flight 7 with column 1's last card, face-up.
        assert_eq!(
            deal.flights.get(7),
            Some(&Flight {
                column: 1,
                row: 1,
                face_up: true
            })
        );
        // The last flight is column 6's seventh card, face-up.
        assert_eq!(
            deal.flights.last(),
            Some(&Flight {
                column: 6,
                row: 6,
                face_up: true
            })
        );
        // Every column receives column+1 cards.
        for column in 0..7_u8 {
            let count = deal
                .flights
                .iter()
                .filter(|flight| flight.column == column)
                .count();
            assert_eq!(count, usize::from(column) + 1);
        }
    }

    #[test]
    fn advance_lands_cards_at_the_flight_cadence() {
        let mut deal = DealAnimation::new();
        assert_eq!(deal.current_flight().unwrap().0.column, 0);
        deal.advance(DEAL_FLIGHT_MS - 1);
        assert_eq!(deal.current_flight().unwrap().1, DEAL_FLIGHT_MS - 1);
        assert_eq!(deal.arrived_rows(0), 0);
        deal.advance(1);
        assert_eq!(deal.arrived_rows(0), 1);
        assert_eq!(deal.current_flight().unwrap().0.column, 1);
        // A big step lands several flights at once.
        deal.advance(DEAL_FLIGHT_MS * 6);
        assert_eq!(deal.arrived_rows(1), 1);
        assert_eq!(deal.current_flight().unwrap().0.row, 1);
        assert!(!deal.is_done());
    }

    #[test]
    fn the_deal_finishes_and_skips() {
        let mut deal = DealAnimation::new();
        deal.advance(DEAL_FLIGHT_MS * 28);
        assert!(deal.is_done());
        assert_eq!(deal.current_flight(), None);
        assert_eq!(deal.arrived_rows(6), 7);

        let mut deal = DealAnimation::new();
        deal.advance(5);
        deal.skip();
        assert!(deal.is_done());
        assert_eq!(deal.arrived_rows(3), 4);
    }

    #[test]
    fn lerp_interpolates_and_clamps() {
        let from = Pt::new(0, 100);
        let to = Pt::new(100, 0);
        assert_eq!(lerp(from, to, 0, 40), from);
        assert_eq!(lerp(from, to, 20, 40), Pt::new(50, 50));
        assert_eq!(lerp(from, to, 40, 40), to);
        assert_eq!(lerp(from, to, 99, 40), to);
        assert_eq!(lerp(from, to, 5, 0), to);
    }
}
