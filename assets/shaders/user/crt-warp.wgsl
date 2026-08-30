// Heavy CRT: barrel distortion, chromatic fringing, and a breathing
// vignette — the scope as a 1970s tube.
struct EffectParams { width: u32, height: u32, time: f32, samples: u32 }
@group(0) @binding(0) var<uniform> params: EffectParams;
@group(0) @binding(1) var<storage, read> wave: array<i32>;
@group(0) @binding(2) var src: texture_storage_2d<rgba8unorm, read>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;

fn sample_at(p: vec2<f32>, size: vec2<f32>) -> vec4<f32> {
    let q = vec2<i32>(clamp(p, vec2<f32>(0.0), size - 1.0));
    return textureLoad(src, q);
}

@compute @workgroup_size(16, 16)
fn effect(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let size = vec2<f32>(f32(params.width), f32(params.height));
    let uv = (vec2<f32>(gid.xy) / size) * 2.0 - 1.0;
    let r2 = dot(uv, uv);
    let warped = uv * (1.0 + 0.12 * r2);
    let base = (warped * 0.5 + 0.5) * size;
    let shift = 1.5 + 1.0 * sin(params.time * 0.7);
    let cr = sample_at(base + vec2<f32>(shift, 0.0), size).r;
    let cg = sample_at(base, size).g;
    let cb = sample_at(base - vec2<f32>(shift, 0.0), size).b;
    let vig = 1.0 - 0.35 * r2;
    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(cr * vig, cg * vig, cb * vig, 1.0));
}
