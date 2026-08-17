//! [`RenderMode`]: a theme's art form — `[theme] render_mode` in the
//! manifest.

use serde::{Deserialize, Serialize};

/// A theme's art form (`[theme] render_mode`).
///
/// Serializes and parses as exactly `"png"` or `"vector"`. How PNG art is
/// *scaled* is not declared here — that is the player's choice, carried by
/// [`crate::CardScaling`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderMode {
    /// Raster card art in PNG files.
    Png,
    /// Scalable vector art (SVG faces and backs).
    Vector,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// TOML documents are always tables at the root, so a bare enum value
    /// is wrapped in a single-field struct to exercise (de)serialization —
    /// exactly how `RenderMode` is actually used, as `[theme] render_mode`.
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Wrapper {
        render_mode: RenderMode,
    }

    #[test]
    fn serializes_as_exactly_the_two_lowercase_strings() {
        let text = |mode| toml::to_string(&Wrapper { render_mode: mode }).unwrap();
        assert_eq!(text(RenderMode::Png), "render_mode = \"png\"\n");
        assert_eq!(text(RenderMode::Vector), "render_mode = \"vector\"\n");
    }

    #[test]
    fn parses_from_exactly_the_two_lowercase_strings() {
        let parse = |text| toml::from_str::<Wrapper>(text).unwrap().render_mode;
        assert_eq!(parse("render_mode = \"png\""), RenderMode::Png);
        assert_eq!(parse("render_mode = \"vector\""), RenderMode::Vector);
    }

    #[test]
    fn rejects_unknown_strings() {
        assert!(toml::from_str::<Wrapper>("render_mode = \"raytraced\"").is_err());
        assert!(toml::from_str::<Wrapper>("render_mode = \"Png\"").is_err());
    }

    /// The two retired spellings are rejected rather than aliased, and the
    /// error names what to write instead.
    #[test]
    fn the_retired_spellings_are_rejected_by_name() {
        for retired in ["pixel", "xbrz"] {
            let error = toml::from_str::<Wrapper>(&format!("render_mode = \"{retired}\""))
                .unwrap_err()
                .to_string();
            assert!(error.contains(retired), "{error}");
            assert!(error.contains("png"), "{error}");
        }
    }
}
