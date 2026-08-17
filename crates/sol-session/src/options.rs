//! Player-controlled cross-game settings: draw mode, scoring mode,
//! timed play, outline dragging, keep-Vegas-score, theme, and sounds.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use sol_engine::{DrawMode, GameConfig, ScoringMode};

/// A theme identifier (e.g. `"default"`, `"winter"`), newtyped over `String`.
///
/// The identifier text is stored as-is; the only enforced invariant is
/// non-emptiness — no case folding, trimming, or character validation.
/// Serializes as a plain JSON string; deserialization enforces the same
/// non-empty invariant as the constructors.
///
/// Orders lexicographically on the identifier text, so a `BTreeMap<ThemeId,
/// _>` — such as [`crate::Settings`]'s per-theme scaling choices — writes its
/// keys in a stable, deterministic order.
///
/// ```
/// use sol_session::ThemeId;
///
/// let theme: ThemeId = "winter".parse()?;
/// assert_eq!(theme.as_str(), "winter");
/// assert_eq!(theme.to_string(), "winter");
///
/// assert!("".parse::<ThemeId>().is_err());
/// # Ok::<(), sol_session::ThemeIdError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ThemeId(String);

/// [`ThemeId`] cannot be built from an empty string — the only enforced
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("theme id must not be empty")]
pub struct ThemeIdError;

impl ThemeId {
    /// The identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ThemeId {
    type Error = ThemeIdError;

    /// # Errors
    ///
    /// Returns [`ThemeIdError`] if `value` is empty.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(ThemeIdError)
        } else {
            Ok(Self(value))
        }
    }
}

impl FromStr for ThemeId {
    type Err = ThemeIdError;

    /// # Errors
    ///
    /// Returns [`ThemeIdError`] if `s` is empty.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for ThemeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ThemeId> for String {
    fn from(theme: ThemeId) -> Self {
        theme.0
    }
}

/// Player-controlled cross-game settings.
///
/// This is exactly the save-format v1 `options` object: field names and
/// their declaration order are locked forever — adding, removing, renaming,
/// or reordering a field is a breaking save-format change. Only
/// `draw_mode`, `scoring`, and `timed` are engine-relevant
/// ([`Options::game_config`] extracts them at deal time); the rest govern
/// the session or frontend layer and are never interpreted by the engine.
// Four bools is inherent to the save-format v1 field set (locked by the
// committed byte-exact fixture); splitting them into enums would break the
// pinned serde shape, so the pedantic suggestion does not apply here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Cards turned per draw: One or Three.
    pub draw_mode: DrawMode,
    /// Standard, Vegas, or None scoring.
    pub scoring: ScoringMode,
    /// Whether the game clock runs (Standard scoring only). The
    /// session clock always runs regardless of this flag.
    pub timed: bool,
    /// Whether a dragged card shows an outline instead of the full image.
    pub outline_dragging: bool,
    /// Whether ending a Vegas game folds its net result into the bankroll
    /// instead of resetting the bankroll to zero.
    pub keep_vegas_score: bool,
    /// The active card/table theme.
    pub theme: ThemeId,
    /// Whether sound effects play.
    pub sounds: bool,
}

impl Default for Options {
    /// The Win98-faithful defaults: Draw Three, Standard scoring, timed on,
    /// outline dragging off, Vegas score not kept across deals, the
    /// `"default"` theme, sounds on.
    fn default() -> Self {
        Self {
            draw_mode: DrawMode::Three,
            scoring: ScoringMode::Standard,
            timed: true,
            outline_dragging: false,
            keep_vegas_score: false,
            // "default" is a compile-time-known non-empty literal, so
            // constructing it directly (rather than through the fallible
            // `TryFrom`) cannot violate the invariant.
            theme: ThemeId(String::from("default")),
            sounds: true,
        }
    }
}

impl Options {
    /// Derives the engine's fixed per-game configuration from these options.
    ///
    /// Only the three engine-relevant fields are mapped; `outline_dragging`,
    /// `keep_vegas_score`, `theme`, and `sounds` are session/frontend
    /// concerns the engine never sees.
    ///
    /// ```
    /// use sol_engine::{DrawMode, ScoringMode};
    /// use sol_session::Options;
    ///
    /// let config = Options::default().game_config();
    /// assert_eq!(config.draw_mode, DrawMode::Three);
    /// assert_eq!(config.scoring, ScoringMode::Standard);
    /// assert!(config.timed);
    /// ```
    #[must_use]
    pub fn game_config(&self) -> GameConfig {
        GameConfig {
            draw_mode: self.draw_mode,
            scoring: self.scoring,
            timed: self.timed,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn theme_id_builds_via_try_from_and_from_str() {
        let via_try_from = ThemeId::try_from(String::from("winter")).unwrap();
        let via_from_str: ThemeId = "winter".parse().unwrap();
        assert_eq!(via_try_from, via_from_str);
        assert_eq!(via_try_from.as_str(), "winter");
    }

    #[test]
    fn theme_id_try_from_rejects_empty_string() {
        assert_eq!(ThemeId::try_from(String::new()), Err(ThemeIdError));
    }

    #[test]
    fn theme_id_from_str_rejects_empty_string() {
        assert_eq!("".parse::<ThemeId>(), Err(ThemeIdError));
    }

    #[test]
    fn theme_id_display_matches_as_str() {
        let theme = ThemeId::try_from(String::from("winter")).unwrap();
        assert_eq!(theme.to_string(), "winter");
        assert_eq!(theme.as_str(), "winter");
    }

    #[test]
    fn theme_id_serializes_as_plain_json_string() {
        let theme = ThemeId::try_from(String::from("winter")).unwrap();
        assert_eq!(serde_json::to_string(&theme).unwrap(), "\"winter\"");
        assert_eq!(
            serde_json::from_str::<ThemeId>("\"winter\"").unwrap(),
            theme
        );
    }

    #[test]
    fn theme_id_deserialize_rejects_empty_string() {
        let result: Result<ThemeId, _> = serde_json::from_str("\"\"");
        assert!(result.is_err());
    }

    #[test]
    fn default_options_match_win98_pinned_values() {
        let options = Options::default();
        assert_eq!(options.draw_mode, DrawMode::Three);
        assert_eq!(options.scoring, ScoringMode::Standard);
        assert!(options.timed);
        assert!(!options.outline_dragging);
        assert!(!options.keep_vegas_score);
        assert_eq!(options.theme.as_str(), "default");
        assert!(options.sounds);
    }

    fn non_default_options() -> Options {
        Options {
            draw_mode: DrawMode::One,
            scoring: ScoringMode::Vegas,
            timed: false,
            outline_dragging: true,
            keep_vegas_score: true,
            theme: ThemeId::try_from(String::from("winter")).unwrap(),
            sounds: false,
        }
    }

    #[test]
    fn options_serde_round_trips() {
        let options = non_default_options();
        let json = serde_json::to_string(&options).unwrap();
        let round_tripped: Options = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, options);
    }

    #[test]
    fn options_serde_shape_locks_field_names_and_engine_representations() {
        let options = non_default_options();
        let value = serde_json::to_value(&options).unwrap();
        let object = value.as_object().unwrap();

        let mut actual_keys: Vec<&str> = object.keys().map(String::as_str).collect();
        actual_keys.sort_unstable();
        let mut expected_keys = [
            "draw_mode",
            "keep_vegas_score",
            "outline_dragging",
            "scoring",
            "sounds",
            "theme",
            "timed",
        ];
        expected_keys.sort_unstable();
        assert_eq!(actual_keys, expected_keys);

        assert_eq!(object.get("draw_mode"), Some(&serde_json::json!("One")));
        assert_eq!(object.get("scoring"), Some(&serde_json::json!("Vegas")));
        assert_eq!(object.get("theme"), Some(&serde_json::json!("winter")));
    }

    #[test]
    fn options_deserialize_rejects_unknown_field() {
        let mut value = serde_json::to_value(non_default_options()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".to_owned(), serde_json::json!(1));

        let result: Result<Options, _> = serde_json::from_value(value);
        assert!(result.is_err());
    }

    #[test]
    fn game_config_maps_draw_mode_scoring_and_timed_faithfully() {
        let cases = [
            (DrawMode::One, ScoringMode::Standard, true),
            (DrawMode::Three, ScoringMode::Standard, false),
            (DrawMode::One, ScoringMode::Vegas, true),
            (DrawMode::Three, ScoringMode::None, false),
        ];

        for (draw_mode, scoring, timed) in cases {
            let options = Options {
                draw_mode,
                scoring,
                timed,
                ..Options::default()
            };

            let config = options.game_config();
            assert_eq!(config.draw_mode, draw_mode);
            assert_eq!(config.scoring, scoring);
            assert_eq!(config.timed, timed);
        }
    }
}
