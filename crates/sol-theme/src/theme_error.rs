//! [`ThemeError`]: every distinct way loading a theme package
//! ([`crate::Theme::from_source`] and its conveniences) can fail.

use crate::back::BackName;
use crate::error::ManifestError;
use crate::source::SourceError;

/// Every distinct way [`crate::Theme::from_source`] (or one of its
/// conveniences) can fail to produce a validated [`crate::Theme`].
///
/// Loading order is manifest, then faces in canonical order, then backs in
/// declaration order, then the background, then sounds — first failure
/// wins, so only one of these is ever returned per call. Every variant
/// names the offending path, face, or back so a theme author can find the
/// problem directly from the error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThemeError {
    /// `theme.toml` was read successfully but failed manifest validation.
    #[error(transparent)]
    Manifest(#[from] ManifestError),

    /// `theme.toml` itself could not be read from the source: missing, or
    /// some other I/O failure.
    #[error("theme.toml: {source}")]
    ManifestUnreadable {
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// The theme package's bytes are not a valid zip archive
    /// ([`crate::Theme::load_zip_bytes`], [`crate::Theme::load_path`]).
    #[error("not a valid zip archive: {message}")]
    MalformedZip {
        /// The underlying zip-parsing failure, rendered to text.
        message: String,
    },

    /// `[table] background` sets `tile` on an image too small to tile.
    #[error(
        "[table] background: {path} is {width}x{height}, but a tiled background must be at least {minimum}x{minimum}"
    )]
    TiledBackgroundTooSmall {
        /// The offending image's path.
        path: String,
        /// The probed width.
        width: u32,
        /// The probed height.
        height: u32,
        /// The smallest legal tile edge, in pixels.
        minimum: u32,
    },

    /// The archive's entries inflate past the size a theme package is allowed
    /// to occupy. A theme package is untrusted input and is decompressed
    /// eagerly, so its compressed size bounds nothing.
    #[error("theme archive inflates past the {limit}-byte limit")]
    ZipTooLarge {
        /// The ceiling that was exceeded, in bytes.
        limit: u64,
    },

    /// [`crate::Theme::load_path`] was given a path that is neither a
    /// directory nor readable as a valid zip archive.
    #[error("{path}: not a directory or a valid zip archive: {message}")]
    UnrecognizedPackage {
        /// The rejected path.
        path: String,
        /// Why it was rejected: the filesystem read failure, or the
        /// underlying zip-parsing failure, rendered to text.
        message: String,
    },

    /// A face image (directory form) could not be read.
    #[error("face `{name}`: {source}")]
    FaceUnreadable {
        /// The canonical face name, e.g. `"spades_01"`.
        name: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// A face image's bytes do not probe as the format its theme's
    /// `render_mode` requires.
    #[error("face `{name}` ({path}): {reason}")]
    FaceInvalidFormat {
        /// The canonical face name, e.g. `"spades_01"`.
        name: String,
        /// The path the face was read from.
        path: String,
        /// Why the bytes did not probe as the expected format.
        reason: String,
    },

    /// A face image does not probe to exactly `[cards] base_size`.
    #[error(
        "face `{name}`: expected {expected_width}x{expected_height}, found {found_width}x{found_height}"
    )]
    FaceWrongSize {
        /// The canonical face name, e.g. `"spades_01"`.
        name: String,
        /// `base_size`'s width.
        expected_width: u32,
        /// `base_size`'s height.
        expected_height: u32,
        /// The face's actual probed width.
        found_width: u32,
        /// The face's actual probed height.
        found_height: u32,
    },

    /// The SVG face sheet ([`crate::FacesSource::SvgSheet`]) could not be
    /// read.
    #[error("face sheet {path}: {source}")]
    FaceSheetUnreadable {
        /// The sheet's path.
        path: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// The SVG face sheet's bytes do not probe as SVG.
    #[error("face sheet {path}: {reason}")]
    FaceSheetInvalidFormat {
        /// The sheet's path.
        path: String,
        /// Why the bytes did not probe as SVG.
        reason: String,
    },

    /// The SVG face sheet does not probe to exactly `(13 * base_w) x (4 *
    /// base_h)`.
    #[error(
        "face sheet {path}: expected {expected_width}x{expected_height} (13x4 grid), found {found_width}x{found_height}"
    )]
    FaceSheetWrongSize {
        /// The sheet's path.
        path: String,
        /// `13 * base_size.width`.
        expected_width: u32,
        /// `4 * base_size.height`.
        expected_height: u32,
        /// The sheet's actual probed width.
        found_width: u32,
        /// The sheet's actual probed height.
        found_height: u32,
    },

    /// A back's image path does not end in the extension its theme's
    /// `render_mode` requires.
    #[error("back `{back}`: {path:?} must end in {expected_ext} for this theme's render_mode")]
    BackWrongExtension {
        /// The offending back.
        back: BackName,
        /// The rejected path.
        path: String,
        /// The extension `render_mode` requires, e.g. `".png"`.
        expected_ext: &'static str,
    },

    /// A back's image could not be read.
    #[error("back `{back}` ({path}): {source}")]
    BackUnreadable {
        /// The offending back.
        back: BackName,
        /// The path that could not be read (list form has more than one;
        /// this names the specific one).
        path: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// A back's image bytes do not probe as the format its theme's
    /// `render_mode` requires.
    #[error("back `{back}` ({path}): {reason}")]
    BackInvalidFormat {
        /// The offending back.
        back: BackName,
        /// The path whose bytes failed to probe.
        path: String,
        /// Why the bytes did not probe as the expected format.
        reason: String,
    },

    /// A back's image does not probe to the size its shape
    /// requires: `base_size` for static and list frames, `frames *` the
    /// layout axis (base on the other axis) for a strip.
    #[error(
        "back `{back}` ({path}): expected {expected_width}x{expected_height}, found {found_width}x{found_height}"
    )]
    BackWrongSize {
        /// The offending back.
        back: BackName,
        /// The path whose probed size was wrong.
        path: String,
        /// The required width.
        expected_width: u32,
        /// The required height.
        expected_height: u32,
        /// The actual probed width.
        found_width: u32,
        /// The actual probed height.
        found_height: u32,
    },

    /// `[table] background`'s image path does not end in the extension its
    /// theme's `render_mode` requires.
    #[error(
        "table.background.image: {path:?} must end in {expected_ext} for this theme's render_mode"
    )]
    BackgroundWrongExtension {
        /// The rejected path.
        path: String,
        /// The extension `render_mode` requires, e.g. `".png"`.
        expected_ext: &'static str,
    },

    /// `[table] background`'s image could not be read.
    #[error("table.background.image ({path}): {source}")]
    BackgroundUnreadable {
        /// The background image's path.
        path: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// `[table] background`'s image bytes do not probe as the format its
    /// theme's `render_mode` requires.
    #[error("table.background.image ({path}): {reason}")]
    BackgroundInvalidFormat {
        /// The background image's path.
        path: String,
        /// Why the bytes did not probe as the expected format.
        reason: String,
    },

    /// A `[placeholders]` entry's image path does not end in the extension
    /// its theme's `render_mode` requires.
    #[error(
        "placeholders.{slot}: {path:?} must end in {expected_ext} for this theme's render_mode"
    )]
    PlaceholderWrongExtension {
        /// The offending `[placeholders]` key.
        slot: &'static str,
        /// The rejected path.
        path: String,
        /// The extension `render_mode` requires, e.g. `".png"`.
        expected_ext: &'static str,
    },

    /// A `[placeholders]` entry's image could not be read.
    #[error("placeholders.{slot} ({path}): {source}")]
    PlaceholderUnreadable {
        /// The offending `[placeholders]` key.
        slot: &'static str,
        /// The path that could not be read.
        path: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },

    /// A `[placeholders]` entry's bytes do not probe as the format its
    /// theme's `render_mode` requires.
    #[error("placeholders.{slot} ({path}): {reason}")]
    PlaceholderInvalidFormat {
        /// The offending `[placeholders]` key.
        slot: &'static str,
        /// The path whose bytes failed to probe.
        path: String,
        /// Why the bytes did not probe as the expected format.
        reason: String,
    },

    /// A `[placeholders]` entry's image does not probe to `base_size`. A
    /// placeholder stands in a pile's card slot, so it must be card-sized.
    #[error(
        "placeholders.{slot} ({path}): expected {expected_width}x{expected_height}, found {found_width}x{found_height}"
    )]
    PlaceholderWrongSize {
        /// The offending `[placeholders]` key.
        slot: &'static str,
        /// The path whose probed size was wrong.
        path: String,
        /// The required width.
        expected_width: u32,
        /// The required height.
        expected_height: u32,
        /// The actual probed width.
        found_width: u32,
        /// The actual probed height.
        found_height: u32,
    },

    /// A `[sounds]` entry's bytes could not be read.
    #[error("sound `{name}` ({path}): {source}")]
    SoundUnreadable {
        /// The sound's `[sounds]` key.
        name: String,
        /// The sound's path.
        path: String,
        /// The underlying read failure.
        #[source]
        source: SourceError,
    },
}
