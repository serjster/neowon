// Color inversion — the simplest possible effect, and the pixel-test
// oracle. Every user shader follows this contract; entry point "effect".
struct EffectParams { width: u32, height: u32, time: f32, samples: u32 }
@group(0) @binding(0) var<uniform> params: EffectParams;
@group(0) @binding(1) var<storage, read> wave: array<i32>;
@group(0) @binding(2) var src: texture_storage_2d<rgba8unorm, read>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn effect(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let c = textureLoad(src, vec2<i32>(gid.xy));
    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(1.0 - c.rgb, c.a));
}
