//! [`Asset`]: one loaded, dimension-probed theme image (a face, a back
//! frame, or the background image). [`AssetKind`] and [`probe`] are shared
//! by every asset-loading module (`load_faces`, `load_backs`,
//! `load_background`).

use crate::path::RelativeAssetPath;
use crate::render_mode::RenderMode;
use crate::size::CardSize;
use crate::{png, svg};

/// One loaded image asset: the package-relative path it was read from, its
/// raw bytes, which format those bytes probed as, and the probed pixel
/// size.
///
/// Sounds are **not** `Asset`s — they have no dimensions to probe, so their
/// bytes are exposed directly instead (see [`crate::theme::Theme::sounds`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The package-relative path this asset was read from, validated: a
    /// consumer that writes assets back out can join it onto an output
    /// directory without re-checking it.
    pub path: RelativeAssetPath,
    /// The asset's raw bytes.
    pub bytes: Vec<u8>,
    /// Which format the bytes probed as.
    pub kind: AssetKind,
    /// The probed pixel size.
    pub size: CardSize,
}

/// An [`Asset`]'s image format — fixed by a theme's `render_mode`: PNG
/// for `png`, SVG for `vector`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// A raster PNG image, probed by [`crate::png`].
    Png,
    /// A scalable SVG image, probed by [`crate::svg`].
    Svg,
}

impl AssetKind {
    /// The [`AssetKind`] a theme's `render_mode` requires for every face,
    /// back, and background image.
    #[must_use]
    pub const fn for_render_mode(render_mode: RenderMode) -> Self {
        match render_mode {
            RenderMode::Png => Self::Png,
            RenderMode::Vector => Self::Svg,
        }
    }

    /// The file extension `render_mode` fixes for this kind, dot included.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => ".png",
            Self::Svg => ".svg",
        }
    }
}

/// Probes `bytes` as `kind`, returning its pixel size or a human-readable
/// failure reason.
///
/// The reason is plain text rather than a typed `#[source]`: `png`'s and
/// `svg`'s probe error enums are this crate's private implementation
/// detail (mirrors `ManifestError::InvalidToml`'s foreign-error handling)
/// — every caller wraps the reason into its own contextual
/// [`crate::ThemeError`] variant, which names the face/back/background the
/// probe was for.
pub(crate) fn probe(bytes: &[u8], kind: AssetKind) -> Result<CardSize, String> {
    let (width, height) = match kind {
        AssetKind::Png => png::probe(bytes).map_err(|error| error.to_string())?,
        AssetKind::Svg => svg::probe(bytes).map_err(|error| error.to_string())?,
    };
    Ok(CardSize { width, height })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn png_render_mode_requires_png() {
        assert_eq!(AssetKind::for_render_mode(RenderMode::Png), AssetKind::Png);
    }

    #[test]
    fn vector_render_mode_requires_svg() {
        assert_eq!(
            AssetKind::for_render_mode(RenderMode::Vector),
            AssetKind::Svg
        );
    }

    #[test]
    fn each_asset_kind_maps_to_its_file_extension() {
        assert_eq!(AssetKind::Png.extension(), ".png");
        assert_eq!(AssetKind::Svg.extension(), ".svg");
    }

    /// A real, minimal PNG via the `png` crate's encoder (8-bit grayscale,
    /// all-zero pixels — their content is never inspected, only
    /// dimensions are): `crate::png::probe` now validates the IHDR CRC, so
    /// a hand-assembled placeholder-CRC fixture would no longer probe
    /// successfully. Paths are written `::png::...` (not `png::...`)
    /// because this file's `use crate::{png, svg}` binds the name `png`
    /// to sol-theme's own module, shadowing the extern crate.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = ::png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(::png::ColorType::Grayscale);
            encoder.set_depth(::png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0_u8; (width * height) as usize])
                .unwrap();
        }
        bytes
    }

    #[test]
    fn probe_dispatches_png_bytes_to_the_png_prober() {
        let size = probe(&png_bytes(71, 96), AssetKind::Png).unwrap();
        assert_eq!(
            size,
            CardSize {
                width: 71,
                height: 96
            }
        );
    }

    #[test]
    fn probe_dispatches_svg_bytes_to_the_svg_prober() {
        let bytes = br#"<svg width="71" height="96"></svg>"#;
        let size = probe(bytes, AssetKind::Svg).unwrap();
        assert_eq!(
            size,
            CardSize {
                width: 71,
                height: 96
            }
        );
    }

    #[test]
    fn probe_returns_a_human_readable_reason_on_failure() {
        let error = probe(b"not a png", AssetKind::Png).unwrap_err();
        assert!(!error.is_empty());
    }
}
