//! Test-only helpers: a complete in-memory vector theme.

#![allow(clippy::expect_used)] // Test fixtures: a broken fixture must abort the suite loudly.

use core::fmt::Write as _;

use sol_theme::{CardSize, MemSource, Theme, canonical_faces};

/// A minimal SVG document of the given pixel size.
fn svg(width: u32, height: u32) -> Vec<u8> {
    format!(r#"<svg width="{width}" height="{height}"></svg>"#).into_bytes()
}

/// A fully validated in-memory theme: 71×96 vector cards, a green table,
/// black drag outline, and four backs covering every back shape — `plain`
/// (static), `strip` (2-frame horizontal strip at 2 fps), `steps`
/// (2 list-form frames with explicit durations 250 ms and 750 ms), and
/// `tall` (2-frame vertical strip at 4 fps).
pub(crate) fn test_theme() -> Theme {
    theme_with_table(r##"background = { color = "#008000" }"##, "", &[])
}

/// [`test_theme`] plus a `[placeholders]` section declaring exactly `slots`
/// — any subset of `"empty_pile"`, `"stock_recycle"`, `"stock_blocked"`.
///
/// Declaring a subset is the point: a theme is free to supply one
/// placeholder and not the others, and the presenter must draw only what
/// it is given.
pub(crate) fn test_theme_with_placeholders(slots: &[&str]) -> Theme {
    let entries = slots.iter().fold(String::new(), |mut acc, slot| {
        let _ = writeln!(acc, "{slot} = {{ image = \"ph/{slot}.svg\" }}");
        acc
    });
    let files: Vec<(String, Vec<u8>)> = slots
        .iter()
        .map(|slot| (format!("ph/{slot}.svg"), svg(71, 96)))
        .collect();
    let extra: Vec<(&str, Vec<u8>)> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.clone()))
        .collect();
    theme_with_table(
        r##"background = { color = "#008000" }"##,
        &format!("[placeholders]\n{entries}"),
        &extra,
    )
}

/// [`test_theme`] with an image table background instead of a color.
///
/// The image is 100×50; `tile` selects tiling over stretching.
pub(crate) fn test_theme_image_bg(tile: bool) -> Theme {
    let table = if tile {
        r#"background = { image = "table.svg", tile = true }"#
    } else {
        r#"background = { image = "table.svg" }"#
    };
    theme_with_table(table, "", &[("table.svg", svg(100, 50))])
}

/// A theme declaring exactly one (static) back — for back-selection
/// clamping tests.
pub(crate) fn test_theme_single_back() -> Theme {
    let manifest = br##"
[theme]
name = "One Back"
render_mode = "vector"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
only = { image = "backs/only.svg" }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#ff00ff"
"##;
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/only.svg", svg(71, 96));
    for (suit, rank) in canonical_faces() {
        source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg(71, 96));
    }
    Theme::from_source(&source).expect("the single-back test theme validates")
}

/// [`test_theme`] at a different `base_size` — for tests about what changes
/// when a theme's card size does.
pub(crate) fn test_theme_at(card: CardSize) -> Theme {
    let (w, h) = (card.width, card.height);
    let manifest = format!(
        r##"
[theme]
name = "Test"
render_mode = "vector"

[cards]
faces = "cards/"
base_size = [{w}, {h}]

[backs]
plain = {{ image = "backs/plain.svg" }}

[table]
background = {{ color = "#008000" }}

[drag]
outline_color = "#000000"
"##
    );
    let mut source = MemSource::new()
        .with_file("theme.toml", manifest.into_bytes())
        .with_file("backs/plain.svg", svg(w, h));
    for (suit, rank) in canonical_faces() {
        source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg(w, h));
    }
    Theme::from_source(&source).expect("the resized test theme validates")
}

fn theme_with_table(table: &str, placeholders: &str, extra: &[(&str, Vec<u8>)]) -> Theme {
    let manifest = format!(
        r##"
[theme]
name = "Test"
render_mode = "vector"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
plain = {{ image = "backs/plain.svg" }}
strip = {{ image = "backs/strip.svg", frames = 2, fps = 2 }}
steps = {{ image = ["backs/steps_0.svg", "backs/steps_1.svg"], durations_ms = [250, 750] }}
tall = {{ image = "backs/tall.svg", frames = 2, fps = 4, layout = "vertical" }}

[table]
{table}

{placeholders}

[drag]
outline_color = "#000000"
"##
    );
    let mut source = MemSource::new()
        .with_file("theme.toml", manifest.into_bytes())
        .with_file("backs/plain.svg", svg(71, 96))
        .with_file("backs/strip.svg", svg(142, 96))
        .with_file("backs/steps_0.svg", svg(71, 96))
        .with_file("backs/steps_1.svg", svg(71, 96))
        .with_file("backs/tall.svg", svg(71, 192));
    for (path, bytes) in extra {
        source = source.with_file(*path, bytes.clone());
    }
    for (suit, rank) in canonical_faces() {
        source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg(71, 96));
    }
    Theme::from_source(&source).expect("the in-memory test theme validates")
}
