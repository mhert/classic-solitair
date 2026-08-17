//! Settings format v1: the persisted user-settings document
//! (`settings.json`), platform-free — this module touches no filesystem at
//! all; the platform I/O layer that reads and writes it lives outside this
//! module — the same format/storage split `save.rs` and `storage.rs` use.
//!
//! [`Settings`] is authoritative for user settings at startup: frontends
//! restore from it and rewrite it on every settings commit. Save-format v1
//! (`SaveGame`) embeds the same [`Options`] type, but that copy is a frozen
//! snapshot of whatever game state was current when a game was last saved —
//! never the live settings source of truth.

use serde::{Deserialize, Serialize};

use crate::options::Options;

/// The settings format version this build writes, and the only version it
/// accepts on load. Bumping it is a breaking format change, versioned
/// independently of the save format's own `FORMAT_VERSION`
/// (`crate::save::FORMAT_VERSION`).
pub const FORMAT_VERSION: u32 = 1;

/// The main window's restored geometry: its non-maximized size, optional
/// position, and maximized flag.
///
/// `x` and `y` are absent from the JSON — never `null` — when `None`;
/// platforms like Wayland expose no window position at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowGeometry {
    /// The restored (non-maximized) width, logical pixels.
    pub width: u32,
    /// The restored (non-maximized) height, logical pixels.
    pub height: u32,
    /// The restored horizontal position, or `None` where the platform
    /// exposes no window position. Absent from the JSON — never `null` —
    /// when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    /// The restored vertical position, or `None` where the platform
    /// exposes no window position. Absent from the JSON — never `null` —
    /// when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Whether the window was maximized.
    pub maximized: bool,
}

/// A persisted user-settings document (`settings.json`).
///
/// Strict-parse philosophy: [`Settings::from_bytes`] rejects any unknown or
/// missing required field as [`SettingsError::Malformed`] — there is no
/// per-field tolerance anywhere in this type. A caller that must survive a
/// corrupt or foreign document falls back to the whole
/// [`Settings::default()`], never to patching individual fields; the
/// `Option` fields (`window`, and within it `x`/`y`) are the only
/// sanctioned-absent shape.
///
/// ```
/// use sol_session::{Settings, WindowGeometry};
///
/// let settings = Settings {
///     back_index: 2,
///     window: Some(WindowGeometry {
///         width: 800,
///         height: 600,
///         x: Some(10),
///         y: Some(20),
///         maximized: false,
///     }),
///     ..Settings::default()
/// };
/// let bytes = settings.to_bytes()?;
/// assert_eq!(Settings::from_bytes(&bytes)?, settings);
/// # Ok::<(), sol_session::SettingsError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The settings format version this document was written as.
    pub format_version: u32,
    /// The player's cross-game options: the single source of truth for the
    /// option fields, reused as-is ([`Options`]'s own strict serde shape
    /// applies here too).
    pub options: Options,
    /// The presenter's index into the active theme's declared card backs.
    pub back_index: usize,
    /// The main window's restored geometry, or `None` when no geometry has
    /// been recorded yet, or on a platform that exposes no window position
    /// (for example Wayland). Absent from the JSON — never `null` — when
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowGeometry>,
}

impl Default for Settings {
    /// [`FORMAT_VERSION`], [`Options::default()`], back index 0, no
    /// recorded window geometry.
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            options: Options::default(),
            back_index: 0,
            window: None,
        }
    }
}

impl Settings {
    /// Serializes this document to its canonical pretty-printed JSON bytes
    /// (`serde_json::to_vec_pretty`) — the file is meant to be
    /// human-readable and hand-editable.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::Malformed`] if serialization fails —
    /// unreachable in practice, since every field of `Settings` serializes
    /// infallibly.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SettingsError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Parses `bytes` as a settings document, two-phase: phase 1 probes
    /// only `format_version`, via a private struct that tolerates any other
    /// shape, so an unsupported version is reported as
    /// [`SettingsError::UnsupportedFormatVersion`] even when the rest of
    /// the document doesn't parse as version 1 at all — the typed
    /// rejection always wins over a shape error a foreign version would
    /// otherwise cause. Only once the version is confirmed does phase 2 run
    /// the full typed parse.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::UnsupportedFormatVersion`] when
    /// `format_version` parses but is not [`FORMAT_VERSION`]. Returns
    /// [`SettingsError::Malformed`] for anything else that keeps `bytes`
    /// from parsing as a version-1 [`Settings`] document — invalid JSON, a
    /// missing or wrongly-typed `format_version`, a missing required
    /// field, or an unknown field.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SettingsError> {
        let probe: FormatProbe = serde_json::from_slice(bytes)?;
        if probe.format_version != FORMAT_VERSION {
            return Err(SettingsError::UnsupportedFormatVersion {
                found: probe.format_version,
            });
        }
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// The phase-1 version probe: reads only `format_version`; every other
/// field — present, absent, or malformed — is tolerated, so this alone can
/// never produce a shape error.
#[derive(Deserialize)]
struct FormatProbe {
    format_version: u32,
}

/// Errors from [`Settings::from_bytes`] (and, in principle,
/// [`Settings::to_bytes`], though every field of [`Settings`] serializes
/// infallibly).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SettingsError {
    /// The document did not parse as a version-1 [`Settings`] — invalid
    /// JSON, a missing or extra field, or a field of the wrong shape.
    #[error("malformed settings data: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The document's `format_version` was read successfully but is not one
    /// this build can load.
    #[error("unsupported settings format version {found}")]
    UnsupportedFormatVersion {
        /// The `format_version` found in the document.
        found: u32,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn non_default_options() -> Options {
        Options {
            timed: false,
            outline_dragging: true,
            keep_vegas_score: true,
            sounds: false,
            ..Options::default()
        }
    }

    fn non_default_settings() -> Settings {
        Settings {
            format_version: FORMAT_VERSION,
            options: non_default_options(),
            back_index: 3,
            window: Some(WindowGeometry {
                width: 1024,
                height: 768,
                x: Some(50),
                y: Some(60),
                maximized: true,
            }),
        }
    }

    #[test]
    fn default_settings_pins_every_field() {
        let settings = Settings::default();
        assert_eq!(settings.format_version, FORMAT_VERSION);
        assert_eq!(settings.options, Options::default());
        assert_eq!(settings.back_index, 0);
        assert_eq!(settings.window, None);
    }

    #[test]
    fn round_trips_through_bytes_and_reserializes_byte_identically() {
        let settings = non_default_settings();
        let bytes = settings.to_bytes().unwrap();
        let parsed = Settings::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, settings);

        let bytes_again = parsed.to_bytes().unwrap();
        assert_eq!(
            bytes_again, bytes,
            "reserializing a parsed document is byte-identical"
        );
    }

    #[test]
    fn to_bytes_is_pretty_printed_json() {
        let bytes = non_default_settings().to_bytes().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("{\n"), "pretty-printed, not compact");
        assert!(text.contains("\"format_version\": 1"));
    }

    #[test]
    fn default_settings_serde_shape_locks_top_level_keys_and_omits_window() {
        let value = serde_json::to_value(Settings::default()).unwrap();
        let object = value.as_object().unwrap();

        let mut actual_keys: Vec<&str> = object.keys().map(String::as_str).collect();
        actual_keys.sort_unstable();
        let mut expected_keys = ["format_version", "options", "back_index"];
        expected_keys.sort_unstable();
        assert_eq!(
            actual_keys, expected_keys,
            "window is absent, not null, for a default document"
        );
    }

    // The key names are the on-disk format: renaming any field silently
    // makes every existing settings.json unreadable (the strict parse
    // rejects the old name as unknown), so they are pinned here.
    #[test]
    fn populated_settings_serde_shape_locks_every_key_name() {
        let value = serde_json::to_value(non_default_settings()).unwrap();
        let object = value.as_object().unwrap();

        let mut actual_keys: Vec<&str> = object.keys().map(String::as_str).collect();
        actual_keys.sort_unstable();
        let mut expected_keys = ["format_version", "options", "back_index", "window"];
        expected_keys.sort_unstable();
        assert_eq!(actual_keys, expected_keys, "top-level keys");

        let window = object.get("window").unwrap().as_object().unwrap();
        let mut actual_window_keys: Vec<&str> = window.keys().map(String::as_str).collect();
        actual_window_keys.sort_unstable();
        let mut expected_window_keys = ["width", "height", "x", "y", "maximized"];
        expected_window_keys.sort_unstable();
        assert_eq!(actual_window_keys, expected_window_keys, "geometry keys");
    }

    #[test]
    fn window_geometry_omits_x_and_y_keys_when_none() {
        let geometry = WindowGeometry {
            width: 800,
            height: 600,
            x: None,
            y: None,
            maximized: false,
        };
        let value = serde_json::to_value(geometry).unwrap();
        let object = value.as_object().unwrap();

        let mut actual_keys: Vec<&str> = object.keys().map(String::as_str).collect();
        actual_keys.sort_unstable();
        let mut expected_keys = ["width", "height", "maximized"];
        expected_keys.sort_unstable();
        assert_eq!(actual_keys, expected_keys);
    }

    #[test]
    fn parses_a_document_without_a_window_field() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("window");
        let bytes = serde_json::to_vec(&value).unwrap();

        let parsed = Settings::from_bytes(&bytes).unwrap();

        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn parses_window_geometry_without_x_or_y() {
        let settings = Settings {
            window: Some(WindowGeometry {
                width: 640,
                height: 480,
                x: None,
                y: None,
                maximized: false,
            }),
            ..Settings::default()
        };
        let bytes = settings.to_bytes().unwrap();

        let parsed = Settings::from_bytes(&bytes).unwrap();

        assert_eq!(parsed, settings);
    }

    #[test]
    fn rejects_an_unknown_top_level_field_as_malformed() {
        let mut value = serde_json::to_value(non_default_settings()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".to_owned(), serde_json::json!(1));
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = Settings::from_bytes(&bytes).unwrap_err();

        assert!(matches!(error, SettingsError::Malformed(_)));
    }

    #[test]
    fn rejects_an_unknown_field_inside_window_geometry_as_malformed() {
        let mut value = serde_json::to_value(non_default_settings()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .get_mut("window")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".to_owned(), serde_json::json!(1));
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = Settings::from_bytes(&bytes).unwrap_err();

        assert!(matches!(error, SettingsError::Malformed(_)));
    }

    #[test]
    fn rejects_a_missing_required_field_as_malformed() {
        let mut value = serde_json::to_value(non_default_settings()).unwrap();
        value.as_object_mut().unwrap().remove("back_index");
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = Settings::from_bytes(&bytes).unwrap_err();

        assert!(matches!(error, SettingsError::Malformed(_)));
    }

    #[test]
    fn rejects_an_unsupported_format_version_even_with_a_garbage_shape() {
        let mut value = serde_json::to_value(non_default_settings()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("format_version".to_owned(), serde_json::json!(99));
        // Wreck everything else: the probe must not care.
        object.insert("options".to_owned(), serde_json::json!("not an object"));
        object.remove("back_index");
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = Settings::from_bytes(&bytes).unwrap_err();

        assert!(matches!(
            error,
            SettingsError::UnsupportedFormatVersion { found: 99 }
        ));
    }

    #[test]
    fn rejects_non_json_bytes_as_malformed() {
        let error = Settings::from_bytes(b"not json at all").unwrap_err();
        assert!(matches!(error, SettingsError::Malformed(_)));
    }

    #[test]
    fn rejects_a_json_array_as_malformed() {
        let error = Settings::from_bytes(b"[1,2,3]").unwrap_err();
        assert!(matches!(error, SettingsError::Malformed(_)));
    }

    #[test]
    fn malformed_display_matches_exact_string() {
        let inner = serde_json::from_str::<Settings>("not json").unwrap_err();
        let inner_display = inner.to_string();

        let error = SettingsError::Malformed(inner);

        assert_eq!(
            error.to_string(),
            format!("malformed settings data: {inner_display}")
        );
    }

    #[test]
    fn unsupported_format_version_display_matches_exact_string() {
        let error = SettingsError::UnsupportedFormatVersion { found: 42 };
        assert_eq!(error.to_string(), "unsupported settings format version 42");
    }
}
