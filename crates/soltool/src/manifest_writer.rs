//! The one `theme.toml` writer: `extract`'s manifest renderer, and the
//! shared TOML-string escaper `pack-strip`'s printed snippet also goes
//! through — no subcommand renders TOML on its own.
//!
//! [`render`] is the inverse of `sol_theme`'s manifest parse: it takes a
//! [`ThemeDoc`] built from `sol_theme`'s own already-validated types
//! ([`BackDef`], [`Background`], [`Color`], [`RenderMode`]) and emits a
//! document those very types parse back. Faces are always written as the
//! directory form `faces = "cards/"` (the theme `extract` writes always
//! uses that layout); the `[sounds]` section is emitted only when there
//! are sounds (it is optional).
//!
//! Rendering and escaping are delegated to `toml_edit`'s [`DocumentMut`] /
//! [`InlineTable`] rather than hand-rolled: every table, key, and value
//! below is a real `toml_edit` node, and [`toml_string`] is a thin
//! delegation to `toml_edit`'s own string encoder. `toml_edit` picks a
//! value's exact TOML representation (basic vs. literal vs. multi-line
//! string, and bare vs. quoted key) itself, which produces a few
//! representation-only deltas from the old hand-rolled writer — never
//! observable after a parse, since they're different spellings of the same
//! value:
//! - A `[sounds]` key that is a valid bare identifier now renders bare
//!   (e.g. `deal = "…"`) instead of always-quoted (`"deal" = "…"`); sound
//!   keys were never validated to need quoting in the first place.
//! - The rarely-used control characters 0x08 (backspace) and 0x0C
//!   (form feed) now render as the mnemonic escapes `\b`/`\f` instead of
//!   the numeric `\u0008`/`\u000C`.
//! - A string containing a `"` but no backslash/control/newline may render
//!   as a literal string (`'…'`) instead of a basic string with `\"`.
//! - A string that needs real escaping *and* contains an embedded newline
//!   may render as a multi-line basic string (`"""…"""`) instead of a
//!   single-line string with `\n` escapes.

use sol_theme::{BackDef, BackLayout, BackTiming, Background, Color, RenderMode};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

/// Everything [`render`] needs to emit a complete `theme.toml`, expressed in
/// `sol_theme`'s validated vocabulary so the writer cannot describe a theme
/// the loader would reject.
pub(crate) struct ThemeDoc {
    /// `[theme] name`.
    pub name: String,
    /// `[theme] author`, omitted when `None`.
    pub author: Option<String>,
    /// `[theme] render_mode`.
    pub render_mode: RenderMode,
    /// `[cards] base_size` as `(width, height)`.
    pub base_size: (u32, u32),
    /// `[backs]` entries in declaration order (name key + validated shape).
    pub backs: Vec<(String, BackDef)>,
    /// `[table] background`.
    pub background: Background,
    /// `[drag] outline_color`.
    pub outline_color: Color,
    /// `[sounds]` entries in declaration order; empty omits the section.
    pub sounds: Vec<(String, String)>,
}

/// Renders `doc` as a `theme.toml` document.
pub(crate) fn render(doc: &ThemeDoc) -> String {
    let mut document = DocumentMut::new();

    let mut theme = Table::new();
    theme.insert("name", Item::Value(doc.name.as_str().into()));
    if let Some(author) = &doc.author {
        theme.insert("author", Item::Value(author.as_str().into()));
    }
    theme.insert(
        "render_mode",
        Item::Value(render_mode_str(doc.render_mode).into()),
    );
    document.insert("theme", Item::Table(theme));

    let mut cards = Table::new();
    cards.insert("faces", Item::Value("cards/".into()));
    cards.insert(
        "base_size",
        Item::Value(Value::Array(base_size_array(doc.base_size))),
    );
    document.insert("cards", Item::Table(cards));

    let mut backs = Table::new();
    for (name, def) in &doc.backs {
        backs.insert(name, Item::Value(Value::InlineTable(render_back(def))));
    }
    document.insert("backs", Item::Table(backs));

    let mut table = Table::new();
    table.insert(
        "background",
        Item::Value(Value::InlineTable(render_background(&doc.background))),
    );
    document.insert("table", Item::Table(table));

    let mut drag = Table::new();
    drag.insert(
        "outline_color",
        Item::Value(doc.outline_color.to_string().into()),
    );
    document.insert("drag", Item::Table(drag));

    if !doc.sounds.is_empty() {
        let mut sounds = Table::new();
        for (key, path) in &doc.sounds {
            sounds.insert(key, Item::Value(path.as_str().into()));
        }
        document.insert("sounds", Item::Table(sounds));
    }

    document.to_string()
}

/// The exact lowercase `render_mode` spelling.
pub(crate) fn render_mode_str(mode: RenderMode) -> &'static str {
    match mode {
        RenderMode::Png => "png",
        RenderMode::Vector => "vector",
    }
}

/// Builds `[cards] base_size` as a two-element integer array: `[width,
/// height]`.
fn base_size_array((width, height): (u32, u32)) -> Array {
    let mut array = Array::new();
    array.push(i64::from(width));
    array.push(i64::from(height));
    array
}

/// Renders one `[backs]` value in its declared shape: a bare image is
/// static, `frames`/timing a strip (with `layout` only when vertical, since
/// horizontal is the loader's default), a list the multi-file form. Field
/// order is fixed: `image`, `frames`, timing (`fps` or `durations_ms`),
/// `layout`.
fn render_back(def: &BackDef) -> InlineTable {
    let mut table = InlineTable::new();
    match def {
        BackDef::Static { image } => {
            table.insert("image", image.as_str().into());
        }
        BackDef::Strip {
            image,
            frames,
            timing,
            layout,
        } => {
            table.insert("image", image.as_str().into());
            table.insert("frames", i64::from(*frames).into());
            insert_timing(&mut table, timing);
            if let BackLayout::Vertical = layout {
                table.insert("layout", "vertical".into());
            }
        }
        BackDef::Frames { images, timing } => {
            let mut list = Array::new();
            for image in images {
                list.push(image.as_str());
            }
            table.insert("image", Value::Array(list));
            insert_timing(&mut table, timing);
        }
    }
    table
}

/// Inserts an animated back's timing as either `fps` (uniform) or
/// `durations_ms` (per-frame) — the inverse of `sol_theme`'s own
/// `BackTiming` mapping, so it round-trips through the real parser.
fn insert_timing(table: &mut InlineTable, timing: &BackTiming) {
    match timing {
        BackTiming::Fps(fps) => {
            table.insert("fps", i64::from(*fps).into());
        }
        BackTiming::DurationsMs(durations) => {
            let mut list = Array::new();
            for duration in durations {
                list.push(i64::from(*duration));
            }
            table.insert("durations_ms", Value::Array(list));
        }
    }
}

/// Renders `[table] background`: a flat color, or an image with its `tile`
/// flag emitted explicitly so the value round-trips whatever it was.
fn render_background(background: &Background) -> InlineTable {
    let mut table = InlineTable::new();
    match background {
        Background::Color(color) => {
            table.insert("color", color.to_string().into());
        }
        Background::Image { path, tile } => {
            table.insert("image", path.as_str().into());
            table.insert("tile", (*tile).into());
        }
    }
    table
}

/// Renders `value` as a TOML string, delegating the choice of
/// representation and all escaping (including U+007F) to `toml_edit`.
/// Shared with `extract`'s summary rendering.
pub(crate) fn toml_string(value: &str) -> String {
    Value::from(value).to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sol_theme::Manifest;

    use super::*;
    use crate::testkit::asset_path;

    /// A doc exercising every optional-field and back-shape branch at once:
    /// an author, both render modes (checked separately), each back
    /// shape (static, horizontal strip, vertical strip, list, and a
    /// `durations_ms` strip), an image background, and sounds.
    fn rich_doc(render_mode: RenderMode) -> ThemeDoc {
        ThemeDoc {
            name: "Rich \"Quoted\"".to_owned(),
            author: Some("Ada".to_owned()),
            render_mode,
            base_size: (10, 20),
            backs: vec![
                (
                    "plain".to_owned(),
                    BackDef::Static {
                        image: asset_path("backs/plain.png"),
                    },
                ),
                (
                    "robot".to_owned(),
                    BackDef::Strip {
                        image: asset_path("backs/robot.png"),
                        frames: 4,
                        timing: BackTiming::Fps(2),
                        layout: BackLayout::Horizontal,
                    },
                ),
                (
                    "lift".to_owned(),
                    BackDef::Strip {
                        image: asset_path("backs/lift.png"),
                        frames: 3,
                        timing: BackTiming::Fps(1),
                        layout: BackLayout::Vertical,
                    },
                ),
                (
                    "bats".to_owned(),
                    BackDef::Frames {
                        images: vec![
                            asset_path("backs/bats_0.png"),
                            asset_path("backs/bats_1.png"),
                        ],
                        timing: BackTiming::Fps(3),
                    },
                ),
                (
                    "palm".to_owned(),
                    BackDef::Strip {
                        image: asset_path("backs/palm.png"),
                        frames: 4,
                        timing: BackTiming::DurationsMs(vec![250, 250, 250, 49_250]),
                        layout: BackLayout::Horizontal,
                    },
                ),
            ],
            background: Background::Image {
                path: asset_path("table.png"),
                tile: true,
            },
            outline_color: Color::new(0x12, 0x34, 0x56),
            sounds: vec![("deal".to_owned(), "sounds/deal.ogg".to_owned())],
        }
    }

    #[test]
    fn render_mode_str_matches_the_manifest_spellings() {
        assert_eq!(render_mode_str(RenderMode::Png), "png");
        assert_eq!(render_mode_str(RenderMode::Vector), "vector");
    }

    #[test]
    fn a_rich_png_doc_round_trips_through_the_real_manifest_parser() {
        // A back's image path must match its render mode's extension only at
        // *theme load* time, not manifest parse time, so a png doc with
        // .png back paths round-trips through `Manifest::from_toml_str`.
        let toml = render(&rich_doc(RenderMode::Png));
        let manifest = Manifest::from_toml_str(&toml).unwrap();

        assert_eq!(manifest.name, "Rich \"Quoted\"");
        assert_eq!(manifest.author.as_deref(), Some("Ada"));
        assert_eq!(manifest.render_mode, RenderMode::Png);
        assert_eq!(manifest.base_size.width, 10);
        assert_eq!(manifest.base_size.height, 20);
        assert_eq!(manifest.backs.len(), 5);
        assert_eq!(
            manifest.background,
            Background::Image {
                path: asset_path("table.png"),
                tile: true
            }
        );
        assert_eq!(manifest.outline_color, Color::new(0x12, 0x34, 0x56));
        assert_eq!(
            manifest.sounds,
            vec![("deal".to_owned(), asset_path("sounds/deal.ogg"))]
        );
    }

    #[test]
    fn every_back_shape_parses_back_to_the_same_def() {
        let toml = render(&rich_doc(RenderMode::Png));
        let manifest = Manifest::from_toml_str(&toml).unwrap();
        let by_name = |want: &str| {
            manifest
                .backs
                .iter()
                .find(|(name, _)| name.as_str() == want)
                .map(|(_, def)| def.clone())
                .unwrap()
        };

        assert_eq!(
            by_name("plain"),
            BackDef::Static {
                image: asset_path("backs/plain.png")
            }
        );
        assert_eq!(
            by_name("robot"),
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 4,
                timing: BackTiming::Fps(2),
                layout: BackLayout::Horizontal,
            }
        );
        assert_eq!(
            by_name("lift"),
            BackDef::Strip {
                image: asset_path("backs/lift.png"),
                frames: 3,
                timing: BackTiming::Fps(1),
                layout: BackLayout::Vertical,
            }
        );
        assert_eq!(
            by_name("bats"),
            BackDef::Frames {
                images: vec![
                    asset_path("backs/bats_0.png"),
                    asset_path("backs/bats_1.png")
                ],
                timing: BackTiming::Fps(3),
            }
        );
        assert_eq!(
            by_name("palm"),
            BackDef::Strip {
                image: asset_path("backs/palm.png"),
                frames: 4,
                timing: BackTiming::DurationsMs(vec![250, 250, 250, 49_250]),
                layout: BackLayout::Horizontal,
            }
        );
    }

    #[test]
    fn a_durations_ms_back_renders_the_documented_syntax() {
        let toml = render(&rich_doc(RenderMode::Png));
        assert!(
            toml.contains("durations_ms = [250, 250, 250, 49250]"),
            "{toml}"
        );
        assert!(
            !toml.contains("palm = { image = \"backs/palm.png\", frames = 4, fps"),
            "{toml}"
        );
    }

    #[test]
    fn a_vector_doc_renders_the_vector_render_mode() {
        let toml = render(&rich_doc(RenderMode::Vector));
        assert!(toml.contains("render_mode = \"vector\""), "{toml}");
    }

    #[test]
    fn an_author_less_color_background_no_sounds_doc_omits_those_pieces() {
        let doc = ThemeDoc {
            name: "Bare".to_owned(),
            author: None,
            render_mode: RenderMode::Png,
            base_size: (2, 2),
            backs: vec![(
                "plain".to_owned(),
                BackDef::Static {
                    image: asset_path("backs/plain.png"),
                },
            )],
            background: Background::Color(Color::new(0x00, 0x80, 0x00)),
            outline_color: Color::new(0, 0, 0),
            sounds: Vec::new(),
        };
        let toml = render(&doc);
        assert!(!toml.contains("author"), "{toml}");
        assert!(!toml.contains("[sounds]"), "{toml}");
        assert!(
            toml.contains("background = { color = \"#008000\" }"),
            "{toml}"
        );

        // Still a valid, parseable document.
        let manifest = Manifest::from_toml_str(&toml).unwrap();
        assert_eq!(manifest.author, None);
        assert!(manifest.sounds.is_empty());
    }

    #[test]
    fn a_del_bearing_theme_name_round_trips_through_the_real_manifest_parser() {
        // U+007F (DEL) is a TOML 1.0 basic-string control character, just
        // like the codepoints below 0x20 -- extract derives theme names
        // from arbitrary file stems, so a name containing DEL must still
        // produce a theme.toml the strict parser accepts.
        let doc = ThemeDoc {
            name: "Robot\u{7F}Name".to_owned(),
            ..rich_doc(RenderMode::Png)
        };
        let toml = render(&doc);
        let manifest = Manifest::from_toml_str(&toml).unwrap();
        assert_eq!(manifest.name, "Robot\u{7F}Name");
    }

    #[test]
    fn toml_string_escapes_special_characters() {
        // Mixed quote + backslash + newline content: `toml_edit` is free to
        // pick whichever TOML string representation it likes (this one
        // happens to become a multi-line basic string, since the input has
        // both characters that need escaping and an embedded newline) -- so
        // this asserts the representation-independent contract (parses back
        // to the exact original value) rather than pinning exact bytes.
        assert_eq!(toml_string("plain"), "\"plain\"");
        let rendered = toml_string("a\"b\\c\nd\re\tf");
        let reparsed: toml_edit::Value = rendered.parse().unwrap();
        assert_eq!(reparsed.as_str(), Some("a\"b\\c\nd\re\tf"));
    }

    #[test]
    fn toml_string_escapes_the_control_boundary_but_not_the_first_printable_char() {
        // The control-char cutoff is `< 0x20`, not `<= 0x20`: 0x1F must be
        // escaped, and 0x20 (space) must pass through unescaped. Neither
        // character forces `toml_edit` off its plain single-line basic
        // string representation, so exact bytes are stable here.
        assert_eq!(toml_string("\u{1F}"), "\"\\u001F\"");
        assert_eq!(toml_string(" "), "\" \"");
    }
}
