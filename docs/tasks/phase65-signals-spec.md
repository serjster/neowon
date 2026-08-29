# Phase 6.5 Track A: signal engine + virtual testbench

You are extending neowon's simulated backend into a full virtual testbench.
Read `PLAN.md` first. You work ONLY in `crates/neowon-sim/` (lib.rs,
backend.rs, new files, `tests/`). Do not touch any other crate — another
agent works on the app in parallel.

## Hard rules

- Files outside `crates/neowon-sim/` are OFF LIMITS (read is fine, write is
  not). You may read `crates/neowon-core`, `crates/neowon-backend`,
  `crates/neowon-dsp` for APIs.
- You have NO permission to read files outside the project directory
  (no ~/.cargo/registry) — such reads are auto-rejected and may kill your
  session. Use `cargo build` errors to learn APIs.
- Never run anything that touches USB (no neowon CLI, no neowon-app).
- No `git commit`.
- Finish with `cargo build` and `cargo test -p neowon-sim -p neowon-dsp`
  fully green, and `cargo build -p neowon-app` still compiling (the app uses
  `SimBackend::new()` — keep that constructor).

## Existing code

`crates/neowon-sim/src/lib.rs` has `SimSource` (per-channel sine/square/
triangle, xorshift noise, i8 quantization to ±125 over `range`) and
`crates/neowon-sim/src/backend.rs` has `SimBackend` implementing
`neowon_backend::Backend` (paced ~30 frames/s). The `Backend` trait just
gained two methods you must implement on `SimBackend`:

```rust
fn set_stimulus(&mut self, name: &str) -> Result<bool, BackendError>; // true if known
fn stimuli(&self) -> Vec<&'static str>;
```

## Work items

### 1. Component-based signal model (new module `signal.rs`)

```rust
pub enum Component {
    Sine { freq: f64, amp: f64, phase: f64 },       // amp = peak, volts
    Square { freq: f64, amp: f64, duty: f64, phase: f64 },
    Triangle { freq: f64, amp: f64, phase: f64 },
    Ramp { freq: f64, amp: f64, phase: f64 },
    /// Square with linear 0-100% edges taking `edge` seconds.
    Trapezoid { freq: f64, amp: f64, duty: f64, edge: f64 },
    Dc { level: f64 },
    Noise { rms: f64 },
    /// Linear frequency sweep f0->f1 over `period`, repeating; phase
    /// phi(t) = 2*pi*(f0*t + (f1-f0)/(2*period)*t^2) for t in [0, period).
    Chirp { f0: f64, f1: f64, period: f64, amp: f64 },
    Am { carrier: f64, mod_freq: f64, depth: f64, amp: f64 },
    /// FM: amp * sin(2*pi*carrier*t + (deviation/mod_freq)*sin(2*pi*mod_freq*t))
    Fm { carrier: f64, mod_freq: f64, deviation: f64, amp: f64 },
}

pub struct SignalSpec { pub components: Vec<Component> } // summed
impl SignalSpec { pub fn sample(&self, t: f64, rng: &mut Xorshift) -> f64; }
```

Keep the existing xorshift64* PRNG (move it into a small public
`Xorshift` struct so specs stay deterministic).

### 2. XY figures (module `figures.rs`)

Parametric (x(u), y(u)) with u = 2*pi*freq*t, both normalized to [-1, 1]:

```rust
pub enum XyFigure { Circle, Lissajous { a: u32, b: u32, phase: f64 }, Rose { k: u32 }, Heart, Butterfly }
```

- Circle: x=cos u, y=sin u
- Lissajous: x=sin(a*u + phase), y=sin(b*u)
- Rose: r=cos(k*u); x=r*cos u, y=r*sin u
- Heart: x=(16 sin^3 u)/17, y=(13 cos u − 5 cos 2u − 2 cos 3u − cos 4u)/17
- Butterfly: f=e^{cos u} − 2 cos 4u + sin^5(u/12); x=f sin u /4, y=f cos u /4
  (period 24*pi in u — handle by letting u run continuously)

### 3. Scenario model + named presets

```rust
pub enum Scenario {
    PerChannel([SignalSpec; 2]),
    /// CH1 = amp*x(t), CH2 = amp*y(t).
    Xy { figure: XyFigure, freq: f64, amp: f64 },
}
impl Scenario {
    pub const PRESETS: [&'static str; N];
    pub fn preset(name: &str) -> Option<Scenario>;
}
```

Presets (names are a stable script/UI API — exact strings):
`probe-comp` (current default: 1 kHz 0..5 V square CH1 + 2.5 kHz 1 V sine
CH2, small noise), `sine-1k` (1 kHz 2 Vpp sine + tiny noise), `dc-1v`,
`two-tone` (1 kHz 1 V + 3.5 kHz 0.4 V sines), `chirp` (100 Hz -> 10 kHz over
20 ms, 1 V), `am` (10 kHz carrier, 500 Hz mod, depth 0.5, 1 V), `fm`
(10 kHz carrier, 500 Hz mod, 2 kHz deviation, 1 V), `trapezoid` (1 kHz,
duty 0.5, edge 200 us, 2 Vpp), `noise` (0.3 V rms), `xy-circle` (1 kHz,
1.5 V), `xy-lissajous-3-2` (phase pi/2), `xy-rose-5`, `xy-heart`,
`xy-butterfly`.

`SimSource` gains `pub fn set_scenario(&mut self, s: Scenario)`; frame
generation samples the scenario (per-channel specs, or figure x/y for
channel 0/1). Keep phase continuity across frames (existing `t0` pattern).
`SimSource::default()` = `probe-comp` at 250 kS/s. Preserve the public
knobs the app/backend already use (`sample_rate`, channel enable/range via
`SimBackend::apply` — ranges now live per-channel; adapt `backend.rs`
accordingly: config.channels[i] gives volts_div/probe -> range, enabled).

### 4. Simulated triggering (in `backend.rs`)

Honor the applied `ScopeConfig.trigger` like real hardware:

- Sweep `Auto`: free-run (as now).
- `Normal`/`Single` with `TriggerKind::Edge { slope }` on an enabled source
  channel: produce frames where the trigger condition is satisfied at
  sample index `round(position * 5000)` (position from config, default
  0.5): the source signal crosses `level` with the right slope there.
  Implementation sketch: generate a candidate frame, scan for a matching
  interpolated crossing, then re-generate with `t0` shifted so the crossing
  lands on the trigger index. If the level is outside the signal's range
  (no crossing found in 2 record lengths), return `Ok(None)` from
  `poll_frame` — a starving trigger, exactly like hardware.
- Non-edge kinds: treat as Auto (document with a comment).

### 5. Virtual testbench (`crates/neowon-sim/tests/testbench.rs`)

Integration tests using neowon-dsp (already a dev-dependency; add
`neowon-backend` as dev-dependency if needed). Cover at least:

- Amplitudes: sine 1 V peak -> vpp within 2% of 2 V, vrms within 2% of
  0.7071 V; triangle vrms = amp/sqrt(3); square vrms = amp.
- Frequency accuracy: sine at 50 Hz (rate 12.5 kS/s), 1 kHz (250 kS/s),
  25 kHz (2.5 MS/s) -> `estimate_frequency` within 0.2%.
- Duty cycle: squares at 10/25/50/75/90% -> `measure().pduty` within 1.5%.
- Trapezoid: edge = 200 us -> measured 10-90% rise within 160 us +- 10%
  (10-90% of a linear edge = 0.8 * edge).
- FFT two-tone: with `spectrum` (Hann, 4096), find the two largest peaks
  (exclude +-3 bins around the first when searching for the second): freqs
  within 2 bins, amplitude ratio 1.0/0.4 within 1 dB.
- FFT AM: carrier peak at 10 kHz and sidebands at +-500 Hz, each sideband
  ~ depth/2 (= 0.25) of carrier amplitude, within 1.5 dB. Pick a sample
  rate where these separate cleanly (e.g. 125 kS/s, bin ~30 Hz).
- Math: d/dt of sine amp A freq f -> measured vpp within 5% of
  2*A*2*pi*f; integral of a +-A square (period T) -> triangle of vpp
  within 5% of A*T/2.
- Chirp: split the record in half; `estimate_frequency` of the second half
  > 2x the first half.
- XY: for `xy-lissajous-3-2`, freq(ch1)/freq(ch2) within 2% of 1.5.
- Trigger alignment: sim backend with Normal sweep, `sine-1k`, edge rising
  level 0: 5 consecutive frames each have `raw[2500]` within +-4 counts of
  the level and rising (raw[2510] > raw[2490]); with level 10 V (outside):
  `poll_frame` yields None 5 times in a row.
- Determinism: two identically-configured sources produce identical frames.

Keep the existing unit tests in lib.rs passing (adapt them to the new API
if their helpers changed, preserving what they verify).

### 6. Wire `set_stimulus`/`stimuli`

`SimBackend::set_stimulus` maps preset names via `Scenario::preset`
(returns Ok(false) for unknown), `stimuli()` returns the preset list.
Frame pacing and capabilities stay as they are.
