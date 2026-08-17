//! Turning a rendered card-back contact sheet into the per-back,
//! per-frame PNG thumbnails a card-back picker's grid shows, and the two
//! numbers that bound how that sheet is rendered in the first place.
//!
//! [`sol_presenter::BackSheet`] lays out where every back's every frame
//! sits on one contact sheet; a renderer draws that layout once and hands
//! back its pixels (`sol_render_wgpu::render_to_rgba` is the one this
//! workspace ships). Neither crate touches an image codec or a windowing
//! toolkit, so the step left — slicing the rendered sheet apart at each
//! cell's own rectangle and encoding every piece ([`png_frames`]) —
//! belongs here, in the core both frontends already share, where it runs
//! without a GPU. Both frontends want the same format besides: QML loads a
//! thumbnail through a `data:` URI, and Windows' image list decodes one
//! through WIC, and PNG is what both take.
//!
//! [`sheet_scale`] and [`resolve_max_texture_dim`] are here for the same
//! reason: they answer "at what integer scale, and within what side, does
//! that sheet get laid out and rendered", they are plain arithmetic over a
//! number each frontend reads from its own toolkit, and the scale a sheet
//! was rendered at is the very scale [`png_frames`] has to cut it at — so
//! the two must never be able to drift apart per frontend.

use sol_presenter::{Rect, SheetCell};

/// The integer scale a card-back preview sheet renders at for a display
/// pixel ratio: the smallest whole factor at or above `dpr` (so a cell's
/// logical rectangle always multiplies to a whole number of physical
/// pixels), clamped to `1..=4` — the ceiling keeps even the densest
/// theme's sheet within the texture limit its layout is bounded by.
///
/// A non-finite `dpr` is answered with `1` up front: it compares false
/// against every bound below and would otherwise fall through to the
/// ceiling, which is the opposite of the safe reading of "no idea how
/// dense this display is". Zero and negative ratios need no such
/// treatment — they are already at or below the first bound, and land on
/// `1` the ordinary way.
#[must_use]
pub fn sheet_scale(dpr: f64) -> u32 {
    if !dpr.is_finite() {
        return 1;
    }
    if dpr <= 1.0 {
        1
    } else if dpr <= 2.0 {
        2
    } else if dpr <= 3.0 {
        3
    } else {
        4
    }
}

/// The texture-size ceiling every wgpu device is guaranteed to support at
/// minimum — the WebGL2-downlevel floor the renderer itself plans atlas
/// content factors against. [`resolve_max_texture_dim`] falls back to this
/// while a frontend's render thread has not yet captured the device's real
/// limit.
const FALLBACK_MAX_TEXTURE_DIM: u32 = 2048;

/// Resolves a captured texture-size-ceiling reading, where `0` means "not
/// yet captured" (the window between a frontend's render thread starting
/// and its first store) rather than a real answer: a sheet laid out
/// against the guaranteed floor still fits once the real ceiling is known.
#[must_use]
pub const fn resolve_max_texture_dim(stored: u32) -> u32 {
    if stored == 0 {
        FALLBACK_MAX_TEXTURE_DIM
    } else {
        stored
    }
}

/// Every way turning a rendered card-back sheet into per-thumbnail PNGs
/// can fail.
///
/// Each variant means the same underlying thing: the pixels and the layout
/// disagree, most likely because they were produced for different themes or
/// a stale one. [`png_frames`] never returns a partial grid over any of
/// these — a frontend falls back to naming the backs instead of showing a
/// grid with a hole in it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreviewError {
    /// `sheet`'s byte length does not match `sheet_size`.
    #[error("sheet is {actual} bytes, expected {expected}")]
    SheetSizeMismatch {
        /// The tightly packed RGBA8 length `sheet_size` implies.
        expected: usize,
        /// The length `sheet` actually has.
        actual: usize,
    },
    /// A cell's rectangle, scaled, does not lie within the sheet: a
    /// negative origin, or an extent that runs past the right or the
    /// bottom edge. Both edges are checked explicitly before any pixel is
    /// read, so an out-of-range cell always ends up here rather than as a
    /// panic.
    #[error("back {back} frame {frame}'s cell lies outside the sheet")]
    CellOutOfBounds {
        /// The offending cell's back index.
        back: usize,
        /// The offending cell's frame index.
        frame: u32,
    },
    /// The PNG encoder rejected a thumbnail.
    #[error("back {back} frame {frame}'s thumbnail failed to encode")]
    Encode {
        /// The offending cell's back index.
        back: usize,
        /// The offending cell's frame index.
        frame: u32,
        /// The underlying encoder failure.
        #[source]
        source: png::EncodingError,
    },
}

/// Cuts `sheet` apart at each of `cells`' rectangles and encodes every
/// piece as its own 8-bit RGBA PNG, **indexed by back**: the returned
/// `Vec` is exactly `back_count` entries long, `[back]` holds that back's
/// own thumbnails in the order its cells appear, and the innermost `Vec`
/// is one frame's PNG bytes.
///
/// Indexing by the theme's own back order — rather than handing back one
/// group per back that happened to contribute a cell — is what keeps a
/// thumbnail's card back from being lost on the way out. A back is allowed
/// to declare no frames at all, and then contributes no cell, so a
/// positional list of groups would shift every later back's thumbnails
/// onto the wrong name; `[back]` is simply empty for such a back instead,
/// wherever in the order it sits. `back_count` is how many backs the theme
/// declares, which is what a picker draws a row for — not how many of them
/// this sheet could picture.
///
/// `sheet` is `sheet_size` **physical** pixels, tightly packed RGBA8 rows
/// — what a renderer's one-shot readback produces. `cells` are the
/// **logical** rectangles the layout placed the sheet's cells at, exactly
/// as [`sol_presenter::BackSheet::cells`] provides them, and `scale` the
/// integer factor the sheet was rendered at ([`sheet_scale`]), so a cell's
/// physical rectangle is its own rectangle times `scale` on every axis.
///
/// An empty `cells` is not an error: a theme whose backs could not be
/// pictured is not a mismatch, it just leaves every entry empty.
///
/// A cell naming a back at or past `back_count` is skipped rather than
/// widening the result. The cells and the count describe the same theme
/// for every caller in this workspace, so they can only disagree if a
/// layout is paired with a foreign count — and inventing an entry past
/// what the caller declared would hand it a thumbnail it has no name to
/// put under.
///
/// Nothing partial is ever returned. The first cell that cannot be sliced
/// or encoded fails the whole call, because a render and a layout that
/// disagree on one cell disagree on all of them, and a grid missing one
/// thumbnail serves a chooser no better than an empty one.
///
/// # Errors
///
/// [`PreviewError::SheetSizeMismatch`] if `sheet`'s length does not match
/// `sheet_size`; [`PreviewError::CellOutOfBounds`] if a cell's scaled
/// rectangle does not lie within the sheet; [`PreviewError::Encode`] if the
/// PNG encoder rejects a thumbnail.
pub fn png_frames(
    sheet: &[u8],
    sheet_size: (u32, u32),
    cells: &[SheetCell],
    scale: u32,
    back_count: usize,
) -> Result<Vec<Vec<Vec<u8>>>, PreviewError> {
    let expected = expected_len(sheet_size);
    if sheet.len() != expected {
        return Err(PreviewError::SheetSizeMismatch {
            expected,
            actual: sheet.len(),
        });
    }

    let mut frames: Vec<Vec<Vec<u8>>> = vec![Vec::new(); back_count];
    for cell in cells {
        let slice = slice_cell(sheet, sheet_size, cell.rect, scale).ok_or(
            PreviewError::CellOutOfBounds {
                back: cell.back,
                frame: cell.frame,
            },
        )?;
        let png = encode_png(&slice, cell.back, cell.frame)?;
        if let Some(back_frames) = frames.get_mut(cell.back) {
            back_frames.push(png);
        }
    }
    Ok(frames)
}

/// One cell's physical pixels: its scaled size and its tightly packed
/// RGBA8 bytes, cut from the sheet.
struct Slice {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// The tightly packed RGBA8 byte length a `sheet_size` implies.
///
/// Saturates rather than panicking or reporting `usize` overflow as its own
/// failure: no real render ever produces a sheet whose byte length would
/// not fit `usize`, so a saturated `expected` still disagrees with
/// `sheet.len()` and the mismatch is reported correctly either way.
fn expected_len((width, height): (u32, u32)) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4)
}

/// A logical coordinate or extent scaled to physical pixels, or `None` if
/// it does not fit a `u32` — which only happens for a negative `value`
/// (including a negative cell origin) or a product past `u32::MAX`.
fn physical(value: i32, scale: u32) -> Option<u32> {
    u32::try_from(i64::from(value) * i64::from(scale)).ok()
}

/// One cell's physical pixels, cut from `sheet`: `None` if `rect` scaled by
/// `scale` does not fit a `u32` on any axis, or if the resulting rectangle
/// runs past `sheet_size` on the right or the bottom.
///
/// Both edges are checked explicitly, before `stride`, any row offset, or
/// the output `Vec`'s capacity are computed, each as a widened `u64`
/// comparison so the check itself cannot overflow. Neither edge can be left
/// to the row read below instead: a too-wide row does not fall off
/// `sheet`'s end at all, it just spills into the next row's bytes and
/// silently returns the wrong pixels rather than failing. A too-tall row is
/// worse: the arithmetic computing its byte offset can overflow before that
/// read ever runs, and for large enough values it panics outright rather
/// than merely producing an offset large enough for the read to refuse.
fn slice_cell(sheet: &[u8], sheet_size: (u32, u32), rect: Rect, scale: u32) -> Option<Slice> {
    let x = physical(rect.x, scale)?;
    let y = physical(rect.y, scale)?;
    let width = physical(rect.w, scale)?;
    let height = physical(rect.h, scale)?;
    let (sheet_w, sheet_h) = sheet_size;
    if u64::from(x) + u64::from(width) > u64::from(sheet_w) {
        return None;
    }
    if u64::from(y) + u64::from(height) > u64::from(sheet_h) {
        return None;
    }

    let stride = (sheet_w as usize) * 4;
    let col_start = (x as usize) * 4;
    let col_end = col_start + (width as usize) * 4;
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for row in 0..height as usize {
        let offset = (y as usize + row) * stride;
        rgba.extend_from_slice(sheet.get(offset + col_start..offset + col_end)?);
    }
    Some(Slice {
        width,
        height,
        rgba,
    })
}

/// Encodes one cell's pixels as an 8-bit RGBA PNG, naming `back`/`frame` in
/// any [`PreviewError::Encode`] this raises.
///
/// `write_header` and `write_image_data` are both `png::EncodingError`
/// sources for the same failure (this thumbnail didn't encode), so they
/// share one `fail` mapping rather than each restating the same
/// `PreviewError::Encode` construction: the two calls are structurally
/// identical failure points, not two different kinds of failure.
fn encode_png(slice: &Slice, back: usize, frame: u32) -> Result<Vec<u8>, PreviewError> {
    let fail = |source| PreviewError::Encode {
        back,
        frame,
        source,
    };
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, slice.width, slice.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(fail)?;
        writer.write_image_data(&slice.rgba).map_err(fail)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_presenter::Pt;

    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];
    const BG: [u8; 4] = [10, 10, 10, 255];

    /// A raw RGBA8 `size` sheet: `background` everywhere except each
    /// `(rect, color)` in `regions`, painted verbatim — the synthetic
    /// stand-in for a rendered contact sheet these tests slice apart.
    fn paint_sheet(size: (u32, u32), background: [u8; 4], regions: &[(Rect, [u8; 4])]) -> Vec<u8> {
        let (width, height) = size;
        let mut sheet = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let pt = Pt::new(i32::try_from(x).unwrap(), i32::try_from(y).unwrap());
                let color = regions
                    .iter()
                    .find(|(rect, _)| rect.contains(pt))
                    .map_or(background, |(_, color)| *color);
                sheet.extend_from_slice(&color);
            }
        }
        sheet
    }

    /// `width × height` RGBA8 pixels, every one `color`.
    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        color.repeat((width * height) as usize)
    }

    /// Decodes `bytes` as a PNG: `(width, height, straight-alpha RGBA8
    /// pixels)`, for asserting on what [`png_frames`] actually sliced
    /// rather than on its raw byte blob.
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        buffer.truncate(info.buffer_size());
        (info.width, info.height, buffer)
    }

    /// Two side-by-side logical cells, back 0 and back 1, one frame each,
    /// 2×2 with a 2px gutter — reused at different render scales.
    fn two_backs() -> Vec<SheetCell> {
        vec![
            SheetCell {
                back: 0,
                frame: 0,
                rect: Rect::new(0, 0, 2, 2),
            },
            SheetCell {
                back: 1,
                frame: 0,
                rect: Rect::new(4, 0, 2, 2),
            },
        ]
    }

    #[test]
    fn each_cell_becomes_its_own_pixels_under_its_own_back() {
        let cells = two_backs();
        let sheet = paint_sheet((6, 2), BG, &[(cells[0].rect, RED), (cells[1].rect, BLUE)]);
        let frames = png_frames(&sheet, (6, 2), &cells, 1, 2).unwrap();

        assert_eq!(frames.len(), 2, "one entry per declared back");
        assert_eq!(frames[0].len(), 1);
        assert_eq!(frames[1].len(), 1);
        assert_eq!(decode(&frames[0][0]), (2, 2, solid(2, 2, RED)));
        assert_eq!(decode(&frames[1][0]), (2, 2, solid(2, 2, BLUE)));
    }

    #[test]
    fn a_backs_frames_stay_in_frame_order() {
        let cells = vec![
            SheetCell {
                back: 0,
                frame: 0,
                rect: Rect::new(0, 0, 2, 2),
            },
            SheetCell {
                back: 0,
                frame: 1,
                rect: Rect::new(4, 0, 2, 2),
            },
        ];
        let sheet = paint_sheet(
            (6, 2),
            BG,
            &[(cells[0].rect, GREEN), (cells[1].rect, YELLOW)],
        );
        let frames = png_frames(&sheet, (6, 2), &cells, 1, 1).unwrap();

        assert_eq!(frames.len(), 1, "one back");
        assert_eq!(frames[0].len(), 2, "both its frames");
        assert_eq!(
            decode(&frames[0][0]),
            (2, 2, solid(2, 2, GREEN)),
            "frame 0 decodes first"
        );
        assert_eq!(
            decode(&frames[0][1]),
            (2, 2, solid(2, 2, YELLOW)),
            "frame 1 decodes second, not swapped with frame 0"
        );
    }

    /// The hazard the back-indexed shape exists to remove: a back that
    /// declares no frames contributes no cell, so anything that paired
    /// thumbnails with backs by position would slide every later back's
    /// artwork onto the wrong name. Checked in all three places a gap can
    /// sit — leading, middle, and trailing — since only the middle case
    /// distinguishes "skips the gap" from "starts one back late".
    #[test]
    fn a_frameless_back_leaves_its_own_entry_empty_and_shifts_no_other() {
        let sheet = paint_sheet(
            (6, 2),
            BG,
            &[(Rect::new(0, 0, 2, 2), RED), (Rect::new(4, 0, 2, 2), BLUE)],
        );
        let cell = |back: usize, x: i32| SheetCell {
            back,
            frame: 0,
            rect: Rect::new(x, 0, 2, 2),
        };
        let red = (2, 2, solid(2, 2, RED));
        let blue = (2, 2, solid(2, 2, BLUE));

        // Middle back frameless: backs 0 and 2 picture, back 1 does not.
        let frames = png_frames(&sheet, (6, 2), &[cell(0, 0), cell(2, 4)], 1, 3).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(decode(&frames[0][0]), red);
        assert!(frames[1].is_empty(), "the frameless back keeps its own gap");
        assert_eq!(
            decode(&frames[2][0]),
            blue,
            "the back after the gap keeps its own thumbnail"
        );

        // Leading back frameless.
        let frames = png_frames(&sheet, (6, 2), &[cell(1, 0), cell(2, 4)], 1, 3).unwrap();
        assert!(frames[0].is_empty());
        assert_eq!(decode(&frames[1][0]), red);
        assert_eq!(decode(&frames[2][0]), blue);

        // Trailing back frameless.
        let frames = png_frames(&sheet, (6, 2), &[cell(0, 0), cell(1, 4)], 1, 3).unwrap();
        assert_eq!(decode(&frames[0][0]), red);
        assert_eq!(decode(&frames[1][0]), blue);
        assert!(frames[2].is_empty());
    }

    /// A layout and a back count that describe different themes: the
    /// cells naming a back the caller never declared are dropped rather
    /// than growing the result past what it asked for.
    #[test]
    fn a_cell_naming_a_back_past_the_count_is_skipped() {
        let cells = two_backs();
        let sheet = paint_sheet((6, 2), BG, &[(cells[0].rect, RED), (cells[1].rect, BLUE)]);
        let frames = png_frames(&sheet, (6, 2), &cells, 1, 1).unwrap();
        assert_eq!(frames.len(), 1, "never wider than the declared count");
        assert_eq!(decode(&frames[0][0]), (2, 2, solid(2, 2, RED)));
    }

    #[test]
    fn scale_multiplies_every_rectangle() {
        let cells = two_backs();
        let sheet = paint_sheet(
            (12, 4),
            BG,
            &[(Rect::new(0, 0, 4, 4), RED), (Rect::new(8, 0, 4, 4), BLUE)],
        );
        let frames = png_frames(&sheet, (12, 4), &cells, 2, 2).unwrap();

        assert_eq!(
            decode(&frames[0][0]),
            (4, 4, solid(4, 4, RED)),
            "a 2-wide logical cell reads 4 physical pixels wide at scale 2"
        );
        assert_eq!(decode(&frames[1][0]), (4, 4, solid(4, 4, BLUE)));
    }

    #[test]
    fn a_sheet_whose_byte_length_contradicts_its_size_is_refused() {
        let sheet = vec![0_u8; 10];
        let error = png_frames(&sheet, (6, 2), &[], 1, 0).unwrap_err();
        assert!(matches!(
            error,
            PreviewError::SheetSizeMismatch {
                expected: 48,
                actual: 10
            }
        ));
    }

    #[test]
    fn a_cell_running_off_the_right_or_bottom_edge_is_refused() {
        let sheet = vec![0_u8; 6 * 2 * 4];
        let off_right = SheetCell {
            back: 0,
            frame: 0,
            rect: Rect::new(5, 0, 2, 2),
        };
        assert!(matches!(
            png_frames(&sheet, (6, 2), &[off_right], 1, 2),
            Err(PreviewError::CellOutOfBounds { back: 0, frame: 0 })
        ));

        let off_bottom = SheetCell {
            back: 1,
            frame: 2,
            rect: Rect::new(0, 1, 2, 2),
        };
        assert!(matches!(
            png_frames(&sheet, (6, 2), &[off_bottom], 1, 2),
            Err(PreviewError::CellOutOfBounds { back: 1, frame: 2 })
        ));
    }

    #[test]
    fn a_cell_with_a_negative_origin_is_refused() {
        let sheet = vec![0_u8; 6 * 2 * 4];
        let cell = SheetCell {
            back: 2,
            frame: 3,
            rect: Rect::new(-1, 0, 2, 2),
        };
        assert!(matches!(
            png_frames(&sheet, (6, 2), &[cell], 1, 3),
            Err(PreviewError::CellOutOfBounds { back: 2, frame: 3 })
        ));
    }

    /// A cell flush against the sheet's right edge still fits: the width
    /// check is `<=`, not the stricter `<` that would wrongly refuse it. A
    /// cell flush against the bottom edge fits too, for the same reason on
    /// the height check.
    #[test]
    fn a_cell_flush_with_the_sheet_edge_on_either_axis_is_accepted() {
        let cells = vec![
            SheetCell {
                back: 0,
                frame: 0,
                rect: Rect::new(4, 0, 2, 2),
            },
            SheetCell {
                back: 1,
                frame: 0,
                rect: Rect::new(0, 2, 2, 2),
            },
        ];
        let sheet = paint_sheet((6, 4), BG, &[(cells[0].rect, RED), (cells[1].rect, BLUE)]);
        let frames = png_frames(&sheet, (6, 4), &cells, 1, 2).unwrap();
        assert_eq!(decode(&frames[0][0]), (2, 2, solid(2, 2, RED)));
        assert_eq!(decode(&frames[1][0]), (2, 2, solid(2, 2, BLUE)));
    }

    /// A sheet can claim an astronomical width while still being an empty
    /// buffer, because `sheet_h = 0` makes `expected_len` `0` no matter how
    /// large `sheet_w` is — the byte-length check alone bounds only the
    /// *product* of the two dimensions, not `sheet_w` in isolation. A cell
    /// whose scaled `y` is itself astronomical (from an `i32::MAX` origin)
    /// then pairs with that huge `sheet_w` to make the row-offset
    /// multiplication overflow `usize` before any read of `sheet` runs, if
    /// the bottom edge is not rejected first. This must come back as
    /// [`PreviewError::CellOutOfBounds`], not a panic.
    #[test]
    fn a_cell_whose_row_offset_would_overflow_is_refused_not_panicked() {
        let cell = SheetCell {
            back: 0,
            frame: 0,
            rect: Rect::new(0, i32::MAX, 1, 1),
        };
        assert!(matches!(
            png_frames(&[], (u32::MAX, 0), &[cell], 1, 1),
            Err(PreviewError::CellOutOfBounds { back: 0, frame: 0 })
        ));
    }

    /// Same shape as
    /// [`a_cell_whose_row_offset_would_overflow_is_refused_not_panicked`],
    /// but this time it is the cell's own `width`/`height` — scaled up by
    /// `scale` from an `i32::MAX` rectangle — that overflow the output
    /// buffer's capacity computation, rather than the row offset.
    #[test]
    fn a_cell_whose_capacity_would_overflow_is_refused_not_panicked() {
        let cell = SheetCell {
            back: 1,
            frame: 2,
            rect: Rect::new(0, 0, i32::MAX, i32::MAX),
        };
        assert!(matches!(
            png_frames(&[], (u32::MAX, 0), &[cell], 2, 2),
            Err(PreviewError::CellOutOfBounds { back: 1, frame: 2 })
        ));
    }

    /// A sheet that pictured nothing still describes the theme's backs:
    /// every declared back gets its own empty entry, so a picker draws a
    /// row per back and simply has no thumbnail to put in it.
    #[test]
    fn an_empty_layout_yields_one_empty_entry_per_declared_back() {
        let sheet = vec![0_u8; 6 * 2 * 4];
        let frames = png_frames(&sheet, (6, 2), &[], 1, 3).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(Vec::is_empty));

        let frames = png_frames(&sheet, (6, 2), &[], 1, 0).unwrap();
        assert!(frames.is_empty(), "a theme declaring no backs at all");
    }

    /// A cell that cannot be cut fails the whole call even when an earlier
    /// cell was perfectly valid: nothing partial ever comes back.
    #[test]
    fn a_later_bad_cell_fails_the_whole_call_rather_than_a_partial_result() {
        let cells = vec![
            SheetCell {
                back: 0,
                frame: 0,
                rect: Rect::new(0, 0, 2, 2),
            },
            SheetCell {
                back: 1,
                frame: 0,
                rect: Rect::new(5, 0, 2, 2),
            },
        ];
        let sheet = paint_sheet((6, 2), BG, &[(cells[0].rect, RED)]);
        assert!(matches!(
            png_frames(&sheet, (6, 2), &cells, 1, 2),
            Err(PreviewError::CellOutOfBounds { back: 1, frame: 0 })
        ));
    }

    /// A cell whose scaled rectangle fits the sheet but is zero-wide is
    /// still refused: the encoder itself rejects a zero dimension, which
    /// this crate surfaces as `PreviewError::Encode` rather than a silent
    /// empty image.
    #[test]
    fn a_degenerate_cell_that_fails_to_encode_reports_the_encoder_error() {
        let sheet = vec![0_u8; 6 * 2 * 4];
        let cell = SheetCell {
            back: 4,
            frame: 5,
            rect: Rect::new(0, 0, 0, 2),
        };
        assert!(matches!(
            png_frames(&sheet, (6, 2), &[cell], 1, 5),
            Err(PreviewError::Encode {
                back: 4,
                frame: 5,
                source: png::EncodingError::Format(_)
            })
        ));
    }

    #[test]
    fn sheet_scale_rounds_up_and_clamps_to_four() {
        assert_eq!(sheet_scale(1.0), 1);
        assert_eq!(sheet_scale(1.0001), 2, "any fraction rounds up");
        assert_eq!(sheet_scale(2.0), 2);
        assert_eq!(sheet_scale(2.5), 3);
        assert_eq!(sheet_scale(3.0), 3);
        assert_eq!(sheet_scale(3.5), 4);
        assert_eq!(sheet_scale(4.0), 4);
        assert_eq!(sheet_scale(5.0), 4, "clamped to the ceiling");
        assert_eq!(sheet_scale(f64::NAN), 1, "non-finite reads as 1x");
        assert_eq!(sheet_scale(f64::INFINITY), 1, "non-finite reads as 1x");
        assert_eq!(sheet_scale(f64::NEG_INFINITY), 1, "non-finite reads as 1x");
        assert_eq!(sheet_scale(0.0), 1, "a nonsense ratio lands on the floor");
        assert_eq!(sheet_scale(-2.0), 1, "a nonsense ratio lands on the floor");
    }

    #[test]
    fn max_texture_dim_falls_back_to_2048_only_when_unset() {
        assert_eq!(resolve_max_texture_dim(0), 2048);
        assert_eq!(resolve_max_texture_dim(4096), 4096);
        assert_eq!(
            resolve_max_texture_dim(1),
            1,
            "even an implausibly small real reading passes through untouched"
        );
    }
}
