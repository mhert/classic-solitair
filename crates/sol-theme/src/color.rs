//! [`Color`]: the `"#rrggbb"` RGB color used by `[table] background.color`
//! and `[drag] outline_color`.

use core::fmt;
use core::str::FromStr;

/// An RGB color, parsed from and displayed as `"#rrggbb"` (`[table]
/// background.color`, `[drag] outline_color`).
///
/// Parsing accepts either case for the hex digits; [`fmt::Display`] always
/// renders lowercase, so `parse -> to_string -> parse` is an identity no
/// matter which case the input used.
///
/// ```
/// use sol_theme::Color;
///
/// let color: Color = "#008000".parse()?;
/// assert_eq!(color.to_string(), "#008000");
/// assert_eq!("#ABCDEF".parse::<Color>()?.to_string(), "#abcdef");
/// # Ok::<(), sol_theme::ColorError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Color {
    /// Builds a color directly from its RGB channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// [`Color`] could not be parsed: the input was not exactly `"#rrggbb"`
/// (a `#` followed by six case-insensitive hex digits).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid color {raw:?}: expected \"#rrggbb\" (a `#` followed by six hex digits)")]
pub struct ColorError {
    raw: String,
}

impl FromStr for Color {
    type Err = ColorError;

    /// # Errors
    ///
    /// Returns [`ColorError`] unless `s` is exactly 7 bytes: a leading `#`
    /// followed by six case-insensitive hex digits.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || ColorError { raw: s.to_owned() };
        let digits = s.strip_prefix('#').ok_or_else(invalid)?;
        if digits.len() != 6 {
            return Err(invalid());
        }
        let byte = |range| {
            digits
                .get(range)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        };
        let r = byte(0..2).ok_or_else(invalid)?;
        let g = byte(2..4).ok_or_else(invalid)?;
        let b = byte(4..6).ok_or_else(invalid)?;
        Ok(Self { r, g, b })
    }
}

impl TryFrom<&str> for Color {
    type Error = ColorError;

    /// # Errors
    ///
    /// Returns [`ColorError`] under the same conditions as [`Color::from_str`].
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use core::str::FromStr;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn parses_uppercase_and_lowercase_hex() {
        assert_eq!(
            Color::from_str("#008000").unwrap(),
            Color::new(0x00, 0x80, 0x00)
        );
        assert_eq!(
            Color::from_str("#ABCDEF").unwrap(),
            Color::new(0xAB, 0xCD, 0xEF)
        );
        assert_eq!(
            Color::from_str("#abcdef").unwrap(),
            Color::new(0xAB, 0xCD, 0xEF)
        );
    }

    #[test]
    fn try_from_str_matches_from_str() {
        assert_eq!(
            Color::try_from("#000000").unwrap(),
            Color::from_str("#000000").unwrap()
        );
    }

    #[test]
    fn display_is_always_lowercase_with_hash() {
        assert_eq!(Color::new(0x00, 0x80, 0x00).to_string(), "#008000");
        assert_eq!(Color::new(0xAB, 0xCD, 0xEF).to_string(), "#abcdef");
        assert_eq!(Color::new(0xFF, 0xFF, 0xFF).to_string(), "#ffffff");
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(Color::from_str("#08000").is_err());
        assert!(Color::from_str("#0080000").is_err());
        assert!(Color::from_str("").is_err());
    }

    #[test]
    fn rejects_missing_hash() {
        assert!(Color::from_str("008000").is_err());
    }

    #[test]
    fn rejects_non_hex_digits() {
        assert!(Color::from_str("#00800g").is_err());
        assert!(Color::from_str("#gggggg").is_err());
    }

    proptest! {
        #[test]
        fn round_trip_parse_display_parse_is_identity(r in any::<u8>(), g in any::<u8>(), b in any::<u8>()) {
            let color = Color::new(r, g, b);
            let text = color.to_string();
            let parsed = Color::from_str(&text).unwrap();
            prop_assert_eq!(parsed, color);
            prop_assert_eq!(parsed.to_string(), text);
        }
    }
}
