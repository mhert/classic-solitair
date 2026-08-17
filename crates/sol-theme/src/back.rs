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
        /// Playback rate in frames per second (at least 1).
        fps: u32,
        /// Which axis the frames are laid out along.
        layout: BackLayout,
    },
    /// An animated back stored as a list of per-frame images.
    Frames {
        /// Validated theme-package-relative paths, one per frame (at least 2).
        images: Vec<RelativeAssetPath>,
        /// Playback rate in frames per second (at least 1).
        fps: u32,
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
        layout,
    } = raw;
    match image {
        RawImage::Single(image) => validate_single(name, &image, frames, fps, layout),
        RawImage::Multiple(images) => {
            validate_multiple(name, &images, frames, fps, layout.is_some())
        }
    }
}

fn validate_single(
    name: &BackName,
    image: &str,
    frames: Option<i64>,
    fps: Option<i64>,
    layout: Option<String>,
) -> Result<BackDef, ManifestError> {
    match (frames, fps) {
        (Some(frames), Some(fps)) => {
            let frames = match u32::try_from(frames) {
                Ok(f) if f >= 2 => f,
                Ok(_) => {
                    return Err(ManifestError::BackTooFewFrames {
                        back: name.clone(),
                        frames,
                    });
                }
                Err(_) => {
                    return Err(ManifestError::BackFramesTooLarge {
                        back: name.clone(),
                        frames,
                    });
                }
            };
            let fps = match u32::try_from(fps) {
                Ok(f) if f >= 1 => f,
                Ok(_) => {
                    return Err(ManifestError::BackZeroFps { back: name.clone() });
                }
                Err(_) => {
                    return Err(ManifestError::BackFpsTooLarge {
                        back: name.clone(),
                        fps,
                    });
                }
            };
            let layout = match layout {
                None => BackLayout::default(),
                Some(value) if value == "horizontal" => BackLayout::Horizontal,
                Some(value) if value == "vertical" => BackLayout::Vertical,
                Some(value) => {
                    return Err(ManifestError::BackInvalidLayout {
                        back: name.clone(),
                        value,
                    });
                }
            };
            let image = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
            Ok(BackDef::Strip {
                image,
                frames,
                fps,
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
        (Some(_), None) => Err(ManifestError::BackFramesWithoutFps { back: name.clone() }),
        (None, Some(_)) => Err(ManifestError::BackFpsWithoutFrames { back: name.clone() }),
    }
}

fn validate_multiple(
    name: &BackName,
    images: &[String],
    frames: Option<i64>,
    fps: Option<i64>,
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
    let fps = match fps {
        Some(value) => match u32::try_from(value) {
            Ok(f) if f >= 1 => f,
            Ok(_) => {
                return Err(ManifestError::BackZeroFps { back: name.clone() });
            }
            Err(_) => {
                return Err(ManifestError::BackFpsTooLarge {
                    back: name.clone(),
                    fps: value,
                });
            }
        },
        None => {
            return Err(ManifestError::BackZeroFps { back: name.clone() });
        }
    };
    let mut parsed = Vec::with_capacity(images.len());
    for image in images {
        let path = RelativeAssetPath::parse(format!("back `{name}` image"), image)?;
        parsed.push(path);
    }
    Ok(BackDef::Frames {
        images: parsed,
        fps,
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
        layout: Option<&str>,
    ) -> RawBackDef {
        RawBackDef {
            image,
            frames,
            fps,
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
        let def = validate(&name(), raw(single("backs/plain.png"), None, None, None)).unwrap();
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
            raw(single("backs/robot.png"), Some(4), Some(2), None),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 4,
                fps: 2,
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
                Some("vertical"),
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 3,
                fps: 1,
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
                Some("horizontal"),
            ),
        )
        .unwrap();
        assert_eq!(
            def,
            BackDef::Strip {
                image: asset_path("backs/robot.png"),
                frames: 3,
                fps: 1,
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
                fps: 3,
            }
        );
    }

    // -- BackDef: invalid field combinations --

    #[test]
    fn rejects_fps_without_frames() {
        let error = validate(&name(), raw(single("p.png"), None, Some(2), None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFpsWithoutFrames { back } if back == name()
        ));
    }

    #[test]
    fn rejects_frames_without_fps() {
        let error = validate(&name(), raw(single("p.png"), Some(4), None, None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackFramesWithoutFps { back } if back == name()
        ));
    }

    #[test]
    fn rejects_layout_alone_without_frames_or_fps() {
        let error =
            validate(&name(), raw(single("p.png"), None, None, Some("vertical"))).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackLayoutWithoutStrip { back } if back == name()
        ));
    }

    #[test]
    fn rejects_frames_below_two() {
        let error = validate(&name(), raw(single("p.png"), Some(1), Some(2), None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewFrames { back, frames: 1 } if back == name()
        ));
    }

    #[test]
    fn rejects_zero_fps_on_a_strip() {
        let error = validate(&name(), raw(single("p.png"), Some(4), Some(0), None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroFps { back } if back == name()
        ));
    }

    #[test]
    fn rejects_an_unrecognized_layout_value() {
        let error = validate(
            &name(),
            raw(single("p.png"), Some(4), Some(2), Some("diagonal")),
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
        let error =
            validate(&name(), raw(multiple(&["only.png"]), None, Some(2), None)).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackTooFewListImages { back, count: 1 } if back == name()
        ));
    }

    #[test]
    fn rejects_a_list_with_frames_present() {
        let error = validate(
            &name(),
            raw(multiple(&["a.png", "b.png"]), Some(2), Some(2), None),
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
            raw(multiple(&["a.png", "b.png"]), None, None, None),
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
            raw(multiple(&["a.png", "b.png"]), None, Some(0), None),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::BackZeroFps { back } if back == name()
        ));
    }

    // -- path hygiene --

    #[test]
    fn rejects_an_absolute_image_path() {
        let error = validate(&name(), raw(single("/etc/passwd"), None, None, None)).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn rejects_a_parent_segment_in_an_image_path() {
        let error = validate(&name(), raw(single("../secret.png"), None, None, None)).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }

    #[test]
    fn rejects_a_backslash_in_an_image_path() {
        let error =
            validate(&name(), raw(single("backs\\robot.png"), None, None, None)).unwrap_err();
        assert!(matches!(error, ManifestError::InvalidPath { .. }));
    }
}
