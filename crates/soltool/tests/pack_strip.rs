//! `soltool pack-strip` integration tests: spawns the real
//! binary end to end, which is how `main.rs` and the library's dispatch
//! get covered (`cargo llvm-cov` merges the spawned binary's own coverage
//! profile in automatically).

#![allow(clippy::unwrap_used)]

mod common;

use soltool::raster::{self, RasterImage};

/// A `width`x`height` image filled with `color` (RGBA8) — mirrors
/// `pack_strip.rs`'s own internal `solid` test helper, duplicated here
/// since integration tests cannot reach a crate's private test code.
fn solid(width: u32, height: u32, color: [u8; 4]) -> RasterImage {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&color);
    }
    RasterImage {
        width,
        height,
        pixels,
    }
}

fn write_frame(dir: &std::path::Path, name: &str, image: &RasterImage) {
    std::fs::write(dir.join(name), raster::encode(image).unwrap()).unwrap();
}

#[test]
fn packs_two_frames_writes_a_valid_strip_and_prints_the_snippet() {
    let dir = tempfile::tempdir().unwrap();
    let red = solid(2, 2, [255, 0, 0, 255]);
    let blue = solid(2, 2, [0, 0, 255, 255]);
    write_frame(dir.path(), "f0.png", &red);
    write_frame(dir.path(), "f1.png", &blue);

    let output = common::run(
        dir.path(),
        &[
            "pack-strip",
            "f0.png",
            "f1.png",
            "-o",
            "robot.png",
            "--fps",
            "2",
        ],
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim_end(),
        "robot = { image = \"robot.png\", frames = 2, fps = 2 }"
    );

    let strip_bytes = std::fs::read(dir.path().join("robot.png")).unwrap();
    let decoded = raster::decode(&strip_bytes).unwrap();
    assert_eq!(decoded.width, 4);
    assert_eq!(decoded.height, 2);
    assert_eq!(
        decoded.pixels.get(0..4).unwrap(),
        red.pixels.get(0..4).unwrap()
    );
    assert_eq!(
        decoded.pixels.get(8..12).unwrap(),
        blue.pixels.get(0..4).unwrap()
    );
}

#[test]
fn mismatched_frame_dimensions_exit_1_and_name_the_offending_frame() {
    let dir = tempfile::tempdir().unwrap();
    write_frame(dir.path(), "f0.png", &solid(2, 2, [1, 1, 1, 255]));
    write_frame(dir.path(), "f1.png", &solid(3, 2, [2, 2, 2, 255]));

    let output = common::run(
        dir.path(),
        &[
            "pack-strip",
            "f0.png",
            "f1.png",
            "-o",
            "out.png",
            "--fps",
            "2",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("f1.png"), "{stderr}");
}

#[test]
fn missing_fps_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    write_frame(dir.path(), "f0.png", &solid(2, 2, [1, 1, 1, 255]));
    write_frame(dir.path(), "f1.png", &solid(2, 2, [2, 2, 2, 255]));

    let output = common::run(
        dir.path(),
        &["pack-strip", "f0.png", "f1.png", "-o", "out.png"],
    );

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_zero_fps_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    write_frame(dir.path(), "f0.png", &solid(2, 2, [1, 1, 1, 255]));
    write_frame(dir.path(), "f1.png", &solid(2, 2, [2, 2, 2, 255]));

    let output = common::run(
        dir.path(),
        &[
            "pack-strip",
            "f0.png",
            "f1.png",
            "-o",
            "out.png",
            "--fps",
            "0",
        ],
    );

    assert_eq!(output.status.code(), Some(2));
}
