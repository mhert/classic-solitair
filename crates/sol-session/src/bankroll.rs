//! [`Bankroll`]: the Vegas cumulative bankroll — committed on win or on
//! redeal, provisional during play. There is always a deal on the table,
//! so redealing is the only way to leave a game unfinished and there is no
//! separate abandon path. See [`crate::Session`] for the commit semantics.

use serde::{Deserialize, Serialize};

/// The Vegas cumulative bankroll, in whole dollars.
///
/// A newtype over `i64`. [`crate::Session`] folds each game's net Vegas
/// result into the bankroll with saturating arithmetic: a commit that would
/// overflow clamps at [`i64::MAX`] (or [`i64::MIN`]) instead of panicking or
/// wrapping.
///
/// ```
/// use sol_session::Bankroll;
///
/// assert_eq!(Bankroll::default().dollars(), 0);
/// assert_eq!(Bankroll::from(-52_i64).dollars(), -52);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bankroll(i64);

impl Bankroll {
    /// The bankroll's value in whole dollars.
    #[must_use]
    pub const fn dollars(self) -> i64 {
        self.0
    }
}

impl From<i64> for Bankroll {
    fn from(dollars: i64) -> Self {
        Self(dollars)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn default_is_zero_dollars() {
        assert_eq!(Bankroll::default().dollars(), 0);
    }

    #[test]
    fn from_i64_round_trips_through_dollars() {
        assert_eq!(Bankroll::from(100_i64).dollars(), 100);
        assert_eq!(Bankroll::from(-52_i64).dollars(), -52);
        assert_eq!(Bankroll::from(0_i64), Bankroll::default());
    }

    #[test]
    fn serializes_as_a_transparent_json_number() {
        let bankroll = Bankroll::from(42_i64);
        assert_eq!(serde_json::to_string(&bankroll).unwrap(), "42");
        assert_eq!(serde_json::from_str::<Bankroll>("42").unwrap(), bankroll);

        let negative = Bankroll::from(-52_i64);
        assert_eq!(serde_json::to_string(&negative).unwrap(), "-52");
    }
}
