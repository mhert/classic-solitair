//! [`Theme`]: a fully validated theme package — [`Manifest`] plus every
//! asset it references, loaded and validated against the theme format's
//! rules (the validation matrix). [`Theme::from_source`] is the core entry point;
//! [`Theme::load_dir`], [`Theme::load_zip_bytes`], and [`Theme::load_path`]
//! are thin conveniences over it for the three ways a theme package is
//! actually stored.

use std::path::Path;

use crate::asset::{Asset, AssetKind};
use crate::back::BackName;
use crate::dir_source::DirSource;
use crate::face::{FaceRank, FaceSuit, canonical_faces};
use crate::load_background::{self, LoadedBackground};
use crate::load_backs::{self, LoadedBack};
use crate::load_faces;
use crate::load_placeholders::{self, LoadedPlaceholders};
use crate::load_sounds;
use crate::manifest::Manifest;
use crate::path::RelativeAssetPath;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;
use crate::zip_source::ZipSource;

/// A theme package, fully loaded and validated: a [`Manifest`]
/// plus every face, back, background, and sound asset it references.
///
/// Loading order is manifest, then faces in canonical order, then backs in
/// declaration order, then the background, then placeholders, then sounds —
/// the first failure wins (see [`ThemeError`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The validated `theme.toml` document this theme was built from.
    pub manifest: Manifest,
    faces: Vec<(FaceSuit, FaceRank, Asset)>,
    backs: Vec<(BackName, LoadedBack)>,
    background: LoadedBackground,
    placeholders: LoadedPlaceholders,
    sounds: Vec<(String, Vec<u8>)>,
}

impl Theme {
    /// Loads and validates a complete theme package from `source`: reads
    /// `theme.toml`, parses it via [`Manifest`], then loads and validates
    /// every asset it references, in the order documented on [`Theme`].
    ///
    /// ```
    /// use sol_theme::{MemSource, Theme};
    ///
    /// fn png_1x1() -> Vec<u8> {
    ///     let mut bytes = Vec::new();
    ///     {
    ///         let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    ///         encoder.set_color(png::ColorType::Grayscale);
    ///         encoder.set_depth(png::BitDepth::Eight);
    ///         let mut writer = encoder.write_header().unwrap();
    ///         writer.write_image_data(&[0_u8]).unwrap();
    ///     }
    ///     bytes
    /// }
    ///
    /// fn main() -> Result<(), sol_theme::ThemeError> {
    ///     let manifest = br##"
    /// [theme]
    /// name = "Example"
    /// render_mode = "png"
    ///
    /// [cards]
    /// faces = "cards/"
    /// base_size = [1, 1]
    ///
    /// [backs]
    /// plain = { image = "backs/plain.png" }
    ///
    /// [table]
    /// background = { color = "#008000" }
    ///
    /// [drag]
    /// outline_color = "#000000"
    /// "##;
    ///
    ///     let mut source = MemSource::new()
    ///         .with_file("theme.toml", &manifest[..])
    ///         .with_file("backs/plain.png", png_1x1());
    ///     for (suit, rank) in sol_theme::canonical_faces() {
    ///         source = source.with_file(format!("cards/{}.png", suit.stem(rank)), png_1x1());
    ///     }
    ///
    ///     let theme = Theme::from_source(&source)?;
    ///     assert_eq!(theme.manifest.name, "Example");
    ///     assert_eq!(theme.backs().len(), 1);
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::ManifestUnreadable`] if `theme.toml` cannot be
    /// read from `source`, [`ThemeError::Manifest`] if it fails manifest
    /// validation, or any other [`ThemeError`] variant if a face, back,
    /// background, placeholder, or sound asset it references is missing, the
    /// wrong format, or the wrong size.
    pub fn from_source(source: &impl AssetSource) -> Result<Self, ThemeError> {
        let toml_bytes = source
            .read(&RelativeAssetPath::generated("theme.toml"))
            .map_err(|source| ThemeError::ManifestUnreadable { source })?;
        let manifest = Manifest::from_toml_bytes(&toml_bytes)?;
        let kind = AssetKind::for_render_mode(manifest.render_mode);

        let face_assets = load_faces::load(source, &manifest.faces, kind, manifest.base_size)?;
        let faces = canonical_faces()
            .zip(face_assets)
            .map(|((suit, rank), asset)| (suit, rank, asset))
            .collect();

        let backs = load_backs::load(source, &manifest.backs, kind, manifest.base_size)?;
        let background = load_background::load(source, &manifest.background, kind)?;
        let placeholders =
            load_placeholders::load(source, &manifest.placeholders, kind, manifest.base_size)?;
        let sounds = load_sounds::load(source, &manifest.sounds)?;

        Ok(Self {
            manifest,
            faces,
            backs,
            background,
            placeholders,
            sounds,
        })
    }

    /// Loads a theme package from a directory at `path`.
    ///
    /// # Errors
    ///
    /// See [`Theme::from_source`].
    pub fn load_dir(path: impl AsRef<Path>) -> Result<Self, ThemeError> {
        Self::from_source(&DirSource::new(path.as_ref()))
    }

    /// Loads a theme package from zip archive bytes, entirely in memory.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::MalformedZip`] if `bytes` is not a valid zip
    /// archive. See [`Theme::from_source`] for every other error.
    pub fn load_zip_bytes(bytes: &[u8]) -> Result<Self, ThemeError> {
        let source = ZipSource::from_bytes(bytes)?;
        Self::from_source(&source)
    }

    /// Loads a theme package from `path`: a directory, or a file (any
    /// name) treated as a zip archive.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::UnrecognizedPackage`] if `path` is neither a
    /// directory nor readable as a valid zip archive. See
    /// [`Theme::from_source`] for every other error.
    pub fn load_path(path: impl AsRef<Path>) -> Result<Self, ThemeError> {
        let path = path.as_ref();
        if path.is_dir() {
            return Self::load_dir(path);
        }
        let unrecognized = |message: String| ThemeError::UnrecognizedPackage {
            path: path.display().to_string(),
            message,
        };
        let bytes = std::fs::read(path).map_err(|error| unrecognized(error.to_string()))?;
        let source =
            ZipSource::from_bytes(&bytes).map_err(|error| unrecognized(error.to_string()))?;
        Self::from_source(&source)
    }

    /// The theme's 52 face assets, in canonical order (spades, hearts,
    /// diamonds, clubs; rank ascending).
    pub fn faces(&self) -> impl Iterator<Item = (FaceSuit, FaceRank, &Asset)> {
        self.faces
            .iter()
            .map(|(suit, rank, asset)| (*suit, *rank, asset))
    }

    /// The loaded asset for one canonical face.
    ///
    /// Always `Some` for a `Theme` built through [`Theme::from_source`]
    /// (which validates all 52 canonical faces before returning); the
    /// accessor stays total (returning `Option` rather than indexing or
    /// `unwrap`) because nothing here can prove that invariant to the
    /// compiler.
    #[must_use]
    pub fn face(&self, suit: FaceSuit, rank: FaceRank) -> Option<&Asset> {
        self.faces
            .iter()
            .find(|(s, r, _)| *s == suit && *r == rank)
            .map(|(_, _, asset)| asset)
    }

    /// Every `[backs]` entry, loaded and validated, in declaration order.
    #[must_use]
    pub fn backs(&self) -> &[(BackName, LoadedBack)] {
        &self.backs
    }

    /// The table background, loaded.
    #[must_use]
    pub fn background(&self) -> &LoadedBackground {
        &self.background
    }

    /// The `[placeholders]` assets, loaded; every field is `None` for a
    /// theme that declares none.
    #[must_use]
    pub fn placeholders(&self) -> &LoadedPlaceholders {
        &self.placeholders
    }

    /// One `[sounds]` entry's bytes, by its `[sounds]` key.
    #[must_use]
    pub fn sound(&self, name: &str) -> Option<&[u8]> {
        self.sounds
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::back::BackLayout;
    use crate::mem_source::MemSource;
    use crate::render_mode::RenderMode;
    use crate::testkit::asset_path;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
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

    fn svg_bytes(width: u32, height: u32) -> Vec<u8> {
        format!(r#"<svg width="{width}" height="{height}"></svg>"#).into_bytes()
    }

    const PNG_MANIFEST: &str = "[theme]\n\
         name = \"PNG Example\"\n\
         render_mode = \"png\"\n\
         \n\
         [cards]\n\
         faces = \"cards/\"\n\
         base_size = [71, 96]\n\
         \n\
         [backs]\n\
         plain = { image = \"backs/plain.png\" }\n\
         robot  = { image = \"backs/robot.png\", frames = 4, fps = 2 }\n\
         \n\
         [table]\n\
         background = { color = \"#008000\" }\n\
         \n\
         [drag]\n\
         outline_color = \"#000000\"\n\
         \n\
         [sounds]\n\
         deal = \"sounds/deal.ogg\"\n";

    /// `(path, bytes)` for a minimal, complete png theme: 52 face PNGs, a
    /// static back, a 4-frame horizontal strip back, a color background,
    /// one sound — the raw pairs, so tests that need to break one entry
    /// can filter or replace it before collecting into a [`MemSource`]
    /// (mirrors `load_faces.rs`'s `face_pairs` helper).
    fn png_entries() -> Vec<(String, Vec<u8>)> {
        let mut entries = vec![
            ("theme.toml".to_owned(), PNG_MANIFEST.as_bytes().to_vec()),
            ("backs/plain.png".to_owned(), png_bytes(71, 96)),
            ("backs/robot.png".to_owned(), png_bytes(71 * 4, 96)),
            ("sounds/deal.ogg".to_owned(), b"deal-bytes".to_vec()),
        ];
        for (suit, rank) in canonical_faces() {
            entries.push((format!("cards/{}.png", suit.stem(rank)), png_bytes(71, 96)));
        }
        entries
    }

    /// A minimal, complete png theme: 52 face PNGs, a static back, a
    /// 4-frame horizontal strip back, a color background, one sound.
    fn png_source() -> MemSource {
        png_entries().into_iter().collect()
    }

    /// [`png_source`] plus a `[placeholders]` section declaring all three
    /// slots. `ghost` sizes the empty-pile image so a test can make exactly
    /// that one off-`base_size` while the other two stay valid.
    fn png_source_with_placeholders(ghost: (u32, u32)) -> MemSource {
        let manifest = PNG_MANIFEST.replace(
            "[drag]",
            "[placeholders]\n\
             empty_pile = { image = \"placeholders/ghost.png\" }\n\
             stock_recycle = { image = \"placeholders/ring.png\" }\n\
             stock_blocked = { image = \"placeholders/cross.png\" }\n\
             \n\
             [drag]",
        );
        let mut entries: Vec<(String, Vec<u8>)> = png_entries()
            .into_iter()
            .filter(|(path, _)| path != "theme.toml")
            .collect();
        entries.push(("theme.toml".to_owned(), manifest.into_bytes()));
        entries.push((
            "placeholders/ghost.png".to_owned(),
            png_bytes(ghost.0, ghost.1),
        ));
        entries.push(("placeholders/ring.png".to_owned(), png_bytes(71, 96)));
        entries.push(("placeholders/cross.png".to_owned(), png_bytes(71, 96)));
        entries.into_iter().collect()
    }

    const VECTOR_MANIFEST: &str = "[theme]\n\
         name = \"Vector Example\"\n\
         render_mode = \"vector\"\n\
         \n\
         [cards]\n\
         faces = \"cards/\"\n\
         base_size = [71, 96]\n\
         \n\
         [backs]\n\
         plain = { image = \"backs/plain.svg\" }\n\
         strip  = { image = \"backs/strip.svg\", frames = 2, fps = 3 }\n\
         \n\
         [table]\n\
         background = { color = \"#008000\" }\n\
         \n\
         [drag]\n\
         outline_color = \"#000000\"\n";

    /// A minimal, complete vector theme: 52 face SVGs, a static back, a
    /// 2-frame horizontal SVG strip back, a color background.
    fn vector_source() -> MemSource {
        let mut source = MemSource::new().with_file("theme.toml", VECTOR_MANIFEST.as_bytes());
        for (suit, rank) in canonical_faces() {
            source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg_bytes(71, 96));
        }
        source
            .with_file("backs/plain.svg", svg_bytes(71, 96))
            .with_file("backs/strip.svg", svg_bytes(71 * 2, 96))
    }

    // -- MemSource end-to-end: png theme --

    #[test]
    fn a_complete_png_theme_loads() {
        let theme = Theme::from_source(&png_source()).unwrap();
        assert_eq!(theme.manifest.name, "PNG Example");
        assert_eq!(theme.manifest.render_mode, RenderMode::Png);
    }

    // -- [placeholders] end-to-end --

    /// Every theme written before the section existed still loads, and
    /// supplies nothing to draw.
    #[test]
    fn a_theme_without_a_placeholders_section_loads_with_none() {
        let theme = Theme::from_source(&png_source()).unwrap();
        assert_eq!(*theme.placeholders(), LoadedPlaceholders::default());
    }

    #[test]
    fn a_theme_with_placeholders_loads_all_three_assets() {
        let theme = Theme::from_source(&png_source_with_placeholders((71, 96))).unwrap();
        let placeholders = theme.placeholders();
        assert_eq!(
            placeholders
                .empty_pile
                .as_ref()
                .map(|asset| asset.path.as_str()),
            Some("placeholders/ghost.png")
        );
        assert_eq!(
            placeholders
                .stock_recycle
                .as_ref()
                .map(|asset| asset.path.as_str()),
            Some("placeholders/ring.png")
        );
        assert_eq!(
            placeholders
                .stock_blocked
                .as_ref()
                .map(|asset| asset.path.as_str()),
            Some("placeholders/cross.png")
        );
    }

    /// One bad placeholder fails the whole package, and the error names the
    /// slot rather than leaving the reader to guess which of the three.
    #[test]
    fn an_off_size_placeholder_fails_the_theme_naming_its_slot() {
        let error = Theme::from_source(&png_source_with_placeholders((71, 95))).unwrap_err();
        let message = error.to_string();
        assert!(
            matches!(error, ThemeError::PlaceholderWrongSize { .. }),
            "{message}"
        );
        assert!(message.contains("empty_pile"), "{message}");
    }

    #[test]
    fn loading_the_same_theme_twice_produces_equal_themes() {
        // `Theme` derives `PartialEq`/`Eq` for whole-value assertions like
        // this one; exercise it directly rather than leaving it unused.
        let a = Theme::from_source(&png_source()).unwrap();
        let b = Theme::from_source(&png_source()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn face_lookup_finds_a_specific_face() {
        let theme = Theme::from_source(&png_source()).unwrap();
        let asset = theme
            .face(FaceSuit::Hearts, FaceRank::try_from(7).unwrap())
            .unwrap();
        assert_eq!(asset.path, asset_path("cards/hearts_07.png"));
        assert_eq!(asset.kind, AssetKind::Png);
    }

    #[test]
    fn faces_iterate_in_canonical_order_spades_01_first_clubs_13_last() {
        let theme = Theme::from_source(&png_source()).unwrap();
        let all: Vec<_> = theme.faces().collect();
        assert_eq!(all.len(), 52);

        let (first_suit, first_rank, first_asset) = all.first().copied().unwrap();
        assert_eq!(first_suit, FaceSuit::Spades);
        assert_eq!(first_rank.get(), 1);
        assert_eq!(first_asset.path, asset_path("cards/spades_01.png"));

        let (last_suit, last_rank, last_asset) = all.last().copied().unwrap();
        assert_eq!(last_suit, FaceSuit::Clubs);
        assert_eq!(last_rank.get(), 13);
        assert_eq!(last_asset.path, asset_path("cards/clubs_13.png"));
    }

    #[test]
    fn backs_expose_frame_math_for_static_and_strip() {
        let theme = Theme::from_source(&png_source()).unwrap();
        let backs = theme.backs();
        assert_eq!(backs.len(), 2);

        let (plain_name, plain) = backs.first().unwrap();
        assert_eq!(plain_name.as_str(), "plain");
        assert_eq!(plain.frame_count, 1);
        assert_eq!(plain.fps, None);
        assert_eq!(plain.layout, None);
        assert_eq!(plain.assets.len(), 1);

        let (robot_name, robot) = backs.get(1).unwrap();
        assert_eq!(robot_name.as_str(), "robot");
        assert_eq!(robot.frame_count, 4);
        assert_eq!(robot.fps, Some(2));
        assert_eq!(robot.layout, Some(BackLayout::Horizontal));
        assert_eq!(robot.assets.len(), 1);
        assert_eq!(robot.assets.first().unwrap().size.width, 71 * 4);
    }

    #[test]
    fn background_loads_as_a_color() {
        let theme = Theme::from_source(&png_source()).unwrap();
        assert_eq!(
            *theme.background(),
            LoadedBackground::Color(crate::color::Color::new(0x00, 0x80, 0x00))
        );
    }

    #[test]
    fn sound_lookup_finds_bytes_by_name() {
        let theme = Theme::from_source(&png_source()).unwrap();
        assert_eq!(theme.sound("deal"), Some(&b"deal-bytes"[..]));
        assert_eq!(theme.sound("nope"), None);
    }

    // -- MemSource end-to-end: vector theme (directory form) --

    #[test]
    fn a_complete_vector_theme_loads() {
        let theme = Theme::from_source(&vector_source()).unwrap();
        assert_eq!(theme.manifest.render_mode, RenderMode::Vector);
        assert_eq!(theme.faces().count(), 52);
        assert!(
            theme
                .faces()
                .all(|(_, _, asset)| asset.kind == AssetKind::Svg)
        );
    }

    #[test]
    fn a_vector_strip_back_loads_with_two_frames() {
        let theme = Theme::from_source(&vector_source()).unwrap();
        let (_, strip) = theme
            .backs()
            .iter()
            .find(|(name, _)| name.as_str() == "strip")
            .unwrap();
        assert_eq!(strip.frame_count, 2);
        assert_eq!(strip.assets.first().unwrap().kind, AssetKind::Svg);
    }

    // -- MemSource end-to-end: vector theme, SVG sheet form --

    #[test]
    fn a_vector_theme_with_an_svg_sheet_loads_52_identical_face_assets() {
        let manifest = "[theme]\n\
             name = \"Sheet Example\"\n\
             render_mode = \"vector\"\n\
             \n\
             [cards]\n\
             faces = \"cards/sheet.svg\"\n\
             base_size = [71, 96]\n\
             \n\
             [backs]\n\
             plain = { image = \"backs/plain.svg\" }\n\
             \n\
             [table]\n\
             background = { color = \"#008000\" }\n\
             \n\
             [drag]\n\
             outline_color = \"#000000\"\n";
        let source = MemSource::new()
            .with_file("theme.toml", manifest.as_bytes())
            .with_file("cards/sheet.svg", svg_bytes(71 * 13, 96 * 4))
            .with_file("backs/plain.svg", svg_bytes(71, 96));

        let theme = Theme::from_source(&source).unwrap();
        assert_eq!(theme.faces().count(), 52);
        assert!(
            theme
                .faces()
                .all(|(_, _, asset)| asset.path == asset_path("cards/sheet.svg"))
        );
    }

    // -- validation matrix row 1 / row 8: manifest presence & validity --

    #[test]
    fn a_missing_theme_toml_is_manifest_unreadable() {
        let source = MemSource::new();
        let error = Theme::from_source(&source).unwrap_err();
        assert!(matches!(error, ThemeError::ManifestUnreadable { .. }));
    }

    #[test]
    fn an_invalid_theme_toml_wraps_the_manifest_error() {
        let source = MemSource::new().with_file("theme.toml", b"not [ valid toml".to_vec());
        let error = Theme::from_source(&source).unwrap_err();
        assert!(matches!(error, ThemeError::Manifest(_)));
    }

    // -- loading order / first-failure-wins across the whole pipeline --

    #[test]
    fn a_missing_face_wins_over_a_bad_back_further_down_the_pipeline() {
        // Break both a face (missing) and a back (corrupt bytes); faces
        // (row 3) are validated before backs (row 5), so the missing-face
        // error must win.
        let broken: MemSource = png_entries()
            .into_iter()
            .filter(|(path, _)| path != "cards/spades_01.png")
            .map(|(path, bytes)| {
                if path == "backs/plain.png" {
                    (path, b"not a png".to_vec())
                } else {
                    (path, bytes)
                }
            })
            .collect();

        let error = Theme::from_source(&broken).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::FaceUnreadable { name, .. } if name == "spades_01"
        ));
    }

    #[test]
    fn a_missing_back_wins_over_a_missing_sound_further_down_the_pipeline() {
        // Faces (row 3) already all succeed here; backs (row 5) are
        // validated before the background (row 6) and sounds (row 7), so
        // this must fail at the back, not fall through to the sound.
        let broken: MemSource = png_entries()
            .into_iter()
            .filter(|(path, _)| path != "backs/robot.png" && path != "sounds/deal.ogg")
            .collect();

        let error = Theme::from_source(&broken).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::BackUnreadable { back, .. } if back.as_str() == "robot"
        ));
    }

    #[test]
    fn a_missing_background_image_wins_over_a_missing_sound_further_down_the_pipeline() {
        // Faces and backs already succeed; the background (row 6) is
        // validated before sounds (row 7).
        let manifest = PNG_MANIFEST.replace(
            "background = { color = \"#008000\" }",
            "background = { image = \"table.png\" }",
        );
        let broken: MemSource = png_entries()
            .into_iter()
            .filter(|(path, _)| path != "sounds/deal.ogg")
            .map(|(path, bytes)| {
                if path == "theme.toml" {
                    (path, manifest.as_bytes().to_vec())
                } else {
                    (path, bytes)
                }
            })
            .collect();
        // Deliberately no "table.png" entry.

        let error = Theme::from_source(&broken).unwrap_err();
        assert!(matches!(error, ThemeError::BackgroundUnreadable { .. }));
    }

    #[test]
    fn a_missing_sound_fails_after_faces_backs_and_background_all_succeeded() {
        let broken: MemSource = png_entries()
            .into_iter()
            .filter(|(path, _)| path != "sounds/deal.ogg")
            .collect();

        let error = Theme::from_source(&broken).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::SoundUnreadable { name, .. } if name == "deal"
        ));
    }

    // -- DirSource --

    #[test]
    fn load_dir_loads_the_same_minimal_theme_from_a_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        write_png_theme_to(dir.path());

        let theme = Theme::load_dir(dir.path()).unwrap();
        assert_eq!(theme.manifest.name, "PNG Example");
        assert_eq!(theme.faces().count(), 52);
    }

    #[test]
    fn load_dir_reports_not_found_for_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("theme.toml"), PNG_MANIFEST).unwrap();
        // Deliberately do not write any face/back files.

        let error = Theme::load_dir(dir.path()).unwrap_err();
        assert!(matches!(error, ThemeError::FaceUnreadable { .. }));
    }

    #[test]
    fn load_dir_on_a_directory_with_no_theme_toml_is_manifest_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        // An empty directory: not even theme.toml is present.

        let error = Theme::load_dir(dir.path()).unwrap_err();
        assert!(matches!(error, ThemeError::ManifestUnreadable { .. }));
    }

    fn write_png_theme_to(root: &Path) {
        std::fs::write(root.join("theme.toml"), PNG_MANIFEST).unwrap();
        std::fs::create_dir(root.join("cards")).unwrap();
        for (suit, rank) in canonical_faces() {
            std::fs::write(
                root.join("cards").join(format!("{}.png", suit.stem(rank))),
                png_bytes(71, 96),
            )
            .unwrap();
        }
        std::fs::create_dir(root.join("backs")).unwrap();
        std::fs::write(root.join("backs").join("plain.png"), png_bytes(71, 96)).unwrap();
        std::fs::write(root.join("backs").join("robot.png"), png_bytes(71 * 4, 96)).unwrap();
        std::fs::create_dir(root.join("sounds")).unwrap();
        std::fs::write(root.join("sounds").join("deal.ogg"), b"deal-bytes").unwrap();
    }

    // -- ZipSource / load_zip_bytes --

    fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn png_zip_entries() -> Vec<(String, Vec<u8>)> {
        let mut entries = vec![("theme.toml".to_owned(), PNG_MANIFEST.as_bytes().to_vec())];
        for (suit, rank) in canonical_faces() {
            entries.push((format!("cards/{}.png", suit.stem(rank)), png_bytes(71, 96)));
        }
        entries.push(("backs/plain.png".to_owned(), png_bytes(71, 96)));
        entries.push(("backs/robot.png".to_owned(), png_bytes(71 * 4, 96)));
        entries.push(("sounds/deal.ogg".to_owned(), b"deal-bytes".to_vec()));
        entries
    }

    #[test]
    fn load_zip_bytes_loads_the_same_minimal_theme() {
        let entries = png_zip_entries();
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let bytes = build_zip(&refs);

        let theme = Theme::load_zip_bytes(&bytes).unwrap();
        assert_eq!(theme.manifest.name, "PNG Example");
        assert_eq!(theme.faces().count(), 52);
    }

    #[test]
    fn load_zip_bytes_on_corrupt_bytes_is_a_typed_error() {
        let error = Theme::load_zip_bytes(b"definitely not a zip").unwrap_err();
        assert!(matches!(error, ThemeError::MalformedZip { .. }));
    }

    #[test]
    fn load_zip_bytes_on_an_archive_with_no_theme_toml_is_manifest_unreadable() {
        // A valid, readable zip archive that simply never contains
        // theme.toml.
        let bytes = build_zip(&[("readme.txt", b"not a theme package")]);
        let error = Theme::load_zip_bytes(&bytes).unwrap_err();
        assert!(matches!(error, ThemeError::ManifestUnreadable { .. }));
    }

    // -- load_path --

    #[test]
    fn load_path_on_a_directory_works() {
        let dir = tempfile::tempdir().unwrap();
        write_png_theme_to(dir.path());

        let theme = Theme::load_path(dir.path()).unwrap();
        assert_eq!(theme.manifest.name, "PNG Example");
    }

    #[test]
    fn load_path_on_a_zip_file_works() {
        let entries = png_zip_entries();
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let bytes = build_zip(&refs);

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("theme.zip");
        std::fs::write(&zip_path, bytes).unwrap();

        // `load_path` is generic over `impl AsRef<Path>`; every call in this
        // module deliberately passes `&Path` (via `.as_path()`/`Path::new`,
        // not a bare `&PathBuf` or `&str`) so all of its branches are
        // exercised through the same monomorphized instantiation.
        let theme = Theme::load_path(zip_path.as_path()).unwrap();
        assert_eq!(theme.manifest.name, "PNG Example");
    }

    #[test]
    fn load_path_on_a_non_zip_file_names_both_accepted_forms() {
        let dir = tempfile::tempdir().unwrap();
        let bogus_path = dir.path().join("not-a-theme.dat");
        std::fs::write(&bogus_path, b"neither a directory nor a zip").unwrap();

        let error = Theme::load_path(bogus_path.as_path()).unwrap_err();
        assert!(matches!(error, ThemeError::UnrecognizedPackage { .. }));
        let message = error.to_string();
        assert!(message.contains("directory"), "{message}");
        assert!(message.contains("zip"), "{message}");
    }

    #[test]
    fn load_path_on_a_nonexistent_path_is_unrecognized_package() {
        let error = Theme::load_path(Path::new("/no/such/path/at/all.zip")).unwrap_err();
        assert!(matches!(error, ThemeError::UnrecognizedPackage { .. }));
    }
}
