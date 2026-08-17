//! [`LoadedPlaceholders`] and placeholder loading/validation.
//!
//! Placeholders are drawn in a pile's slot instead of a card, so each one
//! must be exactly `base_size` — the same size rule static backs obey, and
//! unlike the background, which accepts any size.

use crate::asset::{self, Asset, AssetKind};
use crate::path::RelativeAssetPath;
use crate::placeholders::Placeholders;
use crate::size::CardSize;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;

/// [`Placeholders`], loaded: each declared image read, probed, and
/// size-checked; each undeclared one left `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedPlaceholders {
    /// Drawn on every empty pile.
    pub empty_pile: Option<Asset>,
    /// Drawn on the empty stock while the waste can still be recycled.
    pub stock_recycle: Option<Asset>,
    /// Drawn on the empty stock once no pass remains.
    pub stock_blocked: Option<Asset>,
}

impl LoadedPlaceholders {
    /// Each loaded placeholder as `(its `[placeholders]` key, asset)`, in a
    /// fixed order; undeclared slots are skipped. Lets a consumer walk the
    /// placeholders without repeating the slot list.
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, &Asset)> {
        [
            ("empty_pile", self.empty_pile.as_ref()),
            ("stock_recycle", self.stock_recycle.as_ref()),
            ("stock_blocked", self.stock_blocked.as_ref()),
        ]
        .into_iter()
        .filter_map(|(key, asset)| asset.map(|asset| (key, asset)))
    }
}

/// Loads and validates `[placeholders]`.
///
/// # Errors
///
/// Returns [`ThemeError::PlaceholderWrongExtension`] if a declared path does
/// not end in the extension `kind` requires,
/// [`ThemeError::PlaceholderUnreadable`] if it cannot be read,
/// [`ThemeError::PlaceholderInvalidFormat`] if its bytes do not probe as
/// `kind`, or [`ThemeError::PlaceholderWrongSize`] if it does not probe to
/// `base_size`.
pub(crate) fn load(
    source: &impl AssetSource,
    placeholders: &Placeholders,
    kind: AssetKind,
    base_size: CardSize,
) -> Result<LoadedPlaceholders, ThemeError> {
    Ok(LoadedPlaceholders {
        empty_pile: load_one(
            source,
            "empty_pile",
            placeholders.empty_pile.as_ref(),
            kind,
            base_size,
        )?,
        stock_recycle: load_one(
            source,
            "stock_recycle",
            placeholders.stock_recycle.as_ref(),
            kind,
            base_size,
        )?,
        stock_blocked: load_one(
            source,
            "stock_blocked",
            placeholders.stock_blocked.as_ref(),
            kind,
            base_size,
        )?,
    })
}

/// Reads, extension-checks, probes, and size-checks one declared
/// placeholder; an undeclared one loads as `None`. `slot` names the
/// `[placeholders]` key in any error.
fn load_one(
    source: &impl AssetSource,
    slot: &'static str,
    path: Option<&RelativeAssetPath>,
    kind: AssetKind,
    base_size: CardSize,
) -> Result<Option<Asset>, ThemeError> {
    let Some(path) = path else {
        return Ok(None);
    };

    if !path.as_str().ends_with(kind.extension()) {
        return Err(ThemeError::PlaceholderWrongExtension {
            slot,
            path: path.as_str().to_owned(),
            expected_ext: kind.extension(),
        });
    }

    let bytes = source
        .read(path)
        .map_err(|source| ThemeError::PlaceholderUnreadable {
            slot,
            path: path.as_str().to_owned(),
            source,
        })?;
    let size =
        asset::probe(&bytes, kind).map_err(|reason| ThemeError::PlaceholderInvalidFormat {
            slot,
            path: path.as_str().to_owned(),
            reason,
        })?;
    if size != base_size {
        return Err(ThemeError::PlaceholderWrongSize {
            slot,
            path: path.as_str().to_owned(),
            expected_width: base_size.width,
            expected_height: base_size.height,
            found_width: size.width,
            found_height: size.height,
        });
    }

    Ok(Some(Asset {
        path: path.clone(),
        bytes,
        kind,
        size,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::mem_source::MemSource;
    use crate::testkit::asset_path;

    const BASE: CardSize = CardSize {
        width: 71,
        height: 96,
    };

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0_u8; (width * height) as usize])
                .unwrap();
        }
        bytes
    }

    fn svg_bytes(width: u32, height: u32) -> Vec<u8> {
        format!(r#"<svg width="{width}" height="{height}"></svg>"#).into_bytes()
    }

    fn only(slot: &str, path: &str) -> Placeholders {
        let path = Some(asset_path(path));
        match slot {
            "empty_pile" => Placeholders {
                empty_pile: path,
                ..Placeholders::default()
            },
            "stock_recycle" => Placeholders {
                stock_recycle: path,
                ..Placeholders::default()
            },
            _ => Placeholders {
                stock_blocked: path,
                ..Placeholders::default()
            },
        }
    }

    #[test]
    fn an_empty_section_loads_to_nothing() {
        let source = MemSource::new();
        let loaded = load(&source, &Placeholders::default(), AssetKind::Png, BASE).unwrap();
        assert_eq!(loaded, LoadedPlaceholders::default());
    }

    #[test]
    fn each_declared_slot_loads_into_its_own_field() {
        let source = MemSource::new()
            .with_file("p/ghost.png", png_bytes(71, 96))
            .with_file("p/ring.png", png_bytes(71, 96))
            .with_file("p/cross.png", png_bytes(71, 96));
        let loaded = load(
            &source,
            &Placeholders {
                empty_pile: Some(asset_path("p/ghost.png")),
                stock_recycle: Some(asset_path("p/ring.png")),
                stock_blocked: Some(asset_path("p/cross.png")),
            },
            AssetKind::Png,
            BASE,
        )
        .unwrap();

        assert_eq!(
            loaded.empty_pile.as_ref().map(|a| a.path.as_str()),
            Some("p/ghost.png")
        );
        assert_eq!(
            loaded.stock_recycle.as_ref().map(|a| a.path.as_str()),
            Some("p/ring.png")
        );
        assert_eq!(
            loaded.stock_blocked.as_ref().map(|a| a.path.as_str()),
            Some("p/cross.png")
        );
    }

    #[test]
    fn a_declared_slot_carries_its_bytes_kind_and_probed_size() {
        let source = MemSource::new().with_file("ghost.png", png_bytes(71, 96));
        let loaded = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap();
        assert_eq!(
            loaded.empty_pile,
            Some(Asset {
                path: asset_path("ghost.png"),
                bytes: png_bytes(71, 96),
                kind: AssetKind::Png,
                size: BASE,
            })
        );
    }

    #[test]
    fn undeclared_slots_stay_none_beside_a_declared_one() {
        let source = MemSource::new().with_file("ghost.png", png_bytes(71, 96));
        let loaded = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap();
        assert!(loaded.empty_pile.is_some());
        assert_eq!(loaded.stock_recycle, None);
        assert_eq!(loaded.stock_blocked, None);
    }

    #[test]
    fn entries_yields_every_declared_slot_in_a_fixed_order() {
        let source = MemSource::new()
            .with_file("p/ghost.png", png_bytes(71, 96))
            .with_file("p/ring.png", png_bytes(71, 96))
            .with_file("p/cross.png", png_bytes(71, 96));
        let loaded = load(
            &source,
            &Placeholders {
                empty_pile: Some(asset_path("p/ghost.png")),
                stock_recycle: Some(asset_path("p/ring.png")),
                stock_blocked: Some(asset_path("p/cross.png")),
            },
            AssetKind::Png,
            BASE,
        )
        .unwrap();

        let entries: Vec<(&str, &str)> = loaded
            .entries()
            .map(|(key, asset)| (key, asset.path.as_str()))
            .collect();
        assert_eq!(
            entries,
            vec![
                ("empty_pile", "p/ghost.png"),
                ("stock_recycle", "p/ring.png"),
                ("stock_blocked", "p/cross.png"),
            ]
        );
    }

    #[test]
    fn entries_skips_undeclared_slots() {
        let source = MemSource::new().with_file("ghost.png", png_bytes(71, 96));
        let loaded = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap();
        let keys: Vec<&str> = loaded.entries().map(|(key, _)| key).collect();
        assert_eq!(keys, vec!["empty_pile"]);
    }

    #[test]
    fn entries_is_empty_when_nothing_is_declared() {
        assert_eq!(LoadedPlaceholders::default().entries().count(), 0);
    }

    #[test]
    fn a_vector_placeholder_probes_via_svg() {
        let source = MemSource::new().with_file("ghost.svg", svg_bytes(71, 96));
        let loaded = load(
            &source,
            &only("empty_pile", "ghost.svg"),
            AssetKind::Svg,
            BASE,
        )
        .unwrap();
        assert_eq!(
            loaded.empty_pile.as_ref().map(|a| a.kind),
            Some(AssetKind::Svg)
        );
    }

    /// Every slot reports its own name, so a mis-wired `load` call cannot
    /// pass by blaming the wrong key.
    #[test]
    fn each_slot_names_itself_in_its_errors() {
        for slot in ["empty_pile", "stock_recycle", "stock_blocked"] {
            let source = MemSource::new();
            let error =
                load(&source, &only(slot, "missing.png"), AssetKind::Png, BASE).unwrap_err();
            let message = error.to_string();
            assert!(
                matches!(error, ThemeError::PlaceholderUnreadable { .. }),
                "{message}"
            );
            assert!(message.contains(slot), "{message}");
        }
    }

    #[test]
    fn a_wrong_extension_for_the_render_mode_is_rejected() {
        let source = MemSource::new().with_file("ghost.svg", png_bytes(71, 96));
        let error = load(
            &source,
            &only("empty_pile", "ghost.svg"),
            AssetKind::Png,
            BASE,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ThemeError::PlaceholderWrongExtension {
                expected_ext: ".png",
                ..
            }
        ));
    }

    #[test]
    fn bytes_that_do_not_probe_as_the_expected_format_are_rejected() {
        let source = MemSource::new().with_file("ghost.png", b"not a png".to_vec());
        let error = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap_err();
        assert!(matches!(error, ThemeError::PlaceholderInvalidFormat { .. }));
    }

    /// A placeholder occupies a pile's card slot, so — unlike the
    /// background — an off-`base_size` image is a hard error, not a resize.
    #[test]
    fn a_placeholder_that_is_not_base_size_is_rejected() {
        let source = MemSource::new().with_file("ghost.png", png_bytes(71, 95));
        let error = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ThemeError::PlaceholderWrongSize {
                expected_width: 71,
                expected_height: 96,
                found_width: 71,
                found_height: 95,
                ..
            }
        ));
    }

    #[test]
    fn a_wrong_width_alone_is_rejected_too() {
        // Proves the check compares both axes, not just the height.
        let source = MemSource::new().with_file("ghost.png", png_bytes(70, 96));
        let error = load(
            &source,
            &only("empty_pile", "ghost.png"),
            AssetKind::Png,
            BASE,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ThemeError::PlaceholderWrongSize {
                found_width: 70,
                found_height: 96,
                ..
            }
        ));
    }
}
