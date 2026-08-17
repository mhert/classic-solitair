//! [`RenderError`]: the renderer's fallible surface.
//!
//! Everything here is a CPU-side preparation failure (decoding,
//! rasterizing, or packing theme assets) — with one exception,
//! [`RenderError::Readback`]: a one-shot render-to-image
//! ([`crate::render_to_rgba`]) waits on the device itself (the copy, the
//! poll, the buffer mapping), so that path's failure surfaces through this
//! enum too. Every other GPU-side failure (device loss, validation)
//! surfaces through wgpu's own error machinery, not through this enum.

use sol_presenter::TextureId;

/// A renderer operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// A theme PNG asset failed to decode.
    #[error("failed to decode {path}: {reason}")]
    AssetDecode {
        /// The asset's package-relative path.
        path: String,
        /// What the PNG decoder rejected.
        reason: String,
    },
    /// A theme SVG asset failed to parse or rasterize.
    #[error("failed to rasterize {path}: {reason}")]
    SvgRaster {
        /// The asset's package-relative path.
        path: String,
        /// What the SVG parser or rasterizer rejected.
        reason: String,
    },
    /// xBRZ rejected rescaling a theme asset.
    #[error("failed to xBRZ-rescale {path}")]
    Rescale {
        /// The asset's package-relative path.
        path: String,
        /// The underlying xBRZ rejection.
        #[source]
        source: sol_xbrz::XbrzError,
    },
    /// The theme's assets cannot be packed into one atlas texture within
    /// the device's texture size limit, even at scale factor 1.
    #[error(
        "theme assets do not fit a single {max_dim}x{max_dim} atlas texture (this device's limit)"
    )]
    AtlasOverflow {
        /// The device's `max_texture_dimension_2d`.
        max_dim: u32,
    },
    /// A display list named a texture the renderer's theme does not have —
    /// the presenter and renderer were configured with different themes.
    #[error("display list references {texture:?}, which the renderer's theme does not provide")]
    UnknownTexture {
        /// The unresolvable texture reference.
        texture: TextureId,
    },
    /// [`crate::render_to_rgba`] failed to read its rendered image back
    /// from the device: the copy, the device poll, or the buffer mapping.
    #[error("reading back the rendered image: {reason}")]
    Readback {
        /// What the copy, the poll, or the buffer mapping reported.
        reason: String,
    },
}
