//! Tests for the explicit-scale render and one-shot RGBA readback: a
//! card-back contact sheet drawn through `Renderer::render_at` /
//! `render_to_rgba`, at a scale of its own rather than the renderer's
//! adopted one; and `render_at`'s own non-finite/non-positive scale
//! fallback — a guard `render` never exercises, since every write site to
//! `self.display_scale` (`Renderer::new`, `adopt_scale`,
//! `set_display_scale`) already sanitizes before `render` ever reads it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use sol_engine::{Rank, Seed, Suit};
use sol_presenter::{DisplayList, Presenter, Rect, Rgba, Size, Sprite, TextureId};
use sol_render_wgpu::{Renderer, render_to_rgba};
use sol_session::{Options, Session};
use sol_theme::CardScaling;

const BLACK: [u8; 4] = [0, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// The sheet draws at its own scale (2) while the renderer stays adopted
/// at a different one (3): proves `render_at`/`render_to_rgba` neither
/// borrow the adopted scale nor disturb it, the loaded atlas, or the
/// planned factor, and that the readback is tightly packed even though
/// the target's row byte width (32 × 4 = 128) is not itself a multiple of
/// wgpu's copy row alignment.
#[test]
fn back_sheet_renders_at_its_own_scale_with_a_tightly_packed_readback() {
    let Some(gpu) = common::gpu() else { return };

    let theme = common::tiny_png_theme();
    let presenter = Presenter::new(
        Session::new(Options::default(), Seed::new(1).unwrap()),
        &theme,
    );
    let sheet = presenter
        .back_sheet(Rgba::opaque(0, 0, 0), 1000)
        .expect("plain's one frame plus strip's two fit one generous row");
    assert_eq!(sheet.cells.len(), 3, "plain's one frame plus strip's two");
    assert_eq!(
        sheet.size,
        Size::new(16, 6),
        "three 4x6 cells and two 2px gutters between them"
    );

    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        theme,
        CardScaling::Original,
        3.0,
    )
    .expect("renderer");
    assert_eq!(
        renderer.max_texture_dim(),
        gpu.device.limits().max_texture_dimension_2d,
        "exposes the same device limit atlas planning already uses"
    );

    // The sheet's own logical size (16x6), doubled at the sheet's own
    // render scale of 2 — not the renderer's adopted scale of 3.
    let physical = (32, 12);
    let pixels = render_to_rgba(
        &gpu.device,
        &gpu.queue,
        &mut renderer,
        &sheet.list,
        physical,
        2.0,
    )
    .expect("render_to_rgba");

    assert_eq!(
        pixels.len(),
        (physical.0 * physical.1 * 4) as usize,
        "tightly packed, no GPU row padding"
    );

    // Cell 0 (plain, frame 0) covers physical x 0..8; cell 1 (strip,
    // frame 0) x 12..20; cell 2 (strip, frame 1) x 24..32 — each preceded
    // by a doubled 2px logical gutter (4 physical pixels). Every sample
    // sits at its region's center, well clear of any edge.
    assert_eq!(
        common::pixel_at(&pixels, 32, 4, 6),
        BLUE,
        "cell 0: the plain back's own color"
    );
    assert_eq!(
        common::pixel_at(&pixels, 32, 10, 6),
        BLACK,
        "the gutter before cell 1 is bare background, not a neighbor's sliver"
    );
    assert_eq!(
        common::pixel_at(&pixels, 32, 16, 6),
        GREEN,
        "cell 1: the strip back's frame 0"
    );
    assert_eq!(
        common::pixel_at(&pixels, 32, 22, 6),
        BLACK,
        "the gutter before cell 2 is bare background too"
    );
    assert_eq!(
        common::pixel_at(&pixels, 32, 28, 6),
        YELLOW,
        "cell 2: the strip back's frame 1, sliced rather than repeating frame 0"
    );

    assert_eq!(
        renderer.atlas_factor(),
        1,
        "a png theme at Original holds factor 1 regardless of render_at, \
         proving render_at disturbed nothing"
    );
}

/// One ace-of-spades sprite over a felt clear, positioned away from the
/// target's corners and center so a sign flip (a negative scale) or a
/// collapse to a point (a zero scale) cannot coincidentally reproduce the
/// scale-1.0 layout by symmetry.
fn ace_frame() -> DisplayList {
    DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: vec![Sprite {
            texture: TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            },
            src: Rect::new(0, 0, 4, 6),
            dst: Rect::new(9, 7, 4, 6),
            z: 0,
            tint: Rgba::WHITE,
        }],
    }
}

/// `render_at` (reached here through `render_to_rgba`, its only caller
/// able to hand it a scale directly) re-implements `adopt_scale`'s
/// `is_finite() && > 0.0` sanitizer rather than sharing it, and `render`'s
/// own call site never exercises a degenerate value — every write to
/// `self.display_scale` is already sanitized before `render` reads it.
/// This pins both halves of the guard by comparing rendered output against
/// an explicit scale of 1.0, so a mutant weakening either half (dropping
/// `is_finite()`, dropping `> 0.0`, or loosening it to `>= 0.0`) changes
/// the rendered pixels and fails the assertion:
///
/// - `f32::INFINITY` is the pivotal case: it fails `is_finite()` but
///   passes `scale > 0.0`. Every other value below already fails a bare
///   `scale > 0.0` on its own (IEEE 754 comparisons against NaN are always
///   `false`), so without infinity a mutant that deleted `is_finite()` and
///   kept only `scale > 0.0` would sanitize NaN, 0.0 and -7.0 exactly like
///   the real guard does and slip past unnoticed.
/// - NaN fails `is_finite()` too, and is the shape a real caller would
///   actually hit — a layout division degenerating to `0.0 / 0.0`.
/// - 0.0 fails `> 0.0` right at the boundary, catching a `>= 0.0`
///   weakening that infinity and NaN both sail past (`is_finite()` is
///   already false for both).
/// - -7.0 fails `> 0.0` well clear of the boundary, catching the whole
///   `> 0.0` half being dropped rather than merely loosened at the edge.
#[test]
fn render_at_sanitizes_non_finite_and_non_positive_scale_to_one() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Original,
        1.0,
    )
    .expect("renderer");

    let list = ace_frame();
    let baseline = render_to_rgba(&gpu.device, &gpu.queue, &mut renderer, &list, (32, 32), 1.0)
        .expect("baseline render at an explicit scale of 1.0");
    // Sane fixture guard: if this were empty or uniform, the comparisons
    // below could pass by accident (e.g. an all-felt image matches an
    // all-felt image regardless of scale).
    assert!(
        baseline.chunks_exact(4).any(|px| px != [0, 128, 0, 255]),
        "the baseline must actually draw the card, not just clear"
    );

    for degenerate in [f32::INFINITY, f32::NAN, 0.0, -7.0] {
        let pixels = render_to_rgba(
            &gpu.device,
            &gpu.queue,
            &mut renderer,
            &list,
            (32, 32),
            degenerate,
        )
        .expect("render_to_rgba");
        assert_eq!(
            pixels, baseline,
            "scale {degenerate} must render exactly like an explicit 1.0"
        );
    }
}
