#!/usr/bin/env python3
"""Independent reference generator for the sol-engine deal fixtures.

This script re-implements, in Python, the exact deterministic dealing
algorithm documented in `sol-engine` (`src/rng.rs`, `src/deal.rs`) — which is
the algorithm the original 16-bit Windows Solitaire uses, transcribed from
its machine code rather than from the Rust:

* The Microsoft C runtime generator. `srand(seed)` stores the seed into a
  32-bit state through a 16-bit `unsigned int`; `rand()` steps
  `state = state*214013 + 2531011` and returns `(state >> 16) & 0x7fff`.
* Initial deck order: the 52 cards by deck index, where
  `index = suit_index + 4 * (rank_value - 1)` and the suits are numbered
  clubs 0, diamonds 1, hearts 2, spades 3. So index 0 is the ace of clubs,
  3 the ace of spades, 4 the two of clubs, 51 the king of spades.
* Shuffle: five passes of the naive swap shuffle — five times over, for i
  from 0 up to 51, `swap(deck[i], deck[rand() % 52])`. Not Fisher-Yates.
* Layout: for round r in 0..7, for pile p in r..7, pop the card off the
  *back* of the deck (a pile's top card is its last) onto tableau p, face-up
  if p == r (each pile's last, top card), else face-down. The 24 cards left
  at the front are the stock; the last of them is drawn first.

The committed `seed_<n>.txt` files are this script's output. The Rust test
`tests/deal_fixtures.rs` renders `sol_engine::deal` results in the same
format and compares byte-for-byte, locking the deal for every platform,
forever. Regenerating must always be a no-op; if it is not, the engine's
determinism contract has been broken.
"""

from pathlib import Path

MASK32 = (1 << 32) - 1
MULTIPLIER = 214013
INCREMENT = 2531011

# Suit letters in deck-index order: clubs 0, diamonds 1, hearts 2, spades 3.
SUITS = "CDHS"
SEEDS = (0, 1, 42, 32767)


class MsRand:
    """The Microsoft C runtime rand()/srand() pair."""

    def __init__(self, seed: int) -> None:
        self.state = seed & 0xFFFF

    def next(self) -> int:
        self.state = (self.state * MULTIPLIER + INCREMENT) & MASK32
        return (self.state >> 16) & 0x7FFF


def card_name(index: int) -> str:
    """Card at deck index 0..51, e.g. 0 -> C1, 3 -> S1, 51 -> S13."""
    return f"{SUITS[index % 4]}{index // 4 + 1}"


def shuffled_deck(seed: int) -> list[str]:
    deck = [card_name(i) for i in range(52)]
    rng = MsRand(seed)
    for _ in range(5):
        for i in range(52):
            j = rng.next() % 52
            deck[i], deck[j] = deck[j], deck[i]
    return deck


def render_deal(seed: int) -> str:
    deck = shuffled_deck(seed)
    face_down: list[list[str]] = [[] for _ in range(7)]
    face_up: list[list[str]] = [[] for _ in range(7)]
    for start in range(7):
        for pile in range(start, 7):
            card = deck.pop()
            (face_up if pile == start else face_down)[pile].append(card)
    lines = [f"seed={seed}"]
    for pile in range(7):
        down = ",".join(face_down[pile])
        up = ",".join(face_up[pile])
        lines.append(f"tableau{pile} down={down} up={up}")
    # Draw order: the stock's last card is the first one a draw turns over.
    lines.append(f"stock={','.join(reversed(deck))}")
    return "\n".join(lines) + "\n"


def main() -> None:
    here = Path(__file__).parent
    for seed in SEEDS:
        text = render_deal(seed)
        (here / f"seed_{seed}.txt").write_text(text, encoding="ascii")
        print(text)


if __name__ == "__main__":
    main()
