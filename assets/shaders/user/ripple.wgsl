// Signal-driven ripple: the live CH1 waveform displaces the screen
// vertically, like the trace is shaking its own glass.
struct EffectParams { width: u32, height: u32, time: f32, samples: u32 }
@group(0) @binding(0) var<uniform> params: EffectParams;
@group(0) @binding(1) var<storage, read> wave: array<i32>;
@group(0) @binding(2) var src: texture_storage_2d<rgba8unorm, read>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn effect(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = gid.x * params.samples / params.width;
    let s = f32(wave[i]) / 125.0;   // CH1 sample under this column, ±1
    let phase = f32(gid.x) * 0.03 + params.time * 3.0;
    let dy = i32(s * 10.0 * sin(phase));
    let y = clamp(i32(gid.y) + dy, 0, i32(params.height) - 1);
    textureStore(dst, vec2<i32>(gid.xy), textureLoad(src, vec2<i32>(i32(gid.x), y)));
}
