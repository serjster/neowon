// Digital-phosphor waveform engine: three passes over a shared bind group.
//
//   decay   — exponential fade of the accumulation buffer (persistence)
//   raster  — splat the newest record into the accumulation buffer
//   compose — accumulation -> colormapped display texture
//
// The accumulation buffer is fixed-point (FIXED units per hit) because WGSL
// atomics are integer-only. Vector mode spreads one column's energy over the
// vertical span between adjacent samples, so fast edges render dim and
// dwell-heavy levels render bright — the classic analog-scope look.

struct Params {
    width: u32,
    height: u32,
    samples: u32,
    mode: u32,   // 0 = vectors, 1 = dots, 2 = xy
    decay: f32,
    gain: f32,
    en0: u32,
    en1: u32,
    en2: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    col0: vec4f,
    col1: vec4f,
    col2: vec4f,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> wave: array<i32>;
@group(0) @binding(2) var<storage, read_write> accum: array<atomic<u32>>;
@group(0) @binding(3) var display: texture_storage_2d<rgba8unorm, write>;

const FIXED: f32 = 256.0;

fn accum_idx(ch: u32, x: u32, y: u32) -> u32 {
    return ch * params.width * params.height + y * params.width + x;
}

@compute @workgroup_size(16, 16, 1)
fn decay(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    for (var c = 0u; c < 3u; c++) {
        let i = accum_idx(c, id.x, id.y);
        let v = f32(atomicLoad(&accum[i]));
        atomicStore(&accum[i], u32(v * params.decay));
    }
}

// Sample raw value -> vertical pixel. The display window is +-100 counts
// (+-4 divisions of the 8x10 graticule); +-125 (the ADC rails) pin at the
// plot edge, like a real scope overdriving the graticule.
fn sample_row(ch: u32, i: u32) -> f32 {
    let raw = f32(wave[ch * params.samples + i]);
    let frac = clamp(0.5 - raw / 200.0, 0.0, 1.0);
    return frac * f32(params.height - 1u);
}

fn enabled_for(ch: u32) -> u32 {
    if (ch == 0u) { return params.en0; }
    if (ch == 1u) { return params.en1; }
    return params.en2;
}

@compute @workgroup_size(256, 1, 1)
fn raster(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    let ch = id.y;
    if (i >= params.samples || i == 0u || ch >= 3u) { return; }

    if (params.mode == 2u) {
        // XY: (ch1 -> x, ch2 -> y); needs both channels, one thread row.
        // Segments between consecutive points: a scope's beam is continuous,
        // and coherent sampling would otherwise splat the same few pixels.
        if (ch != 0u || params.en0 == 0u || params.en1 == 0u) { return; }
        // Same +-4-division window as the vertical axis.
        let fx0 = clamp(0.5 + f32(wave[i - 1u]) / 200.0, 0.0, 1.0);
        let fx1 = clamp(0.5 + f32(wave[i]) / 200.0, 0.0, 1.0);
        let x0 = f32(u32(fx0 * f32(params.width - 1u)));
        let x1 = f32(u32(fx1 * f32(params.width - 1u)));
        let y0 = sample_row(1u, i - 1u);
        let y1 = sample_row(1u, i);
        let dx = x1 - x0;
        let dy = y1 - y0;
        let steps = u32(ceil(max(abs(dx), abs(dy))));
        if (steps == 0u) {
            atomicAdd(&accum[accum_idx(0u, u32(x1), u32(y1))], u32(FIXED));
            return;
        }
        let contrib = max(u32(FIXED / (f32(steps) + 1.0)), u32(FIXED / 12.0));
        for (var s = 0u; s <= steps; s++) {
            let t = f32(s) / f32(steps);
            let x = u32(x0 + dx * t + 0.5);
            let y = u32(y0 + dy * t + 0.5);
            atomicAdd(&accum[accum_idx(0u, x, y)], contrib);
        }
        return;
    }

    if (enabled_for(ch) == 0u) { return; }

    let x = i * (params.width - 1u) / (params.samples - 1u);
    let y1 = sample_row(ch, i);
    if (params.mode == 1u) {
        atomicAdd(&accum[accum_idx(ch, x, u32(y1))], u32(FIXED));
        return;
    }
    // Vectors: fill the span from the previous sample; constant energy per
    // segment gives intensity grading. The floor keeps a single trace's fast
    // edges visible instead of vanishing at 1/span brightness.
    let y0 = sample_row(ch, i - 1u);
    let lo = u32(min(y0, y1));
    let hi = u32(max(y0, y1));
    let contrib = max(u32(FIXED / f32(hi - lo + 1u)), u32(FIXED / 12.0));
    for (var y = lo; y <= hi; y++) {
        atomicAdd(&accum[accum_idx(ch, x, y)], contrib);
    }
}

@compute @workgroup_size(16, 16, 1)
fn compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    var rgb = vec3f(0.008, 0.010, 0.014);
    let a0 = f32(atomicLoad(&accum[accum_idx(0u, id.x, id.y)])) / FIXED;
    let a1 = f32(atomicLoad(&accum[accum_idx(1u, id.x, id.y)])) / FIXED;
    let a2 = f32(atomicLoad(&accum[accum_idx(2u, id.x, id.y)])) / FIXED;
    rgb += params.col0.rgb * (1.0 - exp(-a0 * params.gain));
    rgb += params.col1.rgb * (1.0 - exp(-a1 * params.gain));
    rgb += params.col2.rgb * (1.0 - exp(-a2 * params.gain));
    textureStore(display, vec2i(i32(id.x), i32(id.y)), vec4f(rgb, 1.0));
}
