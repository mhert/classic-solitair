//! [`ManifestError`]: every distinct way a `theme.toml` document can fail to
//! become a validated [`crate::Manifest`].

use crate::back::{BackName, BackNameError};
use crate::color::ColorError;
use crate::size::CardSizeError;

/// Every distinct way [`crate::Manifest::from_toml_bytes`] or
/// [`crate::Manifest::from_toml_str`] can fail.
///
/// Variants fall into two families. Pure *shape* problems — invalid UTF-8,
/// invalid TOML syntax, a missing required section/key, an unknown key
/// anywhere (deny-unknown-fields), or a value of the wrong TOML
/// type — are reported through [`ManifestError::InvalidToml`], mirroring
/// `sol_session::SaveError::Malformed`: the underlying `toml` crate's error
/// type is rendered to text at the boundary and never appears in this
/// crate's public API. Every other variant is a *domain* rule —
/// each names the offending section, key, or back so a theme author can find
/// the problem.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// `from_toml_bytes` was given bytes that are not valid UTF-8.
    #[error("theme.toml is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] core::str::Utf8Error),

    /// The document is not syntactically valid TOML, or does not match the
    /// required shape: a missing required section/key, an unknown key
    /// (unknown fields are denied everywhere), or a value of the wrong TOML
    /// type.
    #[error("theme.toml does not match the expected format: {message}")]
    InvalidToml {
        /// The underlying `toml` crate error, rendered to text so the
        /// foreign error type never crosses this crate's public API.
        message: String,
    },

    /// `[theme] name` is present but empty.
    #[error("[theme] name must not be empty")]
    EmptyName,

    /// `[cards] base_size` is not a two-element array of integers each
    /// `1..=u32::MAX`.
    #[error(transparent)]
    InvalidCardSize(#[from] CardSizeError),

    /// `[cards] faces` is neither a directory path (ends `/`) nor an SVG
    /// sheet path (ends `.svg`).
    #[error(
        "[cards] faces = {value:?} must end in \"/\" (a face directory) or \".svg\" (a vector sheet)"
    )]
    InvalidFacesShape {
        /// The rejected `faces` value.
        value: String,
    },

    /// `[cards] faces` names an SVG sheet, but `render_mode` is not
    /// `"vector"`.
    #[error("[cards] faces = {value:?} is an SVG sheet, which requires render_mode = \"vector\"")]
    SvgFacesRequireVectorMode {
        /// The rejected `faces` value.
        value: String,
    },

    /// `[backs]` is present but has no entries (at least one back is
    /// required).
    #[error("[backs] must declare at least one back")]
    NoBacks,

    /// A `[backs]` key is not a valid [`BackName`].
    #[error(transparent)]
    InvalidBackName(#[from] BackNameError),

    /// A back gives `fps` on a single-image back without `frames` — not a
    /// recognized shape.
    #[error("back `{back}`: `fps` was given without `frames`")]
    BackFpsWithoutFrames {
        /// The offending back.
        back: BackName,
    },

    /// A back gives `frames` on a single-image back without `fps` — not a
    /// recognized shape.
    #[error("back `{back}`: `frames` was given without `fps`")]
    BackFramesWithoutFps {
        /// The offending back.
        back: BackName,
    },

    /// A back gives `layout` without both `frames` and `fps` present —
    /// `layout` is only meaningful on the strip shape.
    #[error("back `{back}`: `layout` is only valid together with `frames` and `fps`")]
    BackLayoutWithoutStrip {
        /// The offending back.
        back: BackName,
    },

    /// A strip back's `frames` is below 2.
    #[error("back `{back}`: frames = {frames} must be at least 2")]
    BackTooFewFrames {
        /// The offending back.
        back: BackName,
        /// The rejected `frames` value.
        frames: i64,
    },

    /// A strip back's `frames` does not fit in a u32.
    #[error("back `{back}`: frames = {frames} must fit in a u32")]
    BackFramesTooLarge {
        /// The offending back.
        back: BackName,
        /// The rejected `frames` value.
        frames: i64,
    },

    /// A back's `fps` is zero (or missing where required).
    #[error("back `{back}`: fps must be at least 1")]
    BackZeroFps {
        /// The offending back.
        back: BackName,
    },

    /// A back's `fps` does not fit in a u32.
    #[error("back `{back}`: fps = {fps} must fit in a u32")]
    BackFpsTooLarge {
        /// The offending back.
        back: BackName,
        /// The rejected `fps` value.
        fps: i64,
    },

    /// A strip back's `layout` is neither `"horizontal"` nor `"vertical"`.
    #[error("back `{back}`: layout = {value:?} must be \"horizontal\" or \"vertical\"")]
    BackInvalidLayout {
        /// The offending back.
        back: BackName,
        /// The rejected `layout` value.
        value: String,
    },

    /// A list-form back's image list has fewer than 2 entries.
    #[error("back `{back}`: the image list has {count} entries, at least 2 are required")]
    BackTooFewListImages {
        /// The offending back.
        back: BackName,
        /// The number of images actually given.
        count: usize,
    },

    /// A list-form back also gives `frames` or `layout` — both are invalid
    /// with list form.
    #[error("back `{back}`: `frames` and `layout` are not valid with the list image form")]
    BackListWithFramesOrLayout {
        /// The offending back.
        back: BackName,
    },

    /// `[table] background` has neither `color` nor `image`, or has both.
    #[error("[table] background must set exactly one of `color` or `image`")]
    BackgroundNeedsExactlyOneOfColorOrImage,

    /// `[table] background` sets `tile` without `image`.
    #[error("[table] background: `tile` is only valid together with `image`")]
    BackgroundTileWithoutImage,

    /// A `Color`-valued field did not parse as `"#rrggbb"`.
    #[error("{field}: {source}")]
    InvalidColor {
        /// The dotted path of the offending field, e.g.
        /// `"table.background.color"`.
        field: &'static str,
        /// The underlying parse failure.
        #[source]
        source: ColorError,
    },

    /// A path-valued field (a face directory, a back image, the background
    /// image, or a sound file) is not theme-package-relative: it is
    /// absolute, contains a `..` segment, or uses a backslash separator.
    #[error("{context}: path {path:?} is invalid: {reason}")]
    InvalidPath {
        /// The dotted path (and, for backs/sounds, the entry name) of the
        /// offending field.
        context: String,
        /// The rejected path value.
        path: String,
        /// Which relative-path rule was violated.
        reason: &'static str,
    },
}
