//! Batched wgpu sprite renderer for the playfield.
//!
//! Consumes the presenter's sprite display lists
//! ([`sol_presenter::DisplayList`] — the finalized presenter → renderer
//! seam) and draws them with one textured-quad pipeline over one texture
//! atlas built from the loaded [`sol_theme::Theme`], batched into a
//! single vertex/index buffer pair rebuilt per frame, under an
//! orthographic pixel-space projection. Frontends own the window, the
//! surface, and the device; this crate turns display lists into draws.
//!
//! Scaling follows the theme's art form — `png` or `vector` — plus, for a
//! png theme, the player's [`sol_theme::CardScaling`] choice; a rebuild
//! happens only on an actual content-factor change. A `png` theme's atlas
//! holds a constant factor regardless of display scale, so a resize never
//! rebuilds it either way: native pixels sampled with nearest filtering at
//! integer factors (the felt absorbs the slack) at the default `original`
//! choice, xBRZ's fixed ceiling sampled linearly (already smoothed by the
//! upscaler) at `xbrz`. A `vector` theme re-rasterizes its SVGs via resvg
//! at the exact target size — the one case whose factor does track the
//! display scale — and also samples linearly. The win cascade's
//! don't-clear flag maps to loading (not clearing) the color attachment,
//! which is what produces the smear trail.
//!
//! [`Renderer::set_display_scale`] does that rebuild inline, on the
//! caller's thread. [`Renderer::adopt_scale`] and
//! [`Renderer::apply_atlas`] split it in two so a frontend can run the
//! CPU-heavy part ([`AtlasBuildJob::run`], returning a [`BuiltAtlas`]) on
//! a thread of its own while rendering continues on the atlas already
//! loaded — this crate stays thread-free either way; it only ever hands
//! out self-contained jobs.
//!
//! Targets WebGL2-compatible limits ([`Renderer::required_limits`]) and
//! enables wgpu's `webgl` feature, so a future wasm shell can fall back
//! to WebGL2 where WebGPU is missing; on old Windows machines without
//! D3D12, wgpu's GL backend fills the same role.
//!
//! [`Renderer::render`] always draws at the renderer's own adopted
//! display scale; [`Renderer::render_at`] draws the same list at an
//! explicit scale of the caller's choosing instead, touching none of the
//! adopted scale, the loaded atlas, or the planned factor — for content
//! with a scale of its own, independent of the board's window fit, such as
//! a card-back contact sheet. [`render_to_rgba`] wraps it into a one-shot,
//! self-contained render-to-image: a fresh target, one draw, and a tightly
//! packed RGBA8 readback, with nothing retained across the call — unlike a
//! frontend's own per-frame path, which keeps a canvas and staging buffer
//! alive on purpose.

mod atlas;
mod blit;
mod error;
mod offscreen;
mod raster;
mod renderer;
mod scale;
#[cfg(test)]
mod testkit;
mod vertex;

pub use error::RenderError;
pub use offscreen::render_to_rgba;
pub use renderer::{AtlasBuildJob, BuiltAtlas, Renderer};

/// WGSL for the canvas → surface blit: a fullscreen triangle sampling the
/// persistent canvas this crate renders into.
///
/// A frontend owns its own surface, so it builds this pipeline itself; the
/// source lives here so every frontend blits with the same shader rather
/// than keeping its own copy. GL surfaces cannot be copy destinations,
/// which is why the canvas reaches the surface by drawing at all.
pub const BLIT_SHADER: &str = include_str!("blit.wgsl");

pub use blit::BlitPipeline;
