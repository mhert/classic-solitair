//! The per-frame display list: [`DisplayList`], [`Sprite`], [`TextureId`],
//! and [`Rgba`].
//!
//! This is the presenter → renderer contract. It is plain data — engine
//! value objects and integer geometry, no rendering-API types — and it is
//! complete: a renderer draws a frame by clearing (unless the list says
//! not to) and blitting the sprites in order.

use sol_engine::{Rank, Suit};

use crate::geometry::Rect;

/// An RGBA color with 8-bit channels, straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel: 255 is opaque.
    pub a: u8,
}

impl Rgba {
    /// Opaque white — the identity tint.
    pub const WHITE: Self = Self::opaque(0xFF, 0xFF, 0xFF);

    /// Creates an opaque color.
    #[must_use]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xFF }
    }
}

impl From<sol_theme::Color> for Rgba {
    fn from(color: sol_theme::Color) -> Self {
        Self::opaque(color.r, color.g, color.b)
    }
}

/// Identifies the image a sprite samples from, in theme terms.
///
/// The renderer resolves these against the same loaded
/// [`sol_theme::Theme`] the presenter was configured with:
///
/// - [`TextureId::White`] is a solid 1×1 white pixel the renderer
///   provides itself (tinted quads: drag outlines, highlights).
/// - [`TextureId::Background`] is the theme's `[table]` background image
///   (only emitted when the theme has one).
/// - [`TextureId::Face`] is the face asset for a card. `sol-engine`'s suit
///   and rank enumerations follow the same canonical order as the theme's
///   52 faces (spades, hearts, diamonds, clubs; ace through king), so the
///   mapping is index-for-index.
/// - [`TextureId::Back`] is one asset of the `[backs]` entry at `back`
///   (declaration order). A static or strip back has one asset (`asset`
///   0); a list-form back has one per frame.
/// - [`TextureId::Placeholder`] is the `[placeholders]` asset for `slot`
///   (only emitted when the theme declares that slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureId {
    /// A solid white pixel, for tinted untextured quads.
    White,
    /// The theme's table background image.
    Background,
    /// The face of one card.
    Face {
        /// The card's suit.
        suit: Suit,
        /// The card's rank.
        rank: Rank,
    },
    /// One asset of one theme back.
    Back {
        /// Index into the theme's `[backs]` entries, declaration order.
        back: usize,
        /// Index into that back's assets (0 except for list-form backs).
        asset: usize,
    },
    /// One `[placeholders]` asset, drawn where a pile is empty.
    Placeholder {
        /// Which `[placeholders]` entry to sample.
        slot: PlaceholderSlot,
    },
}

/// Which `[placeholders]` image a [`TextureId::Placeholder`] names.
///
/// The stock distinguishes two states because the original does: an empty
/// stock the player can still recycle looks different from one that has no
/// pass left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceholderSlot {
    /// Drawn on every empty pile.
    EmptyPile,
    /// Drawn on the empty stock while the waste can still be recycled.
    StockRecycle,
    /// Drawn on the empty stock once no pass remains.
    StockBlocked,
}

/// One textured quad of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sprite {
    /// The image to sample.
    pub texture: TextureId,
    /// The source rectangle, in the asset's own (unscaled) pixels — for a
    /// frame strip, the current frame's slice.
    pub src: Rect,
    /// The destination quad in logical pixels; the renderer stretches
    /// these by the continuous display scale.
    pub dst: Rect,
    /// Draw order: sprites are listed back-to-front and `z` ascends with
    /// the list, so drawing in list order and sorting stably by `z` are
    /// the same thing.
    pub z: i32,
    /// Color multiplier; [`Rgba::WHITE`] leaves the texture unchanged.
    pub tint: Rgba,
}

/// One frame's complete draw instructions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayList {
    /// What to clear the target to before drawing, or `None` to draw over
    /// the previous frame — the win cascade's smear trail depends on the
    /// previous frame surviving.
    pub clear: Option<Rgba>,
    /// The sprites, back to front.
    pub sprites: Vec<Sprite>,
}

impl DisplayList {
    /// Appends a sprite, assigning it the next `z`.
    pub(crate) fn push(&mut self, texture: TextureId, src: Rect, dst: Rect, tint: Rgba) {
        let z = crate::geometry::index_to_i32(self.sprites.len());
        self.sprites.push(Sprite {
            texture,
            src,
            dst,
            z,
            tint,
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn rgba_from_theme_color_is_opaque() {
        let color: Rgba = sol_theme::Color::new(0x00, 0x80, 0x00).into();
        assert_eq!(color, Rgba::opaque(0, 128, 0));
        assert_eq!(color.a, 0xFF);
        assert_eq!(Rgba::WHITE, Rgba::opaque(0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn push_assigns_ascending_z() {
        let mut list = DisplayList::default();
        let rect = Rect::new(0, 0, 1, 1);
        list.push(TextureId::White, rect, rect, Rgba::WHITE);
        list.push(TextureId::Background, rect, rect, Rgba::WHITE);
        assert_eq!(list.sprites.len(), 2);
        assert_eq!(list.sprites[0].z, 0);
        assert_eq!(list.sprites[1].z, 1);
        assert_eq!(list.sprites[1].texture, TextureId::Background);
    }
}
