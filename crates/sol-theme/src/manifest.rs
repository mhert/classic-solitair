//! [`Manifest`]: the validated, typed parse of a `theme.toml` document.

use core::str::FromStr;

use serde::Deserialize;

use crate::back::{self, BackDef, BackName};
use crate::background::{self, Background, RawBackground};
use crate::color::Color;
use crate::error::ManifestError;
use crate::faces::{self, FacesSource};
use crate::ordered_map::OrderedMap;
use crate::path::RelativeAssetPath;
use crate::placeholders::{self, Placeholders, RawPlaceholders};
use crate::render_mode::RenderMode;
use crate::size::CardSize;

/// A fully validated `theme.toml` document.
///
/// Built only through [`Manifest::from_toml_bytes`] or
/// [`Manifest::from_toml_str`] — every field here has already passed
/// validation, so downstream code (the asset-loading layer) can
/// trust it without re-checking shapes.
///
/// ```
/// use sol_theme::{Manifest, RenderMode};
///
/// let toml = r##"
/// [theme]
/// name = "Example"
/// render_mode = "png"
///
/// [cards]
/// faces = "cards/"
/// base_size = [71, 96]
///
/// [backs]
/// plain = { image = "backs/plain.png" }
///
/// [table]
/// background = { color = "#008000" }
///
/// [placeholders]
/// empty_pile = { image = "placeholders/empty_pile.png" }
///
/// [drag]
/// outline_color = "#000000"
/// "##;
///
/// let manifest = Manifest::from_toml_str(toml)?;
/// assert_eq!(manifest.name, "Example");
/// assert_eq!(manifest.render_mode, RenderMode::Png);
/// assert_eq!(manifest.backs.len(), 1);
/// assert_eq!(
///     manifest.placeholders.empty_pile.map(|path| path.as_str().to_owned()),
///     Some("placeholders/empty_pile.png".to_owned())
/// );
/// # Ok::<(), sol_theme::ManifestError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// `[theme] name` — non-empty.
    pub name: String,
    /// `[theme] author`, if given.
    pub author: Option<String>,
    /// `[theme] render_mode`.
    pub render_mode: RenderMode,
    /// `[cards] faces`.
    pub faces: FacesSource,
    /// `[cards] base_size`.
    pub base_size: CardSize,
    /// `[backs]`, in declaration order (at least one entry).
    pub backs: Vec<(BackName, BackDef)>,
    /// `[table] background`.
    pub background: Background,
    /// `[placeholders]`; every field is `None` when the section is absent.
    pub placeholders: Placeholders,
    /// `[drag] outline_color`.
    pub outline_color: Color,
    /// `[sounds]`, in declaration order; empty when `[sounds]` is absent.
    /// Each path is validated theme-package-relative.
    pub sounds: Vec<(String, RelativeAssetPath)>,
}

impl Manifest {
    /// Parses and validates a `theme.toml` document from UTF-8 bytes:
    /// bytes → UTF-8 check → TOML → validation.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::InvalidUtf8`] if `bytes` is not valid
    /// UTF-8, or any other [`ManifestError`] variant under the same
    /// conditions as [`Manifest::from_toml_str`].
    pub fn from_toml_bytes(bytes: &[u8]) -> Result<Self, ManifestError> {
        let text = core::str::from_utf8(bytes)?;
        Self::from_toml_str(text)
    }

    /// Parses and validates a `theme.toml` document from text.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::InvalidToml`] if `text` is not syntactically
    /// valid TOML or does not match the required shape (a missing required
    /// section/key or an unknown key anywhere). Returns a
    /// domain-specific [`ManifestError`] variant if the document parses but
    /// violates a validation rule (an empty name, an inconsistent
    /// back definition, a non-relative asset path, and so on).
    pub fn from_toml_str(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|source| ManifestError::InvalidToml {
                message: source.to_string(),
            })?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawManifest) -> Result<Self, ManifestError> {
        if raw.theme.name.is_empty() {
            return Err(ManifestError::EmptyName);
        }

        let faces = faces::validate(&raw.cards.faces, raw.theme.render_mode)?;
        let base_size = CardSize::try_from(raw.cards.base_size)?;

        if raw.backs.0.is_empty() {
            return Err(ManifestError::NoBacks);
        }
        let mut backs = Vec::with_capacity(raw.backs.0.len());
        for (raw_name, raw_def) in raw.backs.0 {
            let name = BackName::try_from(raw_name)?;
            let def = back::validate(&name, raw_def)?;
            backs.push((name, def));
        }

        let background = background::validate(raw.table.background)?;
        let placeholders = placeholders::validate(raw.placeholders)?;

        let outline_color = Color::from_str(&raw.drag.outline_color).map_err(|source| {
            ManifestError::InvalidColor {
                field: "drag.outline_color",
                source,
            }
        })?;

        let mut sounds = Vec::with_capacity(raw.sounds.0.len());
        for (key, path) in raw.sounds.0 {
            let path = RelativeAssetPath::parse(format!("sounds.{key}"), &path)?;
            sounds.push((key, path));
        }

        Ok(Self {
            name: raw.theme.name,
            author: raw.theme.author,
            render_mode: raw.theme.render_mode,
            faces,
            base_size,
            backs,
            background,
            placeholders,
            outline_color,
            sounds,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    theme: RawTheme,
    cards: RawCards,
    backs: OrderedMap<back::RawBackDef>,
    table: RawTable,
    #[serde(default)]
    placeholders: RawPlaceholders,
    drag: RawDrag,
    #[serde(default)]
    sounds: OrderedMap<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    name: String,
    #[serde(default)]
    author: Option<String>,
    render_mode: RenderMode,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCards {
    faces: String,
    base_size: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    background: RawBackground,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDrag {
    outline_color: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::back::{BackLayout, BackNameError};
    use crate::size::CardSizeError;
    use crate::testkit::asset_path;

    /// A minimal, fully valid document: every required section, one static
    /// back, no optional fields. Each error test mutates exactly one piece
    /// of this via [`str::replace`].
    fn base() -> String {
        "[theme]\n\
         name = \"Example\"\n\
         render_mode = \"png\"\n\
         \n\
         [cards]\n\
         faces = \"cards/\"\n\
         base_size = [71, 96]\n\
         \n\
         [backs]\n\
         plain = { image = \"backs/plain.png\" }\n\
         \n\
         [table]\n\
         background = { color = \"#008000\" }\n\
         \n\
         [drag]\n\
         outline_color = \"#000000\"\n"
            .to_owned()
    }

    /// `base()` with `from` replaced by `to`; panics (test-only) if `from`
    /// is not present, so a typo in a test never silently no-ops.
    fn mutate(from: &str, to: &str) -> String {
        let text = base();
        assert!(text.contains(from), "fixture does not contain {from:?}");
        text.replace(from, to)
    }

    // -- baseline sanity --

    #[test]
    fn the_minimal_fixture_parses() {
        Manifest::from_toml_str(&base()).unwrap();
    }

    #[test]
    fn from_toml_bytes_accepts_valid_utf8_and_delegates_to_from_toml_str() {
        let manifest = Manifest::from_toml_bytes(base().as_bytes()).unwrap();
        assert_eq!(manifest.name, "Example");
    }

    // -- the full example document --

    const FULL_EXAMPLE: &str = "[theme]\n\
        name        = \"Example\"\n\
        author      = \"…\"\n\
        render_mode = \"png\"         # \"png\" | \"vector\"\n\
        \n\
        [cards]\n\
        faces     = \"cards/\"        # 52 images named e.g. \"spades_01.png\"..\"clubs_13.png\",\n\
        \x20                           # or a single SVG sheet for vector themes\n\
        base_size = [71, 96]        # logical card size in pixels (Win98 original: 71×96)\n\
        \n\
        [backs]\n\
        # Animated backs are HORIZONTAL FRAME STRIPS: one image, `frames` equal slices.\n\
        # A 4-frame 71×96 back is a single 284×96 PNG. `layout = \"vertical\"` is also allowed.\n\
        # Alternatively a list of per-frame files may be given; soltool can pack them.\n\
        robot  = { image = \"backs/robot.png\", frames = 4, fps = 2 }\n\
        plain  = { image = \"backs/plain.png\" }\n\
        # list form:\n\
        # bats = { image = [\"backs/bats_0.png\", \"backs/bats_1.png\"], fps = 3 }\n\
        \n\
        [table]\n\
        background = { color = \"#008000\" }          # or { image = \"table.png\", tile = true }\n\
        \n\
        [placeholders]                               # optional; every key optional\n\
        empty_pile    = { image = \"placeholders/empty_pile.png\" }\n\
        stock_recycle = { image = \"placeholders/stock_recycle.png\" }\n\
        stock_blocked = { image = \"placeholders/stock_blocked.png\" }\n\
        \n\
        [drag]\n\
        outline_color = \"#000000\"                    # outline-dragging rectangle color\n\
        \n\
        [sounds]                                     # optional\n\
        deal = \"sounds/deal.ogg\"\n";

    #[test]
    fn the_full_example_parses_field_by_field() {
        let manifest = Manifest::from_toml_str(FULL_EXAMPLE).unwrap();

        assert_eq!(manifest.name, "Example");
        assert_eq!(manifest.author.as_deref(), Some("…"));
        assert_eq!(manifest.render_mode, RenderMode::Png);
        assert_eq!(manifest.faces, FacesSource::Directory(asset_path("cards/")));
        assert_eq!(
            manifest.base_size,
            CardSize {
                width: 71,
                height: 96
            }
        );

        assert_eq!(manifest.backs.len(), 2, "bats is commented out");
        let (robot_name, robot_def) = manifest.backs.first().unwrap();
        assert_eq!(robot_name.as_str(), "robot");
        assert_eq!(
            *robot_def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 4,
                fps: 2,
                layout: BackLayout::Horizontal,
            }
        );
        let (plain_name, plain_def) = manifest.backs.get(1).unwrap();
        assert_eq!(plain_name.as_str(), "plain");
        assert_eq!(
            *plain_def,
            BackDef::Static {
                image: asset_path("backs/plain.png")
            }
        );
        assert!(
            manifest
                .backs
                .iter()
                .all(|(name, _)| name.as_str() != "bats"),
            "the commented-out bats entry must not appear"
        );

        assert_eq!(
            manifest.background,
            Background::Color(Color::new(0x00, 0x80, 0x00))
        );
        assert_eq!(
            manifest.placeholders,
            Placeholders {
                empty_pile: Some(asset_path("placeholders/empty_pile.png")),
                stock_recycle: Some(asset_path("placeholders/stock_recycle.png")),
                stock_blocked: Some(asset_path("placeholders/stock_blocked.png")),
            }
        );
        assert_eq!(manifest.outline_color, Color::new(0x00, 0x00, 0x00));
        assert_eq!(
            manifest.sounds,
            vec![("deal".to_owned(), asset_path("sounds/deal.ogg"))]
        );
    }

    // -- [placeholders] --

    /// The section predates no theme: a document without it is still valid
    /// and simply supplies nothing, which is what every existing theme gets.
    #[test]
    fn an_absent_placeholders_section_is_valid_and_empty() {
        let manifest = Manifest::from_toml_str(&base()).unwrap();
        assert_eq!(manifest.placeholders, Placeholders::default());
        assert!(manifest.placeholders.is_empty());
    }

    #[test]
    fn a_partial_placeholders_section_fills_only_the_keys_it_names() {
        let text = mutate(
            "[drag]",
            "[placeholders]\nempty_pile = { image = \"ghost.png\" }\n\n[drag]",
        );
        let manifest = Manifest::from_toml_str(&text).unwrap();
        assert_eq!(
            manifest.placeholders,
            Placeholders {
                empty_pile: Some(asset_path("ghost.png")),
                stock_recycle: None,
                stock_blocked: None,
            }
        );
    }

    #[test]
    fn an_unknown_placeholders_key_is_rejected() {
        let text = mutate(
            "[drag]",
            "[placeholders]\nempty_column = { image = \"ghost.png\" }\n\n[drag]",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn an_unknown_key_inside_a_placeholder_entry_is_rejected() {
        let text = mutate(
            "[drag]",
            "[placeholders]\nempty_pile = { image = \"ghost.png\", tile = true }\n\n[drag]",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn a_non_relative_placeholder_path_is_rejected() {
        let text = mutate(
            "[drag]",
            "[placeholders]\nstock_recycle = { image = \"/abs.png\" }\n\n[drag]",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    // -- list ("bats") form --

    #[test]
    fn list_form_parses_as_frames() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "bats = { image = [\"backs/bats_0.png\", \"backs/bats_1.png\"], fps = 3 }",
        );
        let manifest = Manifest::from_toml_str(&text).unwrap();
        let (name, def) = manifest.backs.first().unwrap();
        assert_eq!(name.as_str(), "bats");
        assert_eq!(
            *def,
            BackDef::Frames {
                images: vec![
                    asset_path("backs/bats_0.png"),
                    asset_path("backs/bats_1.png")
                ],
                fps: 3,
            }
        );
    }

    // -- every ManifestError variant, constructed through from_toml_str --

    #[test]
    fn invalid_utf8_bytes_are_rejected() {
        let error = Manifest::from_toml_bytes(&[0xFF, 0xFE, 0xFD]).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidUtf8(_)));
    }

    #[test]
    fn invalid_toml_syntax_is_rejected() {
        let error = Manifest::from_toml_str("this is not [ toml").unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn base_size_with_too_many_elements_is_rejected() {
        // Regression test: a naive `[i64; 2]` raw field silently drops a
        // third element instead of rejecting it — `CardSize::try_from`
        // must see the whole `Vec` to catch this: wrong arity must be
        // rejected, never silently truncated.
        let text = mutate("base_size = [71, 96]", "base_size = [71, 96, 3]");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidCardSize(CardSizeError::WrongArity { count: 3 })
        ));
    }

    #[test]
    fn base_size_with_too_few_elements_is_rejected() {
        let text = mutate("base_size = [71, 96]", "base_size = [71]");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidCardSize(CardSizeError::WrongArity { count: 1 })
        ));
    }

    #[test]
    fn a_missing_required_section_is_invalid_toml() {
        let error =
            Manifest::from_toml_str("[theme]\nname = \"x\"\nrender_mode = \"png\"\n").unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn an_unknown_top_level_key_is_invalid_toml() {
        let text = mutate("name = \"Example\"", "name = \"Example\"\nbogus = true");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn an_unknown_key_nested_inside_a_back_is_invalid_toml() {
        // Unknown keys are errors anywhere, not just at the top level.
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", bogus = 1 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn an_unknown_key_nested_inside_background_is_invalid_toml() {
        let text = mutate(
            "background = { color = \"#008000\" }",
            "background = { color = \"#008000\", bogus = 1 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidToml { .. }));
    }

    #[test]
    fn empty_name_is_rejected() {
        let text = mutate("name = \"Example\"", "name = \"\"");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::EmptyName));
    }

    /// `max_scale` was an xbrz-only declaration; with that mode gone the
    /// key is not part of the format at all, and the manifest's
    /// deny-unknown-fields parse is what says so.
    #[test]
    fn max_scale_is_no_longer_a_known_key() {
        let text = mutate(
            "render_mode = \"png\"",
            "render_mode = \"png\"\nmax_scale = 3",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err().to_string();
        assert!(error.contains("max_scale"), "{error}");
    }

    #[test]
    fn an_invalid_card_size_is_rejected() {
        let text = mutate("base_size = [71, 96]", "base_size = [0, 96]");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidCardSize(_)));
    }

    #[test]
    fn an_unrecognized_faces_shape_is_rejected() {
        let text = mutate("faces = \"cards/\"", "faces = \"cards\"");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidFacesShape { .. }));
    }

    #[test]
    fn an_svg_sheet_on_a_non_vector_theme_is_rejected() {
        let text = mutate("faces = \"cards/\"", "faces = \"cards/faces.svg\"");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::SvgFacesRequireVectorMode { .. }
        ));
    }

    #[test]
    fn an_empty_backs_table_is_rejected() {
        let text = mutate("plain = { image = \"backs/plain.png\" }", "");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::NoBacks));
    }

    #[test]
    fn an_invalid_back_name_is_rejected() {
        let text = mutate("plain = { image", "\"Bad Name\" = { image");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidBackName(_)));
        assert!(matches!(
            error,
            ManifestError::InvalidBackName(BackNameError { .. })
        ));
    }

    #[test]
    fn back_fps_without_frames_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", fps = 2 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsWithoutFrames { back } if back.as_str() == "plain"
        ));
    }

    #[test]
    fn back_frames_without_fps_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 4 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::BackFramesWithoutFps { .. }));
    }

    #[test]
    fn back_layout_without_strip_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", layout = \"vertical\" }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackLayoutWithoutStrip { .. }
        ));
    }

    #[test]
    fn back_too_few_frames_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 1, fps = 2 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewFrames { frames: 1, .. }
        ));
    }

    #[test]
    fn back_zero_fps_on_a_strip_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 4, fps = 0 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::BackZeroFps { .. }));
    }

    #[test]
    fn back_frames_too_large_is_rejected_with_truthful_error() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 99999999999, fps = 2 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFramesTooLarge {
                frames: 99_999_999_999,
                ..
            }
        ));
    }

    #[test]
    fn back_fps_too_large_is_rejected_with_truthful_error() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 4, fps = 99999999999 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsTooLarge {
                fps: 99_999_999_999,
                ..
            }
        ));
    }

    #[test]
    fn back_invalid_layout_value_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = \"backs/plain.png\", frames = 4, fps = 2, layout = \"diagonal\" }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::BackInvalidLayout { .. }));
    }

    #[test]
    fn back_too_few_list_images_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = [\"backs/only.png\"], fps = 2 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewListImages { count: 1, .. }
        ));
    }

    #[test]
    fn back_list_with_frames_is_rejected() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = [\"a.png\", \"b.png\"], frames = 2, fps = 2 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackListWithFramesOrLayout { .. }
        ));
    }

    #[test]
    fn back_list_fps_too_large_is_rejected_with_truthful_error() {
        let text = mutate(
            "plain = { image = \"backs/plain.png\" }",
            "plain = { image = [\"a.png\", \"b.png\"], fps = 99999999999 }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsTooLarge {
                fps: 99_999_999_999,
                ..
            }
        ));
    }

    #[test]
    fn background_needs_exactly_one_of_color_or_image() {
        let text = mutate("background = { color = \"#008000\" }", "background = {}");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackgroundNeedsExactlyOneOfColorOrImage
        ));
    }

    #[test]
    fn background_tile_without_image_is_rejected() {
        let text = mutate(
            "background = { color = \"#008000\" }",
            "background = { color = \"#008000\", tile = true }",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::BackgroundTileWithoutImage));
    }

    #[test]
    fn an_invalid_color_is_rejected_and_names_its_field() {
        let text = mutate("outline_color = \"#000000\"", "outline_color = \"green\"");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidColor {
                field: "drag.outline_color",
                ..
            }
        ));
    }

    #[test]
    fn a_non_relative_path_is_rejected() {
        let text = mutate("faces = \"cards/\"", "faces = \"/cards/\"");
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn a_non_relative_sound_path_is_rejected() {
        let text = mutate(
            "[drag]\noutline_color = \"#000000\"\n",
            "[drag]\noutline_color = \"#000000\"\n\n[sounds]\ndeal = \"/sounds/deal.ogg\"\n",
        );
        let error = Manifest::from_toml_str(&text).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn sounds_default_to_empty_when_absent() {
        let manifest = Manifest::from_toml_str(&base()).unwrap();
        assert!(manifest.sounds.is_empty());
    }
}
