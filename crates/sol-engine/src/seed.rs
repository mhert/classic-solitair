//! Deal seed: [`Seed`] selects a shuffle deterministically.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// The seed of a game's deal.
///
/// Seeds run `0..=32767`, the range the original Windows Solitaire reaches:
/// it seeds its shuffle with the low 15 bits of the millisecond tick count,
/// so those 32,768 values are exactly the boards it can deal. Every one of
/// them produces a distinct deal.
///
/// The same seed produces the same deal on every platform, forever (see
/// [`crate::deal`]). The seed is shown to the player ("Select Game…", status
/// bar), so it formats as its plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Seed(u16);

/// Why a value could not become a [`Seed`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SeedError {
    /// The value is above [`Seed::MAX`].
    #[error("seed {0} is out of range: seeds run 0..={max}", max = Seed::MAX)]
    OutOfRange(u32),
    /// The text is not a decimal number.
    #[error("`{0}` is not a number: seeds run 0..={max}", max = Seed::MAX)]
    NotANumber(String),
}

impl Seed {
    /// The largest valid seed.
    pub const MAX: u16 = 32_767;

    /// The number of distinct seeds, and so of distinct deals.
    pub const COUNT: u32 = Self::MAX as u32 + 1;

    /// The raw seed value, always in `0..=`[`Seed::MAX`].
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// Creates a seed, or `None` if `value` is above [`Seed::MAX`].
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value > Self::MAX {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Reduces any value into the seed range by taking it modulo
    /// [`Seed::COUNT`].
    ///
    /// This is how entropy becomes a seed; prefer [`Seed::new`] or
    /// [`TryFrom`] wherever a player typed the number, so an out-of-range
    /// one is reported rather than silently folded.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // the modulo lands below 2^15
    pub const fn from_entropy(value: u64) -> Self {
        Self((value % Self::COUNT as u64) as u16)
    }
}

impl TryFrom<u32> for Seed {
    type Error = SeedError;

    /// # Errors
    ///
    /// [`SeedError::OutOfRange`] if `raw` is above [`Seed::MAX`].
    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        u16::try_from(raw)
            .ok()
            .and_then(Self::new)
            .ok_or(SeedError::OutOfRange(raw))
    }
}

impl From<Seed> for u16 {
    fn from(seed: Seed) -> Self {
        seed.get()
    }
}

impl From<Seed> for u32 {
    fn from(seed: Seed) -> Self {
        Self::from(seed.get())
    }
}

impl FromStr for Seed {
    type Err = SeedError;

    /// # Errors
    ///
    /// [`SeedError::NotANumber`] if `text` is not a decimal number, or
    /// [`SeedError::OutOfRange`] if it names a value above [`Seed::MAX`].
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let raw: u32 = text
            .parse()
            .map_err(|_| SeedError::NotANumber(text.to_owned()))?;
        Self::try_from(raw)
    }
}

/// Rejects out-of-range seeds on the way in, so a save naming a deal this
/// engine cannot produce fails to load instead of dealing a different board.
impl<'de> Deserialize<'de> for Seed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn seed_round_trips_through_its_number() {
        let seed = Seed::try_from(42_u32).unwrap();
        assert_eq!(seed.get(), 42);
        assert_eq!(u16::from(seed), 42);
        assert_eq!(u32::from(seed), 42);
    }

    #[test]
    fn seed_accepts_the_whole_documented_range() {
        assert_eq!(Seed::new(0).unwrap().get(), 0);
        assert_eq!(Seed::new(Seed::MAX).unwrap().get(), 32_767);
        assert_eq!(Seed::COUNT, 32_768);
    }

    #[test]
    fn seed_rejects_values_above_the_range() {
        assert_eq!(Seed::new(32_768), None);
        assert_eq!(
            Seed::try_from(32_768_u32),
            Err(SeedError::OutOfRange(32_768))
        );
        assert_eq!(
            Seed::try_from(u32::MAX),
            Err(SeedError::OutOfRange(u32::MAX))
        );
    }

    #[test]
    fn seed_parses_from_text_and_reports_why_it_cannot() {
        assert_eq!("0".parse::<Seed>().unwrap().get(), 0);
        assert_eq!("32767".parse::<Seed>().unwrap().get(), 32_767);
        assert_eq!("32768".parse::<Seed>(), Err(SeedError::OutOfRange(32_768)));
        assert_eq!(
            "twelve".parse::<Seed>(),
            Err(SeedError::NotANumber("twelve".to_owned()))
        );
        assert_eq!(
            "-1".parse::<Seed>(),
            Err(SeedError::NotANumber("-1".to_owned()))
        );
    }

    #[test]
    fn entropy_folds_into_the_seed_range() {
        assert_eq!(Seed::from_entropy(0).get(), 0);
        assert_eq!(Seed::from_entropy(32_767).get(), 32_767);
        assert_eq!(Seed::from_entropy(32_768).get(), 0);
        assert_eq!(Seed::from_entropy(u64::MAX).get(), 32_767);
    }

    #[test]
    fn seed_displays_as_its_number() {
        assert_eq!(Seed::new(0).unwrap().to_string(), "0");
        assert_eq!(Seed::new(Seed::MAX).unwrap().to_string(), "32767");
    }

    #[test]
    fn seed_error_messages_name_the_range() {
        assert_eq!(
            SeedError::OutOfRange(99_999).to_string(),
            "seed 99999 is out of range: seeds run 0..=32767"
        );
        assert_eq!(
            SeedError::NotANumber("x".to_owned()).to_string(),
            "`x` is not a number: seeds run 0..=32767"
        );
    }

    #[test]
    fn seed_serde_is_transparent_and_validates() {
        let seed = Seed::new(7).unwrap();
        assert_eq!(serde_json::to_string(&seed).unwrap(), "7");
        assert_eq!(serde_json::from_str::<Seed>("7").unwrap(), seed);
        assert!(serde_json::from_str::<Seed>("32768").is_err());
        assert!(serde_json::from_str::<Seed>("-1").is_err());
    }
}
