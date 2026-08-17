#!/usr/bin/env python3
"""Generator for classic-solitair's in-tree, CC0-1.0, vector default theme.

Regenerate with:

    python3 themes/default/generate.py

Deterministic by construction: no randomness, no timestamps, no environment
reads — every value here is a literal or computed from literals, so running
this script twice produces byte-identical output. That property is part of
the theme's contract: `crates/soltool/tests/default_theme.rs` loads and
validates the committed output on every `cargo test`, and CI runs
`soltool validate` against it directly.

This script writes every file under `themes/default/` except `LICENSE`
(the CC0-1.0 legal text, hand-authored once — it does not change) and this
script itself. Precedent for a committed, deterministic generator:
`crates/sol-engine/tests/fixtures/generate.py`.

Design:

  * 52 individual `cards/<suit>_<NN>.svg` faces (canonical naming) plus two
    `backs/*.svg`: one static, one 2-frame horizontal strip (142x96,
    dogfooding the strip math for a vector theme).
  * White face, thin dark rounded-corner border. A rank+suit corner index
    sits top-left; the bottom-right copy is the *same* markup wrapped in a
    180-degree rotation about the card center, so the two corners can never
    drift apart from each other.
  * Center field: the standard columnar pip arrangement for A and 2-10.
    Pips in the lower half are rotated 180 degrees in place, exactly like a
    real deck (a spade's stem points up there, a heart's point does too).
  * Court cards (J/Q/K) get an original geometric motif instead of copied
    art: a double-line frame, a large rank glyph, and two suit pips — not a
    likeness of any existing deck's face-card illustrations.
  * Suit pips (heart/spade/club/diamond) are built only from circles and
    polygons, never freehand bezier curves — each shape is a small cluster
    of overlapping primitives in one fill color, which reads as one clean
    silhouette in any conformant SVG renderer without needing hand-tuned
    curve tangents.
  * NO <text> ANYWHERE. `sol-theme`'s prober and the resvg raster
    pipeline must render this theme identically on every machine, forever,
    with zero installed-font dependencies — a `<text>` element's shape
    depends on whichever font happens to be resolved at render time, which
    is exactly the non-determinism this theme cannot have. Every rank
    glyph (A, 2-9, 10, J, Q, K) is instead drawn from a small hand-authored
    stroke font (`_GLYPH_SEGMENTS`): each character is a fixed list of
    straight line segments on a normalized grid, emitted as plain `<path>`
    `M`/`L` commands with round caps.
  * Backs: a repeating diamond-lattice pattern in two colors on a colored
    ground. The animated strip's two frames are otherwise identical except
    for one accent badge that rotates 45 degrees between frames, so the
    frame change is visually unmistakable.
  * Lean SVGs throughout: no scripts, no external references, no CSS
    classes, no filters — plain shapes, fills, strokes, and transforms
    only, so every file sails through `sol-theme`'s prober and later resvg.

Self-check: after writing, every emitted file is re-parsed (`xml.etree` for
the SVGs, `tomllib` for `theme.toml`), and every face's pip count is
compared against the standard count for its rank — so a malformed-markup
or wrong-pip-count regression in this script fails loudly, in the same run
that introduced it, instead of silently shipping.
"""

from __future__ import annotations

import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

# =========================================================================
# Card geometry
# =========================================================================

CARD_W = 71
CARD_H = 96
CARD_CX = CARD_W / 2  # 35.5
CARD_CY = CARD_H / 2  # 48.0

INK = "#000000"
RED = "#d40000"

SUITS = ("spades", "hearts", "diamonds", "clubs")
RED_SUITS = frozenset({"hearts", "diamonds"})
RANKS = range(1, 14)


def suit_color(suit: str) -> str:
    """Red suits render `RED`; spades/clubs render `INK`."""
    return RED if suit in RED_SUITS else INK


def n(value: float) -> str:
    """Formats a coordinate deterministically and compactly: a bare integer
    when `value` is integer-valued, else 2 decimal places with trailing
    zeros (and a bare trailing '.') stripped. Every generated number goes
    through this so the output never carries binary-float noise like
    `3.1000000000000005`."""
    rounded = round(value, 2)
    if rounded == int(rounded):
        return str(int(rounded))
    text = f"{rounded:.2f}".rstrip("0").rstrip(".")
    return text


# =========================================================================
# Stroke font — 14 characters (0-9, A, J, Q, K) on an 8-wide x 14-tall grid.
#
# Digits reuse the standard 7-segment layout (segments a..g); K adds two
# diagonals off a full-height left stroke, and Q extends the digit-9 shape
# with one extra tail diagonal — the minimum needed to make all 14 glyphs
# this theme uses (ranks A,2-9,10,J,Q,K) unambiguous at a glance. This is a
# small original glyph set, not a reproduction of any existing typeface.
# =========================================================================

_GW, _GH = 8.0, 14.0
_TL, _TR = (0.0, 0.0), (_GW, 0.0)
_ML, _MR = (0.0, _GH / 2), (_GW, _GH / 2)
_BL, _BR = (0.0, _GH), (_GW, _GH)
_MM = (_GW / 2, _GH / 2)

Point = tuple[float, float]
Segment = tuple[Point, Point]

_GLYPH_SEGMENTS: dict[str, Segment] = {
    "a": (_TL, _TR),  # top
    "b": (_TR, _MR),  # upper right
    "c": (_MR, _BR),  # lower right
    "d": (_BL, _BR),  # bottom
    "e": (_ML, _BL),  # lower left
    "f": (_TL, _ML),  # upper left
    "g": (_ML, _MR),  # middle
    "k1": (_ML, _TR),  # K: middle-left to top-right diagonal
    "k2": (_ML, _BR),  # K: middle-left to bottom-right diagonal
    "qt": (_MM, _BR),  # Q: center to bottom-right tail diagonal
    "1f": ((_GW * 0.3, _GH * 0.18), _TR),  # 1: small top serif flag
}

_GLYPHS: dict[str, tuple[str, ...]] = {
    "0": ("a", "b", "c", "d", "e", "f"),
    "1": ("1f", "b", "c"),
    "2": ("a", "b", "g", "e", "d"),
    "3": ("a", "b", "g", "c", "d"),
    "4": ("f", "g", "b", "c"),
    "5": ("a", "f", "g", "c", "d"),
    "6": ("a", "f", "g", "e", "c", "d"),
    "7": ("a", "b", "c"),
    "8": ("a", "b", "c", "d", "e", "f", "g"),
    "9": ("a", "b", "c", "d", "f", "g"),
    "A": ("a", "b", "c", "e", "f", "g"),
    "J": ("b", "c", "d", "e"),
    "Q": ("a", "b", "c", "d", "f", "g", "qt"),
    "K": ("f", "e", "k1", "k2"),
}

# The glyph string(s) that spell each rank, e.g. rank 10 is two characters.
RANK_CHARS: dict[int, tuple[str, ...]] = {
    1: ("A",),
    2: ("2",),
    3: ("3",),
    4: ("4",),
    5: ("5",),
    6: ("6",),
    7: ("7",),
    8: ("8",),
    9: ("9",),
    10: ("1", "0"),
    11: ("J",),
    12: ("Q",),
    13: ("K",),
}


def glyph_width(scale: float, count: int, gap: float) -> float:
    """The total width of `count` glyphs at `scale`, `gap` apart."""
    return _GW * scale * count + gap * max(0, count - 1)


def _glyph_path(ch: str, x: float, y: float, scale: float, stroke: float, color: str) -> str:
    """One `<path>` drawing glyph `ch`'s strokes, top-left of its
    `_GW * scale` x `_GH * scale` box anchored at `(x, y)`."""
    commands = []
    for key in _GLYPHS[ch]:
        (x1, y1), (x2, y2) = _GLYPH_SEGMENTS[key]
        commands.append(
            f"M{n(x + x1 * scale)} {n(y + y1 * scale)} "
            f"L{n(x + x2 * scale)} {n(y + y2 * scale)}"
        )
    d = " ".join(commands)
    return (
        f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{n(stroke)}" '
        f'stroke-linecap="round" stroke-linejoin="round"/>'
    )


def rank_glyphs(
    rank: int, x: float, y: float, scale: float, stroke: float, color: str, gap: float = 0.6
) -> str:
    """Renders `rank`'s glyph(s) left to right, top-left of the whole block
    anchored at `(x, y)`."""
    cursor = x
    paths = []
    for ch in RANK_CHARS[rank]:
        paths.append(_glyph_path(ch, cursor, y, scale, stroke, color))
        cursor += _GW * scale + gap
    return "\n".join(paths)


# =========================================================================
# Suit pips — circles and polygons only (deliberately no freehand bezier
# curves): each pip is a small cluster of overlapping same-color primitives
# that reads as one clean silhouette. Every shape is centered at its own
# local origin and sized by one `size` parameter, so `suit_pip` can place
# and (for the lower half of the field / the second court pip) rotate any
# of them uniformly.
# =========================================================================


def _polygon(points: list[Point], color: str) -> str:
    pts = " ".join(f"{n(x)},{n(y)}" for x, y in points)
    return f'<polygon points="{pts}" fill="{color}"/>'


def _circle(cx: float, cy: float, r: float, color: str) -> str:
    return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{color}"/>'


def _diamond_pip(size: float, color: str) -> str:
    h = size
    w = size * 0.82
    return _polygon([(0, -h / 2), (w / 2, 0), (0, h / 2), (-w / 2, 0)], color)


def _heart_pip(size: float, color: str) -> str:
    r = size * 0.24
    o = size * 0.20
    point_y = size * 0.5
    return "\n".join(
        [
            _circle(-r, -o, r, color),
            _circle(r, -o, r, color),
            _polygon([(-r * 1.35, -o + r * 0.2), (r * 1.35, -o + r * 0.2), (0, point_y)], color),
        ]
    )


def _spade_pip(size: float, color: str) -> str:
    r = size * 0.24
    o = size * 0.20
    point_y = size * 0.46
    stem_top = o + r * 0.9
    stem_bottom = size * 0.6
    return "\n".join(
        [
            _circle(-r, o, r, color),
            _circle(r, o, r, color),
            _polygon([(-r * 1.35, o - r * 0.2), (r * 1.35, o - r * 0.2), (0, -point_y)], color),
            _polygon([(-r * 0.38, stem_top), (r * 0.38, stem_top), (0, stem_bottom)], color),
        ]
    )


def _club_pip(size: float, color: str) -> str:
    r = size * 0.23
    top = (0.0, -size * 0.32)
    left = (-r, -size * 0.02)
    right = (r, -size * 0.02)
    stem_top = size * 0.14
    stem_bottom = size * 0.46
    return "\n".join(
        [
            _circle(top[0], top[1], r, color),
            _circle(left[0], left[1], r, color),
            _circle(right[0], right[1], r, color),
            _polygon([(-r * 0.42, stem_top), (r * 0.42, stem_top), (0, stem_bottom)], color),
        ]
    )


_PIP_BUILDERS = {
    "spades": _spade_pip,
    "hearts": _heart_pip,
    "diamonds": _diamond_pip,
    "clubs": _club_pip,
}


def suit_pip(suit: str, cx: float, cy: float, size: float, rotate: bool = False) -> str:
    """A suit pip of nominal `size`, centered at `(cx, cy)`. `rotate=True`
    turns it 180 degrees in place: the classic lower-half pip mirroring,
    reused as-is for the court motif's second pip."""
    inner = _PIP_BUILDERS[suit](size, suit_color(suit))
    transform = f"translate({n(cx)} {n(cy)})"
    if rotate:
        transform += " rotate(180)"
    return f'<g transform="{transform}">\n{inner}\n</g>'


# =========================================================================
# Card face assembly
# =========================================================================

BORDER_WIDTH = 1.3

CORNER_GLYPH_HEIGHT = 8.0
CORNER_GLYPH_SCALE = CORNER_GLYPH_HEIGHT / _GH
CORNER_GLYPH_STROKE = 1.1
CORNER_PIP_SIZE = 6.0
CORNER_X = 3.5
CORNER_Y = 3.5
# Must exceed CORNER_GLYPH_STROKE so two adjacent glyphs' strokes (e.g. "1"
# and "0" in rank 10's corner index) never touch, let alone overlap.
CORNER_GLYPH_GAP = 1.4

ACE_PIP_SIZE = 26.0

# The standard columnar field: 3 columns (left/center/right) x 5 rows
# (top/upper-middle/middle/lower-middle/bottom), evenly spaced and centered
# on the card so the whole field is point-symmetric about (CARD_CX, CARD_CY)
# — required for the lower-half-rotated pips to land exactly where the
# upper half's do, mirrored.
COL_L, COL_C, COL_R = 22.0, CARD_CX, 49.0
ROW_T, ROW_UM, ROW_M, ROW_LM, ROW_B = 22.0, 35.0, 48.0, 61.0, 74.0
ROW_TC = (ROW_T + ROW_UM) / 2
ROW_BC = (ROW_LM + ROW_B) / 2
FIELD_PIP_SIZE = 10.5

# One entry per pip: (column, row, rotate). `rotate=True` for every pip in
# the lower half (row LM or B), matching a real deck.
PipPlacement = tuple[float, float, bool]
_NUMBER_LAYOUT: dict[int, tuple[PipPlacement, ...]] = {
    2: ((COL_C, ROW_T, False), (COL_C, ROW_B, True)),
    3: ((COL_C, ROW_T, False), (COL_C, ROW_M, False), (COL_C, ROW_B, True)),
    4: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    5: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_C, ROW_M, False),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    6: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_L, ROW_M, False),
        (COL_R, ROW_M, False),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    7: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_C, ROW_UM, False),
        (COL_L, ROW_M, False),
        (COL_R, ROW_M, False),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    8: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_C, ROW_UM, False),
        (COL_L, ROW_M, False),
        (COL_R, ROW_M, False),
        (COL_C, ROW_LM, True),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    9: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_L, ROW_UM, False),
        (COL_R, ROW_UM, False),
        (COL_C, ROW_M, False),
        (COL_L, ROW_LM, True),
        (COL_R, ROW_LM, True),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
    10: (
        (COL_L, ROW_T, False),
        (COL_R, ROW_T, False),
        (COL_C, ROW_TC, False),
        (COL_L, ROW_UM, False),
        (COL_R, ROW_UM, False),
        (COL_L, ROW_LM, True),
        (COL_R, ROW_LM, True),
        (COL_C, ROW_BC, True),
        (COL_L, ROW_B, True),
        (COL_R, ROW_B, True),
    ),
}

# Court frame: outer/inner double-line panel, inset far enough from the top
# -left corner index that the two never touch (checked by hand against
# CORNER_* above), and symmetric about the card center on both axes.
COURT_FRAME_OUTER = {"x": 10.0, "y": 21.0, "w": 51.0, "h": 54.0, "rx": 3.0}
COURT_FRAME_INNER = {"x": 13.0, "y": 24.0, "w": 45.0, "h": 48.0, "rx": 2.0}
COURT_GLYPH_HEIGHT = 26.0
COURT_PIP_SIZE = 8.0
COURT_PIP_TOP_Y = 28.7
COURT_PIP_BOTTOM_Y = 2 * CARD_CY - COURT_PIP_TOP_Y


def expected_pip_count(rank: int) -> int:
    """The standard number of suit-pip shapes rank `rank` is drawn with:
    1 for the Ace's single large pip, the standard count for 2-10, 2
    (flanking the rank glyph) for every court card."""
    if rank == 1:
        return 1
    if rank in _NUMBER_LAYOUT:
        return len(_NUMBER_LAYOUT[rank])
    return 2


def _card_frame() -> str:
    x = y = 1.0
    w, h = CARD_W - 2 * x, CARD_H - 2 * y
    return (
        f'<rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}" rx="4" '
        f'fill="#ffffff" stroke="{INK}" stroke-width="{n(BORDER_WIDTH)}"/>'
    )


def _corner_index(suit: str, rank: int) -> str:
    color = suit_color(suit)
    chars = RANK_CHARS[rank]
    glyphs = rank_glyphs(
        rank, CORNER_X, CORNER_Y, CORNER_GLYPH_SCALE, CORNER_GLYPH_STROKE, color, CORNER_GLYPH_GAP
    )
    block_w = glyph_width(CORNER_GLYPH_SCALE, len(chars), CORNER_GLYPH_GAP)
    pip_cx = CORNER_X + block_w / 2
    pip_cy = CORNER_Y + CORNER_GLYPH_HEIGHT + 1.0 + CORNER_PIP_SIZE / 2
    pip = suit_pip(suit, pip_cx, pip_cy, CORNER_PIP_SIZE)
    return f"{glyphs}\n{pip}"


def _corner_indices(suit: str, rank: int) -> str:
    """The top-left corner index, plus the *same* markup rotated 180
    degrees about the card center for the bottom-right corner — one
    source of truth for both, so they cannot drift apart."""
    top_left = _corner_index(suit, rank)
    bottom_right = f'<g transform="rotate(180 {n(CARD_CX)} {n(CARD_CY)})">\n{top_left}\n</g>'
    return f"{top_left}\n{bottom_right}"


def _number_field(suit: str, rank: int) -> tuple[str, int]:
    markup = []
    for col, row, rotate in _NUMBER_LAYOUT[rank]:
        markup.append(suit_pip(suit, col, row, FIELD_PIP_SIZE, rotate))
    return "\n".join(markup), len(markup)


def _court_frame() -> str:
    outer, inner = COURT_FRAME_OUTER, COURT_FRAME_INNER
    rects = []
    for frame, width in ((outer, 1.0), (inner, 0.8)):
        rects.append(
            f'<rect x="{n(frame["x"])}" y="{n(frame["y"])}" width="{n(frame["w"])}" '
            f'height="{n(frame["h"])}" rx="{n(frame["rx"])}" fill="none" '
            f'stroke="{INK}" stroke-width="{n(width)}"/>'
        )
    return "\n".join(rects)


def _court_glyph(suit: str, rank: int) -> str:
    color = suit_color(suit)
    scale = COURT_GLYPH_HEIGHT / _GH
    width = glyph_width(scale, len(RANK_CHARS[rank]), 1.6)
    x = CARD_CX - width / 2
    y = CARD_CY - COURT_GLYPH_HEIGHT / 2
    return rank_glyphs(rank, x, y, scale, 2.0, color, gap=1.6)


def _court_field(suit: str, rank: int) -> tuple[str, int]:
    markup = "\n".join(
        [
            _court_frame(),
            _court_glyph(suit, rank),
            suit_pip(suit, CARD_CX, COURT_PIP_TOP_Y, COURT_PIP_SIZE),
            suit_pip(suit, CARD_CX, COURT_PIP_BOTTOM_Y, COURT_PIP_SIZE, rotate=True),
        ]
    )
    return markup, 2


def _svg_wrap(body: str, width: int, height: int) -> str:
    return (
        "<!-- generated by generate.py — edit the generator, not this file -->\n"
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}">\n{body}\n</svg>\n'
    )


def build_face(suit: str, rank: int) -> str:
    """One `cards/<suit>_<NN>.svg` face: frame, both corner indices, and
    either the standard field pips (A, 2-10) or the court motif (J/Q/K)."""
    parts = [_card_frame(), _corner_indices(suit, rank)]

    if rank == 1:
        parts.append(suit_pip(suit, CARD_CX, CARD_CY, ACE_PIP_SIZE))
        emitted = 1
    elif rank in _NUMBER_LAYOUT:
        field, emitted = _number_field(suit, rank)
        parts.append(field)
    else:
        field, emitted = _court_field(suit, rank)
        parts.append(field)

    expected = expected_pip_count(rank)
    if emitted != expected:
        raise ValueError(
            f"{suit}_{rank:02d}: emitted {emitted} pips, expected {expected} for rank {rank}"
        )

    return _svg_wrap("\n".join(parts), CARD_W, CARD_H)


# =========================================================================
# Backs
# =========================================================================

BACK_GROUND = "#1a3c78"
BACK_PATTERN = "#4a72b8"
BACK_ACCENT = "#f2e9d8"


def _back_frame(w: float, h: float) -> str:
    x = y = 1.0
    return (
        f'<rect x="{n(x)}" y="{n(y)}" width="{n(w - 2 * x)}" height="{n(h - 2 * y)}" rx="4" '
        f'fill="{BACK_GROUND}" stroke="{INK}" stroke-width="{n(BORDER_WIDTH)}"/>'
    )


def _lattice(w: float, h: float) -> str:
    """A grid of small diamond outlines tiling the interior, offset every
    other row (a brick-like lattice) — integer row/column counts, so the
    loop bounds are exact rather than accumulated float comparisons."""
    step = 9.0
    size = 6.0
    margin = 6.0
    cols = int((w - 2 * margin) // step) + 1
    rows = int((h - 2 * margin) // step) + 1
    shapes = []
    for row in range(rows):
        y = margin + row * step
        x_offset = (step / 2) if row % 2 else 0.0
        for col in range(cols):
            x = margin + x_offset + col * step
            if x > w - margin:
                continue
            shapes.append(
                f'<g transform="translate({n(x)} {n(y)})">{_diamond_pip(size, BACK_PATTERN)}</g>'
            )
    return "\n".join(shapes)


def _back_badge(cx: float, cy: float, toggled: bool) -> str:
    """The strip back's one animated element: a small square badge that
    rotates 45 degrees between frames (a diamond in frame 2), so the frame
    change is visually obvious. The static back always uses `toggled=False`."""
    size = 10.0
    angle = 45 if toggled else 0
    rect = (
        f'<rect x="{n(-size / 2)}" y="{n(-size / 2)}" width="{n(size)}" height="{n(size)}" '
        f'fill="{BACK_ACCENT}" stroke="{INK}" stroke-width="1"/>'
    )
    return f'<g transform="translate({n(cx)} {n(cy)}) rotate({angle})">{rect}</g>'


def _back_tile(toggled: bool) -> str:
    return "\n".join(
        [
            _back_frame(CARD_W, CARD_H),
            _lattice(CARD_W, CARD_H),
            _back_badge(CARD_CX, CARD_CY, toggled),
        ]
    )


def build_static_back() -> str:
    """`backs/plain.svg`: one 71x96 tile, `image = "backs/plain.svg"` with
    no `frames`/`fps` (the static back shape)."""
    return _svg_wrap(_back_tile(toggled=False), CARD_W, CARD_H)


def build_strip_back() -> str:
    """`backs/weave.svg`: a 142x96 horizontal strip of two 71x96 frames
    (the strip back shape: `frames = 2`, `fps = 2`), differing only in
    the accent badge's rotation."""
    frame0 = _back_tile(toggled=False)
    frame1 = f'<g transform="translate({n(CARD_W)} 0)">\n{_back_tile(toggled=True)}\n</g>'
    return _svg_wrap(f"{frame0}\n{frame1}", CARD_W * 2, CARD_H)


# =========================================================================
# placeholders
# =========================================================================
#
# Drawn where a pile holds no card, so unlike a face or a back these are
# mostly transparent: the table shows through them. That is also why they
# carry no opaque fill — a placeholder must read correctly on any
# `[table] background`, not just this theme's green.

PLACEHOLDER_FILL_OPACITY = 0.10
PLACEHOLDER_STROKE_OPACITY = 0.45
PLACEHOLDER_MARK_WIDTH = 3.0


def _placeholder_slot() -> str:
    """The empty slot itself: the card frame's silhouette, darkened just
    enough to read as a place a card belongs."""
    x = y = 1.0
    return (
        f'<rect x="{n(x)}" y="{n(y)}" width="{n(CARD_W - 2 * x)}" '
        f'height="{n(CARD_H - 2 * y)}" rx="4" fill="{INK}" '
        f'fill-opacity="{n(PLACEHOLDER_FILL_OPACITY)}" stroke="{INK}" '
        f'stroke-opacity="{n(PLACEHOLDER_STROKE_OPACITY)}" '
        f'stroke-width="{n(BORDER_WIDTH)}"/>'
    )


def build_empty_pile() -> str:
    """`placeholders/empty_pile.svg`: the bare slot, drawn on every empty
    pile."""
    return _svg_wrap(_placeholder_slot(), CARD_W, CARD_H)


def build_stock_recycle() -> str:
    """`placeholders/stock_recycle.svg`: the slot plus a ring, marking an
    empty stock whose waste can still be recycled."""
    ring = (
        f'<circle cx="{n(CARD_CX)}" cy="{n(CARD_CY)}" r="16" fill="none" '
        f'stroke="{INK}" stroke-opacity="{n(PLACEHOLDER_STROKE_OPACITY)}" '
        f'stroke-width="{n(PLACEHOLDER_MARK_WIDTH)}"/>'
    )
    return _svg_wrap(f"{_placeholder_slot()}\n{ring}", CARD_W, CARD_H)


def build_stock_blocked() -> str:
    """`placeholders/stock_blocked.svg`: the slot plus a cross, marking an
    empty stock with no pass left. Red rather than ink, because it means
    the click will do nothing."""
    arm = 12.0
    cross = (
        f'<g stroke="{RED}" stroke-opacity="{n(PLACEHOLDER_STROKE_OPACITY)}" '
        f'stroke-width="{n(PLACEHOLDER_MARK_WIDTH)}" stroke-linecap="round">'
        f'<line x1="{n(CARD_CX - arm)}" y1="{n(CARD_CY - arm)}" '
        f'x2="{n(CARD_CX + arm)}" y2="{n(CARD_CY + arm)}"/>'
        f'<line x1="{n(CARD_CX + arm)}" y1="{n(CARD_CY - arm)}" '
        f'x2="{n(CARD_CX - arm)}" y2="{n(CARD_CY + arm)}"/>'
        "</g>"
    )
    return _svg_wrap(f"{_placeholder_slot()}\n{cross}", CARD_W, CARD_H)


# =========================================================================
# theme.toml
# =========================================================================


def theme_toml() -> str:
    return (
        "# generated by generate.py — edit the generator, not this file\n"
        "[theme]\n"
        'name = "Default"\n'
        'author = "classic-solitair"\n'
        'render_mode = "vector"\n'
        "\n"
        "[cards]\n"
        'faces = "cards/"\n'
        "base_size = [71, 96]\n"
        "\n"
        "[backs]\n"
        'plain = { image = "backs/plain.svg" }\n'
        'weave = { image = "backs/weave.svg", frames = 2, fps = 2 }\n'
        "\n"
        "[table]\n"
        'background = { color = "#008000" }\n'
        "\n"
        "[placeholders]\n"
        'empty_pile = { image = "placeholders/empty_pile.svg" }\n'
        'stock_recycle = { image = "placeholders/stock_recycle.svg" }\n'
        'stock_blocked = { image = "placeholders/stock_blocked.svg" }\n'
        "\n"
        "[drag]\n"
        'outline_color = "#000000"\n'
    )


# =========================================================================
# Orchestration
# =========================================================================


def generate(root: Path) -> list[Path]:
    """Writes every generated file under `root` (`themes/default/`),
    returning every path written for `self_check` to re-parse."""
    written: list[Path] = []

    cards_dir = root / "cards"
    cards_dir.mkdir(parents=True, exist_ok=True)
    for suit in SUITS:
        for rank in RANKS:
            path = cards_dir / f"{suit}_{rank:02d}.svg"
            path.write_text(build_face(suit, rank), encoding="utf-8")
            written.append(path)

    backs_dir = root / "backs"
    backs_dir.mkdir(parents=True, exist_ok=True)
    plain_path = backs_dir / "plain.svg"
    plain_path.write_text(build_static_back(), encoding="utf-8")
    written.append(plain_path)
    weave_path = backs_dir / "weave.svg"
    weave_path.write_text(build_strip_back(), encoding="utf-8")
    written.append(weave_path)

    placeholders_dir = root / "placeholders"
    placeholders_dir.mkdir(parents=True, exist_ok=True)
    for name, build in (
        ("empty_pile", build_empty_pile),
        ("stock_recycle", build_stock_recycle),
        ("stock_blocked", build_stock_blocked),
    ):
        path = placeholders_dir / f"{name}.svg"
        path.write_text(build(), encoding="utf-8")
        written.append(path)

    toml_path = root / "theme.toml"
    toml_path.write_text(theme_toml(), encoding="utf-8")
    written.append(toml_path)

    return written


def self_check(written: list[Path]) -> None:
    """Re-parses every generated file: `xml.etree` for the SVGs (catches
    malformed markup immediately, the same run that introduced it) and
    `tomllib` for `theme.toml`."""
    for path in written:
        if path.suffix == ".svg":
            ET.parse(path)
        elif path.name == "theme.toml":
            with path.open("rb") as handle:
                tomllib.load(handle)


def main() -> None:
    root = Path(__file__).resolve().parent
    written = generate(root)
    self_check(written)
    print(f"generated {len(written)} files under {root}")


if __name__ == "__main__":
    main()
