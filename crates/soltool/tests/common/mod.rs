//! Shared helpers for soltool's integration tests. Each test binary
//! compiles this module separately and uses its own subset of the helpers.

#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::panic)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output};

/// Runs the real `soltool` binary with `args` in `dir`, returning its
/// output. This is how `main.rs` and the library it calls get exercised
/// end to end (`cargo llvm-cov` merges the spawned binary's own coverage
/// profile in automatically).
///
/// # Panics
///
/// Panics if the binary could not be spawned at all — a test-environment
/// precondition, not something under test.
pub fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soltool"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn soltool: {error}"))
}

/// [`run`], but also sets each `(key, value)` in `envs` as an environment
/// variable on the spawned child process — never on this test process
/// itself, so tests exercising platform-specific env (e.g. `XDG_DATA_HOME`)
/// stay isolated from one another.
///
/// # Panics
///
/// Panics if the binary could not be spawned at all — a test-environment
/// precondition, not something under test.
pub fn run_env(dir: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_soltool"));
    command.args(args).current_dir(dir);
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn soltool: {error}"))
}

/// A real, minimal PNG — `width`x`height`, 8-bit grayscale, built via the
/// `png` crate's encoder — with a genuine IHDR CRC.
///
/// This is enough for `sol_theme`'s dimension-probing loader (see
/// `sol_theme`'s own internal test fixtures, e.g. `theme.rs`'s
/// `png_bytes`), which `soltool validate` is a thin shell over. `validate`
/// never decodes pixels, only `pack-strip` does (via `soltool::raster`,
/// which needs and gets genuinely valid PNGs too — see `pack_strip.rs`'s
/// own tests); this fixture's pixel content (all zero) is likewise never
/// inspected here, only its declared dimensions are.
pub fn probe_only_png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0_u8; (width * height) as usize])
            .unwrap();
    }
    bytes
}

const SUITS: [&str; 4] = ["spades", "hearts", "diamonds", "clubs"];

/// `theme.toml` for a minimal, valid png theme: 52 faces, one static
/// back, a color background, no sounds (mirrors `sol_theme`'s own
/// `PNG_MANIFEST` test fixture, trimmed to exactly one back).
pub const MINIMAL_MANIFEST: &str = "[theme]\n\
     name = \"Minimal\"\n\
     render_mode = \"png\"\n\
     \n\
     [cards]\n\
     faces = \"cards/\"\n\
     base_size = [2, 2]\n\
     \n\
     [backs]\n\
     plain = { image = \"backs/plain.png\" }\n\
     \n\
     [table]\n\
     background = { color = \"#008000\" }\n\
     \n\
     [drag]\n\
     outline_color = \"#000000\"\n";

/// `(path, bytes)` for every file in a minimal, complete png theme: 52
/// tiny face PNGs, one static back, `theme.toml` — the raw pairs, so a
/// test that wants to break one entry (or zip them all up) can filter or
/// collect them directly.
pub fn minimal_theme_entries() -> Vec<(String, Vec<u8>)> {
    let mut entries = vec![(
        "theme.toml".to_owned(),
        MINIMAL_MANIFEST.as_bytes().to_vec(),
    )];
    for suit in SUITS {
        for rank in 1..=13_u8 {
            entries.push((format!("cards/{suit}_{rank:02}.png"), probe_only_png(2, 2)));
        }
    }
    entries.push(("backs/plain.png".to_owned(), probe_only_png(2, 2)));
    entries
}

/// `(path, bytes)` for a minimal, complete png theme with **two** static
/// backs rather than [`minimal_theme_entries`]'s one — exists solely so a
/// test can exercise `soltool validate`'s back-count pluralization ("2
/// backs" vs. "1 back").
pub fn two_back_theme_entries() -> Vec<(String, Vec<u8>)> {
    const MANIFEST: &str = "[theme]\n\
         name = \"TwoBacks\"\n\
         render_mode = \"png\"\n\
         \n\
         [cards]\n\
         faces = \"cards/\"\n\
         base_size = [2, 2]\n\
         \n\
         [backs]\n\
         plain = { image = \"backs/plain.png\" }\n\
         other = { image = \"backs/other.png\" }\n\
         \n\
         [table]\n\
         background = { color = \"#008000\" }\n\
         \n\
         [drag]\n\
         outline_color = \"#000000\"\n";
    let mut entries = vec![("theme.toml".to_owned(), MANIFEST.as_bytes().to_vec())];
    for suit in SUITS {
        for rank in 1..=13_u8 {
            entries.push((format!("cards/{suit}_{rank:02}.png"), probe_only_png(2, 2)));
        }
    }
    entries.push(("backs/plain.png".to_owned(), probe_only_png(2, 2)));
    entries.push(("backs/other.png".to_owned(), probe_only_png(2, 2)));
    entries
}

/// Writes `entries` (from [`minimal_theme_entries`] or
/// [`two_back_theme_entries`]) to `root` as real files, ready for
/// `soltool validate <root>`.
fn write_theme_entries(root: &Path, entries: Vec<(String, Vec<u8>)>) {
    std::fs::create_dir_all(root.join("cards")).unwrap();
    std::fs::create_dir_all(root.join("backs")).unwrap();
    for (path, bytes) in entries {
        std::fs::write(root.join(path), bytes).unwrap();
    }
}

/// Writes [`minimal_theme_entries`] to `root` as real files, ready for
/// `soltool validate <root>`.
pub fn write_minimal_theme(root: &Path) {
    write_theme_entries(root, minimal_theme_entries());
}

/// Writes [`two_back_theme_entries`] to `root` as real files, ready for
/// `soltool validate <root>`.
pub fn write_two_back_theme(root: &Path) {
    write_theme_entries(root, two_back_theme_entries());
}

/// Zips [`minimal_theme_entries`] into archive bytes, ready for
/// `soltool validate <zip path>`.
pub fn zip_minimal_theme() -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default();
    for (path, bytes) in minimal_theme_entries() {
        writer.start_file(path, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
