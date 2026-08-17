//! A theme package is untrusted input. These pin that the paths a manifest
//! declares cannot escape the package, using the same rule on every platform —
//! a package rejected on Linux must be rejected on Windows and vice versa.

#![allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures: a broken fixture must abort the suite loudly.

use std::io::Write;

use sol_theme::{Theme, ZipSource};

/// Builds a zip whose manifest declares a sound at `sound_path`, with a
/// matching entry so the path is reachable rather than merely declared.
fn theme_zip_with_sound_path(sound_path: &str) -> Vec<u8> {
    let manifest = format!(
        r##"
[theme]
name = "hostile"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
plain = {{ image = "backs/plain.png" }}

[table]
background = {{ color = "#008000" }}

[drag]
outline_color = "#000000"

[sounds]
evil = "{sound_path}"
"##
    );
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("theme.toml", options).unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.start_file(sound_path, options).unwrap();
        writer.write_all(b"attacker bytes").unwrap();
        writer.finish().unwrap();
    }
    buffer
}

/// The rule reads the raw string rather than `Path::components`, so a drive
/// prefix is rejected on Linux too — where `Path` would have parsed it as
/// two ordinary path segments and let it through.
#[test]
fn a_windows_drive_prefixed_asset_path_is_rejected() {
    let bytes = theme_zip_with_sound_path("C:/Users/Public/escaped.bat");
    let source = ZipSource::from_bytes(&bytes).expect("archive itself is well formed");
    let error = Theme::from_source(&source).expect_err("drive-prefixed path must be rejected");
    let message = error.to_string();
    assert!(message.contains("C:/Users/Public/escaped.bat"), "{message}");
}

#[test]
fn a_traversal_asset_path_is_rejected() {
    let bytes = theme_zip_with_sound_path("../../escaped.bat");
    let source = ZipSource::from_bytes(&bytes).expect("archive itself is well formed");
    assert!(Theme::from_source(&source).is_err());
}

#[test]
fn a_reserved_device_asset_path_is_rejected() {
    let bytes = theme_zip_with_sound_path("NUL");
    let source = ZipSource::from_bytes(&bytes).expect("archive itself is well formed");
    assert!(Theme::from_source(&source).is_err());
}

/// The same rule applies to a `[backs]` image, not only to sounds — every
/// path-valued field goes through one parser.
#[test]
fn a_hostile_back_image_path_is_rejected() {
    let manifest = r##"
[theme]
name = "hostile"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
plain = { image = "C:/evil.png" }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer.start_file("theme.toml", options).unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let source = ZipSource::from_bytes(&buffer).expect("archive itself is well formed");
    let error = Theme::from_source(&source).expect_err("drive-prefixed path must be rejected");
    assert!(error.to_string().contains("C:/evil.png"), "{error}");
}
