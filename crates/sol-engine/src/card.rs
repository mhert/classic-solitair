//! Card value objects: [`Suit`], [`Color`], [`Rank`], and [`Card`].

use core::fmt;

use serde::{Deserialize, Serialize};

/// The four French suits.
///
/// The declaration order — clubs, diamonds, hearts, spades — is the
/// **documented suit numbering** used by [`Card::from_index`]: a card's
/// position in the unshuffled deck is `suit_index + 4 * (rank_value - 1)`.
/// This order is part of the determinism contract and must never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Suit {
    /// ♣ — black.
    Clubs,
    /// ♦ — red.
    Diamonds,
    /// ♥ — red.
    Hearts,
    /// ♠ — black.
    Spades,
}

impl Suit {
    /// All suits in the documented suit-numbering order.
    pub const ALL: [Self; 4] = [Self::Clubs, Self::Diamonds, Self::Hearts, Self::Spades];

    /// The color of this suit: spades and clubs are black, hearts and
    /// diamonds are red.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Spades | Self::Clubs => Color::Black,
            Self::Hearts | Self::Diamonds => Color::Red,
        }
    }

    /// This suit's position in the documented suit numbering: clubs 0,
    /// diamonds 1, hearts 2, spades 3.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::Clubs => 0,
            Self::Diamonds => 1,
            Self::Hearts => 2,
            Self::Spades => 3,
        }
    }

    /// Single-letter tag used by [`Card`]'s `Display` implementation.
    const fn letter(self) -> char {
        match self {
            Self::Spades => 'S',
            Self::Hearts => 'H',
            Self::Diamonds => 'D',
            Self::Clubs => 'C',
        }
    }
}

/// The color of a suit, used by the tableau's alternating-color rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    /// Spades and clubs.
    Black,
    /// Hearts and diamonds.
    Red,
}

/// Card ranks, ace low through king high.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Rank {
    /// 1
    Ace,
    /// 2
    Two,
    /// 3
    Three,
    /// 4
    Four,
    /// 5
    Five,
    /// 6
    Six,
    /// 7
    Seven,
    /// 8
    Eight,
    /// 9
    Nine,
    /// 10
    Ten,
    /// 11
    Jack,
    /// 12
    Queen,
    /// 13
    King,
}

impl Rank {
    /// All ranks ascending, ace first. A rank's position here is its
    /// [`value`](Self::value) minus one, which is the multiplier of four in
    /// the documented deck index (see [`Card::from_index`]).
    pub const ALL: [Self; 13] = [
        Self::Ace,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Seven,
        Self::Eight,
        Self::Nine,
        Self::Ten,
        Self::Jack,
        Self::Queen,
        Self::King,
    ];

    /// The numeric value of this rank: ace = 1 through king = 13.
    #[must_use]
    pub const fn value(self) -> u8 {
        match self {
            Self::Ace => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
            Self::Nine => 9,
            Self::Ten => 10,
            Self::Jack => 11,
            Self::Queen => 12,
            Self::King => 13,
        }
    }

    /// The next-higher rank, or `None` for the king.
    ///
    /// Foundations build `card.rank == top.rank.successor()`; a tableau card
    /// may rest on a card of `moved.rank.successor()`'s rank.
    #[must_use]
    pub const fn successor(self) -> Option<Self> {
        match self {
            Self::Ace => Some(Self::Two),
            Self::Two => Some(Self::Three),
            Self::Three => Some(Self::Four),
            Self::Four => Some(Self::Five),
            Self::Five => Some(Self::Six),
            Self::Six => Some(Self::Seven),
            Self::Seven => Some(Self::Eight),
            Self::Eight => Some(Self::Nine),
            Self::Nine => Some(Self::Ten),
            Self::Ten => Some(Self::Jack),
            Self::Jack => Some(Self::Queen),
            Self::Queen => Some(Self::King),
            Self::King => None,
        }
    }
}

/// A playing card: a [`Suit`] and a [`Rank`].
///
/// Every suit/rank combination is a valid card, so the fields are public.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Card {
    /// The card's suit.
    pub suit: Suit,
    /// The card's rank.
    pub rank: Rank,
}

impl Card {
    /// Creates a card from a suit and a rank.
    #[must_use]
    pub const fn new(suit: Suit, rank: Rank) -> Self {
        Self { suit, rank }
    }

    /// The card at `index` in the unshuffled deck.
    ///
    /// The deck runs ace to king with the four suits interleaved, so
    /// `index = suit.index() + 4 * (rank.value() - 1)`: index 0 is the ace of
    /// clubs, 1 the ace of diamonds, 3 the ace of spades, 4 the two of clubs,
    /// and 51 the king of spades. Only `0..52` names a card; larger values
    /// reduce modulo 52 so that every `u8` maps to one. This numbering is part
    /// of the determinism contract and must never change.
    #[must_use]
    pub const fn from_index(index: u8) -> Self {
        let index = index % 52;
        let suit = match index % 4 {
            0 => Suit::Clubs,
            1 => Suit::Diamonds,
            2 => Suit::Hearts,
            _ => Suit::Spades,
        };
        let rank = match index / 4 {
            0 => Rank::Ace,
            1 => Rank::Two,
            2 => Rank::Three,
            3 => Rank::Four,
            4 => Rank::Five,
            5 => Rank::Six,
            6 => Rank::Seven,
            7 => Rank::Eight,
            8 => Rank::Nine,
            9 => Rank::Ten,
            10 => Rank::Jack,
            11 => Rank::Queen,
            _ => Rank::King,
        };
        Self { suit, rank }
    }

    /// This card's position in the unshuffled deck — the inverse of
    /// [`Card::from_index`], always in `0..52`.
    #[must_use]
    pub const fn index(self) -> u8 {
        self.suit.index() + 4 * (self.rank.value() - 1)
    }

    /// The card's color, from its suit.
    #[must_use]
    pub const fn color(self) -> Color {
        self.suit.color()
    }
}

/// Formats as suit letter + rank value, e.g. `S1` (ace of spades) or `H13`
/// (king of hearts). This compact form is used by the committed deal
/// fixtures, so it is part of the determinism contract.
impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.suit.letter(), self.rank.value())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn suit_order_is_clubs_diamonds_hearts_spades() {
        assert_eq!(
            Suit::ALL,
            [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades]
        );
        for (position, suit) in Suit::ALL.iter().enumerate() {
            let expected = u8::try_from(position).unwrap();
            assert_eq!(suit.index(), expected, "suit {suit:?}");
        }
    }

    #[test]
    fn deck_index_interleaves_suits_ace_first() {
        assert_eq!(Card::from_index(0).to_string(), "C1");
        assert_eq!(Card::from_index(1).to_string(), "D1");
        assert_eq!(Card::from_index(2).to_string(), "H1");
        assert_eq!(Card::from_index(3).to_string(), "S1");
        assert_eq!(Card::from_index(4).to_string(), "C2");
        assert_eq!(Card::from_index(51).to_string(), "S13");
    }

    #[test]
    fn deck_index_round_trips_and_covers_every_card() {
        let mut seen = std::collections::HashSet::new();
        for index in 0..52_u8 {
            let card = Card::from_index(index);
            assert_eq!(card.index(), index, "index {index}");
            seen.insert(card);
        }
        assert_eq!(seen.len(), 52, "every index names a distinct card");
    }

    #[test]
    fn deck_index_reduces_beyond_the_deck() {
        assert_eq!(Card::from_index(52), Card::from_index(0));
        assert_eq!(Card::from_index(255), Card::from_index(255 % 52));
    }

    #[test]
    fn spades_and_clubs_are_black() {
        assert_eq!(Suit::Spades.color(), Color::Black);
        assert_eq!(Suit::Clubs.color(), Color::Black);
    }

    #[test]
    fn hearts_and_diamonds_are_red() {
        assert_eq!(Suit::Hearts.color(), Color::Red);
        assert_eq!(Suit::Diamonds.color(), Color::Red);
    }

    #[test]
    fn ranks_ascend_ace_through_king_with_values_1_to_13() {
        assert_eq!(Rank::ALL.len(), 13);
        for (position, rank) in Rank::ALL.iter().enumerate() {
            let expected = u8::try_from(position).unwrap() + 1;
            assert_eq!(rank.value(), expected, "rank {rank:?}");
        }
    }

    #[test]
    fn successor_climbs_one_rank_and_stops_at_king() {
        assert_eq!(Rank::Ace.successor(), Some(Rank::Two));
        assert_eq!(Rank::Seven.successor(), Some(Rank::Eight));
        assert_eq!(Rank::Queen.successor(), Some(Rank::King));
        assert_eq!(Rank::King.successor(), None);
        for (lower, higher) in Rank::ALL.iter().zip(Rank::ALL.iter().skip(1)) {
            assert_eq!(lower.successor(), Some(*higher));
        }
    }

    #[test]
    fn card_color_comes_from_its_suit() {
        assert_eq!(Card::new(Suit::Hearts, Rank::Ace).color(), Color::Red);
        assert_eq!(Card::new(Suit::Spades, Rank::King).color(), Color::Black);
    }

    #[test]
    fn card_displays_as_suit_letter_and_rank_value() {
        assert_eq!(Card::new(Suit::Spades, Rank::Ace).to_string(), "S1");
        assert_eq!(Card::new(Suit::Hearts, Rank::King).to_string(), "H13");
        assert_eq!(Card::new(Suit::Diamonds, Rank::Seven).to_string(), "D7");
        assert_eq!(Card::new(Suit::Clubs, Rank::Ten).to_string(), "C10");
    }

    #[test]
    fn card_serde_round_trips() {
        let card = Card::new(Suit::Diamonds, Rank::Queen);
        let json = serde_json::to_string(&card).unwrap();
        assert_eq!(serde_json::from_str::<Card>(&json).unwrap(), card);
    }
}
