//! `soltool extract <input> -o <theme-dir>`: turn a user's own
//! local Windows solitaire assets into a validated `render_mode = "png"`
//! theme package.
//!
//! `<input>` is sniffed by *content*, not extension: a directory is ingested
//! as loose bitmaps; a file is probed for the `MZ` signature and then its NE
//! (Win16) or PE (Win32) header, extracting card bitmaps from resources
//! ([`crate::ne`] / [`crate::pe`], decoded by [`crate::dib`]). Either way the
//! result is classified into 52 card faces and any number of backs, a
//! `theme.toml` is generated, and the whole package is written under
//! `<theme-dir>` — which must be empty or absent (never clobbered).
//!
//! # Face id mapping (the classic `CARDS.DLL` resource layout)
//!
//! Resource ids 1..=52 (and the loose numeric convention `1`..`52`) are card
//! faces in suit-major blocks of 13, suits ordered clubs, diamonds, hearts,
//! spades: `id = suit_index·13 + rank`. So id 1 = ace of clubs, id 13 =
//! king of clubs, id 14 = ace of diamonds, id 52 = king of spades. This is
//! the *storage* order of the `CARDS.DLL` bitmap resources — deliberately
//! not the `cdtDraw` API's card index, which interleaves suits rank-major
//! (aces of clubs/diamonds/hearts/spades, then the twos, …); the DLL
//! translates between the two internally, and extraction sees only the
//! storage order. The faces are re-emitted under this crate's canonical
//! `<suit>_<rank>` names (`spades_01`..`clubs_13`).
//!
//! Every *other* integer-id bitmap whose size equals the face size becomes a
//! static back `back_<id>` — except the three the original reuses for pile
//! slots rather than for cards, which become `[placeholders]` instead (see
//! below). Differently-sized bitmaps are skipped and listed. NE/PE
//! resources yield only *static* backs: the original stores animation
//! frames in code, not as strip resources.
//!
//! # Placeholders (resource inputs only)
//!
//! Three `CARDS.DLL` bitmaps are drawn where a pile is empty rather than
//! where a card is, and become `[placeholders]` entries:
//!
//! - **53** — the crosshatch ghost, which the original AND-blits over the
//!   table where a pile is empty.
//! - **68** — the ring, marking an empty stock the player can still recycle.
//! - **67** — the cross, marking an empty stock with no pass left.
//!
//! None of the three is a card back — the original's deck picker offers only
//! 54..=65 — so none reaches the deck. The originals encode transparency two
//! different ways — 53 relies on `SRCAND` (its pixels are strictly black and
//! white, so white lets the table show through), while 67/68 are copied
//! opaquely with the table green baked in — but both reduce to the same
//! alpha conversion: drop white and drop `#008000`. That also frees 67/68
//! from assuming a green table.
//!
//! Loose directories get no placeholders: their classification is by
//! filename, with no resource ids to recognize.
//!
//! # Corner cutout
//!
//! The original rounds a card's corners at draw time, not in its artwork:
//! `cdtDrawExt` saves twelve destination pixels — three at each corner —
//! before blitting a card and restores them afterwards, so the bitmap's own
//! corner pixels never reach the screen. Every classified image is therefore
//! run through [`raster::cut_card_corners`], which turns those twelve into
//! straight alpha. It applies to faces, backs (per strip frame), and the
//! pile slots alike, and to both input kinds — it describes how the original
//! *draws* a card bitmap, not where that bitmap was stored.
//!
//! Output is for the user's **local use only** — the original artwork must
//! never be redistributed or committed; the summary says so on every run.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Write as _};
use std::path::Path;

use sol_theme::{
    BackDef, BackLayout, BackTiming, Background, Color, FaceRank, FaceSuit, RelativeAssetPath,
    RenderMode, canonical_faces,
};

/// The 52 card faces in canonical order, each paired with its decoded image.
type Faces = Vec<(FaceSuit, FaceRank, RasterImage)>;

use crate::bytes::read_u32_le;
use crate::dib;
use crate::manifest_writer::{self, ThemeDoc};
use crate::ne::{self, NeError};
use crate::outdir::{self, OutDirError};
use crate::pack_strip;
use crate::pe::{self, PeError};
use crate::raster::{self, RasterImage};
use crate::resource::{ContainerBitmaps, ResourceBitmap};

/// Default animation rate for a detected loose-frame strip back.
const DEFAULT_FPS: u32 = 2;
/// The mandatory local-use notice, printed on every successful run.
const LOCAL_USE_NOTICE: &str = "output is for your local use only — the original artwork must never be redistributed or committed";
/// The built-in fallback back color (`#000080`) when a source yields no backs.
const FALLBACK_BACK_COLOR: [u8; 3] = [0x00, 0x00, 0x80];
/// The crosshatch ghost blitted where a pile is empty. Not a card back.
const EMPTY_PILE_ID: u32 = 53;
/// The empty-stock cross: no pass left. Not a card back.
const STOCK_BLOCKED_ID: u32 = 67;
/// The empty-stock ring: the waste can still be recycled. Not a card back.
const STOCK_RECYCLE_ID: u32 = 68;
/// The table green the original bakes into resources 67 and 68.
const ORIGINAL_TABLE_COLOR: [u8; 3] = [0x00, 0x80, 0x00];
/// White, which the original's `SRCAND` blit of resource 53 lets the table
/// show through.
const PLACEHOLDER_CLEAR_COLOR: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// Extracts the theme at `input` into the directory `output`, returning the
/// stdout summary (faces, backs, skips, and the local-use notice) on
/// success.
///
/// # Errors
///
/// Returns [`ExtractError::OutputNotEmpty`] if
/// `output` exists and is not an empty directory;
/// [`ExtractError::InputUnreadable`], [`ExtractError::NotExecutable`], or
/// [`ExtractError::UnknownExecutable`] for an unreadable or unrecognized
/// input; [`ExtractError::Ne`] / [`ExtractError::Pe`] for a malformed
/// resource container; a face-related variant if the 52 faces are missing,
/// inconsistent, or ambiguously named; or [`ExtractError::OutputUnwritable`]
/// if the theme cannot be written.
pub fn run(input: &Path, output: &Path) -> Result<String, ExtractError> {
    ensure_output_available(output)?;
    let name = input_stem(input);
    let mut theme = if input.is_dir() {
        classify_loose(input, name)?
    } else {
        classify_file(input, name)?
    };
    cut_corners(&mut theme);
    write_theme(output, theme)
}

/// Applies the original's corner cutout ([`raster::cut_card_corners`]) to
/// every image it draws as a card: the faces, each back — per frame, so a
/// loose-detected strip is cut frame by frame rather than once across the
/// whole strip — and the pile slots. Runs on both input kinds, because the
/// cutout is a property of how the original *draws* a card bitmap, not of
/// where that bitmap was stored.
fn cut_corners(theme: &mut ClassifiedTheme) {
    for (_, _, image) in &mut theme.faces {
        *image = raster::cut_card_corners(image, 1);
    }
    for back in &mut theme.backs {
        back.image = raster::cut_card_corners(&back.image, back.frames.unwrap_or(1));
    }
    for image in theme.placeholders.images_mut() {
        *image = raster::cut_card_corners(image, 1);
    }
}

/// Builds an [`ExtractError::InputUnreadable`] for `path`. A shared helper so
/// every read site uses a short, covered `map_err` closure.
fn input_unreadable(path: &Path, error: &dyn Display) -> ExtractError {
    ExtractError::InputUnreadable {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

/// Builds an [`ExtractError::LooseFileUndecodable`] for `path`.
fn loose_undecodable(path: &Path, error: &dyn Display) -> ExtractError {
    ExtractError::LooseFileUndecodable {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

/// Builds an [`ExtractError::OutputUnwritable`] for `path`.
fn output_unwritable(path: &Path, error: &dyn Display) -> ExtractError {
    ExtractError::OutputUnwritable {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

/// Reads and classifies a resource-container file (NE or PE), sniffed by
/// content.
fn classify_file(input: &Path, name: String) -> Result<ClassifiedTheme, ExtractError> {
    let bytes = std::fs::read(input).map_err(|error| input_unreadable(input, &error))?;
    let bitmaps = match sniff(&bytes)? {
        Container::Ne { header_offset } => ne::extract(&bytes, header_offset)?,
        Container::Pe => pe::extract(&bytes)?,
    };
    classify_resources(bitmaps, name)
}

// ---------------------------------------------------------------------------
// Content sniffing
// ---------------------------------------------------------------------------

/// A recognized executable resource container.
enum Container {
    /// A Win16 NE binary; its NE header begins at this file offset.
    Ne { header_offset: usize },
    /// A Win32 PE binary.
    Pe,
}

/// Classifies `bytes` as an NE or PE container by signature.
///
/// # Errors
///
/// Returns [`ExtractError::NotExecutable`] if `bytes` does not start with `MZ`,
/// or [`ExtractError::UnknownExecutable`] if it is `MZ` but its `e_lfanew`
/// header is neither `NE` nor `PE`.
fn sniff(bytes: &[u8]) -> Result<Container, ExtractError> {
    let magic = bytes.get(0..2).unwrap_or(&[]);
    if magic != b"MZ" {
        return Err(ExtractError::NotExecutable {
            found: describe_bytes(magic),
        });
    }
    let header_offset = read_u32_le(bytes, 0x3C).ok_or_else(|| ExtractError::UnknownExecutable {
        detail: "file ends before the e_lfanew header pointer at 0x3C".to_owned(),
    })? as usize;
    let signature = bytes
        .get(header_offset..header_offset.saturating_add(2))
        .ok_or_else(|| ExtractError::UnknownExecutable {
            detail: format!(
                "the e_lfanew header offset {header_offset} is past the end of the file"
            ),
        })?;
    match signature {
        b"NE" => Ok(Container::Ne { header_offset }),
        b"PE" => Ok(Container::Pe),
        other => Err(ExtractError::UnknownExecutable {
            detail: format!(
                "MZ file has an unrecognized executable signature {} (expected NE or PE)",
                describe_bytes(other)
            ),
        }),
    }
}

/// A short human-readable rendering of up to two signature bytes, or a
/// dedicated message when there are none at all — rather than interpolating
/// zero bytes into the same hex/ASCII template and rendering a dangling
/// `0x ("")`.
fn describe_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "an empty file".to_owned();
    }
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    let ascii: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect();
    format!("0x{} (\"{ascii}\")", hex.join(""))
}

// ---------------------------------------------------------------------------
// Classification: resources (NE / PE)
// ---------------------------------------------------------------------------

/// Classifies decoded container bitmaps into 52 faces plus backs, diverting
/// the three pile-slot bitmaps into placeholders.
fn classify_resources(
    bitmaps: ContainerBitmaps,
    name: String,
) -> Result<ClassifiedTheme, ExtractError> {
    let mut face_candidates = Vec::new();
    let mut back_candidates: Vec<(u32, RasterImage)> = Vec::new();
    let mut skips = Vec::new();
    for ResourceBitmap { id, data } in bitmaps.bitmaps {
        match dib::decode_dib(&data) {
            Ok(image) => match resource_face(id) {
                Some((suit, rank)) => face_candidates.push((suit, rank, image)),
                None => back_candidates.push((id, image)),
            },
            Err(error) => skips.push(format!(
                "resource id {id}: not a decodable bitmap ({error})"
            )),
        }
    }

    let (base_size, faces) = assemble_faces(face_candidates).map_err(|error| match error {
        FacesError::Incomplete { found } => ExtractError::ResourceFacesIncomplete { found },
        FacesError::SizeMismatch(mismatch) => ExtractError::FaceSizeMismatch(mismatch),
    })?;

    back_candidates.sort_by_key(|(id, _)| *id);
    let mut backs = Vec::new();
    let mut placeholders = ClassifiedPlaceholders::default();
    let mut notes = Vec::new();
    for (id, image) in back_candidates {
        if (image.width, image.height) != base_size {
            skips.push(format!(
                "resource id {id}: {}x{} does not match the {}x{} card size",
                image.width, image.height, base_size.0, base_size.1
            ));
            continue;
        }

        // The empty-stock indicators are not selectable card backs: divert
        // them so they never reach the deck.
        if id == STOCK_BLOCKED_ID {
            placeholders.stock_blocked = Some(as_placeholder(&image));
            continue;
        }
        if id == STOCK_RECYCLE_ID {
            placeholders.stock_recycle = Some(as_placeholder(&image));
            continue;
        }
        // The crosshatch ghost marks an empty pile, and the original's deck
        // picker offers only 54..=65: not a selectable card back either.
        if id == EMPTY_PILE_ID {
            placeholders.empty_pile = Some(as_placeholder(&image));
            continue;
        }

        backs.push(ClassifiedBack {
            name: format!("back_{id:02}"),
            image,
            frames: None,
        });
    }

    if bitmaps.string_named_skipped > 0 {
        notes.push(format!(
            "{} string-named bitmap resource(s) skipped (only integer-id bitmaps map to cards)",
            bitmaps.string_named_skipped
        ));
    }

    Ok(ClassifiedTheme {
        name,
        base_size,
        faces,
        backs,
        placeholders,
        skips,
        notes,
    })
}

/// Converts one of the original's pile-slot bitmaps into a straight-alpha
/// placeholder by dropping the two colors that stand for "let the table
/// show through": white (what resource 53's `SRCAND` blit clears, since its
/// pixels are strictly black and white) and the table green baked into
/// resources 67 and 68. One rule covers both because the two encodings never
/// disagree — and dropping the baked green is what lets 67/68 sit on a table
/// of any color instead of only the original's.
fn as_placeholder(image: &RasterImage) -> RasterImage {
    raster::key_transparent(image, &[PLACEHOLDER_CLEAR_COLOR, ORIGINAL_TABLE_COLOR])
}

/// The canonical `(suit, rank)` for resource id `id`, or `None` if it is not a
/// face id (1..=52). See the module doc for the mapping and its source.
fn resource_face(id: u32) -> Option<(FaceSuit, FaceRank)> {
    if !(1..=52).contains(&id) {
        return None;
    }
    let index = id - 1;
    let suit = *[
        FaceSuit::Clubs,
        FaceSuit::Diamonds,
        FaceSuit::Hearts,
        FaceSuit::Spades,
    ]
    .get((index / 13) as usize)?;
    let rank = FaceRank::try_from(u8::try_from(index % 13 + 1).ok()?).ok()?;
    Some((suit, rank))
}

// ---------------------------------------------------------------------------
// Classification: loose bitmap directory
// ---------------------------------------------------------------------------

/// Classifies a directory of loose `*.bmp` / `*.png` bitmaps into 52 faces
/// plus backs (static, or horizontal strips packed from frame-numbered files).
fn classify_loose(dir: &Path, name: String) -> Result<ClassifiedTheme, ExtractError> {
    let mut files = read_loose_files(dir)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let canonical_lookup = canonical_stem_lookup();
    let mut canonical_faces_found = Vec::new();
    let mut numeric_faces_found = Vec::new();
    let mut non_faces: Vec<(String, RasterImage)> = Vec::new();
    for (stem, image) in files {
        if let Some(&(suit, rank)) = canonical_lookup.get(&stem) {
            canonical_faces_found.push((suit, rank, image));
        } else if let Some((suit, rank)) = numeric_face(&stem) {
            numeric_faces_found.push((suit, rank, image));
        } else {
            non_faces.push((stem, image));
        }
    }
    if !canonical_faces_found.is_empty() && !numeric_faces_found.is_empty() {
        return Err(ExtractError::LooseMixedConventions);
    }
    let face_candidates = if canonical_faces_found.is_empty() {
        numeric_faces_found
    } else {
        canonical_faces_found
    };

    let (base_size, faces) = assemble_faces(face_candidates).map_err(|error| match error {
        FacesError::Incomplete { found } => ExtractError::LooseFacesIncomplete { found },
        FacesError::SizeMismatch(mismatch) => ExtractError::FaceSizeMismatch(mismatch),
    })?;

    let (backs, skips) = classify_loose_backs(non_faces, base_size);
    Ok(ClassifiedTheme {
        name,
        base_size,
        faces,
        backs,
        // Loose classification is by filename: there are no resource ids to
        // recognize, so nothing identifies a pile-slot image.
        placeholders: ClassifiedPlaceholders::default(),
        skips,
        notes: Vec::new(),
    })
}

/// Reads every `*.bmp` / `*.png` file directly in `dir` (non-recursive,
/// case-insensitive), decoding each to a [`RasterImage`].
fn read_loose_files(dir: &Path) -> Result<Vec<(String, RasterImage)>, ExtractError> {
    let entries = std::fs::read_dir(dir).map_err(|error| input_unreadable(dir, &error))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| input_unreadable(dir, &error))?.path();
        if !path.is_file() {
            continue;
        }
        let is_bmp = has_extension(&path, "bmp");
        let is_png = has_extension(&path, "png");
        if !is_bmp && !is_png {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(&path).map_err(|error| loose_undecodable(&path, &error))?;
        let image = if is_bmp {
            dib::decode_bmp(&bytes).map_err(|error| loose_undecodable(&path, &error))?
        } else {
            raster::decode(&bytes).map_err(|error| loose_undecodable(&path, &error))?
        };
        files.push((stem, image));
    }
    Ok(files)
}

/// `true` if `path`'s extension equals `ext`, case-insensitively.
fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|found| found.eq_ignore_ascii_case(ext))
}

/// A map from every canonical face stem (`spades_01`..`clubs_13`) to its
/// `(suit, rank)`.
fn canonical_stem_lookup() -> HashMap<String, (FaceSuit, FaceRank)> {
    canonical_faces()
        .map(|(suit, rank)| (suit.stem(rank), (suit, rank)))
        .collect()
}

/// The canonical `(suit, rank)` for a plain-numeric loose face stem
/// (`"1"`..`"52"`), or `None` — resource-hacker-style names, same mapping as
/// resource ids.
fn numeric_face(stem: &str) -> Option<(FaceSuit, FaceRank)> {
    resource_face(stem.parse::<u32>().ok()?)
}

/// Splits base-sized non-face bitmaps into backs and off-size skips: contiguous
/// frame-numbered groups (`<stem>_0`, `<stem>_1`, …) become one horizontal
/// strip back; everything else base-sized becomes a static back.
fn classify_loose_backs(
    non_faces: Vec<(String, RasterImage)>,
    base_size: (u32, u32),
) -> (Vec<ClassifiedBack>, Vec<String>) {
    let mut skips = Vec::new();
    let mut frame_groups: BTreeMap<String, Vec<(u32, RasterImage)>> = BTreeMap::new();
    let mut standalones: Vec<(String, RasterImage)> = Vec::new();
    for (stem, image) in non_faces {
        if (image.width, image.height) != base_size {
            skips.push(format!(
                "{stem}: {}x{} does not match the {}x{} card size",
                image.width, image.height, base_size.0, base_size.1
            ));
            continue;
        }
        match parse_frame_stem(&stem) {
            Some((base, frame)) => frame_groups.entry(base).or_default().push((frame, image)),
            None => standalones.push((stem, image)),
        }
    }

    let mut backs = Vec::new();
    for (base, mut frames) in frame_groups {
        frames.sort_by_key(|(frame, _)| *frame);
        if is_contiguous_from_zero(&frames) {
            let frame_count = u32::try_from(frames.len()).unwrap_or(u32::MAX);
            let images: Vec<RasterImage> = frames.into_iter().map(|(_, image)| image).collect();
            backs.push(ClassifiedBack {
                name: pack_strip::sanitize_back_name(&base),
                image: pack_strip::build_strip(&images),
                frames: Some(frame_count),
            });
        } else {
            // Not a clean 0..n run: keep each frame as its own static back.
            for (frame, image) in frames {
                standalones.push((format!("{base}_{frame}"), image));
            }
        }
    }
    for (stem, image) in standalones {
        backs.push(ClassifiedBack {
            name: pack_strip::sanitize_back_name(&stem),
            image,
            frames: None,
        });
    }

    backs.sort_by(|left, right| left.name.cmp(&right.name));
    (backs, skips)
}

/// Parses a frame-numbered stem `<base>_<n>` into `(base, n)`, or `None` if it
/// has no trailing `_<digits>` suffix.
fn parse_frame_stem(stem: &str) -> Option<(String, u32)> {
    let (base, number) = stem.rsplit_once('_')?;
    if base.is_empty() || number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((base.to_owned(), number.parse::<u32>().ok()?))
}

/// `true` if `frames` (already sorted by frame number) is `0, 1, …, n` with at
/// least two entries.
fn is_contiguous_from_zero(frames: &[(u32, RasterImage)]) -> bool {
    frames.len() >= 2
        && frames
            .iter()
            .enumerate()
            .all(|(index, (frame, _))| usize::try_from(*frame) == Ok(index))
}

// ---------------------------------------------------------------------------
// Face assembly (shared by both paths)
// ---------------------------------------------------------------------------

/// Verifies `collected` covers all 52 canonical faces at one consistent size,
/// returning `(base_size, faces_in_canonical_order)`.
fn assemble_faces(collected: Faces) -> Result<((u32, u32), Faces), FacesError> {
    let mut map: HashMap<(FaceSuit, FaceRank), RasterImage> = collected
        .into_iter()
        .map(|(suit, rank, image)| ((suit, rank), image))
        .collect();

    let mut ordered = Vec::with_capacity(52);
    for (suit, rank) in canonical_faces() {
        if let Some(image) = map.remove(&(suit, rank)) {
            ordered.push((suit, rank, image));
        }
    }
    if ordered.len() < 52 {
        return Err(FacesError::Incomplete {
            found: ordered.len(),
        });
    }

    let base_size = ordered
        .first()
        .map_or((0, 0), |(_, _, image)| (image.width, image.height));
    for (suit, rank, image) in &ordered {
        if (image.width, image.height) != base_size {
            return Err(FacesError::SizeMismatch(FaceSizeMismatch {
                face: suit.stem(*rank),
                expected_width: base_size.0,
                expected_height: base_size.1,
                found_width: image.width,
                found_height: image.height,
            }));
        }
    }
    Ok((base_size, ordered))
}

/// Why [`assemble_faces`] rejected a face set — mapped to the path-specific
/// [`ExtractError`] variant by the caller.
enum FacesError {
    Incomplete { found: usize },
    SizeMismatch(FaceSizeMismatch),
}

// ---------------------------------------------------------------------------
// Theme writing
// ---------------------------------------------------------------------------

/// A classified back: its (valid) name, its single image (a strip is already
/// assembled), and `Some(frame_count)` for an animated strip or `None` for a
/// static back.
struct ClassifiedBack {
    name: String,
    image: RasterImage,
    frames: Option<u32>,
}

/// A fully classified theme, ready to write.
struct ClassifiedTheme {
    name: String,
    base_size: (u32, u32),
    faces: Faces,
    backs: Vec<ClassifiedBack>,
    placeholders: ClassifiedPlaceholders,
    skips: Vec<String>,
    notes: Vec<String>,
}

/// The pile-slot images recovered from a resource input; each is `None` when
/// the source did not carry its resource. Loose directories leave all three
/// `None`.
#[derive(Default)]
struct ClassifiedPlaceholders {
    empty_pile: Option<RasterImage>,
    stock_recycle: Option<RasterImage>,
    stock_blocked: Option<RasterImage>,
}

impl ClassifiedPlaceholders {
    /// Each recovered placeholder as `(theme.toml key, image)`, in a fixed
    /// order so the written package is deterministic.
    fn entries(&self) -> impl Iterator<Item = (&'static str, &RasterImage)> {
        [
            ("empty_pile", self.empty_pile.as_ref()),
            ("stock_recycle", self.stock_recycle.as_ref()),
            ("stock_blocked", self.stock_blocked.as_ref()),
        ]
        .into_iter()
        .filter_map(|(key, image)| image.map(|image| (key, image)))
    }

    /// Each recovered placeholder, mutably — the counterpart to [`entries`]
    /// for the passes that rewrite pixels rather than read them.
    ///
    /// [`entries`]: ClassifiedPlaceholders::entries
    fn images_mut(&mut self) -> impl Iterator<Item = &mut RasterImage> {
        [
            self.empty_pile.as_mut(),
            self.stock_recycle.as_mut(),
            self.stock_blocked.as_mut(),
        ]
        .into_iter()
        .flatten()
    }

    /// Whether nothing was recovered, in which case the package gets no
    /// `placeholders/` directory and no `[placeholders]` section.
    fn is_empty(&self) -> bool {
        self.entries().next().is_none()
    }
}

/// Writes `theme` under `output` (adding a fallback back if none were found)
/// and returns the stdout summary.
fn write_theme(output: &Path, mut theme: ClassifiedTheme) -> Result<String, ExtractError> {
    if theme.backs.is_empty() {
        theme.backs.push(ClassifiedBack {
            name: "back_solid".to_owned(),
            // Cut like every other card image: this stands in for a card
            // back, so it must round off against the table the same way.
            image: raster::cut_card_corners(&solid_image(theme.base_size, FALLBACK_BACK_COLOR), 1),
            frames: None,
        });
        theme
            .notes
            .push("no backs detected; wrote a solid #000080 fallback back".to_owned());
    }
    uniquify_back_names(&mut theme.backs);

    write_dir(output.join("cards").as_path())?;
    write_dir(output.join("backs").as_path())?;
    for (suit, rank, image) in &theme.faces {
        let path = output
            .join("cards")
            .join(format!("{}.png", suit.stem(*rank)));
        write_png(&path, image)?;
    }
    for back in &theme.backs {
        let path = output.join("backs").join(format!("{}.png", back.name));
        write_png(&path, &back.image)?;
    }
    if !theme.placeholders.is_empty() {
        write_dir(output.join("placeholders").as_path())?;
        for (key, image) in theme.placeholders.entries() {
            let path = output.join("placeholders").join(format!("{key}.png"));
            write_png(&path, image)?;
        }
    }
    let toml_path = output.join("theme.toml");
    let toml = manifest_writer::render(&build_doc(&theme)?);
    std::fs::write(&toml_path, toml).map_err(|error| output_unwritable(&toml_path, &error))?;

    Ok(build_summary(&theme))
}

/// Builds the shared [`ThemeDoc`] for a classified extract: a fixed
/// `render_mode = "png"` theme, a `#008000` table and
/// `#000000` drag outline (the classic Win98 baize), no author or sounds
/// (extraction recovers neither). Each classified back becomes a static
/// entry, or a horizontal strip when it carries a frame count, timed at
/// [`DEFAULT_FPS`].
///
/// # Errors
///
/// Returns [`ExtractError::OutputPath`] if a back's generated image path is
/// not theme-package-relative. Back names are validated to `[a-z0-9_-]+`
/// before they get here, so this reports a broken name rule rather than a
/// property of the input file.
fn build_doc(theme: &ClassifiedTheme) -> Result<ThemeDoc, ExtractError> {
    let mut backs = Vec::with_capacity(theme.backs.len());
    for back in &theme.backs {
        let image = RelativeAssetPath::parse(
            format!("back `{}` image", back.name),
            &format!("backs/{}.png", back.name),
        )
        .map_err(ExtractError::OutputPath)?;
        let def = match back.frames {
            Some(frames) => BackDef::Strip {
                image,
                frames,
                timing: BackTiming::Fps(DEFAULT_FPS),
                layout: BackLayout::Horizontal,
            },
            None => BackDef::Static { image },
        };
        backs.push((back.name.clone(), def));
    }
    Ok(ThemeDoc {
        name: theme.name.clone(),
        author: None,
        render_mode: RenderMode::Png,
        base_size: theme.base_size,
        backs,
        background: Background::Color(Color::new(0x00, 0x80, 0x00)),
        placeholders: theme
            .placeholders
            .entries()
            .map(|(key, _)| (key.to_owned(), format!("placeholders/{key}.png")))
            .collect(),
        outline_color: Color::new(0x00, 0x00, 0x00),
        sounds: Vec::new(),
    })
}

/// Ensures back names are unique, appending `_2`, `_3`, … on collision so the
/// generated `theme.toml` never has a duplicate key nor clobbers a file.
fn uniquify_back_names(backs: &mut [ClassifiedBack]) {
    let mut used: HashSet<String> = HashSet::new();
    for back in backs {
        if used.contains(&back.name) {
            let mut suffix = 2_u32;
            loop {
                let candidate = format!("{}_{suffix}", back.name);
                if !used.contains(&candidate) {
                    back.name = candidate;
                    break;
                }
                suffix += 1;
            }
        }
        used.insert(back.name.clone());
    }
}

/// A `width` × `height` opaque solid-color image.
fn solid_image(size: (u32, u32), color: [u8; 3]) -> RasterImage {
    let (width, height) = size;
    let [red, green, blue] = color;
    let pixel_count = (width as usize).saturating_mul(height as usize);
    let mut pixels = Vec::with_capacity(pixel_count.saturating_mul(4));
    for _ in 0..pixel_count {
        pixels.extend_from_slice(&[red, green, blue, 0xFF]);
    }
    RasterImage {
        width,
        height,
        pixels,
    }
}

/// Creates `dir` (and parents), mapping failure to a typed error.
fn write_dir(dir: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(dir).map_err(|error| output_unwritable(dir, &error))
}

/// Encodes `image` as PNG and writes it to `path`.
fn write_png(path: &Path, image: &RasterImage) -> Result<(), ExtractError> {
    let bytes = raster::encode(image).map_err(|error| output_unwritable(path, &error))?;
    std::fs::write(path, bytes).map_err(|error| output_unwritable(path, &error))
}

/// Builds the stdout summary: face/back counts, backs, recovered
/// placeholders, any skips and notes, then the mandatory local-use notice.
fn build_summary(theme: &ClassifiedTheme) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Extracted {name}: {faces} card faces at {width}x{height}, {backs} back(s).",
        name = manifest_writer::toml_string(&theme.name),
        faces = theme.faces.len(),
        width = theme.base_size.0,
        height = theme.base_size.1,
        backs = theme.backs.len(),
    );
    let names: Vec<&str> = theme.backs.iter().map(|back| back.name.as_str()).collect();
    let _ = writeln!(out, "Backs: {}", names.join(", "));
    let placeholders: Vec<&str> = theme.placeholders.entries().map(|(key, _)| key).collect();
    if !placeholders.is_empty() {
        let _ = writeln!(out, "Placeholders: {}", placeholders.join(", "));
    }
    for note in &theme.notes {
        let _ = writeln!(out, "Note: {note}");
    }
    if theme.skips.is_empty() {
        out.push_str("Skipped: nothing.\n");
    } else {
        out.push_str("Skipped:\n");
        for skip in &theme.skips {
            let _ = writeln!(out, "  - {skip}");
        }
    }
    out.push('\n');
    out.push_str(LOCAL_USE_NOTICE);
    out
}

// ---------------------------------------------------------------------------
// Output directory guard
// ---------------------------------------------------------------------------

/// Confirms `output` is safe to write into via the shared [`outdir`] guard:
/// absent, or an existing empty directory.
///
/// # Errors
///
/// Returns [`ExtractError::OutputNotEmpty`] if `output` exists and is a
/// non-empty directory or any non-directory.
fn ensure_output_available(output: &Path) -> Result<(), ExtractError> {
    outdir::ensure_available(output).map_err(|error| match error {
        OutDirError::NotEmpty { path } => ExtractError::OutputNotEmpty { path },
    })
}

/// The display name for the theme: `input`'s file stem, or `"theme"`.
fn input_stem(input: &Path) -> String {
    input
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map_or_else(|| "theme".to_owned(), str::to_owned)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The offending face and the mismatched sizes for
/// [`ExtractError::FaceSizeMismatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceSizeMismatch {
    /// The canonical name of the first face whose size differed.
    pub face: String,
    /// The width the other faces share.
    pub expected_width: u32,
    /// The height the other faces share.
    pub expected_height: u32,
    /// The offending face's width.
    pub found_width: u32,
    /// The offending face's height.
    pub found_height: u32,
}

/// Every way [`run`] can fail to produce a theme.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExtractError {
    /// The input file (or a loose directory) could not be read.
    #[error("cannot read input {path}: {message}")]
    InputUnreadable {
        /// The input path.
        path: String,
        /// The underlying I/O failure, rendered to text.
        message: String,
    },
    /// The input file does not start with the `MZ` executable signature.
    #[error("{found} is not an MZ executable or a directory of loose bitmaps")]
    NotExecutable {
        /// A rendering of the leading bytes found.
        found: String,
    },
    /// The input is `MZ` but its header is neither `NE` nor `PE`.
    #[error("unrecognized executable: {detail}")]
    UnknownExecutable {
        /// What was found instead of an NE or PE header.
        detail: String,
    },
    /// The NE resource table could not be read.
    #[error("NE resource error: {0}")]
    Ne(#[from] NeError),
    /// The PE resources could not be read.
    #[error("PE resource error: {0}")]
    Pe(#[from] PeError),
    /// A loose `*.bmp` / `*.png` file could not be read or decoded.
    #[error("cannot decode loose bitmap {path}: {message}")]
    LooseFileUndecodable {
        /// The offending file path.
        path: String,
        /// The underlying read or decode failure, rendered to text.
        message: String,
    },
    /// A loose directory mixes canonical (`<suit>_NN`) and numeric (`1`..`52`)
    /// face naming, which is ambiguous.
    #[error(
        "loose directory mixes canonical (<suit>_NN, e.g. spades_01) and numeric (1..52) face names; use exactly one convention"
    )]
    LooseMixedConventions,
    /// A resource container did not contain all 52 card faces.
    #[error(
        "found {found} of 52 card faces in the resource container; the full set of integer-id bitmaps 1..=52 is required — CARDS.DLL is the expected face source"
    )]
    ResourceFacesIncomplete {
        /// How many of the 52 faces were present.
        found: usize,
    },
    /// A loose directory did not contain all 52 card faces.
    #[error(
        "found {found} of 52 card faces in the loose directory; all 52 must be present at one consistent size, named either canonically (<suit>_NN, e.g. spades_01) or numerically (1..52)"
    )]
    LooseFacesIncomplete {
        /// How many of the 52 faces were present.
        found: usize,
    },
    /// The 52 faces were present but not all the same size.
    #[error(
        "card face `{}` is {}x{} but the other faces are {}x{}; all faces must share one size",
        .0.face, .0.found_width, .0.found_height, .0.expected_width, .0.expected_height
    )]
    FaceSizeMismatch(FaceSizeMismatch),
    /// The output directory exists and is not empty.
    #[error("output directory {path} already exists and is not empty; refusing to overwrite it")]
    OutputNotEmpty {
        /// The rejected output path.
        path: String,
    },
    /// A generated asset path is not theme-package-relative.
    #[error(transparent)]
    OutputPath(sol_theme::ManifestError),
    /// The theme could not be written to the output directory.
    #[error("cannot write output {path}: {message}")]
    OutputUnwritable {
        /// The path that could not be written.
        path: String,
        /// The underlying failure, rendered to text.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::path::PathBuf;

    use super::*;
    use crate::testkit::asset_path;
    use crate::testkit::{Rsrc, build_ne, build_pe, solid_dib};

    /// Synthetic card size — a made-up 5×7, never a real dimension.
    const W: u32 = 5;
    const H: u32 = 7;
    /// NE integer-resource-type id for `RT_BITMAP`.
    const RT_BITMAP: u16 = 0x8002;

    /// An NE integer resource id (bit 15 set).
    fn int_id(id: u16) -> u16 {
        id | 0x8000
    }

    /// The 52 face resources (ids 1..=52) as `W`×`H` solid-color DIBs.
    fn face_entries() -> Vec<(u16, Vec<u8>)> {
        (1..=52_u16)
            .map(|id| {
                let shade = u8::try_from(id).unwrap();
                (int_id(id), solid_dib(W, H, (shade, 1, 2)))
            })
            .collect()
    }

    /// An NE image with the 52 faces plus `extra` bitmap resources.
    fn ne_image(extra: Vec<(u16, Vec<u8>)>) -> Vec<u8> {
        let mut entries = face_entries();
        entries.extend(extra);
        build_ne(0, &[(RT_BITMAP, entries)]).0
    }

    /// Runs `extract` on `bytes` written as a file input, returning the
    /// temp dir (kept alive), the output path, and the result.
    fn run_file(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, Result<String, ExtractError>) {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.bin");
        std::fs::write(&input, bytes).unwrap();
        let output = dir.path().join("out");
        let result = run(&input, &output);
        (dir, output, result)
    }

    /// The RGBA pixel at `(x, y)` in `image`.
    fn pixel_at(image: &RasterImage, x: u32, y: u32) -> [u8; 4] {
        let row_bytes = (image.width as usize) * 4;
        let start = (y as usize) * row_bytes + (x as usize) * 4;
        [
            image.pixels[start],
            image.pixels[start + 1],
            image.pixels[start + 2],
            image.pixels[start + 3],
        ]
    }

    /// A solid-color PNG.
    fn png_bytes(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        raster::encode(&solid_image((width, height), color)).unwrap()
    }

    /// A solid-color loose `.bmp` file (`BM` header + a DIB; `bfOffBits`
    /// deliberately bogus, to prove the decoder ignores it).
    fn bmp_bytes(width: u32, height: u32, color: (u8, u8, u8)) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&9999_u32.to_le_bytes());
        bytes.extend_from_slice(&solid_dib(width, height, color));
        bytes
    }

    /// Writes the 52 canonically-named face PNGs into `dir`.
    fn write_canonical_faces(dir: &Path) {
        for (suit, rank) in canonical_faces() {
            let name = format!("{}.png", suit.stem(rank));
            std::fs::write(dir.join(name), png_bytes(W, H, [9, 9, 9])).unwrap();
        }
    }

    // -- NE end to end (the defining integration test) --

    #[test]
    fn ne_extract_writes_a_theme_that_validates_green() {
        let extra = vec![
            (int_id(54), solid_dib(W, H, (0, 255, 0))), // card-sized -> back_54
            (int_id(55), solid_dib(W, H, (0, 0, 255))), // card-sized -> back_55
            (int_id(60), solid_dib(9, 9, (1, 2, 3))),   // off-size -> skipped
        ];
        let (_dir, output, result) = run_file(&ne_image(extra));
        let summary = result.unwrap();

        // The generated theme loads green through the real validator.
        crate::validate::run(&output).unwrap();

        assert!(summary.contains("52 card faces at 5x7"), "{summary}");
        assert!(summary.contains("2 back"), "{summary}");
        assert!(summary.contains("back_54"), "{summary}");
        assert!(summary.contains("back_55"), "{summary}");
        assert!(summary.contains("9x9 does not match"), "{summary}");
        assert!(summary.contains(LOCAL_USE_NOTICE), "{summary}");
        // Every resource here is integer-id: zero string-named skips, so the
        // note must not appear (the count-check is `> 0`, not `>= 0`).
        assert!(!summary.contains("string-named"), "{summary}");
        // Files exist where the manifest points.
        assert!(output.join("theme.toml").is_file());
        assert!(output.join("cards").join("spades_01.png").is_file());
        assert!(output.join("backs").join("back_54.png").is_file());
    }

    // -- placeholders (ids 53 / 67 / 68) --

    /// The white and table-green pixels the placeholder conversion clears,
    /// and an ink color it must leave alone.
    const WHITE: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
    const TABLE: (u8, u8, u8) = (0x00, 0x80, 0x00);
    const INK: (u8, u8, u8) = (0xFF, 0x00, 0x00);

    /// A container carrying all three pile-slot resources plus one ordinary
    /// back, so the tests can tell diversion from deletion.
    fn placeholder_entries() -> Vec<(u16, Vec<u8>)> {
        vec![
            (int_id(53), solid_dib(W, H, WHITE)),
            (int_id(54), solid_dib(W, H, (0, 0, 255))),
            (int_id(67), solid_dib(W, H, INK)),
            (int_id(68), solid_dib(W, H, TABLE)),
        ]
    }

    /// The reported bug: 67 and 68 are the empty-stock indicators, not card
    /// backs, and must never reach the deck the player picks from.
    #[test]
    fn stock_indicator_resources_are_not_offered_as_card_backs() {
        let (_dir, output, result) = run_file(&ne_image(placeholder_entries()));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        assert!(!summary.contains("back_67"), "{summary}");
        assert!(!summary.contains("back_68"), "{summary}");
        assert!(!output.join("backs").join("back_67.png").exists());
        assert!(!output.join("backs").join("back_68.png").exists());
        // Diverted, not dropped along with the ordinary back.
        assert!(summary.contains("1 back"), "{summary}");
        assert!(summary.contains("back_54"), "{summary}");
    }

    /// The reported bug: 53 is the empty-pile placeholder, not a card back —
    /// the original's deck picker offers only 54..=65 — so it must be
    /// diverted like the stock indicators, never offered as a deck.
    #[test]
    fn the_ghost_resource_is_not_offered_as_a_card_back() {
        let (_dir, output, result) = run_file(&ne_image(placeholder_entries()));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        assert!(!summary.contains("back_53"), "{summary}");
        assert!(!output.join("backs").join("back_53.png").exists());
        // Diverted, not dropped: the ghost still lands in `[placeholders]`.
        assert!(output.join("placeholders").join("empty_pile.png").is_file());
    }

    #[test]
    fn every_pile_slot_resource_becomes_a_placeholder() {
        let (_dir, output, result) = run_file(&ne_image(placeholder_entries()));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        for key in ["empty_pile", "stock_recycle", "stock_blocked"] {
            assert!(summary.contains(key), "{summary}");
            assert!(
                output
                    .join("placeholders")
                    .join(format!("{key}.png"))
                    .is_file(),
                "{key}"
            );
        }
        let toml = std::fs::read_to_string(output.join("theme.toml")).unwrap();
        let manifest = sol_theme::Manifest::from_toml_str(&toml).unwrap();
        assert_eq!(
            manifest.placeholders.empty_pile,
            Some(asset_path("placeholders/empty_pile.png"))
        );
        assert_eq!(
            manifest.placeholders.stock_recycle,
            Some(asset_path("placeholders/stock_recycle.png"))
        );
        assert_eq!(
            manifest.placeholders.stock_blocked,
            Some(asset_path("placeholders/stock_blocked.png"))
        );
    }

    /// The two colors that mean "let the table show through" become
    /// transparent; ink is untouched. This is what makes the ghost composite
    /// to the original's `SRCAND` result and frees 67/68 from the baked-in
    /// green.
    #[test]
    fn a_placeholder_clears_white_and_table_green_but_keeps_its_ink() {
        let (_dir, output, result) = run_file(&ne_image(placeholder_entries()));
        result.unwrap();

        let read = |name: &str| {
            let bytes = std::fs::read(output.join("placeholders").join(name)).unwrap();
            raster::decode(&bytes).unwrap()
        };
        // Sampled away from the corners, which the cutout clears in every
        // image regardless of color.
        // 53 is solid white -> fully cleared.
        assert_eq!(pixel_at(&read("empty_pile.png"), 2, 3), [0, 0, 0, 0]);
        // 68 is solid table green -> fully cleared.
        assert_eq!(pixel_at(&read("stock_recycle.png"), 2, 3), [0, 0, 0, 0]);
        // 67 is solid red ink -> kept opaque, unchanged.
        assert_eq!(
            pixel_at(&read("stock_blocked.png"), 2, 3),
            [0xFF, 0x00, 0x00, 0xFF]
        );
    }

    /// A source carrying only some of the three yields only those; the
    /// section must not invent entries the source never had.
    #[test]
    fn a_source_without_the_stock_resources_gets_only_the_ghost() {
        let extra = vec![(int_id(53), solid_dib(W, H, WHITE))];
        let (_dir, output, result) = run_file(&ne_image(extra));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        assert!(summary.contains("empty_pile"), "{summary}");
        assert!(!summary.contains("stock_recycle"), "{summary}");
        assert!(!summary.contains("stock_blocked"), "{summary}");
        assert!(
            !output
                .join("placeholders")
                .join("stock_recycle.png")
                .exists()
        );
    }

    /// No pile-slot resources at all: no directory, no section, and a theme
    /// that still validates.
    #[test]
    fn a_source_with_no_pile_slot_resources_writes_no_placeholders() {
        let extra = vec![(int_id(54), solid_dib(W, H, (0, 0, 255)))];
        let (_dir, output, result) = run_file(&ne_image(extra));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        assert!(!summary.contains("Placeholders:"), "{summary}");
        assert!(!output.join("placeholders").exists());
        let toml = std::fs::read_to_string(output.join("theme.toml")).unwrap();
        assert!(!toml.contains("[placeholders]"), "{toml}");
    }

    /// Off-size 67/68 are not usable as placeholders, and must still be
    /// reported rather than vanishing silently.
    #[test]
    fn off_size_stock_resources_are_skipped_with_a_note() {
        let extra = vec![
            (int_id(53), solid_dib(W, H, WHITE)),
            (int_id(67), solid_dib(9, 9, INK)),
        ];
        let (_dir, output, result) = run_file(&ne_image(extra));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();

        assert!(summary.contains("9x9 does not match"), "{summary}");
        assert!(!summary.contains("stock_blocked"), "{summary}");
    }

    #[test]
    fn ne_string_named_bitmaps_are_skipped_with_a_note() {
        // A string-named RT_BITMAP resource (id bit 15 clear) is counted.
        let (_dir, output, result) =
            run_file(&ne_image(vec![(0x0040, solid_dib(W, H, (1, 1, 1)))]));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();
        assert!(summary.contains("string-named"), "{summary}");
    }

    #[test]
    fn ne_undecodable_resource_is_skipped_with_a_note() {
        // A card-id-range resource is fine (faces complete); an extra resource
        // whose bytes are not a valid DIB is skipped, not fatal.
        let (_dir, output, result) =
            run_file(&ne_image(vec![(int_id(70), vec![0xFF, 0xFF, 0xFF, 0xFF])]));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();
        assert!(summary.contains("not a decodable bitmap"), "{summary}");
    }

    #[test]
    fn ne_with_no_backs_gets_a_solid_fallback_back() {
        let (_dir, output, result) = run_file(&ne_image(vec![]));
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();
        assert!(summary.contains("back_solid"), "{summary}");
        assert!(summary.contains("fallback"), "{summary}");
        assert!(output.join("backs").join("back_solid.png").is_file());
    }

    #[test]
    fn ne_missing_faces_is_the_incomplete_faces_error() {
        // Only 10 faces present: the SOL.EXE-alone scenario.
        let partial: Vec<(u16, Vec<u8>)> = (1..=10_u16)
            .map(|id| (int_id(id), solid_dib(W, H, (0, 0, 0))))
            .collect();
        let image = build_ne(0, &[(RT_BITMAP, partial)]).0;
        let (_dir, _output, result) = run_file(&image);
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            ExtractError::ResourceFacesIncomplete { found: 10 }
        ));
        assert!(error.to_string().contains("CARDS.DLL"));
    }

    #[test]
    fn ne_faces_of_inconsistent_size_are_rejected() {
        let mut entries = face_entries();
        // Replace one face with a differently-sized bitmap.
        entries.retain(|(id, _)| *id != int_id(1));
        entries.push((int_id(1), solid_dib(W + 1, H, (0, 0, 0))));
        let image = build_ne(0, &[(RT_BITMAP, entries)]).0;
        let (_dir, _output, result) = run_file(&image);
        assert!(matches!(
            result.unwrap_err(),
            ExtractError::FaceSizeMismatch(_)
        ));
    }

    #[test]
    fn a_truncated_ne_resource_table_propagates_an_ne_error() {
        let mut image = ne_image(vec![]);
        // Point the resource table pointer far past EOF.
        image[0x40 + 0x24..0x40 + 0x26].copy_from_slice(&0xFFFF_u16.to_le_bytes());
        let (_dir, _output, result) = run_file(&image);
        assert!(matches!(result.unwrap_err(), ExtractError::Ne(_)));
    }

    // -- corner cutout (what cdtDrawExt saves and restores around a blit) --

    /// The twelve frame-local coordinates the original never paints, for a
    /// `width` x `height` frame — spelled out rather than derived, so this
    /// pins the shape independently of the code under test.
    fn corner_coords(width: u32, height: u32) -> Vec<(u32, u32)> {
        vec![
            (0, 0),
            (1, 0),
            (0, 1),
            (width - 1, 0),
            (width - 2, 0),
            (width - 1, 1),
            (width - 1, height - 1),
            (width - 1, height - 2),
            (width - 2, height - 1),
            (0, height - 1),
            (1, height - 1),
            (0, height - 2),
        ]
    }

    /// Asserts every frame of `image` (a `frames`-frame strip, or a single
    /// image when `frames` is 1) has its twelve corner pixels cleared.
    fn assert_corners_cut(image: &RasterImage, frames: u32, label: &str) {
        let frame_width = image.width / frames;
        for frame in 0..frames {
            for (x, y) in corner_coords(frame_width, image.height) {
                assert_eq!(
                    pixel_at(image, frame * frame_width + x, y),
                    [0, 0, 0, 0],
                    "{label} frame {frame} pixel ({x}, {y})"
                );
            }
        }
    }

    /// Reads a written PNG from the extracted package.
    fn read_png(output: &Path, relative: &str) -> RasterImage {
        raster::decode(&std::fs::read(output.join(relative)).unwrap()).unwrap()
    }

    /// The defect this reproduces: the original rounds a card's corners at
    /// draw time, so an extract that emits them opaque paints white notches
    /// at every corner of every card.
    #[test]
    fn every_written_card_image_has_the_originals_corners_cut_away() {
        let (_dir, output, result) = run_file(&ne_image(placeholder_entries()));
        result.unwrap();
        crate::validate::run(&output).unwrap();

        for relative in [
            "cards/spades_01.png",
            "cards/clubs_13.png",
            "backs/back_54.png",
            "placeholders/empty_pile.png",
            "placeholders/stock_recycle.png",
            "placeholders/stock_blocked.png",
        ] {
            assert_corners_cut(&read_png(&output, relative), 1, relative);
        }

        // Only the corners: an interior pixel of the red-ink stock indicator
        // keeps its color, so this is a cutout and not a wholesale clear.
        assert_eq!(
            pixel_at(&read_png(&output, "placeholders/stock_blocked.png"), 2, 3),
            [0xFF, 0x00, 0x00, 0xFF]
        );
    }

    #[test]
    fn the_solid_fallback_back_is_cut_like_any_other_card_image() {
        let (_dir, output, result) = run_file(&ne_image(vec![]));
        result.unwrap();
        assert_corners_cut(&read_png(&output, "backs/back_solid.png"), 1, "back_solid");
    }

    #[test]
    fn a_loose_directory_extract_is_cut_too() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("loose");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        std::fs::write(input.join("myback.png"), png_bytes(W, H, [7, 7, 7])).unwrap();
        let output = dir.path().join("out");
        run(&input, &output).unwrap();

        assert_corners_cut(&read_png(&output, "cards/spades_01.png"), 1, "face");
        assert_corners_cut(&read_png(&output, "backs/myback.png"), 1, "myback");
    }

    // -- PE end to end --

    #[test]
    fn pe_extract_writes_a_theme_that_validates_green() {
        let mut entries: Vec<Rsrc> = (1..=52_u32)
            .map(|id| Rsrc::Id(id, solid_dib(W, H, (0, 0, 0))))
            .collect();
        entries.push(Rsrc::Id(54, solid_dib(W, H, (7, 7, 7)))); // -> back_54
        let image = build_pe(&[(RT_BITMAP & 0x7FFF, entries)]);
        let (_dir, output, result) = run_file(&image);
        let summary = result.unwrap();
        crate::validate::run(&output).unwrap();
        assert!(summary.contains("52 card faces"), "{summary}");
        assert!(summary.contains("back_54"), "{summary}");
    }

    #[test]
    fn a_pe_signature_with_garbage_body_propagates_a_pe_error() {
        // MZ + a PE signature at e_lfanew, but not a valid PE image.
        let mut image = vec![0_u8; 0x80];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40_u32.to_le_bytes());
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        let (_dir, _output, result) = run_file(&image);
        assert!(matches!(result.unwrap_err(), ExtractError::Pe(_)));
    }

    // -- sniffing errors --

    #[test]
    fn a_non_mz_file_is_not_executable() {
        let (_dir, _output, result) = run_file(b"ZZ not an executable");
        let error = result.unwrap_err();
        assert!(matches!(error, ExtractError::NotExecutable { .. }));
        assert!(error.to_string().contains("ZZ"), "{error}");
    }

    #[test]
    fn a_non_mz_file_with_non_graphic_bytes_renders_them_as_dots() {
        let (_dir, _output, result) = run_file(&[0x00, 0x01, 0x02]);
        let error = result.unwrap_err();
        assert!(matches!(error, ExtractError::NotExecutable { .. }));
        assert!(error.to_string().contains(".."), "{error}");
    }

    #[test]
    fn a_zero_byte_file_reads_as_an_empty_file_not_a_bare_hex_prefix() {
        let (_dir, _output, result) = run_file(&[]);
        let error = result.unwrap_err();
        assert!(matches!(error, ExtractError::NotExecutable { .. }));
        let message = error.to_string();
        assert!(message.contains("empty file"), "{message}");
        // The old rendering interpolated zero bytes as a bare, dangling
        // "0x" prefix with empty quotes: `0x ("")`. That artifact must be
        // gone from the cleaned-up message.
        assert!(!message.contains("0x ("), "{message}");
    }

    #[test]
    fn an_mz_file_too_short_for_e_lfanew_is_unknown() {
        let (_dir, _output, result) = run_file(b"MZ");
        assert!(matches!(
            result.unwrap_err(),
            ExtractError::UnknownExecutable { .. }
        ));
    }

    #[test]
    fn an_mz_file_whose_header_offset_is_past_eof_is_unknown() {
        let mut image = vec![0_u8; 0x40];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        let (_dir, _output, result) = run_file(&image);
        assert!(matches!(
            result.unwrap_err(),
            ExtractError::UnknownExecutable { .. }
        ));
    }

    #[test]
    fn an_mz_file_with_an_le_header_is_unknown() {
        let mut image = vec![0_u8; 0x50];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40_u32.to_le_bytes());
        image[0x40..0x42].copy_from_slice(b"LE");
        let (_dir, _output, result) = run_file(&image);
        let error = result.unwrap_err();
        assert!(matches!(error, ExtractError::UnknownExecutable { .. }));
        assert!(error.to_string().contains("LE"), "{error}");
    }

    #[test]
    fn a_nonexistent_input_file_is_input_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(&dir.path().join("nope.dll"), &dir.path().join("out"));
        assert!(matches!(
            result.unwrap_err(),
            ExtractError::InputUnreadable { .. }
        ));
    }

    // -- output directory guard --

    #[test]
    fn a_non_empty_output_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("keep.txt"), b"do not clobber").unwrap();
        let input = dir.path().join("in.bin");
        std::fs::write(&input, ne_image(vec![])).unwrap();
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::OutputNotEmpty { .. }
        ));
    }

    #[test]
    fn an_existing_empty_output_directory_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::create_dir(&output).unwrap();
        let input = dir.path().join("in.bin");
        std::fs::write(&input, ne_image(vec![])).unwrap();
        run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();
    }

    #[test]
    fn an_unwritable_output_location_is_a_typed_error() {
        // The output path's parent is a regular file, so creating the theme's
        // `cards/` subdirectory under it fails — surfaced as a typed error.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a directory").unwrap();
        let output = blocker.join("theme");
        let input = dir.path().join("in.bin");
        std::fs::write(&input, ne_image(vec![])).unwrap();
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::OutputUnwritable { .. }
        ));
    }

    #[test]
    fn an_output_path_that_is_a_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"i am a file").unwrap();
        let input = dir.path().join("in.bin");
        std::fs::write(&input, ne_image(vec![])).unwrap();
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::OutputNotEmpty { .. }
        ));
    }

    // -- loose directory: canonical & numeric faces --

    #[test]
    fn loose_canonical_faces_and_a_static_back_validate() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("mytheme");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        // A base-sized non-face .bmp becomes a static back by its stem.
        std::fs::write(input.join("castle.bmp"), bmp_bytes(W, H, (3, 4, 5))).unwrap();
        // A non-bitmap file and a subdirectory are ignored.
        std::fs::write(input.join("notes.txt"), b"ignore me").unwrap();
        std::fs::create_dir(input.join("subdir")).unwrap();

        let output = dir.path().join("out");
        let summary = run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();
        assert!(summary.contains("castle"), "{summary}");
        assert!(summary.contains("Skipped: nothing"), "{summary}");
        // Name comes from the directory stem.
        assert!(summary.contains("mytheme"), "{summary}");
    }

    #[test]
    fn loose_numeric_faces_validate() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("nums");
        std::fs::create_dir(&input).unwrap();
        for id in 1..=52 {
            std::fs::write(input.join(format!("{id}.png")), png_bytes(W, H, [1, 2, 3])).unwrap();
        }
        let output = dir.path().join("out");
        run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();
    }

    #[test]
    fn loose_mixing_canonical_and_numeric_face_names_is_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("mixed");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(input.join("spades_01.png"), png_bytes(W, H, [0, 0, 0])).unwrap();
        std::fs::write(input.join("1.png"), png_bytes(W, H, [0, 0, 0])).unwrap();
        let output = dir.path().join("out");
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::LooseMixedConventions
        ));
    }

    #[test]
    fn loose_incomplete_faces_is_the_incomplete_error() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("partial");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(input.join("spades_01.png"), png_bytes(W, H, [0, 0, 0])).unwrap();
        let output = dir.path().join("out");
        let error = run(&input, &output).unwrap_err();
        assert!(matches!(
            error,
            ExtractError::LooseFacesIncomplete { found: 1 }
        ));
        assert!(error.to_string().contains("numerically"));
    }

    #[test]
    fn loose_faces_of_inconsistent_size_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("badsize");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        // Overwrite one face at a different size.
        std::fs::write(input.join("spades_01.png"), png_bytes(W + 2, H, [0, 0, 0])).unwrap();
        let output = dir.path().join("out");
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::FaceSizeMismatch(_)
        ));
    }

    // -- loose directory: back detection --

    #[test]
    fn loose_frame_numbered_files_pack_into_one_animated_strip() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("anim");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        std::fs::write(input.join("robot_0.png"), png_bytes(W, H, [1, 0, 0])).unwrap();
        std::fs::write(input.join("robot_1.png"), png_bytes(W, H, [0, 1, 0])).unwrap();
        let output = dir.path().join("out");
        run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();

        let toml = std::fs::read_to_string(output.join("theme.toml")).unwrap();
        assert!(
            toml.contains("robot = { image = \"backs/robot.png\", frames = 2, fps = 2 }"),
            "{toml}"
        );
        // The packed strip is frames × base wide.
        let strip = raster::decode(&std::fs::read(output.join("backs").join("robot.png")).unwrap())
            .unwrap();
        assert_eq!((strip.width, strip.height), (W * 2, H));
    }

    #[test]
    fn loose_non_contiguous_frames_become_individual_static_backs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("seq");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        // seq_1, seq_2 but no seq_0: not a clean run, so kept individual.
        std::fs::write(input.join("seq_1.png"), png_bytes(W, H, [1, 0, 0])).unwrap();
        std::fs::write(input.join("seq_2.png"), png_bytes(W, H, [0, 1, 0])).unwrap();
        // A single-frame group (lone_0) also stays a static back.
        std::fs::write(input.join("lone_0.png"), png_bytes(W, H, [0, 0, 1])).unwrap();
        let output = dir.path().join("out");
        run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();

        let toml = std::fs::read_to_string(output.join("theme.toml")).unwrap();
        assert!(toml.contains("seq_1 = { image"), "{toml}");
        assert!(toml.contains("seq_2 = { image"), "{toml}");
        assert!(toml.contains("lone_0 = { image"), "{toml}");
        // None of them are strips.
        assert!(!toml.contains("frames ="), "{toml}");
    }

    #[test]
    fn loose_off_size_files_are_skipped_with_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("off");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        std::fs::write(input.join("banner.png"), png_bytes(W * 3, H, [0, 0, 0])).unwrap();
        let output = dir.path().join("out");
        let summary = run(&input, &output).unwrap();
        // Only the fallback back remains; the banner is skipped.
        assert!(summary.contains("does not match"), "{summary}");
        assert!(summary.contains("back_solid"), "{summary}");
    }

    #[test]
    fn a_corrupt_loose_bitmap_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("corrupt");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(input.join("broken.bmp"), b"BM not really a bitmap at all").unwrap();
        let output = dir.path().join("out");
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::LooseFileUndecodable { .. }
        ));
    }

    #[test]
    fn a_corrupt_loose_png_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("corrupt_png");
        std::fs::create_dir(&input).unwrap();
        std::fs::write(input.join("broken.png"), b"not a png at all").unwrap();
        let output = dir.path().join("out");
        assert!(matches!(
            run(&input, &output).unwrap_err(),
            ExtractError::LooseFileUndecodable { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_directory_without_read_permission_is_input_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("locked");
        std::fs::create_dir(&input).unwrap();
        std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o000)).unwrap();
        let output = dir.path().join("out");

        let result = run(&input, &output);
        // Restore before any assertion can panic, so the tempdir can clean itself up.
        std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(matches!(
            result.unwrap_err(),
            ExtractError::InputUnreadable { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_file_without_read_permission_is_undecodable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("has_locked_file");
        std::fs::create_dir(&input).unwrap();
        let blocked = input.join("blocked.png");
        std::fs::write(&blocked, png_bytes(W, H, [1, 2, 3])).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let output = dir.path().join("out");

        let result = run(&input, &output);
        // Restore before any assertion can panic, so the tempdir can clean itself up.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(
            result.unwrap_err(),
            ExtractError::LooseFileUndecodable { .. }
        ));
    }

    // -- unit tests for the small pure helpers --

    #[test]
    fn resource_face_maps_the_cards_dll_layout() {
        let face = |suit, rank: u8| Some((suit, FaceRank::try_from(rank).unwrap()));
        // Suit-major blocks of 13: clubs, diamonds, hearts, spades. The
        // block boundaries (13/14) and all four block starts pin the
        // layout; a rank-major misread would turn id 4 into the ace of
        // spades instead of the four of clubs.
        assert_eq!(resource_face(1), face(FaceSuit::Clubs, 1));
        assert_eq!(resource_face(4), face(FaceSuit::Clubs, 4));
        assert_eq!(resource_face(13), face(FaceSuit::Clubs, 13));
        assert_eq!(resource_face(14), face(FaceSuit::Diamonds, 1));
        assert_eq!(resource_face(27), face(FaceSuit::Hearts, 1));
        assert_eq!(resource_face(40), face(FaceSuit::Spades, 1));
        assert_eq!(resource_face(52), face(FaceSuit::Spades, 13));
        assert_eq!(resource_face(0), None);
        assert_eq!(resource_face(53), None);
    }

    #[test]
    fn parse_frame_stem_recognizes_only_a_trailing_number() {
        assert_eq!(parse_frame_stem("robot_2"), Some(("robot".to_owned(), 2)));
        assert_eq!(parse_frame_stem("a_b_0"), Some(("a_b".to_owned(), 0)));
        assert_eq!(parse_frame_stem("robot"), None);
        assert_eq!(parse_frame_stem("robot_"), None);
        assert_eq!(parse_frame_stem("_2"), None);
        assert_eq!(parse_frame_stem("robot_x"), None);
    }

    #[test]
    fn parse_frame_stem_rejects_a_leading_plus_sign_even_though_u32_parse_accepts_it() {
        // "+5" is not all ASCII digits (the digit check must reject it), but
        // `str::parse::<u32>` itself DOES accept a leading '+' -- so this
        // proves the digit check does the rejecting, not a lucky downstream
        // parse failure.
        assert_eq!(parse_frame_stem("robot_+5"), None);
    }

    #[test]
    fn uniquify_back_names_disambiguates_collisions() {
        let mut backs = vec![
            ClassifiedBack {
                name: "castle".to_owned(),
                image: solid_image((W, H), [0, 0, 0]),
                frames: None,
            },
            ClassifiedBack {
                name: "castle".to_owned(),
                image: solid_image((W, H), [0, 0, 0]),
                frames: None,
            },
            ClassifiedBack {
                name: "castle".to_owned(),
                image: solid_image((W, H), [0, 0, 0]),
                frames: None,
            },
        ];
        uniquify_back_names(&mut backs);
        let names: Vec<&str> = backs.iter().map(|back| back.name.as_str()).collect();
        assert_eq!(names, vec!["castle", "castle_2", "castle_3"]);
    }

    #[test]
    fn loose_backs_that_sanitize_to_the_same_name_are_disambiguated_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("dup");
        std::fs::create_dir(&input).unwrap();
        write_canonical_faces(&input);
        // Distinct files on a case-sensitive fs, all sanitizing to "castle".
        std::fs::write(input.join("castle.png"), png_bytes(W, H, [1, 0, 0])).unwrap();
        std::fs::write(input.join("Castle.png"), png_bytes(W, H, [0, 1, 0])).unwrap();
        let output = dir.path().join("out");
        run(&input, &output).unwrap();
        crate::validate::run(&output).unwrap();
        let toml = std::fs::read_to_string(output.join("theme.toml")).unwrap();
        assert!(toml.contains("castle = { image"), "{toml}");
        assert!(toml.contains("castle_2 = { image"), "{toml}");
    }

    // -- write helpers --

    #[test]
    fn input_stem_falls_back_to_theme_for_a_path_with_no_file_stem() {
        assert_eq!(input_stem(Path::new("/")), "theme");
    }

    #[test]
    fn write_png_reports_an_encode_failure_as_output_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        // Pixel buffer length doesn't match width * height * 4: encode fails.
        let image = RasterImage {
            width: 2,
            height: 2,
            pixels: vec![0; 3],
        };
        assert!(matches!(
            write_png(&path, &image).unwrap_err(),
            ExtractError::OutputUnwritable { .. }
        ));
    }

    #[test]
    fn write_png_reports_a_write_failure_as_output_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked.png");
        // The target path is already a directory, so the byte write fails
        // even though encoding the (valid) image succeeds.
        std::fs::create_dir(&path).unwrap();
        let image = solid_image((W, H), [1, 2, 3]);
        assert!(matches!(
            write_png(&path, &image).unwrap_err(),
            ExtractError::OutputUnwritable { .. }
        ));
    }

    #[test]
    fn write_theme_reports_a_theme_toml_write_failure_as_output_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::create_dir(&output).unwrap();
        // `theme.toml` pre-exists as a directory, so the final write fails
        // after the card/back PNGs have already been written successfully.
        std::fs::create_dir(output.join("theme.toml")).unwrap();
        let theme = ClassifiedTheme {
            name: "theme".to_owned(),
            base_size: (W, H),
            faces: Vec::new(),
            backs: Vec::new(),
            placeholders: ClassifiedPlaceholders::default(),
            skips: Vec::new(),
            notes: Vec::new(),
        };
        assert!(matches!(
            write_theme(&output, theme).unwrap_err(),
            ExtractError::OutputUnwritable { .. }
        ));
    }

    #[test]
    fn every_error_variant_renders_a_non_empty_message() {
        let mismatch = FaceSizeMismatch {
            face: "spades_01".to_owned(),
            expected_width: 5,
            expected_height: 7,
            found_width: 6,
            found_height: 7,
        };
        for error in [
            ExtractError::InputUnreadable {
                path: "p".to_owned(),
                message: "m".to_owned(),
            },
            ExtractError::NotExecutable {
                found: "0x5A5A".to_owned(),
            },
            ExtractError::UnknownExecutable {
                detail: "d".to_owned(),
            },
            ExtractError::Ne(NeError::TypeInfoTruncated),
            ExtractError::Pe(PeError::Parse {
                message: "m".to_owned(),
            }),
            ExtractError::LooseFileUndecodable {
                path: "p".to_owned(),
                message: "m".to_owned(),
            },
            ExtractError::LooseMixedConventions,
            ExtractError::ResourceFacesIncomplete { found: 3 },
            ExtractError::LooseFacesIncomplete { found: 3 },
            ExtractError::FaceSizeMismatch(mismatch),
            ExtractError::OutputNotEmpty {
                path: "p".to_owned(),
            },
            ExtractError::OutputUnwritable {
                path: "p".to_owned(),
                message: "m".to_owned(),
            },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
