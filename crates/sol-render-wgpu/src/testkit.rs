//! Tiny deterministic in-memory themes for unit tests: 4×6 cards whose
//! solid colors encode their canonical face index, one static back, one
//! two-frame strip back.

#![allow(clippy::unwrap_used)]

use sol_theme::{MemSource, Theme, canonical_faces};

/// Parses `raw` as an asset path, panicking if it is not a valid one.
///
/// Fixtures spell out paths as literals; a literal that does not parse is a
/// broken fixture, not a case under test.
pub(crate) fn asset_path(raw: &str) -> sol_theme::RelativeAssetPath {
    sol_theme::RelativeAssetPath::parse("test fixture".to_owned(), raw).unwrap()
}

/// The solid color of canonical face `index` (0..52): ace of spades is
/// pure red, later faces trade red for green.
pub(crate) fn face_color(index: u8) -> [u8; 3] {
    [255 - index * 4, index * 4, 0]
}

fn manifest(render_mode: &str, ext: &str) -> Vec<u8> {
    format!(
        r##"
[theme]
name = "Renderer test"
render_mode = "{render_mode}"

[cards]
faces = "cards/"
base_size = [4, 6]

[backs]
plain = {{ image = "backs/plain.{ext}" }}
strip = {{ image = "backs/strip.{ext}", frames = 2, fps = 2 }}

[table]
background = {{ color = "#008000" }}

[drag]
outline_color = "#000000"
"##
    )
    .into_bytes()
}

fn png_bytes(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixel = [color[0], color[1], color[2], 0xFF];
        let data: Vec<u8> = pixel
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect();
        writer.write_image_data(&data).unwrap();
    }
    bytes
}

fn svg_bytes(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="#{:02x}{:02x}{:02x}"/></svg>"##,
        color[0], color[1], color[2]
    )
    .into_bytes()
}

/// A two-frame horizontal strip: frame 0 green, frame 1 yellow.
fn strip_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 6);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut data = Vec::new();
        for _row in 0..6 {
            for x in 0..8 {
                data.extend_from_slice(if x < 4 {
                    &[0, 0xFF, 0, 0xFF]
                } else {
                    &[0xFF, 0xFF, 0, 0xFF]
                });
            }
        }
        writer.write_image_data(&data).unwrap();
    }
    bytes
}

fn strip_svg() -> Vec<u8> {
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect width="4" height="6" fill="#00ff00"/><rect x="4" width="4" height="6" fill="#ffff00"/></svg>"##.to_vec()
}

fn theme_from(render_mode: &str) -> Theme {
    let strip = if render_mode == "vector" {
        strip_svg()
    } else {
        strip_png()
    };
    theme_from_with_strip(render_mode, strip)
}

fn theme_from_with_strip(render_mode: &str, strip: Vec<u8>) -> Theme {
    let (ext, is_svg) = match render_mode {
        "vector" => ("svg", true),
        _ => ("png", false),
    };
    let image = |color: [u8; 3]| {
        if is_svg {
            svg_bytes(4, 6, color)
        } else {
            png_bytes(4, 6, color)
        }
    };
    let mut source = MemSource::new()
        .with_file("theme.toml", manifest(render_mode, ext))
        .with_file(format!("backs/plain.{ext}"), image([0, 0, 0xFF]))
        .with_file(format!("backs/strip.{ext}"), strip);
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        source = source.with_file(
            format!("cards/{}.{ext}", suit.stem(rank)),
            image(face_color(u8::try_from(index).unwrap())),
        );
    }
    Theme::from_source(&source).unwrap()
}

/// Straight-alpha RGBA pixels of one 4×6 corner-strip frame: a solid
/// body (blue for frame 0, red for frame 1) with transparent corner
/// pixels — the rounded-corner shape that makes xBRZ's cross-frame
/// bleed visible at a strip's interior frame edges.
pub(crate) fn corner_strip_frame_pixels(index: u32) -> Vec<u8> {
    let body: [u8; 4] = if index == 0 {
        [0, 0, 0xFF, 0xFF]
    } else {
        [0xFF, 0, 0, 0xFF]
    };
    let mut pixels = Vec::with_capacity(4 * 6 * 4);
    for y in 0..6_u32 {
        for x in 0..4_u32 {
            let corner = (x == 0 || x == 3) && (y == 0 || y == 5);
            pixels.extend_from_slice(if corner { &[0, 0, 0, 0] } else { &body });
        }
    }
    pixels
}

/// The [`corner_strip_frame_pixels`] frame as a real PNG.
pub(crate) fn corner_strip_frame_png(index: u32) -> Vec<u8> {
    encode_rgba(4, 6, &corner_strip_frame_pixels(index))
}

/// A `png` theme whose two-frame strip back has the corner-strip
/// frames side by side.
pub(crate) fn test_theme_png_corner_strip() -> Theme {
    let mut data = Vec::with_capacity(8 * 6 * 4);
    let frames = [corner_strip_frame_pixels(0), corner_strip_frame_pixels(1)];
    let row_bytes = 4 * 4;
    for row in 0..6_usize {
        for frame in &frames {
            if let Some(chunk) = frame.chunks_exact(row_bytes).nth(row) {
                data.extend_from_slice(chunk);
            }
        }
    }
    theme_from_with_strip("png", encode_rgba(8, 6, &data))
}

/// Encodes straight-alpha RGBA8 pixels as a real PNG.
fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
    }
    bytes
}

/// A `render_mode = "png"` theme with 4×6 PNG cards.
pub(crate) fn test_theme_png() -> Theme {
    theme_from("png")
}

/// A `render_mode = "vector"` theme with 4×6 SVG cards.
pub(crate) fn test_theme_vector() -> Theme {
    theme_from("vector")
}

/// The color of the placeholder for `slot` in
/// [`test_theme_png_placeholders`]: distinct per slot so a crossed
/// catalog entry is visible in the atlas pixels.
pub(crate) fn placeholder_color(slot: &str) -> [u8; 3] {
    match slot {
        "empty_pile" => [0x11, 0x22, 0x33],
        "stock_recycle" => [0x44, 0x55, 0x66],
        _ => [0x77, 0x88, 0x99],
    }
}

/// [`test_theme_png`] plus a `[placeholders]` section declaring exactly
/// `slots` — any subset of `"empty_pile"`, `"stock_recycle"`,
/// `"stock_blocked"`, since a theme may supply only some of them.
pub(crate) fn test_theme_png_placeholders(slots: &[&str]) -> Theme {
    use std::fmt::Write as _;
    let mut section = String::from("\n[placeholders]\n");
    for slot in slots {
        // Writing to a String cannot fail.
        let _ = writeln!(section, r#"{slot} = {{ image = "ph/{slot}.png" }}"#);
    }
    let mut bytes = manifest("png", "png");
    bytes.extend_from_slice(section.as_bytes());

    let mut source = MemSource::new()
        .with_file("theme.toml", bytes)
        .with_file("backs/plain.png", png_bytes(4, 6, [0, 0, 0xFF]))
        .with_file("backs/strip.png", strip_png());
    for slot in slots {
        source = source.with_file(
            format!("ph/{slot}.png"),
            png_bytes(4, 6, placeholder_color(slot)),
        );
    }
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        source = source.with_file(
            format!("cards/{}.png", suit.stem(rank)),
            png_bytes(4, 6, face_color(u8::try_from(index).unwrap())),
        );
    }
    Theme::from_source(&source).unwrap()
}

/// The color of sheet cell `(col, row)` in [`test_theme_vector_sheet`]'s
/// sheet: distinct per cell so slicing mistakes are visible.
pub(crate) fn sheet_cell_color(col: u32, row: u32) -> [u8; 3] {
    [
        u8::try_from(col * 19).unwrap_or(0),
        u8::try_from(row * 80).unwrap_or(0),
        55,
    ]
}

/// A `render_mode = "vector"` theme using the single-SVG **sheet form**:
/// `faces = "cards/sheet.svg"`, a 13-wide × 4-high grid of 4×6 cells,
/// each filled with [`sheet_cell_color`].
pub(crate) fn test_theme_vector_sheet() -> Theme {
    use std::fmt::Write as _;
    let mut cells = String::new();
    for row in 0..4_u32 {
        for col in 0..13_u32 {
            let [r, g, b] = sheet_cell_color(col, row);
            // Writing to a String cannot fail.
            let _ = write!(
                cells,
                r##"<rect x="{}" y="{}" width="4" height="6" fill="#{r:02x}{g:02x}{b:02x}"/>"##,
                col * 4,
                row * 6,
            );
        }
    }
    let sheet =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="52" height="24">{cells}</svg>"#)
            .into_bytes();
    let manifest = br##"
[theme]
name = "Renderer sheet test"
render_mode = "vector"

[cards]
faces = "cards/sheet.svg"
base_size = [4, 6]

[backs]
plain = { image = "backs/plain.svg" }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("cards/sheet.svg", sheet)
        .with_file("backs/plain.svg", svg_bytes(4, 6, [0, 0, 0xFF]));
    Theme::from_source(&source).unwrap()
}

/// A `render_mode = "png"` theme whose table background is a 6×4
/// magenta image instead of a color.
pub(crate) fn test_theme_png_image_bg() -> Theme {
    let manifest = br##"
[theme]
name = "Renderer background test"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [4, 6]

[backs]
plain = { image = "backs/plain.png" }

[table]
background = { image = "table.png" }

[drag]
outline_color = "#000000"
"##;
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/plain.png", png_bytes(4, 6, [0, 0, 0xFF]))
        .with_file("table.png", png_bytes(6, 4, [0xFF, 0, 0xFF]));
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        source = source.with_file(
            format!("cards/{}.png", suit.stem(rank)),
            png_bytes(4, 6, face_color(u8::try_from(index).unwrap())),
        );
    }
    Theme::from_source(&source).unwrap()
}
