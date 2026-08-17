//! [`BackName`] and [`BackDef`]: `[backs]` keys and their validated
//! definitions.

use core::fmt;
use core::str::FromStr;

use serde::Deserialize;

use crate::error::ManifestError;
use crate::path::RelativeAssetPath;

/// A `[backs]` table key, e.g. `"robot"`.
///
/// Non-empty ASCII `[a-z0-9_-]+` only — theme authors control these, and
/// keeping them filesystem- and TOML-safe means they can be reused
/// directly as file stems by tooling (`soltool`) without re-validation.
///
/// ```
/// use sol_theme::BackName;
///
/// let name: BackName = "robot_2".parse()?;
/// assert_eq!(name.as_str(), "robot_2");
///
/// assert!("Robot".parse::<BackName>().is_err()); // uppercase
/// assert!("".parse::<BackName>().is_err()); // empty
/// # Ok::<(), sol_theme::BackNameError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackName(String);

impl BackName {
    /// The back name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// [`BackName`] could not be built: `raw` was empty or contained a byte
/// outside ASCII lowercase letters, digits, `_`, or `-`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid back name {raw:?}: must be non-empty ASCII lowercase letters, digits, `_`, or `-`"
)]
pub struct BackNameError {
    raw: String,
}

impl TryFrom<String> for BackName {
    type Error = BackNameError;

    /// # Errors
    ///
    /// Returns [`BackNameError`] if `value` is empty or contains a byte
    /// outside ASCII lowercase letters, digits, `_`, or `-`.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid_byte =
            |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-';
        if !value.is_empty() && value.bytes().all(valid_byte) {
            Ok(Self(value))
        } else {
            Err(BackNameError { raw: value })
        }
    }
}

impl FromStr for BackName {
    type Err = BackNameError;

    /// # Errors
    ///
    /// Returns [`BackNameError`] under the same conditions as
    /// [`BackName::try_from`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for BackName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The axis a [`BackDef::Strip`]'s frames are laid out along
/// (`[backs] <name>.layout`). Defaults to [`BackLayout::Horizontal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackLayout {
    /// Frames run left to right; the strip is `frames × base width` wide
    /// and `base height` tall.
    #[default]
    Horizontal,
    /// Frames run top to bottom; the strip is `base width` wide and
    /// `frames × base height` tall.
    Vertical,
}

/// How an animated back's frames advance over time.
///
/// An animated back ([`BackDef::Strip`] or [`BackDef::Frames`]) carries
/// exactly one of these — `[backs] <name>` gives either `fps` or
/// `durations_ms`, never both (see [`crate::ManifestError::BackFpsAndDurations`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackTiming {
    /// Uniform playback rate in frames per second (at least 1).
    Fps(u32),
    /// Explicit per-frame display duration in milliseconds, one entry per
    /// frame in frame order (each at least 1).
    DurationsMs(Vec<u32>),
}

/// A validated `[backs]` entry, in exactly one of the three recognized
/// shapes: a single static image, one strip of frames, or a list of
/// per-frame images.
///
/// Image *pixel dimensions* are not checked here — that requires reading
/// the asset bytes, which is the asset-loading layer's job (this crate does
/// no I/O). This type only validates the shape TOML declares: which keys
/// are present and whether their values are self-consistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackDef {
    /// A single, unanimated back image.
    Static {
        /// Validated theme-package-relative path to the image.
        image: RelativeAssetPath,
    },
    /// An animated back stored as one horizontal- or vertical-frame strip
    /// image.
    Strip {
        /// Validated theme-package-relative path to the strip image.
        image: RelativeAssetPath,
        /// Number of frames in the strip (at least 2).
        frames: u32,
        /// How the strip's frames advance over time.
        timing: BackTiming,
        /// Which axis the frames are laid out along.
        layout: BackLayout,
    },
    /// An animated back stored as a list of per-frame images.
    Frames {
        /// Validated theme-package-relative paths, one per frame (at least 2).
        images: Vec<RelativeAssetPath>,
        /// How the list's frames advance over time.
        timing: BackTiming,
    },
}

/// `image = "single/path.png"` or `image = ["frame0.png", "frame1.png"]`
/// — the one field that switches shape.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawImage {
    Single(String),
    Multiple(Vec<String>),
}

/// The permissive, shape-only parse of a `[backs]` entry. Every field
/// combination is syntactically legal here; [`validate`] applies the
/// semantic rules.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBackDef {
    image: RawImage,
    #[serde(default)]
    frames: Option<i64>,
    #[serde(default)]
    fps: Option<i64>,
    #[serde(default)]
    durations_ms: Option<Vec<i64>>,
    #[serde(default)]
    layout: Option<String>,
}

/// Validates a raw `[backs]` entry against the three back-definition
/// shapes, naming `name` in every rejection.
///
/// # Errors
///
/// Returns a [`ManifestError`] back-shape variant if `raw` matches none of
/// Static / Strip / Frames, or [`ManifestError::InvalidPath`] if an image
/// path is not theme-package-relative.
pub(crate) fn validate(name: &BackName, raw: RawBackDef) -> Result<BackDef, ManifestError> {
    let RawBackDef {
        image,
        frames,
        fps,
        durations_ms,
        layout,
    } = raw;
    match image {
        RawImage::Single(image) => validate_single(name, &image, frames, fps, durations_ms, layout),
        RawImage::Multiple(images) => {
            validate_multiple(name, &images, frames, fps, durations_ms, layout.is_some())
        }
    }
}

/// The requested timing form, once `fps` and `durations_ms` have been
/// reconciled to at most one of the two (see
/// [`ManifestError::BackFpsAndDurations`]). `None` means neither was given.
enum RawTiming {
    Fps(i64),
    DurationsMs(Vec<i64>),
}

/// Reconciles a back's raw `fps` and `durations_ms` into at most one
/// requested timing form — an animated back's timing is always exactly one
/// of the two, regardless of shape.
fn raw_timing(
    name: &BackName,
    fps: Option<i64>,
    durations_ms: Option<Vec<i64>>,
) -> Result<Option<RawTiming>, ManifestError> {
    match (fps, durations_ms) {
        (Some(_), Some(_)) => Err(ManifestError::BackFpsAndDurations { back: name.clone() }),
        (Some(fps), None) => Ok(Some(RawTiming::Fps(fps))),
        (None, Some(durations_ms)) => Ok(Some(RawTiming::DurationsMs(durations_ms))),
        (None, None) => Ok(None),
    }
}

/// Validates a strip's `frames` count: must fit `u32` and be at least 2.
fn validate_frames(name: &BackName, frames: i64) -> Result<u32, ManifestError> {
    match u32::try_from(frames) {
        Ok(f) if f >= 2 => Ok(f),
        Ok(_) => Err(ManifestError::BackTooFewFrames {
            back: name.clone(),
            frames,
        }),
        Err(_) => Err(ManifestError::BackFramesTooLarge {
            back: name.clone(),
            frames,
        }),
    }
}

/// Validates a back's `fps`: must fit `u32` and be at least 1.
fn validate_fps(name: &BackName, fps: i64) -> Result<u32, ManifestError> {
    match u32::try_from(fps) {
        Ok(f) if f >= 1 => Ok(f),
        Ok(_) => Err(ManifestError::BackZeroFps { back: name.clone() }),
        Err(_) => Err(ManifestError::BackFpsTooLarge {
            back: name.clone(),
            fps,
        }),
    }
}

/// Validates a strip's `layout` axis, defaulting to horizontal when absent.
fn validate_layout(name: &BackName, layout: Option<String>) -> Result<BackLayout, ManifestError> {
    match layout {
        None => Ok(BackLayout::default()),
        Some(value) if value == "horizontal" => Ok(BackLayout::Horizontal),
        Some(value) if value == "vertical" => Ok(BackLayout::Vertical),
        Some(value) => Err(ManifestError::BackInvalidLayout {
            back: name.clone(),
            value,
        }),
    }
}

/// Validates `durations_ms` against the required `expected` frame count
/// (strip) or image count (list): the length must match exactly, and each
/// duration must fit `u32` and be at least 1.
fn validate_durations(
    name: &BackName,
    durations_ms: Vec<i64>,
    expected: u32,
) -> Result<Vec<u32>, ManifestError> {
    let expected_len = usize::try_from(expected).unwrap_or(usize::MAX);
    if durations_ms.len() != expected_len {
        return Err(ManifestError::BackDurationsLengthMismatch {
            back: name.clone(),
            expected,
            got: durations_ms.len(),
        });
    }
    durations_ms
        .into_iter()
        .map(|value| match u32::try_from(value) {
            Ok(duration) if duration >= 1 => Ok(duration),
            Ok(_) => Err(ManifestError::BackZeroDuration { back: name.clone() }),
            Err(_) => Err(ManifestError::BackDurationTooLarge {
                back: name.clone(),
                value,
            }),
        })
        .collect()
}

fn validate_single(
    name: &BackName,
    image: &str,
    frames: Option<i64>,
    fps: Option<i64>,
    durations_ms: Option<Vec<i64>>,
    layout: Option<String>,
) -> Result<BackDef, ManifestError> {
    let timing = raw_timing(name, fps, durations_ms)?;
    match (frames, timing) {
        (Some(frames), Some(RawTiming::Fps(fps))) => {
            let frames = validate_frames(name, frames)?;
            let fps = validate_fps(name, fps)?;
            let layout = validate_layout(name, layout)?;
            let image = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
            Ok(BackDef::Strip {
                image,
                frames,
                timing: BackTiming::Fps(fps),
                layout,
            })
        }
        (Some(frames), Some(RawTiming::DurationsMs(durations_ms))) => {
            let frames = validate_frames(name, frames)?;
            let durations = validate_durations(name, durations_ms, frames)?;
            let layout = validate_layout(name, layout)?;
            let image = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
            Ok(BackDef::Strip {
                image,
                frames,
                timing: BackTiming::DurationsMs(durations),
                layout,
            })
        }
        (None, None) if layout.is_some() => {
            Err(ManifestError::BackLayoutWithoutStrip { back: name.clone() })
        }
        (None, None) => {
            let image = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
            Ok(BackDef::Static { image })
        }
        (Some(_), None) => Err(ManifestError::BackFramesWithoutTiming { back: name.clone() }),
        (None, Some(RawTiming::Fps(_))) => {
            Err(ManifestError::BackFpsWithoutFrames { back: name.clone() })
        }
        (None, Some(RawTiming::DurationsMs(_))) => {
            Err(ManifestError::BackDurationsWithoutFrames { back: name.clone() })
        }
    }
}

fn validate_multiple(
    name: &BackName,
    images: &[String],
    frames: Option<i64>,
    fps: Option<i64>,
    durations_ms: Option<Vec<i64>>,
    has_layout: bool,
) -> Result<BackDef, ManifestError> {
    if frames.is_some() || has_layout {
        return Err(ManifestError::BackListWithFramesOrLayout { back: name.clone() });
    }
    if images.len() < 2 {
        return Err(ManifestError::BackTooFewListImages {
            back: name.clone(),
            count: images.len(),
        });
    }
    let timing = match raw_timing(name, fps, durations_ms)? {
        Some(RawTiming::Fps(fps)) => BackTiming::Fps(validate_fps(name, fps)?),
        Some(RawTiming::DurationsMs(durations_ms)) => {
            let expected = u32::try_from(images.len()).unwrap_or(u32::MAX);
            BackTiming::DurationsMs(validate_durations(name, durations_ms, expected)?)
        }
        None => return Err(ManifestError::BackZeroFps { back: name.clone() }),
    };
    let mut parsed = Vec::with_capacity(images.len());
    for image in images {
        let path = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
        parsed.push(path);
    }
    Ok(BackDef::Frames {
        images: parsed,
        timing,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    fn raw(
        image: RawImage,
        frames: Option<i64>,
        fps: Option<i64>,
        durations_ms: Option<Vec<i64>>,
        layout: Option<&str>,
    ) -> RawBackDef {
        RawBackDef {
            image,
            frames,
            fps,
            durations_ms,
            layout: layout.map(str::to_owned),
        }
    }

    fn single(path: &str) -> RawImage {
        RawImage::Single(path.to_owned())
    }

    fn multiple(paths: &[&str]) -> RawImage {
        RawImage::Multiple(paths.iter().map(|p| (*p).to_owned()).collect())
    }

    fn name() -> BackName {
        BackName::try_from("robot".to_owned()).unwrap()
    }

    // -- BackName --

    #[test]
    fn back_name_accepts_lowercase_digits_underscore_dash() {
        assert!(BackName::try_from("robot-2_a9".to_owned()).is_ok());
    }

    #[test]
    fn back_name_rejects_empty() {
        assert_eq!(
            BackName::try_from(String::new()),
            Err(BackNameError { raw: String::new() })
        );
    }

    #[test]
    fn back_name_rejects_uppercase_and_other_bytes() {
        assert!(BackName::try_from("Robot".to_owned()).is_err());
        assert!(BackName::try_from("robot 2".to_owned()).is_err());
        assert!(BackName::try_from("robot.png".to_owned()).is_err());
    }

    #[test]
    fn back_name_from_str_matches_try_from() {
        assert_eq!("robot".parse::<BackName>().unwrap(), name());
    }

    #[test]
    fn back_name_display_matches_as_str() {
        assert_eq!(name().to_string(), "robot");
        assert_eq!(name().as_str(), "robot");
    }

    #[test]
    fn back_name_error_names_the_raw_text() {
        let error = BackName::try_from("BAD NAME".to_owned()).unwrap_err();
        assert!(error.to_string().contains("BAD NAME"));
    }

    // -- BackDef: valid shapes --

    #[test]
    fn validates_a_bare_image_as_static() {
        let def = validate(
            &name(),
            raw(single("backs/plain.png"), None, None, None, None),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Static {
                image: asset_path("backs/plain.png")
            }
        );
    }

    #[test]
    fn validates_frames_and_fps_as_a_horizontal_strip_by_default() {
        let def = validate(
            &name(),
            raw(single("backs/robot.png"), Some(4), Some(2), None, None),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 4,
                timing: BackTiming::Fps(2),
                layout: BackLayout::Horizontal,
            }
        );
    }

    #[test]
    fn validates_an_explicit_vertical_layout() {
        let def = validate(
            &name(),
            raw(
                single("backs/robot.png"),
                Some(3),
                Some(1),
                None,
                Some("vertical"),
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 3,
                timing: BackTiming::Fps(1),
                layout: BackLayout::Vertical,
            }
        );
    }

    #[test]
    fn validates_an_explicit_horizontal_layout() {
        let def = validate(
            &name(),
            raw(
                single("backs/robot.png"),
                Some(3),
                Some(1),
                None,
                Some("horizontal"),
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 3,
                timing: BackTiming::Fps(1),
                layout: BackLayout::Horizontal,
            }
        );
    }

    #[test]
    fn validates_a_list_of_paths_as_frames() {
        let def = validate(
            &name(),
            raw(
                multiple(&["backs/bats_0.png", "backs/bats_1.png"]),
                None,
                Some(3),
                None,
                None,
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Frames {
                images: vec![
                    asset_path("backs/bats_0.png"),
                    asset_path("backs/bats_1.png")
                ],
                timing: BackTiming::Fps(3),
            }
        );
    }

    #[test]
    fn validates_frames_and_durations_ms_as_a_strip() {
        let def = validate(
            &name(),
            raw(
                single("backs/palm.png"),
                Some(4),
                None,
                Some(vec![250, 250, 250, 49_250]),
                None,
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/palm.png"),
                frames: 4,
                timing: BackTiming::DurationsMs(vec![250, 250, 250, 49_250]),
                layout: BackLayout::Horizontal,
            }
        );
    }

    #[test]
    fn validates_a_list_of_paths_with_durations_ms_as_frames() {
        let def = validate(
            &name(),
            raw(
                multiple(&["backs/bats_0.png", "backs/bats_1.png"]),
                None,
                None,
                Some(vec![100, 200]),
                None,
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Frames {
                images: vec![
                    asset_path("backs/bats_0.png"),
                    asset_path("backs/bats_1.png")
                ],
                timing: BackTiming::DurationsMs(vec![100, 200]),
            }
        );
    }

    // -- BackDef: invalid field combinations --

    #[test]
    fn rejects_fps_without_frames() {
        let error = validate(&name(), raw(single("p.png"), None, Some(2), None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsWithoutFrames { back } if back == name()
        ));
    }

    #[test]
    fn rejects_frames_without_fps() {
        let error = validate(&name(), raw(single("p.png"), Some(4), None, None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFramesWithoutTiming { back } if back == name()
        ));
    }

    #[test]
    fn rejects_layout_alone_without_frames_or_fps() {
        let error = validate(
            &name(),
            raw(single("p.png"), None, None, None, Some("vertical")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackLayoutWithoutStrip { back } if back == name()
        ));
    }

    #[test]
    fn rejects_frames_below_two() {
        let error =
            validate(&name(), raw(single("p.png"), Some(1), Some(2), None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewFrames { back, frames: 1 } if back == name()
        ));
    }

    #[test]
    fn rejects_zero_fps_on_a_strip() {
        let error =
            validate(&name(), raw(single("p.png"), Some(4), Some(0), None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroFps { back } if back == name()
        ));
    }

    #[test]
    fn rejects_an_unrecognized_layout_value() {
        let error = validate(
            &name(),
            raw(single("p.png"), Some(4), Some(2), None, Some("diagonal")),
        )
        .unwrap_err();
        assert!(matches!(
            &error,
            ManifestError::BackInvalidLayout { back, value }
                if *back == name() && value == "diagonal"
        ));
    }

    #[test]
    fn rejects_a_list_of_fewer_than_two_images() {
        let error = validate(
            &name(),
            raw(multiple(&["only.png"]), None, Some(2), None, None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewListImages { back, count: 1 } if back == name()
        ));
    }

    #[test]
    fn rejects_a_list_with_frames_present() {
        let error = validate(
            &name(),
            raw(multiple(&["a.png", "b.png"]), Some(2), Some(2), None, None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackListWithFramesOrLayout { back } if back == name()
        ));
    }

    #[test]
    fn rejects_a_list_with_layout_present() {
        let error = validate(
            &name(),
            raw(
                multiple(&["a.png", "b.png"]),
                None,
                Some(2),
                None,
                Some("vertical"),
            ),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackListWithFramesOrLayout { back } if back == name()
        ));
    }

    #[test]
    fn rejects_a_list_missing_fps() {
        let error = validate(
            &name(),
            raw(multiple(&["a.png", "b.png"]), None, None, None, None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroFps { back } if back == name()
        ));
    }

    #[test]
    fn rejects_a_list_with_zero_fps() {
        let error = validate(
            &name(),
            raw(multiple(&["a.png", "b.png"]), None, Some(0), None, None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroFps { back } if back == name()
        ));
    }

    // -- BackDef: durations_ms (per-frame timing) --

    #[test]
    fn rejects_fps_and_durations_ms_both_present() {
        let error = validate(
            &name(),
            raw(
                single("backs/palm.png"),
                Some(4),
                Some(2),
                Some(vec![250, 250, 250, 49_250]),
                None,
            ),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsAndDurations { back } if back == name()
        ));
    }

    #[test]
    fn rejects_durations_ms_without_frames() {
        let error = validate(
            &name(),
            raw(single("p.png"), None, None, Some(vec![250]), None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackDurationsWithoutFrames { back } if back == name()
        ));
    }

    #[test]
    fn rejects_a_durations_ms_length_mismatch() {
        let error = validate(
            &name(),
            raw(
                single("backs/palm.png"),
                Some(4),
                None,
                Some(vec![250, 250, 250]),
                None,
            ),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackDurationsLengthMismatch {
                back,
                expected: 4,
                got: 3,
            } if back == name()
        ));
    }

    #[test]
    fn rejects_a_zero_duration() {
        let error = validate(
            &name(),
            raw(
                single("backs/palm.png"),
                Some(4),
                None,
                Some(vec![250, 250, 250, 0]),
                None,
            ),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroDuration { back } if back == name()
        ));
    }

    #[test]
    fn rejects_a_duration_too_large_for_u32() {
        let overflow = i64::from(u32::MAX) + 1;
        let error = validate(
            &name(),
            raw(
                single("backs/palm.png"),
                Some(4),
                None,
                Some(vec![250, 250, 250, overflow]),
                None,
            ),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackDurationTooLarge { back, value }
                if back == name() && value == overflow
        ));
    }

    // -- path hygiene --

    #[test]
    fn rejects_an_absolute_image_path() {
        let error =
            validate(&name(), raw(single("/etc/passwd"), None, None, None, None)).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn rejects_a_parent_segment_in_an_image_path() {
        let error = validate(
            &name(),
            raw(single("../secret.png"), None, None, None, None),
        )
        .unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn rejects_a_backslash_in_an_image_path() {
        let error = validate(
            &name(),
            raw(single("backs\\robot.png"), None, None, None, None),
        )
        .unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }
}
