// Kaleidoscope: fold the screen into a slowly rotating 6-fold mirror.
struct EffectParams { width: u32, height: u32, time: f32, samples: u32 }
@group(0) @binding(0) var<uniform> params: EffectParams;
@group(0) @binding(1) var<storage, read> wave: array<i32>;
@group(0) @binding(2) var src: texture_storage_2d<rgba8unorm, read>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn effect(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let size = vec2<f32>(f32(params.width), f32(params.height));
    let uv = (vec2<f32>(gid.xy) - size * 0.5) / size.y;
    var ang = atan2(uv.y, uv.x) + params.time * 0.15;
    let r = length(uv);
    let sector = 3.14159265 / 3.0;
    ang = abs((ang % sector) - sector * 0.5);
    let p = vec2<f32>(cos(ang), sin(ang)) * r * size.y + size * 0.5;
    let q = vec2<i32>(clamp(p, vec2<f32>(0.0), size - 1.0));
    textureStore(dst, vec2<i32>(gid.xy), textureLoad(src, q));
}
