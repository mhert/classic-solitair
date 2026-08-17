//! `soltool extract` integration tests: spawns the real binary end
//! to end, which is how `main.rs`, the CLI parse of `extract`, and the
//! library's dispatch get covered. The resource (NE/PE) and classification
//! paths are unit-tested in-process (`src/extract.rs`); here the loose-bitmap
//! path exercises the whole binary and the extract-then-validate contract.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use soltool::raster::{self, RasterImage};

/// A real, decodable solid-color PNG (the loose path decodes every file).
fn real_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&color);
    }
    raster::encode(&RasterImage {
        width,
        height,
        pixels,
    })
    .unwrap()
}

const SUITS: [&str; 4] = ["spades", "hearts", "diamonds", "clubs"];

/// Writes the 52 canonically-named face PNGs (5×7) into `dir`.
fn write_faces(dir: &Path) {
    for suit in SUITS {
        for rank in 1..=13 {
            let name = format!("{suit}_{rank:02}.png");
            std::fs::write(dir.join(name), real_png(5, 7, [9, 9, 9, 255])).unwrap();
        }
    }
}

#[test]
fn extract_a_loose_directory_writes_a_theme_that_validates() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("assets");
    std::fs::create_dir(&input).unwrap();
    write_faces(&input);
    std::fs::write(input.join("plainback.png"), real_png(5, 7, [1, 2, 3, 255])).unwrap();
    let output = tmp.path().join("theme");

    let extracted = common::run(
        tmp.path(),
        &[
            "extract",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(extracted.status.code(), Some(0));
    assert!(extracted.stderr.is_empty());
    let stdout = String::from_utf8(extracted.stdout).unwrap();
    assert!(stdout.contains("local use only"), "{stdout}");
    assert!(stdout.contains("52 card faces"), "{stdout}");

    // The generated theme loads green through `soltool validate`.
    let validated = common::run(tmp.path(), &["validate", output.to_str().unwrap()]);
    assert_eq!(validated.status.code(), Some(0));
}

#[test]
fn extract_a_missing_input_exits_1_with_a_message_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("out");
    let result = common::run(
        tmp.path(),
        &[
            "extract",
            "no/such/input.dll",
            "-o",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(!result.stderr.is_empty());
}
