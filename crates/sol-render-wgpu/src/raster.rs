//! CPU rasterization of theme assets into premultiplied RGBA8 pixels.
//!
//! One entry point, [`rasterize`], turns a loaded [`sol_theme::Asset`]
//! into pixels at an integer content factor: PNG assets decode at native
//! size (factor 1) or through xBRZ (factors 2..=6), SVG assets render
//! through resvg at exactly `probed size × factor`. Everything downstream
//! (atlas, shader blending) speaks **premultiplied** alpha, so linear
//! filtering never bleeds fringe colors; conversion from the PNG's
//! straight alpha happens here, and resvg's tiny-skia output is already
//! premultiplied.

use std::io::Cursor;

use resvg::{tiny_skia, usvg};
use sol_theme::{Asset, AssetKind, BackLayout};

use crate::error::RenderError;

/// A rasterized asset: width, height, and **premultiplied** RGBA8 pixels,
/// row-major, 4 bytes per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Raster {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied RGBA8 pixel bytes.
    pub rgba: Vec<u8>,
}

/// Rasterizes `asset` at integer content `factor` (≥ 1; PNG factors above
/// 1 must be within xBRZ's 2..=6, which the scale policy guarantees).
pub(crate) fn rasterize(asset: &Asset, factor: u32) -> Result<Raster, RenderError> {
    match asset.kind {
        AssetKind::Png => {
            let decoded = decode_png(asset)?;
            let scaled = if factor >= 2 {
                xbrz_scale(&decoded, factor, asset.path.as_str())?
            } else {
                decoded
            };
            Ok(premultiply(scaled))
        }
        AssetKind::Svg => rasterize_svg(asset, factor),
    }
}

/// Rasterizes a strip-form back asset at `factor`, scaling every frame in
/// isolation: xBRZ reads neighboring pixels, so scaling the whole strip
/// would bleed each frame's edge pixels into the frames next to it. The
/// scaled frames are rejoined along the strip's own axis, so entries and
/// `src × factor` slicing stay exactly as for a whole asset.
///
/// Factor 1 (native), single-frame calls, SVG strips (resvg renders
/// geometry, nothing bleeds), and strip geometry that does not divide
/// into `frames` (unreachable for a validated theme) all fall back to
/// the plain whole-asset path.
pub(crate) fn rasterize_strip(
    asset: &Asset,
    factor: u32,
    frames: u32,
    layout: BackLayout,
) -> Result<Raster, RenderError> {
    if factor < 2 || frames < 2 || asset.kind != AssetKind::Png {
        return rasterize(asset, factor);
    }
    let decoded = decode_png(asset)?;
    let Some(split) = split_frames(&decoded, frames, layout) else {
        return rasterize(asset, factor);
    };
    let mut scaled = Vec::with_capacity(split.len());
    for frame in &split {
        scaled.push(xbrz_scale(frame, factor, asset.path.as_str())?);
    }
    Ok(premultiply(join_frames(&scaled, layout)))
}

/// Upscales straight-alpha RGBA8 through xBRZ at `factor` (2..=6; the
/// scale policy guarantees the range, the conversion stays total).
fn xbrz_scale(source: &Raster, factor: u32, path: &str) -> Result<Raster, RenderError> {
    let rescale = |source| RenderError::Rescale {
        path: path.to_owned(),
        source,
    };
    let factor = u8::try_from(factor)
        .map_err(|_| rescale(sol_xbrz::XbrzError::InvalidFactor { factor: u8::MAX }))?;
    let rgba =
        sol_xbrz::scale_rgba(&source.rgba, source.width, source.height, factor).map_err(rescale)?;
    Ok(Raster {
        width: source.width * u32::from(factor),
        height: source.height * u32::from(factor),
        rgba,
    })
}

/// Splits a decoded strip into its frames along `layout`'s axis, or
/// `None` when the strip does not divide evenly into `frames` non-empty
/// slices (a validated theme always divides).
fn split_frames(strip: &Raster, frames: u32, layout: BackLayout) -> Option<Vec<Raster>> {
    let count = usize::try_from(frames).ok()?;
    match layout {
        BackLayout::Horizontal => {
            let frame_width = strip.width / frames;
            if frame_width == 0 || frame_width * frames != strip.width {
                return None;
            }
            let row_bytes = strip.width as usize * 4;
            let frame_row_bytes = frame_width as usize * 4;
            Some(
                (0..count)
                    .map(|index| Raster {
                        width: frame_width,
                        height: strip.height,
                        rgba: strip
                            .rgba
                            .chunks_exact(row_bytes)
                            .filter_map(|row| row.chunks_exact(frame_row_bytes).nth(index))
                            .flatten()
                            .copied()
                            .collect(),
                    })
                    .collect(),
            )
        }
        BackLayout::Vertical => {
            let frame_height = strip.height / frames;
            if frame_height == 0 || frame_height * frames != strip.height {
                return None;
            }
            let frame_bytes = strip.width as usize * 4 * frame_height as usize;
            if frame_bytes == 0 {
                return None;
            }
            Some(
                strip
                    .rgba
                    .chunks_exact(frame_bytes)
                    .take(count)
                    .map(|chunk| Raster {
                        width: strip.width,
                        height: frame_height,
                        rgba: chunk.to_vec(),
                    })
                    .collect(),
            )
        }
    }
}

/// Rejoins equally sized frames into one strip along `layout`'s axis —
/// the exact inverse of [`split_frames`] after every frame scaled by the
/// same factor.
fn join_frames(frames: &[Raster], layout: BackLayout) -> Raster {
    match layout {
        BackLayout::Horizontal => {
            let height = frames.first().map_or(0, |frame| frame.height);
            let width = frames.iter().map(|frame| frame.width).sum();
            let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
            for row in 0..height as usize {
                for frame in frames {
                    let row_bytes = frame.width as usize * 4;
                    if row_bytes == 0 {
                        continue;
                    }
                    if let Some(chunk) = frame.rgba.chunks_exact(row_bytes).nth(row) {
                        rgba.extend_from_slice(chunk);
                    }
                }
            }
            Raster {
                width,
                height,
                rgba,
            }
        }
        BackLayout::Vertical => Raster {
            width: frames.first().map_or(0, |frame| frame.width),
            height: frames.iter().map(|frame| frame.height).sum(),
            rgba: frames
                .iter()
                .flat_map(|frame| frame.rgba.iter().copied())
                .collect(),
        },
    }
}

/// Decodes a PNG asset to straight-alpha RGBA8, normalizing grayscale,
/// indexed, and RGB color types.
fn decode_png(asset: &Asset) -> Result<Raster, RenderError> {
    let invalid = |reason: String| RenderError::AssetDecode {
        path: asset.path.as_str().to_owned(),
        reason,
    };

    let mut decoder = png::Decoder::new(Cursor::new(&asset.bytes));
    // EXPAND (palette -> RGB(A), <8-bit -> 8-bit, tRNS -> alpha) plus
    // STRIP_16: unlike soltool, which rejects a 16-bit source outright, the
    // renderer draws whatever theme a user points it at, so a 16-bit source
    // is displayed at 8-bit rather than rejected.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| invalid(error.to_string()))?;
    let buffer_len = reader
        .output_buffer_size()
        .ok_or_else(|| invalid("PNG dimensions are too large to decode".to_owned()))?;
    let mut buffer = vec![0_u8; buffer_len];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| invalid(error.to_string()))?;
    buffer.truncate(frame.buffer_size());

    let rgba = to_rgba8(&buffer, frame.color_type)
        .ok_or_else(|| invalid(format!("unsupported color type {:?}", frame.color_type)))?;
    Ok(Raster {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

/// Normalizes an `EXPAND`ed, 8-bit PNG output buffer to `RGBA8`. Returns
/// `None` for color types `EXPAND` can still emit but this renderer
/// cannot map (none today; the arm exists to stay total if the decoder
/// grows). The inner match fallbacks never run — `chunks_exact` yields
/// exactly sized chunks — and merely keep the pixel mapping total.
fn to_rgba8(buffer: &[u8], color_type: png::ColorType) -> Option<Vec<u8>> {
    let rgba = match color_type {
        png::ColorType::Rgba => buffer.to_vec(),
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .filter_map(|px| match *px {
                [r, g, b] => Some([r, g, b, 0xFF]),
                _ => None,
            })
            .flatten()
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .filter_map(|px| match *px {
                [g, a] => Some([g, g, g, a]),
                _ => None,
            })
            .flatten()
            .collect(),
        png::ColorType::Indexed => return None,
    };
    Some(rgba)
}

/// Converts straight-alpha RGBA8 to premultiplied in place. The slice
/// pattern always matches `chunks_exact_mut(4)` chunks; it merely keeps
/// the loop free of indexing.
fn premultiply(mut raster: Raster) -> Raster {
    for px in raster.rgba.chunks_exact_mut(4) {
        if let [r, g, b, a] = px {
            let alpha = u16::from(*a);
            if alpha == 0xFF {
                continue;
            }
            // +127 rounds to nearest, matching tiny-skia's premultiply.
            let mul = |c: u8| u8::try_from((u16::from(c) * alpha + 127) / 255).unwrap_or(u8::MAX);
            (*r, *g, *b) = (mul(*r), mul(*g), mul(*b));
        }
    }
    raster
}

/// Renders an SVG asset at `probed size × factor` via resvg. The output
/// pixels are premultiplied (tiny-skia's native representation).
fn rasterize_svg(asset: &Asset, factor: u32) -> Result<Raster, RenderError> {
    let fail = |reason: String| RenderError::SvgRaster {
        path: asset.path.as_str().to_owned(),
        reason,
    };

    let tree = usvg::Tree::from_data(&asset.bytes, &sol_theme::hardened_options())
        .map_err(|error| fail(error.to_string()))?;
    let width = asset.size.width.saturating_mul(factor);
    let height = asset.size.height.saturating_mul(factor);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| fail(format!("cannot allocate a {width}x{height} pixmap")))?;
    #[allow(clippy::cast_precision_loss)] // factors are small integers
    let transform = tiny_skia::Transform::from_scale(factor as f32, factor as f32);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(Raster {
        width,
        height,
        rgba: pixmap.take(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_theme::CardSize;

    use super::*;
    use crate::testkit::asset_path;

    fn png_asset(width: u32, height: u32, pixels: &[u8], color: png::ColorType) -> Asset {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        Asset {
            path: asset_path("test.png"),
            bytes,
            kind: AssetKind::Png,
            size: CardSize { width, height },
        }
    }

    fn svg_asset(width: u32, height: u32, body: &str) -> Asset {
        let bytes = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">{body}</svg>"#
        )
        .into_bytes();
        Asset {
            path: asset_path("test.svg"),
            bytes,
            kind: AssetKind::Svg,
            size: CardSize { width, height },
        }
    }

    #[test]
    fn png_rgba_decodes_at_native_size_and_premultiplies() {
        // One opaque red pixel, one half-transparent green pixel.
        let asset = png_asset(
            2,
            1,
            &[255, 0, 0, 255, 0, 200, 0, 128],
            png::ColorType::Rgba,
        );
        let raster = rasterize(&asset, 1).unwrap();
        assert_eq!((raster.width, raster.height), (2, 1));
        assert_eq!(&raster.rgba[0..4], &[255, 0, 0, 255]);
        // 200 * 128 / 255 rounds to 100 — premultiplied.
        assert_eq!(&raster.rgba[4..8], &[0, 100, 0, 128]);
    }

    #[test]
    fn png_rgb_and_grayscale_normalize_to_opaque_rgba() {
        let rgb = rasterize(&png_asset(1, 1, &[10, 20, 30], png::ColorType::Rgb), 1).unwrap();
        assert_eq!(rgb.rgba, vec![10, 20, 30, 255]);
        let gray = rasterize(&png_asset(1, 1, &[77], png::ColorType::Grayscale), 1).unwrap();
        assert_eq!(gray.rgba, vec![77, 77, 77, 255]);
        let ga = rasterize(
            &png_asset(1, 1, &[77, 255], png::ColorType::GrayscaleAlpha),
            1,
        )
        .unwrap();
        assert_eq!(ga.rgba, vec![77, 77, 77, 255]);
    }

    #[test]
    fn png_factor_two_runs_xbrz() {
        let asset = png_asset(2, 2, &[255u8; 16], png::ColorType::Rgba);
        let raster = rasterize(&asset, 2).unwrap();
        assert_eq!((raster.width, raster.height), (4, 4));
        assert_eq!(raster.rgba.len(), 4 * 4 * 4);
        // A solid white block stays white everywhere (xBRZ preserves
        // uniform RGB; corner alpha dips are premultiplied consistently).
        assert!(raster.rgba.chunks_exact(4).all(|px| px[0] == px[1]));
    }

    /// A vertical two-frame strip PNG asset from the corner-strip frames.
    fn vertical_corner_strip() -> Asset {
        let mut pixels = crate::testkit::corner_strip_frame_pixels(0);
        pixels.extend(crate::testkit::corner_strip_frame_pixels(1));
        png_asset(4, 12, &pixels, png::ColorType::Rgba)
    }

    fn frame_asset(index: u32) -> Asset {
        png_asset(
            4,
            6,
            &crate::testkit::corner_strip_frame_pixels(index),
            png::ColorType::Rgba,
        )
    }

    #[test]
    fn vertical_strip_frames_scale_in_isolation() {
        let scaled = rasterize_strip(&vertical_corner_strip(), 2, 2, BackLayout::Vertical).unwrap();
        assert_eq!((scaled.width, scaled.height), (8, 24));
        let top = rasterize(&frame_asset(0), 2).unwrap();
        let bottom = rasterize(&frame_asset(1), 2).unwrap();
        // Vertical join is plain concatenation: the halves must be the
        // frames scaled alone, byte for byte.
        let half = top.rgba.len();
        assert_eq!(scaled.rgba.get(..half), Some(&top.rgba[..]));
        assert_eq!(scaled.rgba.get(half..), Some(&bottom.rgba[..]));
    }

    #[test]
    fn strip_factor_one_and_single_frame_take_the_whole_path() {
        let strip = vertical_corner_strip();
        let native = rasterize(&strip, 1).unwrap();
        assert_eq!(
            rasterize_strip(&strip, 1, 2, BackLayout::Vertical).unwrap(),
            native,
            "factor 1 never splits"
        );
        let doubled = rasterize(&strip, 2).unwrap();
        assert_eq!(
            rasterize_strip(&strip, 2, 1, BackLayout::Vertical).unwrap(),
            doubled,
            "a single frame is a whole asset"
        );
    }

    #[test]
    fn svg_strips_scale_whole() {
        // resvg renders geometry; there is no neighbor bleed to avoid.
        let asset = svg_asset(8, 6, r##"<rect width="8" height="6" fill="#123456"/>"##);
        assert_eq!(
            rasterize_strip(&asset, 2, 2, BackLayout::Horizontal).unwrap(),
            rasterize(&asset, 2).unwrap()
        );
    }

    #[test]
    fn an_indivisible_strip_falls_back_to_whole_scaling() {
        // 5 pixels wide cannot split into 2 frames; a validated theme
        // never produces this, the fallback merely keeps the call total.
        let asset = png_asset(5, 2, &[128u8; 5 * 2 * 4], png::ColorType::Rgba);
        assert_eq!(
            rasterize_strip(&asset, 2, 2, BackLayout::Horizontal).unwrap(),
            rasterize(&asset, 2).unwrap()
        );
        // Same for a vertical strip taller than it divides: 3 rows, 2 frames.
        let tall = png_asset(2, 3, &[128u8; 2 * 3 * 4], png::ColorType::Rgba);
        assert_eq!(
            rasterize_strip(&tall, 2, 2, BackLayout::Vertical).unwrap(),
            rasterize(&tall, 2).unwrap()
        );
    }

    #[test]
    fn garbage_png_bytes_fail_with_the_asset_path() {
        let asset = Asset {
            path: asset_path("cards/broken.png"),
            bytes: b"not a png".to_vec(),
            kind: AssetKind::Png,
            size: CardSize {
                width: 1,
                height: 1,
            },
        };
        let error = rasterize(&asset, 1).unwrap_err();
        assert!(matches!(error, RenderError::AssetDecode { .. }));
        assert!(error.to_string().contains("cards/broken.png"));
    }

    #[test]
    fn svg_renders_at_exact_factor_size() {
        // A full-bleed opaque red rect: every pixel lands exactly red.
        let asset = svg_asset(3, 4, r##"<rect width="3" height="4" fill="#ff0000"/>"##);
        for factor in [1_u32, 2, 5] {
            let raster = rasterize(&asset, factor).unwrap();
            assert_eq!((raster.width, raster.height), (3 * factor, 4 * factor));
            assert!(
                raster.rgba.chunks_exact(4).all(|px| px == [255, 0, 0, 255]),
                "factor {factor} fills red"
            );
        }
    }

    #[test]
    fn svg_image_hrefs_never_resolve() {
        // The href resolvers are disabled, so an <image> pointing at a
        // local path contributes nothing — the cell stays transparent
        // instead of compositing file contents.
        let asset = svg_asset(
            2,
            2,
            r#"<image href="/etc/hostname" width="2" height="2"/>"#,
        );
        let raster = rasterize(&asset, 1).unwrap();
        assert!(raster.rgba.chunks_exact(4).all(|px| px == [0, 0, 0, 0]));
    }

    #[test]
    fn malformed_svg_fails_with_the_asset_path() {
        let asset = Asset {
            path: asset_path("cards/broken.svg"),
            bytes: b"<svg".to_vec(),
            kind: AssetKind::Svg,
            size: CardSize {
                width: 1,
                height: 1,
            },
        };
        let error = rasterize(&asset, 1).unwrap_err();
        assert!(matches!(error, RenderError::SvgRaster { .. }));
        assert!(error.to_string().contains("cards/broken.svg"));
    }

    #[test]
    fn indexed_stays_unreachable_after_expand() {
        // EXPAND turns indexed PNGs into RGB(A) before to_rgba8 ever sees
        // them; the arm is a totality guard, exercised directly here.
        assert_eq!(to_rgba8(&[0], png::ColorType::Indexed), None);
    }
}
