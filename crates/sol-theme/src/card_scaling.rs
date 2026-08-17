//! [`CardScaling`]: how PNG card art is scaled up.

use serde::{Deserialize, Serialize};

/// How a PNG theme's card art is scaled up, chosen by the player rather
/// than declared by the theme.
///
/// Serializes and parses as exactly `"original"` or `"xbrz"`. Vector themes
/// scale by rasterizing their SVGs and ignore this entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardScaling {
    /// The theme's pixels as authored, scaled by the renderer's pixel-art
    /// sampling. The default.
    #[default]
    Original,
    /// Upscaled through xBRZ before it reaches the atlas.
    Xbrz,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Wrapper {
        scaling: CardScaling,
    }

    #[test]
    fn serializes_as_exactly_the_two_lowercase_strings() {
        let text = |scaling| toml::to_string(&Wrapper { scaling }).unwrap();
        assert_eq!(text(CardScaling::Original), "scaling = \"original\"\n");
        assert_eq!(text(CardScaling::Xbrz), "scaling = \"xbrz\"\n");
    }

    #[test]
    fn parses_from_exactly_the_two_lowercase_strings() {
        let parse = |text| toml::from_str::<Wrapper>(text).unwrap().scaling;
        assert_eq!(parse("scaling = \"original\""), CardScaling::Original);
        assert_eq!(parse("scaling = \"xbrz\""), CardScaling::Xbrz);
    }

    /// The value is persisted as JSON in the settings document, so the
    /// same two spellings have to survive that encoder too.
    #[test]
    fn round_trips_through_json() {
        for scaling in [CardScaling::Original, CardScaling::Xbrz] {
            let json = serde_json::to_string(&scaling).unwrap();
            assert_eq!(serde_json::from_str::<CardScaling>(&json).unwrap(), scaling);
        }
        assert_eq!(
            serde_json::to_string(&CardScaling::Xbrz).unwrap(),
            "\"xbrz\""
        );
    }

    #[test]
    fn rejects_unknown_strings() {
        assert!(toml::from_str::<Wrapper>("scaling = \"hq4x\"").is_err());
        assert!(toml::from_str::<Wrapper>("scaling = \"Original\"").is_err());
    }

    #[test]
    fn defaults_to_original() {
        assert_eq!(CardScaling::default(), CardScaling::Original);
    }
}
