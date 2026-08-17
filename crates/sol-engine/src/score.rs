//! Scoring constants — a named constant for every scoring rule.
//!
//! These constants are the single source of truth for score deltas, pass
//! thresholds, and timing rules; the exhaustive scoring tests pin their
//! observable behavior. If a value ever needs correcting against original
//! Win98 behavior, it is a one-line change here plus updated fixtures.

/// Standard: moving the top waste card onto a tableau pile.
pub(crate) const WASTE_TO_TABLEAU: i32 = 5;

/// Standard: moving the top waste card onto a foundation.
pub(crate) const WASTE_TO_FOUNDATION: i32 = 10;

/// Standard: moving a tableau card onto a foundation.
pub(crate) const TABLEAU_TO_FOUNDATION: i32 = 10;

/// Standard: a tableau card turned face-up (auto-flip).
pub(crate) const TABLEAU_FLIP: i32 = 5;

/// Standard: moving a foundation card back onto a tableau pile.
pub(crate) const FOUNDATION_TO_TABLEAU: i32 = -15;

/// Standard, Draw One: penalty per waste recycle after the free passes.
pub(crate) const RECYCLE_PENALTY_DRAW_ONE: i32 = -100;

/// Standard, Draw Three: penalty per waste recycle after the free passes.
pub(crate) const RECYCLE_PENALTY_DRAW_THREE: i32 = -20;

/// Standard, Draw One: passes without a recycle penalty ("each pass after
/// the 1st" costs).
pub(crate) const FREE_PASSES_DRAW_ONE: u32 = 1;

/// Standard, Draw Three: passes without a recycle penalty ("each pass after
/// the 4th" costs).
pub(crate) const FREE_PASSES_DRAW_THREE: u32 = 4;

/// Timed Standard: seconds per decay step.
pub(crate) const TIME_DECAY_INTERVAL_SECS: u32 = 10;

/// Timed Standard: score change per elapsed decay interval.
pub(crate) const TIME_DECAY_DELTA: i32 = -2;

/// Timed Standard: dividend of the win bonus `700_000 / seconds`.
pub(crate) const WIN_BONUS_NUMERATOR: u32 = 700_000;

/// Timed Standard: the win bonus applies only when the game took strictly
/// more than this many seconds.
pub(crate) const WIN_BONUS_MIN_ELAPSED_SECS: u32 = 30;

/// Vegas: buy-in charged at the deal.
pub(crate) const VEGAS_BUY_IN: i32 = -52;

/// Vegas: dollars per card moved onto a foundation.
pub(crate) const VEGAS_CARD_TO_FOUNDATION: i32 = 5;

/// Vegas: dollars per card leaving a foundation.
pub(crate) const VEGAS_CARD_OFF_FOUNDATION: i32 = -5;

/// Vegas, Draw One: total passes through the stock.
pub(crate) const VEGAS_PASS_LIMIT_DRAW_ONE: u32 = 1;

/// Vegas, Draw Three: total passes through the stock.
pub(crate) const VEGAS_PASS_LIMIT_DRAW_THREE: u32 = 3;
