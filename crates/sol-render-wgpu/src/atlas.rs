//! The single texture atlas: every theme image the display list can
//! reference, rasterized at one content factor and shelf-packed into one
//! premultiplied RGBA8 sheet.
//!
//! The catalog order is fixed (white pixel, background image if any, the
//! 52 faces in canonical order, backs in declaration order with each
//! back's assets in order, then any declared placeholders) and the packer
//! sorts stably, so the same theme at the same factor always yields
//! byte-identical atlas pixels — the golden-image tests depend on that.
//!
//! Entries are keyed by the display list's [`TextureId`]; a sprite's
//! `src` rectangle (in the asset's own unscaled pixels) maps into the
//! atlas as `entry origin + src × factor`. One texel of transparent
//! padding surrounds every entry so linear sampling at clamped scales
//! cannot bleed a neighbor in.
//!
//! Rasterization fans out one thread per catalog image; the sheet form's
//! shared SVG is rasterized once regardless of how many cells slice it,
//! and the copy into the sheet stays serial (it is memcpy-bound). Order is
//! unaffected — results are joined in catalog order — so the same theme at
//! the same factor still yields byte-identical atlas pixels.

use std::collections::HashMap;

use sol_presenter::{PlaceholderSlot, TextureId};
use sol_theme::{LoadedBackground, Theme};

use crate::error::RenderError;
use crate::raster::{Raster, rasterize, rasterize_strip};

/// One image's placement in the atlas, in atlas texels (content scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtlasEntry {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

/// The built atlas: one premultiplied RGBA8 sheet plus the entry map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Atlas {
    /// Sheet width in texels.
    pub width: u32,
    /// Sheet height in texels.
    pub height: u32,
    /// Premultiplied RGBA8 sheet pixels.
    pub rgba: Vec<u8>,
    /// Placement of every catalog image.
    pub entries: HashMap<TextureId, AtlasEntry>,
    /// The content factor the images were rasterized at.
    pub factor: u32,
}

/// A texel of transparent padding on every side of every entry.
const PAD: u32 = 1;

/// The largest factor `1..=desired` whose packed sheet fits within
/// `max_dim × max_dim` — pure integer math, no rasterization, so callers
/// can compare against the atlas they already have before paying for a
/// rebuild.
///
/// # Errors
///
/// [`RenderError::AtlasOverflow`] if not even factor 1 fits.
pub(crate) fn plan_factor(theme: &Theme, desired: u32, max_dim: u32) -> Result<u32, RenderError> {
    let catalog = catalog(theme);
    (1..=desired.max(1))
        .rev()
        .find(|&factor| pack(&scaled_sizes(&catalog, factor), max_dim).is_some())
        .ok_or(RenderError::AtlasOverflow { max_dim })
}

/// Builds the atlas for `theme` at the factor [`plan_factor`] picks.
///
/// # Errors
///
/// [`RenderError::AtlasOverflow`] if not even factor 1 fits, or an asset
/// rasterization error.
pub(crate) fn build(theme: &Theme, desired: u32, max_dim: u32) -> Result<Atlas, RenderError> {
    let factor = plan_factor(theme, desired, max_dim)?;
    let catalog = catalog(theme);
    // plan_factor just proved this factor packs; the error arm keeps the
    // call total.
    let placements = pack(&scaled_sizes(&catalog, factor), max_dim)
        .ok_or(RenderError::AtlasOverflow { max_dim })?;
    let (rasters, sheet) = rasterize_all(&catalog, factor)?;
    Ok(blit(
        &catalog,
        factor,
        &placements,
        &rasters,
        sheet.as_ref(),
    ))
}

/// Where one catalog image's pixels come from.
enum CatalogSource<'theme> {
    /// The renderer-provided 1×1 white texel.
    White,
    /// A whole theme asset (a face, a back, or the background image).
    Whole(&'theme sol_theme::Asset),
    /// A strip-form animated back: one asset holding several frames,
    /// each of which must be scaled in isolation so xBRZ cannot bleed
    /// neighboring frames across a frame edge.
    Strip {
        /// The strip asset.
        asset: &'theme sol_theme::Asset,
        /// Number of frames in the strip (at least 2).
        frames: u32,
        /// The axis the frames are laid out along.
        layout: sol_theme::BackLayout,
    },
    /// One card-sized cell of the vector sheet form's single SVG: the
    /// 13-wide (ranks ascending) × 4-high (suits in canonical order)
    /// grid `sol_theme` validates but leaves for the renderer to slice.
    SheetCell {
        /// The sheet asset (shared by all 52 cells).
        sheet: &'theme sol_theme::Asset,
        /// Grid column, `0..13` (rank).
        col: u32,
        /// Grid row, `0..4` (suit).
        row: u32,
    },
}

/// One catalog image: its display-list id, its unscaled content size,
/// and where its pixels come from.
struct CatalogItem<'theme> {
    id: TextureId,
    size: (u32, u32),
    source: CatalogSource<'theme>,
}

impl CatalogItem<'_> {
    /// Content size at `factor`. The renderer-provided white texel never
    /// scales — one texel is one texel at any factor.
    fn scaled(&self, factor: u32) -> (u32, u32) {
        match self.source {
            CatalogSource::White => (1, 1),
            CatalogSource::Whole(_)
            | CatalogSource::Strip { .. }
            | CatalogSource::SheetCell { .. } => (
                self.size.0.saturating_mul(factor).max(1),
                self.size.1.saturating_mul(factor).max(1),
            ),
        }
    }
}

/// The theme's declared placeholder assets paired with their slots, in a
/// fixed order; undeclared slots are skipped, exactly as the presenter
/// skips emitting them.
fn placeholder_assets(theme: &Theme) -> impl Iterator<Item = (PlaceholderSlot, &sol_theme::Asset)> {
    let placeholders = theme.placeholders();
    [
        (PlaceholderSlot::EmptyPile, placeholders.empty_pile.as_ref()),
        (
            PlaceholderSlot::StockRecycle,
            placeholders.stock_recycle.as_ref(),
        ),
        (
            PlaceholderSlot::StockBlocked,
            placeholders.stock_blocked.as_ref(),
        ),
    ]
    .into_iter()
    .filter_map(|(slot, asset)| asset.map(|asset| (slot, asset)))
}

/// The engine's suit for one of the theme's — the two enums name the same
/// four suits but declare them in different orders, so this is a match, not
/// a cast.
const fn engine_suit(suit: sol_theme::FaceSuit) -> sol_engine::Suit {
    match suit {
        sol_theme::FaceSuit::Spades => sol_engine::Suit::Spades,
        sol_theme::FaceSuit::Hearts => sol_engine::Suit::Hearts,
        sol_theme::FaceSuit::Diamonds => sol_engine::Suit::Diamonds,
        sol_theme::FaceSuit::Clubs => sol_engine::Suit::Clubs,
    }
}

/// The fixed catalog: every [`TextureId`] the presenter can emit for this
/// theme. Order is deterministic (white, background, faces canonical,
/// backs in declaration order, then placeholders) — golden images depend
/// on that.
fn catalog(theme: &Theme) -> Vec<CatalogItem<'_>> {
    let mut items = vec![CatalogItem {
        id: TextureId::White,
        size: (1, 1),
        source: CatalogSource::White,
    }];
    if let LoadedBackground::Image { asset, .. } = theme.background() {
        items.push(CatalogItem {
            id: TextureId::Background,
            size: (asset.size.width, asset.size.height),
            source: CatalogSource::Whole(asset),
        });
    }
    let sheet_form = matches!(theme.manifest.faces, sol_theme::FacesSource::SvgSheet(_));
    let base = theme.manifest.base_size;
    // The theme enumerates its 52 faces in its own canonical order (spades,
    // hearts, diamonds, clubs; ace through king), which is *not* the
    // engine's deck-index order. Pair the two by identity — suit and rank —
    // never by position: a positional pairing silently relabels all 52 cards
    // the moment either order changes. `index` stays the theme's own
    // enumeration index, because that is what addresses a sheet cell.
    for (index, (face_suit, face_rank, asset)) in theme.faces().enumerate() {
        let card = sol_engine::Card::from_index(
            engine_suit(face_suit).index() + 4 * (face_rank.get() - 1),
        );
        let id = TextureId::Face {
            suit: card.suit,
            rank: card.rank,
        };
        let index = u32::try_from(index).unwrap_or(0);
        items.push(if sheet_form {
            CatalogItem {
                id,
                size: (base.width, base.height),
                source: CatalogSource::SheetCell {
                    sheet: asset,
                    col: index % 13,
                    row: index / 13,
                },
            }
        } else {
            CatalogItem {
                id,
                size: (asset.size.width, asset.size.height),
                source: CatalogSource::Whole(asset),
            }
        });
    }
    for (back, (_, loaded)) in theme.backs().iter().enumerate() {
        // The strip form: several frames sharing one asset. List-form
        // backs (one asset per frame) and static backs scale whole.
        let strip = loaded.assets.len() == 1 && loaded.frame_count > 1;
        for (asset_index, asset) in loaded.assets.iter().enumerate() {
            items.push(CatalogItem {
                id: TextureId::Back {
                    back,
                    asset: asset_index,
                },
                size: (asset.size.width, asset.size.height),
                source: if strip {
                    CatalogSource::Strip {
                        asset,
                        frames: loaded.frame_count,
                        layout: loaded.layout.unwrap_or_default(),
                    }
                } else {
                    CatalogSource::Whole(asset)
                },
            });
        }
    }
    for (slot, asset) in placeholder_assets(theme) {
        items.push(CatalogItem {
            id: TextureId::Placeholder { slot },
            size: (asset.size.width, asset.size.height),
            source: CatalogSource::Whole(asset),
        });
    }
    items
}

/// Every catalog item's content size at `factor`.
fn scaled_sizes(catalog: &[CatalogItem<'_>], factor: u32) -> Vec<(u32, u32)> {
    catalog.iter().map(|item| item.scaled(factor)).collect()
}

/// The placement of every item (content origin, padding already inside),
/// plus the sheet size.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placements {
    width: u32,
    height: u32,
    origins: Vec<(u32, u32)>,
}

/// Shelf-packs `sizes` (content sizes; padding is added here) into a
/// sheet within `max_dim × max_dim`, or `None` if they cannot fit.
fn pack(sizes: &[(u32, u32)], max_dim: u32) -> Option<Placements> {
    let padded = |&(w, h): &(u32, u32)| (w.saturating_add(2 * PAD), h.saturating_add(2 * PAD));

    let widest = sizes.iter().map(|size| padded(size).0).max().unwrap_or(1);
    if widest > max_dim {
        return None;
    }
    let total_area: u64 = sizes
        .iter()
        .map(|size| {
            let (w, h) = padded(size);
            u64::from(w) * u64::from(h)
        })
        .sum();
    // A width near the square root keeps the sheet roughly square; the
    // widest item and the device limit bound it on either side.
    let side = u32::try_from(total_area.isqrt()).unwrap_or(max_dim);
    let width = side.max(widest).min(max_dim);

    // Tallest (then widest) first, index as the stable tiebreak: shelf
    // packing wastes least when heights descend, and the order is fully
    // deterministic for golden images.
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        let (w, h) = sizes.get(i).map_or((1, 1), padded);
        (core::cmp::Reverse(h), core::cmp::Reverse(w), i)
    });

    let mut origins = vec![(0_u32, 0_u32); sizes.len()];
    let mut cursor_x = 0_u32;
    let mut shelf_y = 0_u32;
    let mut shelf_h = 0_u32;
    let mut used_h = 0_u32;
    for &i in &order {
        let (w, h) = sizes.get(i).map_or((1, 1), padded);
        if cursor_x.saturating_add(w) > width {
            shelf_y = shelf_y.saturating_add(shelf_h);
            cursor_x = 0;
            shelf_h = 0;
        }
        if let Some(slot) = origins.get_mut(i) {
            *slot = (cursor_x.saturating_add(PAD), shelf_y.saturating_add(PAD));
        }
        cursor_x = cursor_x.saturating_add(w);
        shelf_h = shelf_h.max(h);
        used_h = shelf_y.saturating_add(shelf_h);
    }
    if used_h > max_dim {
        return None;
    }
    Some(Placements {
        width,
        height: used_h.max(1),
        origins,
    })
}

/// Joins a scoped rasterization thread, re-raising a worker panic in this
/// thread rather than folding it into an error: a panic out of the PNG
/// decoder or resvg propagates exactly as it would from an inline call.
fn join_or_resume<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

/// Rasterizes every catalog image, one thread per image.
///
/// Returns the per-item rasters (`None` for the white texel, which needs
/// no rasterization, and for sheet cells, which slice the shared sheet)
/// alongside the sheet raster itself — rasterized exactly once no matter
/// how many cells reference it.
fn rasterize_all(
    catalog: &[CatalogItem<'_>],
    factor: u32,
) -> Result<(Vec<Option<Raster>>, Option<Raster>), RenderError> {
    let sheet = catalog.iter().find_map(|item| match item.source {
        CatalogSource::SheetCell { sheet, .. } => Some(sheet),
        CatalogSource::White | CatalogSource::Whole(_) | CatalogSource::Strip { .. } => None,
    });

    let (items, sheet) = std::thread::scope(|scope| {
        let handles: Vec<_> = catalog
            .iter()
            .map(|item| match item.source {
                CatalogSource::Whole(asset) => Some(scope.spawn(move || rasterize(asset, factor))),
                CatalogSource::Strip {
                    asset,
                    frames,
                    layout,
                } => Some(scope.spawn(move || rasterize_strip(asset, factor, frames, layout))),
                CatalogSource::White | CatalogSource::SheetCell { .. } => None,
            })
            .collect();
        let sheet = sheet.map(|sheet| scope.spawn(move || rasterize(sheet, factor)));
        (
            handles
                .into_iter()
                .map(|handle| handle.map(join_or_resume))
                .collect::<Vec<_>>(),
            sheet.map(join_or_resume),
        )
    });

    let items = items
        .into_iter()
        .map(Option::transpose)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((items, sheet.transpose()?))
}

/// Copies every catalog image's precomputed raster into the sheet at its
/// placement. The vector sheet form's single SVG was rasterized once (by
/// [`rasterize_all`]) and is sliced per cell here.
fn blit(
    catalog: &[CatalogItem<'_>],
    factor: u32,
    placements: &Placements,
    rasters: &[Option<Raster>],
    sheet_raster: Option<&Raster>,
) -> Atlas {
    let row_bytes = placements.width as usize * 4;
    let mut rgba = vec![0_u8; row_bytes * placements.height as usize];
    let mut entries = HashMap::new();

    for ((item, &(x, y)), raster) in catalog.iter().zip(&placements.origins).zip(rasters) {
        let (entry_w, entry_h) = item.scaled(factor);
        match item.source {
            CatalogSource::White => {
                let white = Raster {
                    width: 1,
                    height: 1,
                    rgba: vec![0xFF; 4],
                };
                copy_rect(&mut rgba, placements.width, x, y, &white, 0, 0, 1, 1);
            }
            CatalogSource::Whole(_) | CatalogSource::Strip { .. } => {
                // rasterize_all fills exactly these two; None cannot
                // happen, and skipping is the totality guard.
                if let Some(raster) = raster {
                    copy_rect(
                        &mut rgba,
                        placements.width,
                        x,
                        y,
                        raster,
                        0,
                        0,
                        raster.width,
                        raster.height,
                    );
                }
            }
            CatalogSource::SheetCell { col, row, .. } => {
                if let Some(raster) = sheet_raster {
                    copy_rect(
                        &mut rgba,
                        placements.width,
                        x,
                        y,
                        raster,
                        col.saturating_mul(entry_w),
                        row.saturating_mul(entry_h),
                        entry_w,
                        entry_h,
                    );
                }
            }
        }
        entries.insert(
            item.id,
            AtlasEntry {
                x,
                y,
                w: entry_w,
                h: entry_h,
            },
        );
    }

    Atlas {
        width: placements.width,
        height: placements.height,
        rgba,
        entries,
        factor,
    }
}

/// Copies the `w`×`h` rectangle at `(src_x, src_y)` of `raster` into the
/// sheet with its top-left texel at `(x, y)`. The packer (and, for sheet
/// cells, sol-theme's 13×4 size validation) guarantees both ranges are in
/// bounds; the lookups keep the copy total regardless.
#[allow(clippy::too_many_arguments)] // a plain blit signature: dst, src, and their rects
fn copy_rect(
    sheet: &mut [u8],
    sheet_width: u32,
    x: u32,
    y: u32,
    raster: &Raster,
    src_x: u32,
    src_y: u32,
    w: u32,
    h: u32,
) {
    let copy_bytes = w as usize * 4;
    if copy_bytes == 0 {
        return;
    }
    for row in 0..h as usize {
        let src_start = ((src_y as usize + row) * raster.width as usize + src_x as usize) * 4;
        let dest_start = ((y as usize + row) * sheet_width as usize + x as usize) * 4;
        if let (Some(src), Some(dest)) = (
            raster.rgba.get(src_start..src_start + copy_bytes),
            sheet.get_mut(dest_start..dest_start + copy_bytes),
        ) {
            dest.copy_from_slice(src);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_engine::{Rank, Suit};

    use super::*;
    use crate::testkit::{
        face_color, placeholder_color, sheet_cell_color, test_theme_png,
        test_theme_png_corner_strip, test_theme_png_image_bg, test_theme_png_placeholders,
        test_theme_vector, test_theme_vector_sheet,
    };

    fn pixel_at(atlas: &Atlas, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * atlas.width + x) * 4) as usize;
        [
            atlas.rgba[i],
            atlas.rgba[i + 1],
            atlas.rgba[i + 2],
            atlas.rgba[i + 3],
        ]
    }

    #[test]
    fn pack_places_everything_within_bounds_without_overlap() {
        let sizes = [(4, 6), (4, 6), (8, 3), (1, 1), (4, 6)];
        let placed = pack(&sizes, 64).unwrap();
        assert!(placed.width <= 64 && placed.height <= 64);
        // Padded boxes must be pairwise disjoint.
        let boxes: Vec<(u32, u32, u32, u32)> = sizes
            .iter()
            .zip(&placed.origins)
            .map(|(&(w, h), &(x, y))| (x - PAD, y - PAD, w + 2 * PAD, h + 2 * PAD))
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in boxes.iter().skip(i + 1) {
                let disjoint =
                    a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
                assert!(disjoint, "{a:?} overlaps {b:?}");
            }
            assert!(a.0 + a.2 <= placed.width && a.1 + a.3 <= placed.height);
        }
    }

    #[test]
    fn pack_is_deterministic() {
        let sizes = [(4, 6), (2, 2), (8, 3), (1, 1)];
        assert_eq!(pack(&sizes, 64), pack(&sizes, 64));
    }

    #[test]
    fn pack_refuses_what_cannot_fit() {
        assert!(pack(&[(100, 1)], 32).is_none(), "too wide");
        assert!(pack(&[(1, 100)], 32).is_none(), "too tall");
        let many = vec![(10, 10); 20];
        assert!(pack(&many, 32).is_none(), "too much area");
        assert!(pack(&many, 128).is_some());
    }

    #[test]
    fn build_covers_every_display_list_texture() {
        let theme = test_theme_png();
        let atlas = build(&theme, 1, 2048).unwrap();
        assert_eq!(atlas.factor, 1);
        assert!(atlas.entries.contains_key(&TextureId::White));
        for suit in Suit::ALL {
            for rank in Rank::ALL {
                let entry = atlas.entries[&TextureId::Face { suit, rank }];
                assert_eq!((entry.w, entry.h), (4, 6), "{suit:?} {rank:?}");
            }
        }
        // One static back plus one 2-frame horizontal strip back.
        assert_eq!(atlas.entries[&TextureId::Back { back: 0, asset: 0 }].w, 4);
        assert_eq!(atlas.entries[&TextureId::Back { back: 1, asset: 0 }].w, 8);
        assert!(!atlas.entries.contains_key(&TextureId::Background));
        // A theme declaring no placeholders catalogs none.
        for slot in ALL_SLOTS {
            assert!(
                !atlas.entries.contains_key(&TextureId::Placeholder { slot }),
                "{slot:?}"
            );
        }
        // 1 white + 52 faces + 2 backs.
        assert_eq!(atlas.entries.len(), 55);
    }

    const ALL_SLOTS: [PlaceholderSlot; 3] = [
        PlaceholderSlot::EmptyPile,
        PlaceholderSlot::StockRecycle,
        PlaceholderSlot::StockBlocked,
    ];

    fn slot_name(slot: PlaceholderSlot) -> &'static str {
        match slot {
            PlaceholderSlot::EmptyPile => "empty_pile",
            PlaceholderSlot::StockRecycle => "stock_recycle",
            PlaceholderSlot::StockBlocked => "stock_blocked",
        }
    }

    /// Every placeholder the presenter can emit for this theme has an
    /// atlas entry holding that placeholder's own pixels — a crossed
    /// catalog entry would show up as the wrong color.
    #[test]
    fn declared_placeholders_get_entries_with_their_pixels() {
        let theme = test_theme_png_placeholders(&ALL_SLOTS.map(slot_name));
        let atlas = build(&theme, 1, 2048).unwrap();
        for slot in ALL_SLOTS {
            let entry = atlas.entries[&TextureId::Placeholder { slot }];
            assert_eq!((entry.w, entry.h), (4, 6), "{slot:?}");
            let [r, g, b] = placeholder_color(slot_name(slot));
            assert_eq!(
                pixel_at(&atlas, entry.x, entry.y),
                [r, g, b, 0xFF],
                "{slot:?}"
            );
        }
        // 1 white + 52 faces + 2 backs + 3 placeholders.
        assert_eq!(atlas.entries.len(), 58);
    }

    /// A theme may supply one placeholder and not the others; cataloging
    /// an undeclared slot would make the renderer claim pixels the theme
    /// never provided.
    #[test]
    fn only_declared_placeholders_are_catalogued() {
        for declared in ALL_SLOTS {
            let theme = test_theme_png_placeholders(&[slot_name(declared)]);
            let atlas = build(&theme, 1, 2048).unwrap();
            for slot in ALL_SLOTS {
                assert_eq!(
                    atlas.entries.contains_key(&TextureId::Placeholder { slot }),
                    slot == declared,
                    "declared {declared:?}, asked {slot:?}"
                );
            }
            assert_eq!(atlas.entries.len(), 56);
        }
    }

    #[test]
    fn strip_frames_scale_exactly_as_if_alone() {
        // The defining no-bleed property: at every xBRZ factor, each
        // frame of a strip back lands in the atlas byte-identical to
        // that frame rasterized as its own image — the neighboring
        // frame must contribute nothing.
        let theme = test_theme_png_corner_strip();
        for factor in [2_u32, 5] {
            let atlas = build(&theme, factor, 4096).unwrap();
            assert_eq!(atlas.factor, factor);
            let entry = atlas.entries[&TextureId::Back { back: 1, asset: 0 }];
            let (frame_w, frame_h) = (4 * factor, 6 * factor);
            for frame in 0..2_u32 {
                let alone = rasterize(&corner_frame_asset(frame), factor).unwrap();
                assert_eq!((alone.width, alone.height), (frame_w, frame_h));
                for y in 0..frame_h {
                    for x in 0..frame_w {
                        let got = pixel_at(&atlas, entry.x + frame * frame_w + x, entry.y + y);
                        let want = raster_pixel(&alone, x, y);
                        assert_eq!(got, want, "frame {frame} factor {factor} at ({x},{y})");
                    }
                }
            }
        }
    }

    /// One corner-strip frame wrapped as a standalone PNG asset — the
    /// isolation reference for [`strip_frames_scale_exactly_as_if_alone`].
    fn corner_frame_asset(index: u32) -> sol_theme::Asset {
        sol_theme::Asset {
            path: crate::testkit::asset_path(&format!("frame_{index}.png")),
            bytes: crate::testkit::corner_strip_frame_png(index),
            kind: sol_theme::AssetKind::Png,
            size: sol_theme::CardSize {
                width: 4,
                height: 6,
            },
        }
    }

    fn raster_pixel(raster: &Raster, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * raster.width + x) * 4) as usize;
        [
            raster.rgba[i],
            raster.rgba[i + 1],
            raster.rgba[i + 2],
            raster.rgba[i + 3],
        ]
    }

    #[test]
    fn build_steps_the_factor_down_until_it_fits() {
        let theme = test_theme_vector();
        // At factor 4 the cards would need 16x24 each; a 64-texel cap
        // cannot hold 54 of those, so the build steps down.
        let atlas = build(&theme, 4, 64).unwrap();
        assert!(atlas.factor < 4);
        let full = build(&theme, 4, 2048).unwrap();
        assert_eq!(full.factor, 4);
        let entry = full.entries[&TextureId::White];
        assert_eq!((entry.w, entry.h), (1, 1), "white never scales");
        let face = full.entries[&TextureId::Face {
            suit: Suit::Spades,
            rank: Rank::Ace,
        }];
        assert_eq!((face.w, face.h), (16, 24), "vector scales exactly");
    }

    #[test]
    fn build_overflows_on_an_impossible_cap() {
        let theme = test_theme_png();
        let error = build(&theme, 1, 8).unwrap_err();
        assert!(matches!(error, RenderError::AtlasOverflow { max_dim: 8 }));
        assert!(error.to_string().contains('8'));
    }

    #[test]
    fn plan_factor_matches_what_build_produces_without_rasterizing() {
        let theme = test_theme_vector();
        assert_eq!(plan_factor(&theme, 4, 2048).unwrap(), 4);
        let clamped = plan_factor(&theme, 4, 64).unwrap();
        assert_eq!(clamped, build(&theme, 4, 64).unwrap().factor);
        assert!(clamped < 4);
        assert!(matches!(
            plan_factor(&theme, 1, 8),
            Err(RenderError::AtlasOverflow { max_dim: 8 })
        ));
    }

    #[test]
    fn sheet_form_faces_slice_into_card_sized_entries() {
        let theme = test_theme_vector_sheet();
        for factor in [1_u32, 2] {
            let atlas = build(&theme, factor, 2048).unwrap();
            assert_eq!(atlas.factor, factor);
            // 1 white + 52 faces + 1 back — the sheet itself is not an
            // entry, and faces are card-sized, not sheet-sized.
            assert_eq!(atlas.entries.len(), 54);
            let ace = atlas.entries[&TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            }];
            assert_eq!((ace.w, ace.h), (4 * factor, 6 * factor));
            // Cell colors land at their entries: spades ace = cell (0,0),
            // clubs king = cell (12,3), hearts two = cell (1,1).
            for (suit, rank, col, row) in [
                (Suit::Spades, Rank::Ace, 0_u32, 0_u32),
                (Suit::Hearts, Rank::Two, 1, 1),
                (Suit::Clubs, Rank::King, 12, 3),
            ] {
                let entry = atlas.entries[&TextureId::Face { suit, rank }];
                let [r, g, b] = sheet_cell_color(col, row);
                // Sample the entry center: cell interiors are solid.
                let sample = pixel_at(&atlas, entry.x + entry.w / 2, entry.y + entry.h / 2);
                assert_eq!(sample, [r, g, b, 0xFF], "{suit:?} {rank:?} at {factor}x");
            }
        }
    }

    /// The vector sheet form's 52 cells share one asset. Rasterizing it
    /// once is not an optimization: 52 rasterizations of a full sheet at a
    /// high factor is enough memory to fail outright, so the sliced cells
    /// must all come from the same pixels.
    #[test]
    fn sheet_cells_all_slice_one_rasterization() {
        let theme = test_theme_vector_sheet();
        let atlas = build(&theme, 2, 4096).unwrap();
        let faces: Vec<_> = sol_engine::Suit::ALL
            .into_iter()
            .flat_map(|suit| {
                sol_engine::Rank::ALL
                    .into_iter()
                    .map(move |rank| TextureId::Face { suit, rank })
            })
            .collect();
        assert_eq!(faces.len(), 52);
        for id in faces {
            assert!(atlas.entries.contains_key(&id), "{id:?}");
        }
    }

    /// The property the test above cannot see: pixels alone cannot tell a
    /// once-rasterized sheet from 52 redundant rasterizations of it, since
    /// rasterizing the same asset at the same factor is deterministic. So
    /// this checks `rasterize_all`'s actual contract directly — every
    /// `SheetCell` slot in its per-item rasters stays `None` and the sheet
    /// comes back exactly once. A thread-per-catalog-item mistake that
    /// rasterizes the sheet again for each of the 52 cells would put
    /// `Some` in those slots, which no amount of pixel-comparing would
    /// otherwise reveal.
    #[test]
    fn rasterize_all_hoists_the_shared_sheet_out_of_the_per_item_rasters() {
        let theme = test_theme_vector_sheet();
        let items = catalog(&theme);
        let (rasters, sheet) = rasterize_all(&items, 2).unwrap();
        assert!(sheet.is_some(), "the shared sheet must be rasterized");
        let mut sheet_cells = 0;
        for (item, raster) in items.iter().zip(&rasters) {
            if matches!(item.source, CatalogSource::SheetCell { .. }) {
                sheet_cells += 1;
                assert!(
                    raster.is_none(),
                    "sheet cell {:?} must slice the shared sheet instead of \
                     carrying its own rasterization",
                    item.id
                );
            }
        }
        assert_eq!(sheet_cells, 52);
    }

    #[test]
    fn image_backgrounds_get_an_entry_with_their_pixels() {
        let theme = test_theme_png_image_bg();
        let atlas = build(&theme, 1, 2048).unwrap();
        let background = atlas.entries[&TextureId::Background];
        assert_eq!((background.w, background.h), (6, 4));
        assert_eq!(
            pixel_at(&atlas, background.x, background.y),
            [0xFF, 0, 0xFF, 0xFF]
        );
        assert_eq!(
            pixel_at(&atlas, background.x + 5, background.y + 3),
            [0xFF, 0, 0xFF, 0xFF]
        );
    }

    #[test]
    fn atlas_pixels_land_at_their_entries() {
        let theme = test_theme_png();
        let atlas = build(&theme, 1, 2048).unwrap();
        let white = atlas.entries[&TextureId::White];
        let at = |x: u32, y: u32| {
            let i = ((y * atlas.width + x) * 4) as usize;
            [
                atlas.rgba[i],
                atlas.rgba[i + 1],
                atlas.rgba[i + 2],
                atlas.rgba[i + 3],
            ]
        };
        assert_eq!(at(white.x, white.y), [0xFF, 0xFF, 0xFF, 0xFF]);
        // Padding texels around the white entry stay transparent.
        assert_eq!(at(white.x - 1, white.y), [0, 0, 0, 0]);
        assert_eq!(at(white.x + 1, white.y), [0, 0, 0, 0]);
        // The ace of spades fixture is solid opaque red.
        let face = atlas.entries[&TextureId::Face {
            suit: Suit::Spades,
            rank: Rank::Ace,
        }];
        assert_eq!(at(face.x, face.y), [0xFF, 0, 0, 0xFF]);
        assert_eq!(
            at(face.x + face.w - 1, face.y + face.h - 1),
            [0xFF, 0, 0, 0xFF]
        );
    }

    /// Every one of the 52 faces must carry *its own* card's pixels. The
    /// theme and the engine declare their suits in different orders, so a
    /// catalog that paired them by position instead of by identity would
    /// relabel all 52 faces at once — and still pass any test that checks
    /// only one card.
    #[test]
    fn every_face_entry_holds_its_own_cards_pixels() {
        let theme = test_theme_png();
        let atlas = build(&theme, 1, 2048).unwrap();
        for (index, (face_suit, face_rank)) in sol_theme::canonical_faces().enumerate() {
            let expected = face_color(u8::try_from(index).unwrap());
            let card = sol_engine::Card::from_index(
                super::engine_suit(face_suit).index() + 4 * (face_rank.get() - 1),
            );
            let entry = atlas.entries[&TextureId::Face {
                suit: card.suit,
                rank: card.rank,
            }];
            let i = ((entry.y * atlas.width + entry.x) * 4) as usize;
            assert_eq!(
                [atlas.rgba[i], atlas.rgba[i + 1], atlas.rgba[i + 2]],
                expected,
                "{face_suit:?} {} maps to {card}",
                face_rank.get()
            );
        }
    }
}
