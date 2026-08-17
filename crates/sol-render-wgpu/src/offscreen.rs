//! [`render_to_rgba`]: one-shot render-to-image, nothing retained across
//! the call.
//!
//! A frontend's own per-frame readback (`sol-qt`'s `offscreen` module is
//! one) keeps a canvas and a staging buffer alive across frames on
//! purpose — the win cascade's smear trail depends on the canvas
//! surviving. This is the opposite shape: draw one list once into a
//! target this function creates for the call and drops before returning,
//! and hand back tightly packed RGBA8 rows. A card-back contact sheet —
//! drawn at its own scale whenever the player opens the Options dialog,
//! never kept around between frames — is exactly this shape.

use sol_presenter::DisplayList;

use crate::error::RenderError;
use crate::renderer::Renderer;

/// The offscreen target's pixel format. Every [`Renderer`] this crate's
/// callers build is itself built against `Rgba8Unorm`: the render
/// pipeline is fixed to its construction-time target format and
/// [`Renderer`] does not hand that format back out, so this matches the
/// convention every caller already follows rather than reading it from
/// `renderer`.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Renders `list` at `scale` into a fresh `size` target and reads the
/// result back as tightly packed RGBA8 rows: exactly `size.0 * size.1 *
/// 4` bytes, the GPU's row-alignment padding stripped before it ever
/// reaches the caller.
///
/// A composition of [`Renderer::render_at`] (drawing at `scale` without
/// disturbing `renderer`'s adopted scale, loaded atlas, or planned
/// factor) and a texture → buffer → CPU readback. Everything this
/// function allocates — the render target, its view, the command
/// encoder, the readback buffer — is local to the call and dropped
/// before it returns, unlike a frontend's own persistent per-frame
/// canvas.
///
/// A zero width or height is clamped to 1, matching
/// [`Renderer::render_at`]'s own viewport clamp.
///
/// # Errors
///
/// A [`RenderError`] if the draw itself fails (see
/// [`Renderer::render_at`]), or [`RenderError::Readback`] if the copy,
/// the device poll, or the buffer mapping fails.
pub fn render_to_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    list: &DisplayList,
    size: (u32, u32),
    scale: f32,
) -> Result<Vec<u8>, RenderError> {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let readback = |reason: String| RenderError::Readback { reason };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sol offscreen target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_at(device, queue, &view, (width, height), scale, list)?;

    let bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sol offscreen readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("sol offscreen readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| readback(error.to_string()))?;
    let map_result = rx.recv().map_err(|_| {
        readback("the readback completion callback was dropped unresolved".to_owned())
    })?;
    map_result.map_err(|error| readback(error.to_string()))?;
    let mapped = buffer
        .get_mapped_range(..)
        .map_err(|error| readback(error.to_string()))?;
    let rgba = unpad_rows(&mapped, width, height, bytes_per_row);
    drop(mapped);
    buffer.unmap();
    rgba.ok_or_else(|| readback("readback buffer layout mismatch".to_owned()))
}

/// Tightly packed `width × height × 4` RGBA8 bytes out of `bytes_per_row`
/// -strided `mapped` input, `None` if `mapped` is too short for that
/// layout. Nothing at this function's one call site can produce that
/// mismatch — `bytes_per_row` and the buffer it reads are both computed
/// from the same `width`/`height` — but `indexing_slicing` is denied
/// crate-wide, so the row copy goes through checked bounds rather than
/// asserting the invariant away.
fn unpad_rows(mapped: &[u8], width: u32, height: u32, bytes_per_row: u32) -> Option<Vec<u8>> {
    let row_bytes = (width as usize).checked_mul(4)?;
    let stride = bytes_per_row as usize;
    let mut rgba = Vec::with_capacity(row_bytes.checked_mul(height as usize)?);
    for row in 0..height as usize {
        let start = row.checked_mul(stride)?;
        rgba.extend_from_slice(mapped.get(start..start.checked_add(row_bytes)?)?);
    }
    Some(rgba)
}
