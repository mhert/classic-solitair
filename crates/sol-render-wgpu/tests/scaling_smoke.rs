//! Smoke tests for the png and vector scaling pipelines: the atlas is
//! rebuilt at the planned content factor, and a rendered frame contains
//! sane pixels. No golden lock here — resvg's exact output belongs to its
//! own version.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use sol_engine::{Rank, Seed, Suit};
use sol_presenter::{DisplayList, Presenter, Rect, Rgba, Sprite, TextureId};
use sol_render_wgpu::Renderer;
use sol_session::{Options, Session};
use sol_theme::CardScaling;

const FELT: [u8; 4] = [0, 128, 0, 255];

#[allow(clippy::cast_precision_loss)] // tiny test factors
fn scale(n: u32) -> f32 {
    n as f32
}

/// One ace-of-spades sprite over a felt clear.
fn ace_frame(card: Rect) -> DisplayList {
    DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: vec![Sprite {
            texture: TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            },
            src: Rect::new(0, 0, 4, 6),
            dst: card,
            z: 0,
            tint: Rgba::WHITE,
        }],
    }
}

/// A fractional display scale: the atlas covers ceil(scale) and the
/// scene transform stretches the logical quads to physical pixels. Needs
/// a `vector` theme — a `png` theme's atlas factor never tracks the
/// display scale at all; it depends only on the chosen `CardScaling`.
#[test]
fn fractional_display_scale_fills_the_physical_surface() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    renderer
        .set_display_scale(&gpu.device, &gpu.queue, 1.5)
        .expect("set_display_scale");
    assert_eq!(renderer.atlas_factor(), 2, "atlas covers ceil(1.5)");

    // One 4×6 red card at logical (10, 10): at 1.5× it covers physical
    // x 15..21, y 15..24.
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(10, 10, 4, 6))],
    );
    let center = common::pixel_at(&pixels, 64, 18, 19);
    assert!(center[0] > 200, "card interior is red at 1.5x: {center:?}");
    assert_eq!(
        common::pixel_at(&pixels, 64, 12, 12),
        FELT,
        "left of the scaled card stays felt"
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 40, 40),
        FELT,
        "far felt stays felt"
    );

    // Back to 1.0: the same logical list draws at native size again.
    renderer
        .set_display_scale(&gpu.device, &gpu.queue, 1.0)
        .expect("set_display_scale");
    assert_eq!(renderer.atlas_factor(), 1, "scale 1.0 is the native atlas");
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(10, 10, 4, 6))],
    );
    assert!(common::pixel_at(&pixels, 64, 11, 11)[0] > 200);
    assert_eq!(common::pixel_at(&pixels, 64, 18, 19), FELT);
}

/// Degenerate scales fall back to 1.0 instead of poisoning the
/// transform or the atlas policy.
#[test]
fn non_finite_scales_fall_back_to_native() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Original,
        f32::NAN,
    )
    .expect("renderer");
    assert_eq!(renderer.atlas_factor(), 1, "NaN builds the native atlas");
    renderer
        .set_display_scale(&gpu.device, &gpu.queue, f32::INFINITY)
        .expect("set_display_scale");
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        32,
        32,
        &[ace_frame(Rect::new(10, 10, 4, 6))],
    );
    assert!(
        common::pixel_at(&pixels, 32, 11, 11)[0] > 200,
        "renders at the native fallback scale"
    );
    assert_eq!(common::pixel_at(&pixels, 32, 18, 19), FELT);
}

/// A frame larger than the initial quad capacity grows the vertex and
/// index buffers instead of truncating.
#[test]
fn oversized_frames_grow_the_batch_buffers() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    let sprites = (0..80_i32)
        .map(|i| Sprite {
            texture: TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            },
            src: Rect::new(0, 0, 4, 6),
            dst: Rect::new(i % 8, i / 8, 4, 6),
            z: i,
            tint: Rgba::WHITE,
        })
        .collect();
    let frame = DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites,
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 32, 32, &[frame]);
    // Sprite `i` covers x in [i%8, i%8+4) and y in [i/8, i/8+6), so the
    // first 64 reach no further down than y = 12. A pixel at y = 13 is
    // therefore covered only by sprites past the batch capacity — which is
    // the whole question: a probe inside the first 64's area cannot tell 64
    // drawn quads from 80.
    assert!(
        common::pixel_at(&pixels, 32, 7, 13)[0] > 200,
        "the sprites past the batch capacity are drawn too"
    );
    // And nothing runs away past them: no sprite reaches (20, 20), so it
    // must still be the clear color.
    assert_eq!(
        common::pixel_at(&pixels, 32, 20, 20),
        [0, 128, 0, 255],
        "a pixel no sprite covers stays the clear color"
    );
}

#[test]
fn strip_back_frames_sample_their_slice() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    // Frame 1 of the two-frame strip back: src is the second 4×6 slice of
    // the 8×6 strip asset — the yellow half.
    let frame = DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: vec![Sprite {
            texture: TextureId::Back { back: 1, asset: 0 },
            src: Rect::new(4, 0, 4, 6),
            dst: Rect::new(10, 10, 4, 6),
            z: 0,
            tint: Rgba::WHITE,
        }],
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 32, 32, &[frame]);
    assert_eq!(
        common::pixel_at(&pixels, 32, 12, 13),
        [255, 255, 0, 255],
        "the yellow frame, not the green one"
    );
}

#[test]
fn image_backgrounds_resolve_and_render() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_bg_image_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    // What the presenter emits for a stretched image background: the full
    // asset stretched over the viewport, under a black clear.
    let frame = DisplayList {
        clear: Some(Rgba::opaque(0, 0, 0)),
        sprites: vec![Sprite {
            texture: TextureId::Background,
            src: Rect::new(0, 0, 6, 4),
            dst: Rect::new(0, 0, 32, 32),
            z: 0,
            tint: Rgba::WHITE,
        }],
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 32, 32, &[frame]);
    assert_eq!(
        common::pixel_at(&pixels, 32, 16, 16),
        [255, 0, 255, 255],
        "the magenta table image covers the viewport"
    );
}

#[test]
fn vector_rerasterizes_at_the_exact_size_and_renders() {
    let Some(gpu) = common::gpu() else { return };
    let theme = common::default_theme();
    let mut presenter = Presenter::new(
        Session::new(Options::default(), Seed::new(1).unwrap()),
        &theme,
    );
    presenter.key_down();
    presenter.fit_viewport(1170, 768);

    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        theme,
        CardScaling::Original,
        scale(2),
    )
    .expect("renderer");
    assert_eq!(renderer.atlas_factor(), 2, "resvg re-raster at exact size");

    let pixels = common::render_and_read(&gpu, &mut renderer, 1170, 768, &[presenter.frame()]);
    assert_eq!(
        common::pixel_at(&pixels, 1170, 1169, 767),
        FELT,
        "the bottom-right corner is bare felt"
    );
    let non_felt = pixels.chunks_exact(4).filter(|px| *px != FELT).count();
    assert!(
        non_felt > 10_000,
        "a dealt board draws substantial card area, found {non_felt} non-felt pixels"
    );

    // The ink floor above says "something was drawn"; these say *where*.
    // 585x384 logical fitted into 1170x768 is exactly 2x, so a logical
    // point (x, y) lands at (2x, 2y): the stock sits at logical (11, 5),
    // the columns start at logical y 107 and step 82 apart, and the cards
    // are 71x96. Testing occupancy rather than colour keeps this
    // independent of resvg's exact anti-aliasing — it pins placement, which
    // is the property a re-raster at a new size could actually break.
    let logical = |x: u32, y: u32| common::pixel_at(&pixels, 1170, x * 2, y * 2);

    assert_ne!(logical(46, 53), FELT, "the stock's card back is drawn");
    assert_ne!(
        logical(46, 155),
        FELT,
        "tableau column 0 draws its single card"
    );
    assert_eq!(
        logical(87, 155),
        FELT,
        "the gap between columns 0 and 1 stays bare"
    );
    assert_eq!(
        logical(46, 300),
        FELT,
        "below column 0's one card is bare felt"
    );
}

/// The player's xBRZ choice, proven through the real renderer pipeline —
/// not just the pure `content_factor` policy `scale.rs` unit-tests: a PNG
/// theme's atlas sits at factor 1 by default, jumps to xBRZ's fixed
/// ceiling once the player picks it, and then never rebuilds again no
/// matter how the display scale changes, unlike a vector theme's atlas
/// (which the tests above show tracking the requested scale exactly).
#[test]
fn png_xbrz_atlas_is_the_fixed_ceiling_and_never_rebuilds_on_resize() {
    let Some(gpu) = common::gpu() else { return };
    let original = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    assert_eq!(
        original.atlas_factor(),
        1,
        "the default stays native pixels"
    );

    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_png_theme(),
        CardScaling::Xbrz,
        scale(1),
    )
    .expect("renderer");
    assert_eq!(
        renderer.atlas_factor(),
        u32::from(sol_xbrz::SCALE_FACTOR_MAX),
        "an xbrz PNG theme builds at xBRZ's ceiling from the start"
    );

    let job = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 4.0)
        .expect("adopt_scale");
    assert!(
        job.is_none(),
        "a PNG theme's atlas factor does not track the display scale"
    );
    assert_eq!(
        renderer.atlas_factor(),
        u32::from(sol_xbrz::SCALE_FACTOR_MAX),
        "unchanged after the resize"
    );

    // The upscaled art still renders where it should: the ace of spades'
    // interior is solid red at the new, larger display scale, drawn
    // through the same never-rebuilt xBRZ atlas.
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(10, 10, 4, 6))],
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 48, 50),
        [255, 0, 0, 255],
        "the ace of spades renders through the xBRZ-upscaled atlas"
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 5, 5),
        FELT,
        "far felt stays felt"
    );

    // Nothing so far distinguishes `fs_pixel_aa` from plain linear
    // sampling: every probe above samples deep inside a solid-colour
    // card, where a hard-snapped, evenly sized texel and a bilinear one
    // land on the same colour. The two fragment entry points only differ
    // within about half an atlas texel of a texel boundary — the card's
    // own edge against the atlas's transparent padding is exactly that
    // boundary. A much larger resize (still no rebuild: the atlas stays
    // at xBRZ's ceiling) magnifies that half-texel band to several
    // physical pixels, wide enough to sample reliably.
    let no_rebuild = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 60.0)
        .expect("adopt_scale");
    assert!(
        no_rebuild.is_none(),
        "still no rebuild at a far larger scale"
    );

    let edge_pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        256,
        384,
        &[ace_frame(Rect::new(0, 0, 4, 6))],
    );
    // A few physical pixels in from the card's left edge: plain linear
    // sampling (what `(Png, Xbrz)` must use) is still blending toward the
    // transparent padding here, so the red channel sits well below 255.
    // `fs_pixel_aa`'s hard, evenly sized texels would already show pure,
    // unblended red this close to the edge — confirmed by temporarily
    // forcing `pixel_aa` to return `true` for `(Png, Xbrz)`: every probed
    // pixel from the true edge onward flipped to exactly
    // `[255, 0, 0, 255]`, which is exactly the value this assertion rules
    // out.
    let edge = common::pixel_at(&edge_pixels, 256, 3, 180);
    assert!(
        edge[0] < 250,
        "png+xbrz should sample linearly (still blending toward the \
         transparent padding this close to the edge), not through the \
         pixel-art AA entry point: got {edge:?}"
    );
}
