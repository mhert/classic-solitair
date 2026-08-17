//! Embeds the default theme: zips `themes/default` into
//! `OUT_DIR/default-theme.zip`, which `themes.rs` pulls in with
//! `include_bytes!`. Entries are written in sorted order; the archive is a
//! transport for the same files the on-disk theme holds, and the drift
//! guard in `themes.rs` compares the *loaded* theme, so byte-level archive
//! determinism is not required for correctness.

use std::error::Error;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR unset")?);
    let theme_dir = manifest_dir.join("../../themes/default");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR unset")?)
        .join("default-theme.zip");

    // Rebuild the archive whenever the source theme changes (the directory
    // catches added/removed files; each file catches edits).
    println!("cargo:rerun-if-changed={}", theme_dir.display());

    let mut files = Vec::new();
    collect(&theme_dir, &theme_dir, &mut files)?;
    files.sort();

    let mut zip = ZipWriter::new(File::create(&out)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (rel, abs) in &files {
        println!("cargo:rerun-if-changed={}", abs.display());
        zip.start_file(rel.as_str(), options)?;
        zip.write_all(&fs::read(abs)?)?;
    }
    zip.finish()?;
    Ok(())
}

/// Collects every file under `dir` as `(package-relative path, absolute
/// path)`, recursing in sorted order.
fn collect(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), Box<dyn Error>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}
