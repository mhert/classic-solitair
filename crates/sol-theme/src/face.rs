//! [`FaceSuit`] and [`FaceRank`]: canonical identifiers for one of a
//! theme's 52 card faces, and the canonical order they are validated,
//! iterated, and named in.

use core::fmt;

/// One of the four card suits, spelled and ordered exactly as the engine's
/// deal does: spades, hearts, diamonds, clubs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceSuit {
    /// ♠
    Spades,
    /// ♥
    Hearts,
    /// ♦
    Diamonds,
    /// ♣
    Clubs,
}

impl FaceSuit {
    /// All four suits, in canonical order.
    pub const ALL: [Self; 4] = [Self::Spades, Self::Hearts, Self::Diamonds, Self::Clubs];

    /// The lowercase file-stem name, e.g. `"spades"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spades => "spades",
            Self::Hearts => "hearts",
            Self::Diamonds => "diamonds",
            Self::Clubs => "clubs",
        }
    }

    /// The canonical face-image file stem for this suit and `rank`, e.g.
    /// `"spades_01"`, `"clubs_13"`.
    #[must_use]
    pub fn stem(self, rank: FaceRank) -> String {
        format!("{}_{:02}", self.as_str(), rank.get())
    }
}

impl fmt::Display for FaceSuit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A card rank: `1..=13` (1 = Ace … 13 = King).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceRank(u8);

impl FaceRank {
    /// The raw rank number, `1..=13`.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Builds a rank without validating `raw`. Only used internally, where
    /// `raw` is already statically known to be `1..=13` (the literal
    /// `1..=13` loop in [`canonical_faces`]) — [`FaceRank::try_from`] stays
    /// the only constructor reachable from outside this crate.
    const fn new_unchecked(raw: u8) -> Self {
        Self(raw)
    }
}

/// [`FaceRank`] could not be built: `raw` was outside `1..=13`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid face rank {raw}: must be 1..=13")]
pub struct FaceRankError {
    raw: u8,
}

impl TryFrom<u8> for FaceRank {
    type Error = FaceRankError;

    /// # Errors
    ///
    /// Returns [`FaceRankError`] unless `raw` is `1..=13`.
    fn try_from(raw: u8) -> Result<Self, Self::Error> {
        if (1..=13).contains(&raw) {
            Ok(Self::new_unchecked(raw))
        } else {
            Err(FaceRankError { raw })
        }
    }
}

/// The 52 `(suit, rank)` pairs in canonical order: spades, hearts,
/// diamonds, clubs, each rank ascending from Ace to King — the order
/// every validation, iteration, and sheet-layout rule in this crate
/// follows.
pub fn canonical_faces() -> impl Iterator<Item = (FaceSuit, FaceRank)> {
    FaceSuit::ALL
        .into_iter()
        .flat_map(|suit| (1..=13_u8).map(move |rank| (suit, FaceRank::new_unchecked(rank))))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn as_str_matches_every_suit() {
        assert_eq!(FaceSuit::Spades.as_str(), "spades");
        assert_eq!(FaceSuit::Hearts.as_str(), "hearts");
        assert_eq!(FaceSuit::Diamonds.as_str(), "diamonds");
        assert_eq!(FaceSuit::Clubs.as_str(), "clubs");
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(FaceSuit::Spades.to_string(), "spades");
    }

    #[test]
    fn stem_renders_a_two_digit_rank() {
        assert_eq!(
            FaceSuit::Spades.stem(FaceRank::try_from(1).unwrap()),
            "spades_01"
        );
        assert_eq!(
            FaceSuit::Clubs.stem(FaceRank::try_from(13).unwrap()),
            "clubs_13"
        );
    }

    #[test]
    fn rank_accepts_the_full_1_to_13_range() {
        for raw in 1..=13 {
            assert_eq!(FaceRank::try_from(raw).unwrap().get(), raw);
        }
    }

    #[test]
    fn rank_rejects_zero() {
        let error = FaceRank::try_from(0).unwrap_err();
        assert_eq!(error, FaceRankError { raw: 0 });
    }

    #[test]
    fn rank_rejects_above_thirteen() {
        assert!(FaceRank::try_from(14).is_err());
        assert!(FaceRank::try_from(255).is_err());
    }

    #[test]
    fn rank_error_message_names_the_raw_value() {
        let error = FaceRank::try_from(99).unwrap_err();
        assert!(error.to_string().contains("99"));
    }

    #[test]
    fn canonical_faces_yields_exactly_52_pairs_in_order() {
        let pairs: Vec<(FaceSuit, FaceRank)> = canonical_faces().collect();
        assert_eq!(pairs.len(), 52);

        let (first_suit, first_rank) = pairs.first().copied().unwrap();
        assert_eq!(first_suit, FaceSuit::Spades);
        assert_eq!(first_rank.get(), 1);

        let (last_suit, last_rank) = pairs.last().copied().unwrap();
        assert_eq!(last_suit, FaceSuit::Clubs);
        assert_eq!(last_rank.get(), 13);

        let stems: Vec<String> = pairs.iter().map(|(s, r)| s.stem(*r)).collect();
        assert_eq!(stems.first().map(String::as_str), Some("spades_01"));
        assert_eq!(stems.get(12).map(String::as_str), Some("spades_13"));
        assert_eq!(stems.get(13).map(String::as_str), Some("hearts_01"));
        assert_eq!(stems.last().map(String::as_str), Some("clubs_13"));
    }

    #[test]
    fn canonical_faces_suits_run_spades_hearts_diamonds_clubs() {
        let suits: Vec<FaceSuit> = canonical_faces().map(|(suit, _)| suit).collect();
        let mut expected = Vec::new();
        for suit in FaceSuit::ALL {
            expected.extend(std::iter::repeat_n(suit, 13));
        }
        assert_eq!(suits, expected);
    }
}
