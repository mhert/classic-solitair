//! Shared harness for the renderer's golden-image and smoke tests:
//! headless device acquisition with a software-adapter preference,
//! render-and-read-back, and golden PNG comparison.
//!
//! # Adapter strategy (the CI gate)
//!
//! Tests enumerate Vulkan and GL adapters and prefer a software
//! rasterizer (`DeviceType::Cpu` — lavapipe on Vulkan, llvmpipe on GL):
//! deterministic output, compared **exactly** against the committed
//! goldens. Without one, the first hardware adapter runs with a small
//! per-channel tolerance (rounding in fixed-function blending is the only
//! driver-visible difference this pipeline exposes). With no adapter at
//! all the tests skip. CI sets `SOL_RENDER_REQUIRE_SOFTWARE=1` (and runs
//! under Mesa), which turns both fallbacks into hard failures — the gate
//! cannot silently soften.
//!
//! Goldens are regenerated with `SOL_RENDER_BLESS=1` on a software
//! adapter (`LIBGL_ALWAYS_SOFTWARE=1` forces Mesa's llvmpipe on GL).
//! Both software adapters (lavapipe and llvmpipe) share Mesa's llvmpipe
//! rasterizer core; like a resvg bump, a Mesa version bump may
//! legitimately change blended edge pixels and require a deliberate
//! re-bless.

#![allow(dead_code)] // each test binary uses its own slice of this harness
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use sol_render_wgpu::Renderer;
use sol_theme::{MemSource, Theme, canonical_faces};

/// A headless device, and whether it is a software rasterizer.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub software: bool,
    pub adapter_name: String,
}

impl Gpu {
    /// The per-channel tolerance for golden comparison on this adapter:
    /// exact on software rasterizers, ±2 on hardware (blend rounding).
    pub const fn tolerance(&self) -> u8 {
        if self.software { 0 } else { 2 }
    }
}

/// Acquires a headless device, preferring a software adapter. Returns
/// `None` (test skips) only when no adapter exists at all and
/// `SOL_RENDER_REQUIRE_SOFTWARE` is unset.
pub fn gpu() -> Option<Gpu> {
    let require_software = std::env::var_os("SOL_RENDER_REQUIRE_SOFTWARE").is_some();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapters = pollster::block_on(
        instance.enumerate_adapters(wgpu::Backends::VULKAN | wgpu::Backends::GL),
    );
    let software = adapters
        .iter()
        .position(|adapter| adapter.get_info().device_type == wgpu::DeviceType::Cpu);
    if require_software {
        assert!(
            software.is_some(),
            "SOL_RENDER_REQUIRE_SOFTWARE is set but no software adapter (llvmpipe/lavapipe) \
             was found; adapters: {:?}",
            adapters
                .iter()
                .map(|adapter| adapter.get_info().name)
                .collect::<Vec<_>>()
        );
    }
    let Some(adapter) = software
        .and_then(|i| adapters.get(i))
        .or_else(|| adapters.first())
    else {
        eprintln!("skipping: no Vulkan or GL adapter available");
        return None;
    };
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sol render test device"),
        required_limits: Renderer::required_limits(adapter),
        ..Default::default()
    }))
    .expect("request_device");
    Some(Gpu {
        device,
        queue,
        software: software.is_some(),
        adapter_name: info.name,
    })
}

/// Renders `lists` in order into one fresh `width`×`height` target (each
/// list is one frame; later frames see earlier contents, which is what
/// the don't-clear test needs) and reads the final RGBA8 pixels back.
pub fn render_and_read(
    gpu: &Gpu,
    renderer: &mut Renderer,
    width: u32,
    height: u32,
    lists: &[sol_presenter::DisplayList],
) -> Vec<u8> {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sol test target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    for list in lists {
        renderer
            .render(&gpu.device, &gpu.queue, &view, (width, height), list)
            .expect("render");
    }

    let bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sol test readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
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
    gpu.queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        tx.send(result).expect("map channel");
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map result").expect("map buffer");
    let mapped = buffer.get_mapped_range(..).expect("mapped range");
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    pixels
}

/// The RGBA of pixel `(x, y)` in a readback of width `width`.
pub fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Compares `actual` against the committed golden `name`, within the
/// adapter's tolerance. `SOL_RENDER_BLESS=1` (re)writes the golden first.
/// On mismatch, writes `<name>.actual.png` next to the golden and fails
/// with the worst offender.
pub fn compare_golden(gpu: &Gpu, name: &str, width: u32, height: u32, actual: &[u8]) {
    let path = golden_path(name);
    if std::env::var_os("SOL_RENDER_BLESS").is_some() {
        assert!(
            gpu.software,
            "goldens must be blessed on a software adapter, not {}",
            gpu.adapter_name
        );
        write_png(&path, width, height, actual);
        eprintln!("blessed {}", path.display());
    }
    let golden = read_png(&path);
    assert_eq!(
        (golden.0, golden.1),
        (width, height),
        "golden {name} has different dimensions"
    );
    let tolerance = gpu.tolerance();
    let mut worst = 0_u8;
    let mut worst_at = (0_u32, 0_u32);
    let mut failing = 0_usize;
    for (i, (a, g)) in actual.iter().zip(&golden.2).enumerate() {
        let delta = a.abs_diff(*g);
        if delta > worst {
            worst = delta;
            let px = u32::try_from(i / 4).unwrap_or(u32::MAX);
            worst_at = (px % width, px / width);
        }
        if delta > tolerance {
            failing += 1;
        }
    }
    if failing > 0 {
        let actual_path = path.with_extension("actual.png");
        write_png(&actual_path, width, height, actual);
        panic!(
            "{name}: {failing} channel values differ beyond ±{tolerance} on {} \
             (worst Δ{worst} at {worst_at:?}); actual written to {}",
            gpu.adapter_name,
            actual_path.display()
        );
    }
}

pub fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}.png"))
}

pub fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); bless with SOL_RENDER_BLESS=1 on a software adapter",
            path.display()
        )
    });
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("golden header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("golden size")];
    let info = reader.next_frame(&mut buffer).expect("golden frame");
    assert_eq!(info.color_type, png::ColorType::Rgba, "goldens are RGBA8");
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("goldens dir");
    }
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

/// The in-tree default theme (vector), loaded from `themes/default`.
pub fn default_theme() -> Theme {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes/default");
    Theme::load_dir(dir).expect("themes/default loads")
}

/// The default theme's artwork converted to a `render_mode = "png"`
/// theme: every SVG rasterized at 1× (through the same resvg the
/// renderer's vector pipeline uses) and PNG-encoded. The manifest mirrors
/// the known structure of `themes/default/theme.toml`; the assertions
/// fail loudly if the generator ever changes that structure.
pub fn pixel_default_theme() -> Theme {
    let vector = default_theme();
    assert_eq!(
        vector.backs().len(),
        2,
        "themes/default gained backs; update this fixture"
    );
    // The goldens deliberately render *without* placeholders: adding them
    // changes every empty pile's pixels, and these goldens may only be
    // re-blessed on a software adapter (see `compare_golden`). This guard
    // keeps that omission deliberate rather than silent — if the default
    // theme's placeholder set changes, come back here and decide.
    assert_eq!(
        vector.placeholders().entries().count(),
        3,
        "themes/default changed its placeholders; decide whether the goldens \
         should now cover them (re-bless on a software adapter) and update this count"
    );
    let manifest = br##"
[theme]
name = "Default (png)"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
plain = { image = "backs/plain.png" }
weave = { image = "backs/weave.png", frames = 2, fps = 2 }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let mut source = MemSource::new().with_file("theme.toml", &manifest[..]);
    for (suit, rank) in canonical_faces() {
        let asset = vector.face(suit, rank).expect("face");
        source = source.with_file(
            format!("cards/{}.png", suit.stem(rank)),
            svg_asset_to_png(asset),
        );
    }
    for (name, back) in vector.backs() {
        let asset = back.assets.first().expect("back asset");
        source = source.with_file(
            format!("backs/{}.png", name.as_str()),
            svg_asset_to_png(asset),
        );
    }
    Theme::from_source(&source).expect("pixel default theme")
}

/// Rasterizes one SVG asset at 1× and encodes it as a straight-alpha PNG.
fn svg_asset_to_png(asset: &sol_theme::Asset) -> Vec<u8> {
    let tree = resvg::usvg::Tree::from_data(&asset.bytes, &resvg::usvg::Options::default())
        .expect("default theme svg parses");
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(asset.size.width, asset.size.height).expect("pixmap");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut rgba = pixmap.take();
    // tiny-skia pixels are premultiplied; PNG stores straight alpha.
    for px in rgba.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        if a > 0 && a < 255 {
            for c in px.iter_mut().take(3) {
                *c = u8::try_from((u16::from(*c) * 255 + a / 2) / a).unwrap_or(255);
            }
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, asset.size.width, asset.size.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&rgba).expect("png data");
    }
    bytes
}

/// A tiny 4×6-card PNG theme in `render_mode = "png"`: solid colors,
/// one static blue back, and one two-frame strip back whose frames are
/// green (frame 0) and yellow (frame 1), so frame-slice sampling is
/// visible in a readback.
pub fn tiny_png_theme() -> Theme {
    let manifest = br##"
[theme]
name = "Tiny png"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [4, 6]

[backs]
plain = { image = "backs/plain.png" }
strip = { image = "backs/strip.png", frames = 2, fps = 2 }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let solid = |width: u32, height: u32, color: [u8; 4]| {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let data: Vec<u8> = color
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect();
            writer.write_image_data(&data).expect("png data");
        }
        bytes
    };
    // Frame 0 green, frame 1 yellow, side by side in one 8×6 strip.
    let strip = {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 8, 6);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let mut data = Vec::new();
            for _row in 0..6 {
                for x in 0..8 {
                    data.extend_from_slice(if x < 4 {
                        &[0, 255, 0, 255]
                    } else {
                        &[255, 255, 0, 255]
                    });
                }
            }
            writer.write_image_data(&data).expect("png data");
        }
        bytes
    };
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/plain.png", solid(4, 6, [0, 0, 255, 255]))
        .with_file("backs/strip.png", strip);
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        let index = u8::try_from(index).expect("52 faces");
        source = source.with_file(
            format!("cards/{}.png", suit.stem(rank)),
            solid(4, 6, [255 - index * 4, index * 4, 0, 255]),
        );
    }
    Theme::from_source(&source).expect("tiny png theme")
}

/// A tiny 4×6-card SVG theme in `render_mode = "vector"`, with the same
/// solid colors and back layout as [`tiny_png_theme`]. Only a `vector`
/// theme's atlas factor tracks the display scale — a `png` theme's factor
/// depends solely on the chosen `CardScaling` — so a test that needs the
/// atlas to actually grow past factor 1 with display scale reaches for
/// this fixture rather than the png one.
pub fn tiny_vector_theme() -> Theme {
    let manifest = br##"
[theme]
name = "Tiny vector"
render_mode = "vector"

[cards]
faces = "cards/"
base_size = [4, 6]

[backs]
plain = { image = "backs/plain.svg" }
strip = { image = "backs/strip.svg", frames = 2, fps = 2 }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let rect = |width: u32, height: u32, color: (u8, u8, u8)| -> Vec<u8> {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="#{:02x}{:02x}{:02x}"/></svg>"##,
            color.0, color.1, color.2
        )
        .into_bytes()
    };
    // Frame 0 green, frame 1 yellow, side by side in one 8×6 strip.
    let strip = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect width="4" height="6" fill="#00ff00"/><rect x="4" width="4" height="6" fill="#ffff00"/></svg>"##.to_vec();
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/plain.svg", rect(4, 6, (0, 0, 255)))
        .with_file("backs/strip.svg", strip);
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        let index = u8::try_from(index).expect("52 faces");
        source = source.with_file(
            format!("cards/{}.svg", suit.stem(rank)),
            rect(4, 6, (255 - index * 4, index * 4, 0)),
        );
    }
    Theme::from_source(&source).expect("tiny vector theme")
}

/// A tiny 4×6-card `render_mode = "png"` theme whose table background
/// is a 6×4 magenta image.
pub fn tiny_bg_image_theme() -> Theme {
    let manifest = br##"
[theme]
name = "Tiny background"
render_mode = "png"

[cards]
faces = "cards/"
base_size = [4, 6]

[backs]
plain = { image = "backs/plain.png" }

[table]
background = { image = "table.png" }

[drag]
outline_color = "#000000"
"##;
    let solid = |width: u32, height: u32, color: [u8; 4]| {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let data: Vec<u8> = color
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect();
            writer.write_image_data(&data).expect("png data");
        }
        bytes
    };
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/plain.png", solid(4, 6, [0, 0, 255, 255]))
        .with_file("table.png", solid(6, 4, [255, 0, 255, 255]));
    for (index, (suit, rank)) in canonical_faces().enumerate() {
        let index = u8::try_from(index).expect("52 faces");
        source = source.with_file(
            format!("cards/{}.png", suit.stem(rank)),
            solid(4, 6, [255 - index * 4, index * 4, 0, 255]),
        );
    }
    Theme::from_source(&source).expect("tiny background theme")
}
