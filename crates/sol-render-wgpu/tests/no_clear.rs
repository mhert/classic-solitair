//! The cascade's don't-clear contract: a display list with `clear: None`
//! draws **over** the previous frame (color attachment `LoadOp::Load`),
//! which is what the win cascade's smear trail is made of; a clearing
//! list wipes it again.
//!
//! Opaque sprites over an opaque clear are exact on every driver (the
//! blend reduces to a copy), so the assertions here are byte-exact even
//! on hardware adapters.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use sol_engine::{Rank, Suit};
use sol_presenter::{DisplayList, Rect, Rgba, Sprite, TextureId};
use sol_render_wgpu::Renderer;
use sol_theme::CardScaling;

const FELT: [u8; 4] = [0, 128, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

fn ace(dst: Rect) -> Sprite {
    Sprite {
        texture: TextureId::Face {
            suit: Suit::Spades,
            rank: Rank::Ace,
        },
        src: Rect::new(0, 0, 4, 6),
        dst,
        z: 0,
        tint: Rgba::WHITE,
    }
}

#[test]
fn no_clear_frames_smear_over_the_previous_frame() {
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

    // Frame 1 clears to felt and drops the card at (10, 10). Frame 2 does
    // NOT clear and drops it again at (30, 20): both stay visible.
    let frame1 = DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: vec![ace(Rect::new(10, 10, 4, 6))],
    };
    let frame2 = DisplayList {
        clear: None,
        sprites: vec![ace(Rect::new(30, 20, 4, 6))],
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 64, 64, &[frame1.clone(), frame2]);
    assert_eq!(
        common::pixel_at(&pixels, 64, 11, 11),
        RED,
        "first card survives"
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 31, 21),
        RED,
        "second card lands"
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 50, 50),
        FELT,
        "felt everywhere else"
    );

    // An empty no-clear frame (a cascade tick with nothing newly stepped)
    // changes nothing.
    let idle = DisplayList {
        clear: None,
        sprites: Vec::new(),
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 64, 64, &[frame1.clone(), idle]);
    assert_eq!(common::pixel_at(&pixels, 64, 11, 11), RED);

    // A clearing frame wipes the smear again.
    let wipe = DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: Vec::new(),
    };
    let pixels = common::render_and_read(&gpu, &mut renderer, 64, 64, &[frame1, wipe]);
    assert_eq!(common::pixel_at(&pixels, 64, 11, 11), FELT, "cleared");
}
