//! Golden-image tests for the **png** scaling pipeline at 1× and 2×,
//! using the default theme's artwork (its SVGs rasterized to a PNG theme
//! at 1× — same resvg the vector pipeline uses, pinned by Cargo.lock; a
//! resvg version bump legitimately regenerates these goldens).
//!
//! The frame is a settled seed-1 deal from the real presenter, rendered,
//! read back, and compared byte-for-byte on a software adapter (see
//! `common` for the adapter and tolerance strategy).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]

mod common;

use sol_engine::Seed;
use sol_presenter::Presenter;
use sol_render_wgpu::Renderer;
use sol_session::{Options, Session};

/// A settled (deal animation skipped) seed-1 presenter over the png
/// default theme at `scale`.
fn settled_presenter(theme: &sol_theme::Theme, scale: u32) -> Presenter {
    let mut presenter = Presenter::new(
        Session::new(Options::default(), Seed::new(1).unwrap()),
        theme,
    );
    presenter.key_down();
    assert!(!presenter.is_animating());
    let fit = presenter.fit_viewport(585 * scale, 384 * scale);
    assert_eq!(fit.scale, scale as f32, "an exact integer window fit");
    presenter
}

fn golden_at(scale: u32, name: &str) {
    let Some(gpu) = common::gpu() else { return };
    let theme = common::pixel_default_theme();
    let presenter = settled_presenter(&theme, scale);
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        theme,
        sol_theme::CardScaling::Original,
        scale as f32,
    )
    .expect("renderer");
    assert_eq!(
        renderer.atlas_factor(),
        1,
        "png mode keeps a native atlas at any display scale"
    );
    let frame = presenter.frame();
    let (width, height) = (585 * scale, 384 * scale);
    let pixels = common::render_and_read(&gpu, &mut renderer, width, height, &[frame]);
    // Degenerate-frame guard, independent of the goldens: a settled board
    // covers substantial card area, so a bless run can never enshrine a
    // bare-felt (or empty) frame as the golden.
    let non_felt = pixels
        .chunks_exact(4)
        .filter(|px| *px != [0, 128, 0, 255])
        .count();
    assert!(
        non_felt > 20_000 * (scale * scale) as usize,
        "the {scale}x frame drew only {non_felt} non-felt pixels"
    );
    common::compare_golden(&gpu, name, width, height, &pixels);
}

#[test]
fn png_default_theme_1x_matches_golden() {
    golden_at(1, "pixel-default-1x");
}

#[test]
fn png_default_theme_2x_matches_golden() {
    golden_at(2, "pixel-default-2x");
}

/// Integer-only scaling, concretely: the 2× frame is the 1× frame with
/// every destination doubled and nearest-sampled — spot-checked at the
/// felt and at a known card interior, independent of the goldens.
#[test]
fn png_2x_nearest_scales_the_1x_frame() {
    let Some(gpu) = common::gpu() else { return };
    let theme = common::pixel_default_theme();

    let one = settled_presenter(&theme, 1);
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        theme.clone(),
        sol_theme::CardScaling::Original,
        1.0,
    )
    .expect("renderer");
    let base = common::render_and_read(&gpu, &mut renderer, 585, 384, &[one.frame()]);

    let two = settled_presenter(&theme, 2);
    renderer
        .set_display_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("set_display_scale");
    let doubled = common::render_and_read(&gpu, &mut renderer, 1170, 768, &[two.frame()]);

    // Every 2×2 block of the doubled frame equals its source pixel: pure
    // integer duplication, zero smoothing at any window size.
    let tolerance = gpu.tolerance().saturating_mul(2);
    for (x, y) in [(11, 5), (40, 40), (100, 130), (300, 200), (584, 383)] {
        let src = common::pixel_at(&base, 585, x, y);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let dst = common::pixel_at(&doubled, 1170, 2 * x + dx, 2 * y + dy);
            for c in 0..4 {
                assert!(
                    src[c].abs_diff(dst[c]) <= tolerance,
                    "2×({x},{y})+({dx},{dy}): {src:?} vs {dst:?}"
                );
            }
        }
    }
}
