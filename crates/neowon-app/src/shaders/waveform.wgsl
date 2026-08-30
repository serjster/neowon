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
    crt: u32,
    palette: u32,
    view_start: f32,
    view_span: f32,
    deep: u32,
    _pad: u32,
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

// The visible window is +-100 counts; beyond it the beam is off the plot.
fn off_screen(raw: i32) -> bool {
    return raw > 100 || raw < -100;
}

// No acquired data in this column (see neowon_dsp::timeline::NO_DATA).
// Real samples are within +-127, so the code is unambiguous.
const NO_DATA: i32 = -128;

// Horizontal zoom window: sample index -> fraction of the visible plot
// (0..1). Samples outside the window map outside [0, 1] and are skipped.
//
// A deep trace stores two values (min, max) per column, so both must land
// on the *same* column: indexing by i would put every pair from the second
// onwards astride a boundary, smearing gap edges by a column.
fn view_x(i: u32) -> f32 {
    var k = i;
    var n = params.samples;
    if (params.deep == 1u) {
        k = i / 2u;
        n = params.samples / 2u;
    }
    let fx = f32(k) / f32(max(n, 2u) - 1u);
    return (fx - params.view_start) / params.view_span;
}

fn plot_col(xf: f32) -> u32 {
    return u32(clamp(xf, 0.0, 1.0) * f32(params.width - 1u));
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
        // No beam when the segment is entirely off the plot.
        let out0 = off_screen(wave[i - 1u]) || off_screen(wave[params.samples + i - 1u]);
        let out1 = off_screen(wave[i]) || off_screen(wave[params.samples + i]);
        if (out0 && out1) { return; }
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
        // Faint floor: beam repositioning between shapes stays a ghost
        // line, like the real oscilloscope-music videos.
        let contrib = max(u32(FIXED / (f32(steps) + 1.0)), u32(FIXED / 64.0));
        for (var s = 0u; s <= steps; s++) {
            let t = f32(s) / f32(steps);
            let x = u32(x0 + dx * t + 0.5);
            let y = u32(y0 + dy * t + 0.5);
            atomicAdd(&accum[accum_idx(0u, x, y)], contrib);
        }
        return;
    }

    if (enabled_for(ch) == 0u) { return; }

    // Off-screen suppression: when the signal is beyond the display window
    // (over-ranged or ADC-clipped), a real scope shows the trace running off
    // the plot edge — never a false line pinned along the border.
    let r0 = wave[ch * params.samples + i - 1u];
    let r1 = wave[ch * params.samples + i];
    // A gap is a hole, not a value: either endpoint missing means no beam.
    // (Testing both would draw a spike to the plot edge at every gap edge,
    // because sample_row clamps rather than culling.)
    if (r0 == NO_DATA || r1 == NO_DATA) { return; }
    if ((r0 > 100 && r1 > 100) || (r0 < -100 && r1 < -100)) { return; }

    // Zoom window: cull samples outside; segments crossing the edge clamp
    // to the boundary column.
    let xf0 = view_x(i - 1u);
    let xf1 = view_x(i);
    if ((xf0 < 0.0 && xf1 < 0.0) || (xf0 > 1.0 && xf1 > 1.0)) { return; }
    let x = plot_col(xf1);
    let y1 = sample_row(ch, i);
    if (params.mode == 1u) {
        if (off_screen(r1) || xf1 < 0.0 || xf1 > 1.0) { return; }
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

// Heat colormap for the thermal palette (dark red -> orange -> white).
fn thermal(t: f32) -> vec3f {
    let x = clamp(t, 0.0, 1.0);
    return vec3f(
        smoothstep(0.02, 0.45, x),
        smoothstep(0.30, 0.80, x),
        smoothstep(0.70, 1.00, x) * 0.9 + 0.10 * (1.0 - smoothstep(0.0, 0.25, x)),
    );
}

// Per-channel accumulated intensity at a (clamped) texel.
fn amp3(x: i32, y: i32) -> vec3f {
    let cx = u32(clamp(x, 0, i32(params.width) - 1));
    let cy = u32(clamp(y, 0, i32(params.height) - 1));
    return vec3f(
        f32(atomicLoad(&accum[accum_idx(0u, cx, cy)])),
        f32(atomicLoad(&accum[accum_idx(1u, cx, cy)])),
        f32(atomicLoad(&accum[accum_idx(2u, cx, cy)])),
    ) / FIXED;
}

@compute @workgroup_size(16, 16, 1)
fn compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    let x = i32(id.x);
    let y = i32(id.y);
    var a = amp3(x, y);
    if (params.crt == 1u) {
        // Phosphor halo: neighboring beam energy bleeds into this texel.
        a += (amp3(x - 1, y) + amp3(x + 1, y) + amp3(x, y - 1) + amp3(x, y + 1)) * 0.22
            + (amp3(x - 2, y) + amp3(x + 2, y) + amp3(x, y - 2) + amp3(x, y + 2)) * 0.08;
    }
    var rgb = vec3f(0.008, 0.010, 0.014);
    if (params.palette == 1u) {
        // Thermal DPO grading: combined dwell intensity -> heat colormap.
        let t = 1.0 - exp(-(a.x + a.y + a.z) * params.gain);
        rgb += thermal(t);
    } else if (params.palette == 2u) {
        // Monochrome green CRT (P31 phosphor).
        let t = 1.0 - exp(-(a.x + a.y + a.z) * params.gain);
        rgb += vec3f(0.25, 1.0, 0.35) * t + vec3f(0.4, 0.25, 0.1) * t * t;
    } else {
        rgb += params.col0.rgb * (1.0 - exp(-a.x * params.gain));
        rgb += params.col1.rgb * (1.0 - exp(-a.y * params.gain));
        rgb += params.col2.rgb * (1.0 - exp(-a.z * params.gain));
    }
    if (params.crt == 1u) {
        // Scanlines + gentle vignette; subtle enough to keep traces crisp.
        let scan = 0.88 + 0.12 * cos(6.2831853 * f32(id.y) / 3.0);
        let u = (f32(id.x) / f32(params.width) - 0.5) * 2.0;
        let v = (f32(id.y) / f32(params.height) - 0.5) * 2.0;
        let vig = 1.0 - 0.10 * (u * u + v * v);
        rgb *= scan * vig;
    }
    // Discontinuity markers: a red squiggle down every column the
    // instrument was not acquiring in. Drawn here, in the compose pass,
    // rather than as an overlay — the display texture is what `shot` and
    // the MCP screenshot tool read back, so a marker drawn anywhere else
    // would be invisible in every capture and unassertable in any test.
    if (params.deep == 1u) {
        let cols = params.samples / 2u;
        // Wobble the column by a couple of pixels down the height so the
        // mark reads as a tear rather than as signal.
        let wob = i32(round(2.0 * sin(f32(id.y) * 0.35)));
        let c = x - wob;
        if (c >= 0 && c < i32(cols) && wave[3u * params.samples + u32(c)] != 0) {
            // A wide gap gets a squiggly line down each edge and only a
            // faint wash between them: the empty width already carries the
            // duration, and flooding it red would shout over the signal
            // either side.
            let prev = select(0, wave[3u * params.samples + u32(c - 1)], c > 0);
            let next = select(0, wave[3u * params.samples + u32(c + 1)], c < i32(cols) - 1);
            let edge = c == 0 || c == i32(cols) - 1 || prev == 0 || next == 0;
            let strength = select(0.10, 0.85, edge);
            rgb = mix(rgb, vec3f(0.85, 0.10, 0.10), strength);
        }
    }
    textureStore(display, vec2i(x, y), vec4f(rgb, 1.0));
}
