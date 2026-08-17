//! Display list → batched vertices: the CPU half of the draw.
//!
//! Each sprite becomes one textured quad (4 vertices, 6 indices) with
//! positions in logical pixels (the shader's orthographic transform
//! folds in the continuous display scale and maps them to clip space),
//! normalized atlas UVs, and a premultiplied tint. Sprites are emitted in list order, which the display-list
//! contract defines as back-to-front — a painter's-algorithm draw needs
//! no depth buffer.

use sol_presenter::{DisplayList, Rgba, Sprite, TextureId};

use crate::atlas::Atlas;
use crate::error::RenderError;

/// One quad corner.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Vertex {
    /// Position in logical pixels.
    pub pos: [f32; 2],
    /// Normalized atlas coordinates.
    pub uv: [f32; 2],
    /// Premultiplied tint multiplier.
    pub tint: [f32; 4],
}

/// Lossless for every coordinate this renderer sees: logical pixels and
/// atlas texels are far below f32's 2^24 exact-integer ceiling.
#[allow(clippy::cast_precision_loss)]
fn px(value: i32) -> f32 {
    value as f32
}

/// See [`px`].
#[allow(clippy::cast_precision_loss)]
fn texel(value: u32) -> f32 {
    value as f32
}

/// A straight-alpha tint as the shader's premultiplied multiplier.
fn premultiplied(tint: Rgba) -> [f32; 4] {
    let a = f32::from(tint.a) / 255.0;
    [
        f32::from(tint.r) / 255.0 * a,
        f32::from(tint.g) / 255.0 * a,
        f32::from(tint.b) / 255.0 * a,
        a,
    ]
}

/// Builds the vertex/index batch for one frame.
///
/// # Errors
///
/// [`RenderError::UnknownTexture`] if a sprite references a texture the
/// atlas has no entry for (presenter and renderer themes out of sync).
pub(crate) fn build_batch(
    list: &DisplayList,
    atlas: &Atlas,
) -> Result<(Vec<Vertex>, Vec<u32>), RenderError> {
    let mut vertices = Vec::with_capacity(list.sprites.len() * 4);
    let mut indices = Vec::with_capacity(list.sprites.len() * 6);
    for sprite in &list.sprites {
        push_sprite(&mut vertices, &mut indices, sprite, atlas)?;
    }
    Ok((vertices, indices))
}

fn push_sprite(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    sprite: &Sprite,
    atlas: &Atlas,
) -> Result<(), RenderError> {
    let entry = atlas
        .entries
        .get(&sprite.texture)
        .ok_or(RenderError::UnknownTexture {
            texture: sprite.texture,
        })?;

    let sheet_w = texel(atlas.width.max(1));
    let sheet_h = texel(atlas.height.max(1));
    let (u0, v0, u1, v1) = if sprite.texture == TextureId::White {
        // The white texel's center on all four corners: no neighbor can
        // ever be sampled in, under either filter.
        let u = (texel(entry.x) + 0.5) / sheet_w;
        let v = (texel(entry.y) + 0.5) / sheet_h;
        (u, v, u, v)
    } else {
        // src is in the asset's own unscaled pixels; the atlas holds the
        // asset at `factor`, offset to the entry origin.
        let f = texel(atlas.factor.max(1));
        let left = texel(entry.x) + px(sprite.src.x) * f;
        let top = texel(entry.y) + px(sprite.src.y) * f;
        (
            left / sheet_w,
            top / sheet_h,
            (left + px(sprite.src.w) * f) / sheet_w,
            (top + px(sprite.src.h) * f) / sheet_h,
        )
    };

    let x0 = px(sprite.dst.x);
    let y0 = px(sprite.dst.y);
    let x1 = x0 + px(sprite.dst.w);
    let y1 = y0 + px(sprite.dst.h);
    let tint = premultiplied(sprite.tint);

    let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    vertices.extend_from_slice(&[
        Vertex {
            pos: [x0, y0],
            uv: [u0, v0],
            tint,
        },
        Vertex {
            pos: [x1, y0],
            uv: [u1, v0],
            tint,
        },
        Vertex {
            pos: [x0, y1],
            uv: [u0, v1],
            tint,
        },
        Vertex {
            pos: [x1, y1],
            uv: [u1, v1],
            tint,
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::float_cmp)]

    use std::collections::HashMap;

    use sol_engine::{Rank, Suit};
    use sol_presenter::Rect;

    use super::*;
    use crate::atlas::AtlasEntry;

    /// A hand-built 100×50 atlas at factor 2: white at (0,0), one face at
    /// (10, 20) holding a 4×6 asset (8×12 texels).
    fn atlas() -> Atlas {
        let face = TextureId::Face {
            suit: Suit::Spades,
            rank: Rank::Ace,
        };
        let mut entries = HashMap::new();
        entries.insert(
            TextureId::White,
            AtlasEntry {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
        );
        entries.insert(
            face,
            AtlasEntry {
                x: 10,
                y: 20,
                w: 8,
                h: 12,
            },
        );
        Atlas {
            width: 100,
            height: 50,
            rgba: Vec::new(),
            entries,
            factor: 2,
        }
    }

    fn face_sprite(src: Rect, dst: Rect) -> Sprite {
        Sprite {
            texture: TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            },
            src,
            dst,
            z: 0,
            tint: Rgba::WHITE,
        }
    }

    #[test]
    fn a_full_asset_quad_maps_entry_and_dst_exactly() {
        let list = DisplayList {
            clear: None,
            sprites: vec![face_sprite(Rect::new(0, 0, 4, 6), Rect::new(30, 40, 8, 12))],
        };
        let (vertices, indices) = build_batch(&list, &atlas()).unwrap();
        assert_eq!(vertices.len(), 4);
        assert_eq!(indices, vec![0, 1, 2, 2, 1, 3]);
        // Positions: the dst rect's corners in pixels.
        assert_eq!(vertices[0].pos, [30.0, 40.0]);
        assert_eq!(vertices[3].pos, [38.0, 52.0]);
        // UVs: entry (10,20)..(18,32) of a 100×50 sheet.
        assert_eq!(vertices[0].uv, [0.1, 0.4]);
        assert_eq!(vertices[3].uv, [0.18, 0.64]);
        assert_eq!(vertices[0].tint, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_strip_frame_src_offsets_by_the_content_factor() {
        // Frame slice src (2,0,2,6) of the 4×6 asset: texels 10+2·2=14 …
        // 14+2·2=18 horizontally.
        let list = DisplayList {
            clear: None,
            sprites: vec![face_sprite(Rect::new(2, 0, 2, 6), Rect::new(0, 0, 4, 12))],
        };
        let (vertices, _) = build_batch(&list, &atlas()).unwrap();
        assert_eq!(vertices[0].uv, [0.14, 0.4]);
        assert_eq!(vertices[3].uv, [0.18, 0.64]);
    }

    #[test]
    fn white_quads_pin_all_corners_to_the_texel_center() {
        let list = DisplayList {
            clear: None,
            sprites: vec![Sprite {
                texture: TextureId::White,
                src: Rect::new(0, 0, 1, 1),
                dst: Rect::new(5, 6, 70, 1),
                z: 0,
                tint: Rgba::opaque(0, 0, 0),
            }],
        };
        let (vertices, _) = build_batch(&list, &atlas()).unwrap();
        for vertex in &vertices {
            assert_eq!(vertex.uv, [0.5 / 100.0, 0.5 / 50.0]);
            assert_eq!(vertex.tint, [0.0, 0.0, 0.0, 1.0]);
        }
        assert_eq!(vertices[0].pos, [5.0, 6.0]);
        assert_eq!(vertices[3].pos, [75.0, 7.0]);
    }

    #[test]
    fn tints_premultiply_their_alpha() {
        let half = Rgba {
            r: 255,
            g: 0,
            b: 255,
            a: 51,
        };
        let tint = premultiplied(half);
        assert_eq!(tint[3], 0.2);
        assert_eq!(tint[0], 0.2);
        assert_eq!(tint[1], 0.0);
        assert_eq!(tint[2], 0.2);
    }

    #[test]
    fn an_unknown_texture_is_a_theme_mismatch_error() {
        let list = DisplayList {
            clear: None,
            sprites: vec![Sprite {
                texture: TextureId::Back { back: 7, asset: 0 },
                src: Rect::new(0, 0, 4, 6),
                dst: Rect::new(0, 0, 4, 6),
                z: 0,
                tint: Rgba::WHITE,
            }],
        };
        let error = build_batch(&list, &atlas()).unwrap_err();
        assert!(matches!(
            error,
            RenderError::UnknownTexture {
                texture: TextureId::Back { back: 7, asset: 0 }
            }
        ));
        assert!(error.to_string().contains("back: 7"));
    }

    #[test]
    fn quads_batch_in_list_order() {
        let list = DisplayList {
            clear: None,
            sprites: vec![
                face_sprite(Rect::new(0, 0, 4, 6), Rect::new(0, 0, 8, 12)),
                face_sprite(Rect::new(0, 0, 4, 6), Rect::new(50, 0, 8, 12)),
            ],
        };
        let (vertices, indices) = build_batch(&list, &atlas()).unwrap();
        assert_eq!(vertices.len(), 8);
        assert_eq!(&indices[6..], &[4, 5, 6, 6, 5, 7]);
        assert_eq!(vertices[4].pos, [50.0, 0.0]);
    }
}
