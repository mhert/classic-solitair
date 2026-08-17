//! [`Placeholders`]: `[placeholders]` — the card-sized images drawn where a
//! pile is empty rather than where a card is.
//!
//! The whole section is optional and so is every key in it; an absent key
//! simply means nothing is drawn for that slot, which is what a theme that
//! predates the section gets.

use serde::Deserialize;

use crate::error::ManifestError;
use crate::path::RelativeAssetPath;

/// The placeholder images (`[placeholders]`).
///
/// Each field is a validated theme-package-relative path to a card-sized
/// image, or `None` when the theme does not supply that placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placeholders {
    /// Drawn on every empty pile.
    pub empty_pile: Option<RelativeAssetPath>,
    /// Drawn on the empty stock while the waste can still be recycled.
    pub stock_recycle: Option<RelativeAssetPath>,
    /// Drawn on the empty stock once no pass remains.
    pub stock_blocked: Option<RelativeAssetPath>,
}

impl Placeholders {
    /// Whether the theme supplies no placeholder at all, in which case the
    /// `[placeholders]` section carries nothing and may be omitted.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.empty_pile.is_none() && self.stock_recycle.is_none() && self.stock_blocked.is_none()
    }
}

/// The permissive, shape-only parse of `[placeholders]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPlaceholders {
    #[serde(default)]
    empty_pile: Option<RawPlaceholder>,
    #[serde(default)]
    stock_recycle: Option<RawPlaceholder>,
    #[serde(default)]
    stock_blocked: Option<RawPlaceholder>,
}

/// One `{ image = "..." }` entry. An inline table rather than a bare string,
/// matching `[backs]` and leaving room for per-placeholder options.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPlaceholder {
    image: String,
}

/// Validates a raw `[placeholders]` table.
///
/// # Errors
///
/// Returns [`ManifestError::InvalidPath`] if any present entry's `image` is
/// not theme-package-relative.
pub(crate) fn validate(raw: RawPlaceholders) -> Result<Placeholders, ManifestError> {
    Ok(Placeholders {
        empty_pile: validate_one("placeholders.empty_pile", raw.empty_pile)?,
        stock_recycle: validate_one("placeholders.stock_recycle", raw.stock_recycle)?,
        stock_blocked: validate_one("placeholders.stock_blocked", raw.stock_blocked)?,
    })
}

/// Validates one optional entry, naming `field` in any path error.
fn validate_one(
    field: &str,
    raw: Option<RawPlaceholder>,
) -> Result<Option<RelativeAssetPath>, ManifestError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    Ok(Some(RelativeAssetPath::parse(
        field.to_owned(),
        &raw.image,
    )?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::asset_path;

    fn entry(image: &str) -> RawPlaceholder {
        RawPlaceholder {
            image: image.to_owned(),
        }
    }

    #[test]
    fn an_absent_section_yields_no_placeholders() {
        let placeholders = validate(RawPlaceholders::default()).unwrap();
        assert_eq!(placeholders, Placeholders::default());
        assert!(placeholders.is_empty());
    }

    #[test]
    fn each_key_lands_in_its_own_field() {
        let placeholders = validate(RawPlaceholders {
            empty_pile: Some(entry("placeholders/empty_pile.png")),
            stock_recycle: Some(entry("placeholders/stock_recycle.png")),
            stock_blocked: Some(entry("placeholders/stock_blocked.png")),
        })
        .unwrap();
        assert_eq!(
            placeholders,
            Placeholders {
                empty_pile: Some(asset_path("placeholders/empty_pile.png")),
                stock_recycle: Some(asset_path("placeholders/stock_recycle.png")),
                stock_blocked: Some(asset_path("placeholders/stock_blocked.png")),
            }
        );
        assert!(!placeholders.is_empty());
    }

    #[test]
    fn keys_are_independently_optional() {
        let placeholders = validate(RawPlaceholders {
            empty_pile: Some(entry("ghost.png")),
            stock_recycle: None,
            stock_blocked: None,
        })
        .unwrap();
        assert_eq!(placeholders.empty_pile, Some(asset_path("ghost.png")));
        assert_eq!(placeholders.stock_recycle, None);
        assert_eq!(placeholders.stock_blocked, None);
        assert!(!placeholders.is_empty());
    }

    /// `is_empty` is only true when *every* slot is absent — one present
    /// entry is enough to make the section carry something, whichever it is.
    #[test]
    fn any_single_present_entry_makes_the_section_non_empty() {
        for raw in [
            RawPlaceholders {
                empty_pile: Some(entry("a.png")),
                ..RawPlaceholders::default()
            },
            RawPlaceholders {
                stock_recycle: Some(entry("b.png")),
                ..RawPlaceholders::default()
            },
            RawPlaceholders {
                stock_blocked: Some(entry("c.png")),
                ..RawPlaceholders::default()
            },
        ] {
            assert!(!validate(raw).unwrap().is_empty());
        }
    }

    /// Each field reports its own name, so a mis-wired validate call cannot
    /// pass by blaming the wrong key.
    #[test]
    fn a_non_relative_path_is_rejected_naming_its_own_field() {
        for (raw, field) in [
            (
                RawPlaceholders {
                    empty_pile: Some(entry("/abs.png")),
                    ..RawPlaceholders::default()
                },
                "placeholders.empty_pile",
            ),
            (
                RawPlaceholders {
                    stock_recycle: Some(entry("../up.png")),
                    ..RawPlaceholders::default()
                },
                "placeholders.stock_recycle",
            ),
            (
                RawPlaceholders {
                    stock_blocked: Some(entry("back\\slash.png")),
                    ..RawPlaceholders::default()
                },
                "placeholders.stock_blocked",
            ),
        ] {
            let error = validate(raw).unwrap_err();
            let message = error.to_string();
            assert!(
                matches!(error, ManifestError::InvalidPath { .. }),
                "{message}"
            );
            assert!(message.contains(field), "{message}");
        }
    }
}
