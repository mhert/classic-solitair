// Presents the shell's persistent canvas: one fullscreen triangle
// sampling the canvas 1:1 onto the surface. A render pass works on every
// backend (the GL backend's surfaces only support being a color target,
// so a texture copy would not).

@group(0) @binding(0) var canvas_texture: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

struct BlitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> BlitOut {
    // Corners (0,0), (2,0), (0,2): one triangle covering clip space.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: BlitOut;
    out.clip = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    // Clip space is y-up, the canvas image is y-down.
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

@fragment
fn fs_main(in: BlitOut) -> @location(0) vec4<f32> {
    return textureSample(canvas_texture, canvas_sampler, in.uv);
}
