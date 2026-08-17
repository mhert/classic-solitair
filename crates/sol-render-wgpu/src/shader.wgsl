// The one textured-quad pipeline: orthographic pixel-space projection in
// the vertex stage, atlas sample × premultiplied tint in the fragment
// stage. Everything (atlas texels, tints, blending) is premultiplied
// alpha.

struct Globals {
    // xy: pixels → clip scale (2/w, -2/h); zw: clip offset (-1, 1).
    transform: vec4<f32>,
}

@group(0) @binding(0) var atlas_texture: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var<uniform> globals: Globals;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = vec4<f32>(in.pos * globals.transform.xy + globals.transform.zw, 0.0, 1.0);
    out.uv = in.uv;
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(atlas_texture, atlas_sampler, in.uv) * in.tint;
}

// Pixel-art AA for png themes at Original scaling, at continuous display
// scales: sample on a linear filter, but snap UVs so texels stay
// uniformly sized and only the one-screen-pixel band across each texel
// seam blends. At integer scales every sample lands exactly on a texel
// center — bit-identical to nearest filtering.
@fragment
fn fs_pixel_aa(in: VertexOut) -> @location(0) vec4<f32> {
    let res = vec2<f32>(textureDimensions(atlas_texture));
    let pixel = in.uv * res;
    let seam = floor(pixel + vec2<f32>(0.5));
    let dudv = max(fwidth(pixel), vec2<f32>(1e-6));
    let snapped = seam + clamp((pixel - seam) / dudv, vec2<f32>(-0.5), vec2<f32>(0.5));
    return textureSample(atlas_texture, atlas_sampler, snapped / res) * in.tint;
}
