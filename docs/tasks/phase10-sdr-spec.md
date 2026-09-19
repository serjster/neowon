# Phase 10 — SDR backend & signal intelligence

neowon becomes one instrument with two modes: **Scope** and **SDR**. The SDR mode
is RTL-SDR, with signal detection, modulation analysis, classification,
protocol/source identification, advanced scanning, and a persistent catalog with
full management. Read `PLAN.md` §4 Phase 10 first; this spec is the contract and
the record of decisions. The researched feature program lives in
`docs/sdr-feature-catalog.md`, which this spec references rather than restates.

## Purpose (D0)

neowon is currently a hardware-verified oscilloscope. This program adds a second
instrument — a signal-intelligence receiver — because the reuse is concrete, not
asserted: `Acquisition::Stream` already exists (`neowon-audio` is the first
streaming backend), the recorder/timeline and phosphor engine are source-agnostic,
`neowon-dsp` is an engine-free oracle, and the control socket and MCP already
exist. What a scope-only project cannot do is answer *what is on the air*:
identify a signal, classify its modulation, name its protocol and source, and
remember it across sessions.

The counter-case is real and recorded (`docs/sdr-feature-catalog.md`): the
sub-phases past the backend are each their own project. D0 does not claim
otherwise — it claims only that 10.0–10.1 ride existing machinery, and gates
SDR-G1/SDR-G2 exist to stop the rest when the return is not there. `PLAN.md` §2 is
amended in the same commit: the SDR program is a goal, but it does **not** preempt
the scope's feature-parity work (Phases 8–9).

### Gates (a "stop when", not only a "done when")

A gate may only be passed on a recorded, numeric result. A failed gate stops the
program and is recorded in `PLAN.md`. These are `SDR-G1`/`SDR-G2` to avoid colliding
with the "G1/G2" gap labels in Phase 7.8.

- **SDR-G1 — PASSED 2026-09-18** on D1b (33 files / net 362, within the
  revised 35/1500) and P0.1 (`rs-rtl`, `pass:true`, docs/protocol-rtlsdr.md).
- **SDR-G1 — after 10.0–10.1.** The D1b spike completes within its command-checked
  budget and the P0.1 driver spike produces a tune/gain/stream readout on the V3
  dongle (D2). Nothing else; in particular no detection capability is a gate, so a
  peak-finder limit cannot falsely stop the program.
- **SDR-G2 — after 10.5.** The learned path clears the D9 floor on a held-out-
  frequency **and** cross-day split and beats `DspClassifier` by the D9 margin,
  else the learned path is **abandoned** (the DSP oracle ships). Failure also
  gates 10.7's learned fingerprinting; 10.8's dataset pipeline may continue
  (independent value) but its training targets pause. The corpus is collected from
  10.0 (D9), so it exists when the gate fires.

## Decisions (user-approved 2026-09-18)

- **D1a — separate `IqFrame` type (REJECTED).** Would fork every DSP consumer;
  retained only for the decision log; not a fallback; no work item designs it.
- **D1b — core sample representation (CHOSEN).** Frame-level `SampleLayout`
  (`Real`/`Complex`); channel samples `f32`; `Complex` interleaved I,Q;
  per-component calibration (`IqCal`) so IQ imbalance/DC offset round-trip. The
  scope's i8 ±125 convention survives as scope backends' **wire encoding**.
  AGENTS.md amended in the same commit (field names below).
- **D2 — driver: librtlsdr bindings (presumed).** P0.1 decides. Preference:
  **pure-Rust `rs-rtl` if it is gap-free on V3** (keeps the macOS "no libusb"
  advantage); otherwise `librtlsdr-rs` + system `librtlsdr`/libusb is an
  **accepted cost**, recorded with provisioning for a clean checkout and CI; if
  neither works, the program stops.
  **P0.1 outcome (2026-09-18): `rs-rtl` 0.5.0 passes** tune/gain/stream on the
  dongle (readout in `docs/protocol-rtlsdr.md`). It is pure Rust on `nusb`
  0.2, which the workspace already uses, so there is no libusb. Two facts
  changed since D2 was written: `librtlsdr-rs` 0.3 is no longer a binding
  (it is a pure-Rust port on `rusb`, i.e. libusb). And `rs-rtl` is **not
  gap-free**: no ppm correction, no RTL AGC, no direct sampling, no offset
  tuning. Licences are not a selection criterion (operator, 2026-09-18).
  **D2 decided (operator, 2026-09-18): an in-tree driver on `nusb`,
  ported from `librtlsdr-rs`** (`tmp-inspiration/librtlsdr-rs`, the porting
  reference; comparison readout in `docs/protocol-rtlsdr.md`). This is the
  `neowon-vds1022` pattern: one USB stack for both instruments, no C library on
  any CI platform. Scope: RTL2832U + R820T/R828D only (no E4000/FC00xx, no
  Blog V4 upconverter). Full ppm, RTL AGC, direct sampling and bias-T;
  several bulk transfers in flight, as `rs-rtl` does. Two departures from
  the reference: leaving direct sampling below the tuner's range does not
  retune (it leaves the device untuned for the caller to tune), and
  streaming owns only the bulk endpoint, so control calls stay on the owner
  while samples flow. `rs-rtl` stays only until the in-tree driver passes
  P0.1, then it is removed.
- **D3 — two classifier paths.** `neowon-dsp` classical oracle + `neowon-ml`
  learned path on ONNX Runtime (`ort`; fallbacks `candle`/`tract`), default-off
  cargo feature; feature-on CI job.
- **D4 — file-based catalog.** Single-writer manifest + append-only WAL + zstd
  blobs. `serde`/`serde_json` approved. No database dependency.
- **D5 — in-tree decoders only.** No subprocess wrappers this phase.
- **D6 — backend/config surface (plan-level).** `neowon-core` owns
  `Capabilities` (`enum { Scope(ScopeCaps), Sdr(SdrCaps) }`), `ScopeCaps`,
  `SdrCaps`, **`Acquisition`** (moved from `neowon-backend`, re-exported there so
  the caps types do not depend on the outer crate), and
  `InstrumentConfig::Scope(ScopeConfig) | InstrumentConfig::Sdr(SdrConfig)`.
  `Command`/`Event` carry `InstrumentConfig`; `Backend::apply(&InstrumentConfig)`.
  `Supervisor` keeps command coalescing. `neowon-sdr` supplies the `SdrDevice`
  port and depends on core/backend — the reverse never holds. Valid
  `SampleLayout × Acquisition` cells: **Real×Record, Real×Stream, Complex×Stream**;
  Complex×Record is rejected and enforced by `CaptureFrame::new`, which takes
  `layout` and `acq` and rejects that cell. A `Complex` frame's `AcqMode` is
  `AcqMode::Sample`.
- **D7 — canonical record owner.** `neowon-core` owns `SignalObservation`;
  `neowon-dsp` emits it; `neowon-catalog::Observation` is the persisted wrapper
  (identity + provenance) containing a `SignalObservation`; `neowon-ml` annotates.
- **D8 — determinism contract.** `neowon-sim` owns the deterministic IQ
  generator; `neowon-cli` depends on `neowon-sim` and exposes `sim iq`. Samples
  are a pure function of seed and index: `splitmix64` counter draws, no stateful
  PRNG, no `Instant::now()` on the sample path. Fixture: seed `1`, N `1024` complex
  samples, little-endian f32, interleaved I,Q, stored as raw bytes in
  `crates/neowon-sim/tests/fixtures/iq_seed1_n1024.f32` and compared **bit-exact**;
  a published `splitmix64` reference vector (seed 0 → `0xE220A8397B1DCDAF`) is
  asserted independently. No hash crate.
- **D9 — validation floor (single quantity: precision).** The learned path and
  fingerprinting must achieve **macro-averaged per-class precision ≥ 0.80 at
  SNR ≥ 10 dB** and **≥ 0.55 macro-averaged over the full SNR range**, on a
  held-out-frequency **and** cross-day split, and beat `DspClassifier` by
  **≥ 5 percentage points** on the held-out set. 10.7's open-set emitter
  separation is scored as open-set precision with the same floors. Below the floor
  the DSP oracle ships (SDR-G2). The corpus may be a **public over-the-air
  dataset** (provenance/licence recorded in `docs/protocol-rtlsdr.md`) and/or an
  **in-house capture starting in 10.0**; either supplies a cross-day split.
  Numbers are stated once, here; 10.5/10.7/phase-done reference D9.
- **D10 — centre vs tuned (CHOSEN, operator 2026-09-19).** The hardware
  window **Centre** and the operator's **Tuned** frequency are distinct and
  must not share a field. `Tuned` lives in `SdrState` (host-side), not
  `SdrConfig`: tuning inside the band must not mark the hardware config dirty.
  `sdr tune`/`sdr step` move Tuned and leave the window alone while Tuned is
  inside the band (amended 2026-09-19, operator bug report: a target outside
  the band — beyond 45% of the sample rate from Centre — recentres the window
  on it, and one outside a zoomed view pans the view); `sdr centre`
  moves the window; `sdr follow on` pins the window to Tuned (off by default);
  right-drag on the canvas moves the window by hand. The canvas shows a red
  tuned cursor carrying the frequency and a shaded **Width** band (auto from
  the nearest detection's OBW, or manual). `catalog add` with no argument files
  the channel at the tuned frequency — the detection covering it measured, else
  the tuned frequency with the manual width and operator provenance — never the
  hardware centre. Rejected: `VFO` as vocabulary (scope-first audience), a
  persistent follow in the default path (the UX critic's hidden-mode objection:
  it is off by default, and the window has its own explicit gesture), and
  recording the hardware centre. Audio (demod → cpal output) is its own
  sub-phase **10.10**, AM/FM first.
- **D12 — scope and SDR are separate workspaces (CHOSEN, operator
  2026-09-19).** A `Workspace { Scope, Sdr }` owns the screen geometry. Shared
  chrome is one app bar and one mode-aware front panel; the middle is per
  workspace with its own named ROIs — `sdr_view`, `sdr_dock`, `sdr_spectrum`,
  `sdr_waterfall` — and `sdr_view`/`sdr_dock` publish their own ROIs instead
  of borrowing `plot`/`descriptors`/`dialog`. The layout dump carries a
  `workspace` key and `Roi::for_workspace` gates the ROI set. Scope windows
  (spectrum, waterfall, 3D, effects, Measure/Cursors/Decode/PF) are **not
  constructed** in SDR; window and dock state is per workspace, so an SDR
  window never reappears in scope. The View menu and front panel are
  workspace-aware, and `ui_geometry` asserts the no-overlap invariant per
  workspace (scope: chrome vs `plot`; SDR: chrome vs `sdr_view`, with
  `sdr_spectrum`/`sdr_waterfall` tiling it).
- **D13 — automatic state persistence (CHOSEN, operator 2026-09-19).** The app
  auto-saves the existing NEOWON_SCRIPT session format to
  `~/.neowon/state.nws` (atomic `.tmp` + rename), extended to carry the
  workspace, UI scale, window size and position (new `windowpos X Y`), open
  windows and dock sections, and both scope and SDR settings. Save is
  debounced (~2 s after a change) and synchronous on graceful exit; restore
  runs after the backend connects. **Precedence is env > saved state >
  auto-fit**, and **auto-persistence is disabled when `NEOWON_SCRIPT` or
  `NEOWON_NO_STATE` is set**, so regression and geometry runs stay
  deterministic. Restored instrument values are validated against the
  attached `caps`; invalid ones are dropped with a visible status line. One
  global file, not per serial. File→Save setup stays a separate, named file.
- **D14 — channel width by filter-edge handles (CHOSEN, operator 2026-09-19).**
  Left-drag on a channel filter edge resizes Width symmetrically about the
  tuned frequency (live width tag, `ResizeHorizontal` cursor, 3 px dead-zone
  so a click never edits); left-drag elsewhere pans; shift+left-drag always
  pans. No new mode and no new script action (`sdr width` already covers it).
  **Landed 2026-09-19.**
- **D15 — channel-relative visualizations (CHOSEN, operator 2026-09-19).** A
  `sdr view wide|channel` selector (default wideband). Channel views are
  computed from the channelised, decimated baseband — **not** a crop of the
  wideband FFT — so the x-axis is offset from the tuned frequency, 0 Hz at
  the carrier, extent ±Width/2, and every label is anchored
  (`channel · 0 Hz @ <tuned>`), never a bare 0. Channel spectrum and
  waterfall, plus 3D Terrain/Phase/Tunnel/XyTime fed the channel baseband.
  Scope-only views are absent in SDR (D12).

## Existing code

- `neowon-backend::Backend::apply(&ScopeConfig)`; `Acquisition` and `AcqMode` live
  in `neowon-backend`/`neowon-core` (neowon-backend/src/lib.rs:17, :132, :179;
  neowon-core/src/lib.rs:40). D6 moves `Acquisition` into `neowon-core`.
- `CaptureFrame`/`ChannelCapture` store `raw: Vec<i8>` with
  `volts_per_lsb`/`zero_volts` (neowon-core/src/frame.rs:8, :54); i8 sentinels live
  in `neowon-dsp::timeline` (`NO_DATA`), `neowon-app::deep`, and the i32 GPU wave
  buffer.
- `neowon-audio` is the existing streaming backend; `neowon-sim` paces on
  `Instant::now()`. The control socket (`neowon-app/src/control.rs`) serves `get
  status|config|measure`; the recorder, timeline, phosphor and MCP exist.

## Scope fence

Work is confined to: `crates/neowon-core`, `crates/neowon-backend`,
`crates/neowon-dsp`, `crates/neowon-sim`, the new `crates/neowon-sdr`,
`crates/neowon-catalog`, `crates/neowon-ml`, `crates/neowon-app`,
`crates/neowon-mcp`, `crates/neowon-cli`, and docs: new
`docs/sdr-feature-catalog.md`, new `docs/protocol-rtlsdr.md`, and the existing
`docs/ui-anatomy.md` (extended, not created). Do not restructure `neowon-vds1022`
beyond D1b frame-construction call sites.

## Hard rules

- **Sim first.** Every feature has a deterministic sim oracle before hardware;
  automated runs use `--sim` only.
- **One code path.** Sim and hardware produce the same frame types/encoding.
- **Library crates engine-free** (no Bevy/GPU); `neowon-ml` defaults off and builds
  without `ort`.
- **Script parity.** Every UI control gets a script action; every catalog op gets a
  script action and an MCP tool.
- **Honest confidence.** Every result carries confidence and a trust state;
  `unknown` is first-class.
- **Derived, not recorded.** Every expectation is computed by the named command;
  no recorded snapshot decides behaviour.
- **File budgets** ~500 soft / 700 hard.
- **No new dependency outside the approved set** (`librtlsdr-rs` or `rs-rtl`,
  `ort` or `candle`/`tract`, `serde`/`serde_json`, the SigMF/JSON codec choice).

## D1b spike (`neowon-core/src/frame.rs`) — before any 10.0 work

```rust
pub enum SampleLayout { Real, Complex }

pub struct IqCal { pub scale_i: f64, pub scale_q: f64,
                   pub offset_i: f64, pub offset_q: f64 }  // Real: q == i

pub struct CaptureFrame {
    pub seq: u64, pub t_capture: Option<f64>,
    pub sample_rate: f64,          // Complex: pairs/s
    pub acq: AcqMode,
    pub layout: SampleLayout,      // the ONE layout home
    pub channels: Vec<ChannelCapture>,
}

pub struct ChannelCapture {
    pub ch: usize,
    pub data: Vec<f32>,            // Real: n scalars; Complex: interleaved I,Q (2n)
    pub cal: IqCal,
    pub clipped: bool, pub freq_meter: Option<f64>,
}
```

- `CaptureFrame::new(layout, acq, …)` rejects `Complex×Record` (D6).
- `duration()` = `data.len()/(2*sample_rate)` (Complex) or `data.len()/sample_rate`;
  `t_start` follows. `iq_at`/`iter_iq`; `volts = value * scale + offset` per
  component (`scale_i`/`offset_i` for I, `scale_q`/`offset_q` for Q).
- `.nwc` writes the frame `layout` and all four `IqCal` fields — identical to the
  struct, so IQ imbalance survives. New magic/version; reader keeps the old i8 path.

Per-consumer matrix (frame-level `layout` only; the spike adds any site it finds):

| consumer | i8-specific thing to redesign |
|---|---|
| `neowon-core/src/frame.rs` | `raw: Vec<i8>`; `duration`; `volts_at`/`iter_volts`; add `layout`+`IqCal`; `CaptureFrame::new` validation |
| `neowon-core/src/nwc.rs` | magic/version, `[i8;n]` reader → layout+`IqCal` branch |
| `neowon-core/src/owon_cap.rs` | constructs `ChannelCapture{raw, volts_per_lsb}`; reads `.raw` |
| `neowon-dsp/src/timeline.rs` | `Tiles{min/max:Vec<i8>}`, `NO_DATA=i8::MIN` → `Option`/NaN |
| `neowon-app/src/deep.rs` | `Reduced{pairs:Vec<i8>}`, `NO_DATA` |
| `neowon-dsp/src/fft.rs` | `spectrum(&[i8])` → `&[f32]` |
| `neowon-dsp/src/measure.rs` | `estimate_frequency(&[i8])`, envelope/measure signatures |
| `neowon-app/src/gpu.rs` + `shaders/waveform.wgsl` | i32-packed wave buffer; shader ÷125 |
| `neowon-app/src/refs.rs`, `record.rs`, export | channel capture copies |
| `neowon-dsp/src/decode` | threshold/digitize over i8 |
| `neowon-sim`, `neowon-vds1022` | frame construction (i8 wire preserved) |

**Budget (command-checked):** overrun if > 35 files, > 1500 net lines, or not green
within two working days. Command:
`git diff --stat $(git merge-base HEAD <spike-branch>)..<spike-branch>`; the
two-day box is a dated review. **Overrun stops the program (SDR-G1); D1a is not
invoked.** *(Revised 2026-09-18 by the operator: the census measured 27 files and
the spike landed at 33 crate files / net 362 lines; the original ≤18/≤450 was a
conservative guess.)* **Spike outcome:** done at 33 files / 729+/367− (net 362);
`cargo fmt/clippy/build/test`, `shaders`, and the ignored pixel/geometry/flow
tests all pass. Semantic decisions recorded in the spike report: audio keeps
f32 counts (not raw volts) to preserve fixed-range semantics; the GPU buffer is
`array<f32>` with `NaN` gap sentinels; `NO_DATA = f32::NAN`; `Complex` is
rejected against `Peak`/`Average` (the `Acquisition`-based check waits for D6).

### Capabilities enum change — its own matrix and budget (D6)

`Capabilities` is a public surface as wide as D1b; it gets a matrix and a fallback.
Sites: `neowon-backend/src/lib.rs` (trait/config/caps), `supervisor.rs` (Command/
Event), `neowon-app` (UI builds from caps), `neowon-sim`, `neowon-vds1022`,
`neowon-audio` (read), `neowon-cli`, `neowon-mcp`. **Budget:** ≤ 12 files, ≤ 250 net
lines; overrun falls back to a single `Capabilities` struct with
`scope: ScopeCaps, sdr: Option<SdrCaps>` and the fallback recorded here.
**Outcome (2026-09-18): enum shape kept, 10 files / 432+/199− (net 233).**
The types live in `neowon-core/src/instrument.rs` (with `ChannelConfig`/
`TriggerConfig`/`ScopeConfig`, which `InstrumentConfig` needs), re-exported by
`neowon-backend`. Scope backends take `&InstrumentConfig` and unwrap it with
`neowon_backend::scope_config`, which answers an SDR config with a transient
error. `Backend::autoset` still returns `ScopeConfig` (only scopes autoset);
the supervisor wraps it. The app keeps `Link.caps: Option<ScopeCaps>` and
reports an attached SDR as "SDR mode not supported yet" until 10.9's mode
switch, which kept the app's share of the change to `main.rs`.
`CaptureFrame::new` now takes the producer's `Acquisition` and rejects
`Complex` unless it is a `Stream` in `AcqMode::Sample`.

## Sub-phases and work items

### 10.0 — Backend, core IQ, sim, first views

D1b spike; D6 (types in `neowon-core`, `Acquisition` moved, `Command`/`Event`
carry `InstrumentConfig`); D8 generator in `neowon-sim` + `neowon-cli` depends on
it + `sim iq --seed`; views (spectrum, waterfall, IQ scope); debug surface
(schemas below); scripts `sdr tune|span|rate|gain|agc|ppm|mode|squelch`,
`sim iq --seed <s>`.

**Done when (mechanical):**

| quantity | unit | criterion | class | command |
|---|---|---|---|---|
| IQ bytes vs fixture, seed 1, N 1024 | bytes | bit-identical | bit-exact | `cargo test -p neowon-sim --test iq_determinism` |
| splitmix64 seed 0 first draw | u64 | equals `0xE220A8397B1DCDAF` | exact | `cargo test -p neowon-sim --test splitmix64_vectors` |
| two fresh processes, same seed | bytes | identical | bit-exact | `cargo run -p neowon-cli -- sim iq --seed 1 --n 1024 --out a.f32 && … --out b.f32 && cmp a.f32 b.f32` |
| D1b spike | files/lines | ≤35 / ≤1500 (revised) | exact | spike report |

**Status 2026-09-18: 10.0 DONE** (all four rows met; see Deviations for
what was deferred). Views, backends and scripts landed as described in
`PLAN.md` §4; `cargo test -p neowon-app --test sdr_mode -- --ignored`
drives them end to end on the sim.

`get iq` → `{seed:u64, n:usize, layout:"real"|"complex", bytes_fnv:u64}`;
`get detections` → `{centre_hz, bandwidth_hz, power_dbfs, snr_db, first_seen_s,
last_seen_s}[]`; `get modmeas` → `{symbol_rate_hz, evm_rms_pct, obw99_hz,
snr_db, cumulants:{c20:-f64,c21:-f64,c40:-f64,c41:-f64,c42:-f64,c63:-f64}}`;
`get classify` → `{label, confidence, trust:"validated"|"unproven"|"unprovable",
unknown:bool, top2_margin}`.

### 10.0 item 4 — in-tree RTL-SDR driver (`neowon-sdr::rtl`, D2)

Files under `crates/neowon-sdr/src/rtl/`: `usb.rs` (vendor control
transfers, demod/I2C/GPIO access, baseband init), `r82xx.rs` + `r82xx_tables.rs`
(tuner: init, filter calibration, mux, PLL, bandwidth, gain), `device.rs`
(`RtlSdr`: open/probe, rate, centre, ppm, direct sampling, AGC, gain,
bias-T, drop to standby), `stream.rs` (`Stream`: N bulk transfers in flight on
a thread, bounded channel, overflow counted not silent).

**Done when:**

| quantity | class | command |
|---|---|---|
| PLL/IF/rate register maths match the reference formulas | exact | `cargo test -p neowon-sdr` (unit, no hardware) |
| P0.1 on the in-tree driver: stream ±1% no drops, tune −300 kHz ±10 kHz, gain sweep monotonic and > 10 dB end to end | tolerance | `cargo run -p neowon-sdr --example p01` (hardware, manual) |
| ppm ±100 moves the band 2·(f + IF)·100e-6 ± 10% ; RTL AGC Δ > 5 dB ; HF direct sampling produces a peak > 10 dB ; leaving HF leaves the device untuned, not failed | tolerance | same example |
| `rs-rtl` removed from the workspace | exact | `! grep -q rs-rtl Cargo.toml` |

**Status 2026-09-18: DONE.** All rows met; readout in
`docs/protocol-rtlsdr.md`. Two criteria were corrected against hardware
rather than loosened. The gain row was "Δ > 10 dB", but the step size is
scene-dependent (the reference driver measures the same +14.5 dB at
98.3 MHz), so it became a monotonic sweep. The ppm row was 2·f·ppm, but the
correction acts on the LO at f + IF.

### 10.1 — Detection & measurement

`neowon-dsp::detect` (rolling-median floor, excess-threshold peaks, gap
clustering) emits `SignalObservation`; identity/tracking with debounce;
`neowon-dsp::modmeas` (99% OBW, channel power, SNR, flatness, instantaneous
A/P/F).

**Done when (mechanical):** `cargo test -p neowon-dsp --test detect_golden`,
fixture table (class tolerance):

| signal | seed | N | SNR | expected |
|---|---|---|---|---|
| 1.000 kHz tone | 7 | 8192 | clean | centre within ±½ bin; one peak |
| 10 ms burst at 0.5 fs | 7 | 8192 | 30 dB | present; bandwidth within ±1 bin |
| linear chirp 1→2 kHz | 7 | 8192 | 30 dB | bandwidth within ±2 bins |
| noise only | 7 | 8192 | — | zero peaks above threshold |
| transient 0.5×min_duration | 7 | 8192 | 30 dB | absent from active set |

**Status 2026-09-18: 10.1 DONE.** `detect_golden` passes all five rows;
readouts print as JSON (`-- --nocapture`). Interpretations are in
Deviations.

### 10.2 — Catalog v1 (`neowon-catalog`)

Model (D4/D7): opaque immutable ids; `Signal`, `Transmission`, `Source`,
`Emitter`, `BandPlanEntry`, `Survey`, `Observation` (wraps `SignalObservation`
with `signal_id`, optional `transmission_id`). Alias/rename are timestamped
edges; merge writes a redirect tombstone with cycle detection/compression and
bounded chain depth; `history(signal_id)` resolves redirects to the canonical id;
delete cascades only explicitly; purge refuses while referencing `Observation`s
exist unless `--cascade`, which writes tombstones; `pinned` first-class.

Persistence: single-writer actor; WAL with monotonic sequence, **length+checksum
framing**, **fsync before ack**, **fsync(temp) before rename + directory fsync**;
manifest `last_seq` watermark makes replay idempotent; a **checkpoint** compacts
the WAL (on clean close or every N records) and replay starts after the
watermark; durability = **zero loss for acknowledged writes**. **WAL segments are
versioned** (`format`/`schema`) so a v0 tail is checkpointed, not lost, on
upgrade. Ordered index `(signal_id, time, seq)` serves history/sparkline.

Migrations: manifest `"format":"neowon-catalog","schema":1`; v0 = no `schema` key,
`Emission` read into `Transmission`; version > current refused.

Provenance: `{kind ∈ {user,decoder,classifier,db,import,merge,fingerprint}, tool,
tool_version, timestamp: RFC3339, input_ref}`; `input_ref` follows redirects;
confidence only on derived values. Results ordered `(time, id)`.

Script: `catalog list|add|rename|delete|purge|merge|tag|alias|edit|bulk|undo|pin|
unpin|export|import`; MCP mirrors. UI: Catalog window.

**Done when (mechanical):**

| quantity | class | command |
|---|---|---|
| all entities field-exact round-trip | exact | `cargo test -p neowon-catalog --test roundtrip` |
| integrity query empty after merge | exact | `cargo test -p neowon-catalog --test integrity` |
| crash at WAL append / before-rename / after-rename-before-fsync (failpoint) | fixed-order | `NEOWON_CATALOG_KILL=<point> cargo test -p neowon-catalog --test crash` |
| v0 fixture loads, re-saves at v1 | exact | `cargo test -p neowon-catalog --test migrate_v0` |
| purge with a pinned entry → 0 pinned removed | exact | `cargo test -p neowon-catalog --test purge_pinned` |
| history under a merge redirect returns canonical rows | fixed-order | `cargo test -p neowon-catalog --test redirect_history` |

**Status 2026-09-19: 10.2 DONE.** All six rows pass
(`cargo test -p neowon-catalog`; `NEOWON_CATALOG_KILL=<point>` runs one
failpoint, and without it the crash suite runs all three). App and MCP
integration: `cargo test -p neowon-app --test catalog_flow -- --ignored`,
`cargo test -p neowon-mcp -- --ignored`.

### 10.3 — Modulation analysis

M1 constellation + recovery; M2 EVM/MER; M3 cumulants C20–C63; M4 cyclic
autocorrelation/SCD + symbol-rate/carrier-offset; M5 parameter estimation; M6
synchroniser + slicer → bits.

**Done when:** `cargo test -p neowon-dsp --test mod_estimators`, fixture table
(class tolerance): 16-QAM EVM at 30 dB = closed form ±0.5%; QPSK symbol rate 100
ksym/s within 1%; BPSK/QPSK C42 = published values ±0.02 (N=8192).

**Status 2026-09-19: 10.3 DONE.** `cargo test -p neowon-dsp --test
mod_estimators -- --nocapture`:
- 16QAM EVM 3.156% against the 3.162% closed form (±0.5 is read as
  percentage points);
- QPSK Rs 100 001.9 Hz;
- C42 −2.0000 (BPSK) and −1.0000 (QPSK) over 8192 recovered symbols.
In-app: `cargo test -p neowon-app --test sdr_modlab -- --ignored`.

### 10.4 — Scanning & survey

`Survey.coverage: Vec<BandCoverage>` with `BandCoverage{band, scanned: bool, bins,
threshold, truncated, peak_cap, selection_rule, retained_power_floor}`; an
unscanned or pruned peak is `unknown`, never `gone`.

**Done when:** `cargo test -p neowon-sdr --test survey_diff` (class fixed-order):
seeded new tone `new`, removed tone `gone`, raised tone `stronger`, truncated band
overflow `unknown`, unscanned band all `unknown`.

**Status 2026-09-19: 10.4 DONE.** `cargo test -p neowon-sdr --test
survey_diff -- --nocapture` passes the fixed-order row set (same, gone,
new, stronger; a truncated band's weak peaks unknown; a skipped band
unknown). `BandCoverage.band` is stored as `lo_hz`/`hi_hz`, and its
`retained_power_floor` is a finite sentinel (`f64::MIN`) when nothing was
cut, so catalog JSON stays valid. Coverage and peaks are clipped to the
requested range.

### 10.5 — Classification & recognition

C1 `DspClassifier`; C2 `MlClassifier` (default-off `ort`); C3 `unknown` + top-2 +
trust; C5 precision curves regenerated by a checked-in harness; C6 held-out
frequency; C7 preset labels; C9 corpus rotation; D9 floor (precision).

**Done when:** `cargo test -p neowon-dsp --test classify_golden` (statistical,
seed 42, N 500/class); `cargo run -p neowon-ml --features ort --bin eval -- --model
assets/models/<m>.json --split heldout_freq,cross_day --out <dir>` regenerates the
**precision** curve and asserts D9; `cargo test -p neowon-ml --features ort --test
gates` re-checks D9. If D9 fails, the record says the learned path is abandoned.

**Status 2026-09-19: 10.5 partial.**
- Done: C1 (`neowon_dsp::classify`), C3 (unknown, top-2, trust) and C7
  (preset labels). `cargo test -p neowon-dsp --test classify_golden --
  --nocapture` (statistical, seed 42, N 500/class) gives macro precision
  1.0 at 10 and 20 dB on the simulator.
- Blocked on a D9 corpus: C2 (`MlClassifier` on `ort`), C5 (precision
  curves from a checked-in harness), C6 (held-out frequency), C9 (corpus
  rotation), and SDR-G2. Choosing and collecting that corpus (a public
  over-the-air dataset, and/or in-house captures across days) is the
  operator's decision.
- Every DSP class stays `unproven` until then. The on-air run in
  docs/protocol-rtlsdr.md is why: broadcast FM once read 64QAM at
  confidence 0.95, and now reads `unknown`.

### 10.6 — Protocol & source identification

In-tree decoders; demod→bits; frequency/emission DB (ITU designators); source
records; correlation; exportable decode table.

**Done when:** `cargo test -p neowon-dsp --test decode_vectors` decodes an
independent reference vector per protocol
(`crates/neowon-dsp/tests/vectors/<proto>.bin`, not from our encoder); ISM fixture
`rtl_433-Acurite-986` fields; export `decode.csv` columns named in the test.
Class exact.

### 10.7 — RF fingerprinting

Embeddings; cheap physical fingerprints; protocol-truth bootstrap labels; emitter
DB; enrolment; drift; privacy off by default.

**Done when:** `cargo test -p neowon-ml --features ort --test sei_crossday` —
open-set precision at the D9 floors on the cross-day corpus; enrolment survives
restart; guardrails default off (asserted). Class statistical (declared seed/N).

### 10.8 — Dataset & training pipeline (parallel)

Builders, impairments, wideband compositing, hierarchical metadata, recipes +
SigMF/torchsig export, QC panel; all randomness a declared seed.

**Done when:** `cargo test -p neowon-sim --test dataset_recipe` — same recipe+seed
→ bit-identical dataset bytes; per-transform metadata truthfulness; occupancy
rectangles exact.

### 10.9 — UX and control/MCP parity

Mode switch/workspace; tuning widget; mode-aware controls; script/MCP parity; SDR
vocabulary in `docs/ui-anatomy.md`.

**Done when:** `cargo test -p neowon-app --test sdr_integration` — every UI
control and catalog op has a script action, and a scripted sim run drives tune →
detect → analyse → classify → decode → catalog → export. Class fixed-order.

### 10.10 — Demodulation & audio

The workspace's first cpal **output** sink and a streaming demod chain, so the
tuned channel is audible. Scope: **AM, NFM, WFM (mono)** first; SSB/CW after.

**D11 — receiver shape (CHOSEN, operator 2026-09-19).**
- `neowon_dsp::demod` is the engine-free oracle: a streaming `Receiver` takes
  interleaved complex IQ at the hardware rate and emits f32 audio at the sink
  rate. It owns the NCO mixer, a decimating FIR channel filter, the mode
  demodulator (AM envelope + slow AGC; FM quadrature discriminator + optional
  de-emphasis), and a windowed-sinc resampler to the sound-card rate.
- The tuned offset is `tuned_hz - centre_hz`; the filter uses D10's **Width**
  (the module supplies per-mode defaults). De-emphasis is 75 µs in the app and
  configurable (off in the deviation-golden tests, so the measurement is not
  filtered).
- `neowon-audio::sink` is the output. A thread owns the cpal stream (a
  `cpal::Stream` is not `Send`); the app holds a `Send + Sync` handle and
  pushes audio over a channel. Underruns are counted, never hidden; a missing
  device is a state, not an error.
- Demod/squelch/volume are host-side and do not enter `SdrConfig` (D6 note).
- Squelch gates the channel power; the status line distinguishes
  `playing | muted | squelched | no device | off`.

Work items:
1. `neowon-dsp/src/demod.rs`: `DemodMode`, `ReceiverConfig`, `Receiver`;
   resampler unit-tested on tones.
2. `neowon-audio/src/sink.rs`: `AudioOut` handle + thread; the buffer policy
   and underrun counter unit-tested without a device.
3. App: `SdrState.demod/volume/mute/squelch`, feed in `sdr::update`, dock
   **Audio** section, `get audio`, actions `sdr demod|volume|mute|squelch`.
4. MCP mirrors.

**Done when (mechanical):**

| quantity | unit | criterion | class | command |
|---|---|---|---|---|
| AM: 1 kHz tone, 30% depth | Hz / ratio | recovered tone ±1%; depth ±5% | tolerance | `cargo test -p neowon-dsp --test demod_golden` |
| NFM: 1 kHz tone, ±3 kHz deviation | Hz / ratio | tone ±1%; deviation ±5% | tolerance | same |
| WFM: 1 kHz tone, ±30 kHz deviation | Hz / ratio | tone ±1%; deviation ±5% | tolerance | same |
| Resampler 227.5k→48k | Hz / dB | 1 kHz tone ±0.1%; 30 kHz tone rejected > 40 dB | tolerance | same |
| Sink buffer | — | underrun counted; mute is exact zero; volume monotone | exact | `cargo test -p neowon-audio` |
| App audio states | — | demod on → `playing` with rms > 0; mute → `muted`; squelch above the signal → `squelched`; bad mode refused | fixed-order | `cargo test -p neowon-app --test sdr_audio -- --ignored` |

**Status 2026-09-19: 10.10 DONE.** `neowon_dsp::demod` (streaming NCO mixer +
decimating FIR + AM/NFM/WFM + windowed-sinc resampler) passes
`cargo test -p neowon-dsp --test demod_golden` and the in-module resampler
test (227.5k→48k: 1 kHz within 1 Hz, 30 kHz rejected > 40 dB). The cpal sink's
queue policy passes `cargo test -p neowon-audio`; the app path (`sdr
demod|volume|mute|squelch`, `get audio`, dock Audio section, MCP
`sdr_audio`/`sdr_demod`) passes `cargo test -p neowon-app --test sdr_audio --
--ignored`. Sim scenes `rf-am`/`rf-fm` added. SSB/CW deferred.

### 10.11 — Scope/SDR workspace split

Per D12: `Workspace`; SDR ROIs and `put()`; `workspace` in the layout dump and
`Roi::for_workspace`; workspace-aware View menu and front panel; scope windows
not constructed in SDR; per-workspace window state; `ui_geometry` per workspace.

**Done when:** `cargo test -p neowon-app --test ui_geometry`, extended to a
`--sdr-sim` case, asserts for each window×scale×workspace that no painted
chrome overlaps the workspace's primary view and every published ROI is
painted. Class fixed-order.

### 10.12 — Automatic state persistence

Per D13: extend the session emitter/replay with workspace, UI scale, window
size and position, open windows/dock, and SDR state; auto-save to
`~/.neowon/state.nws` (atomic, debounced and on exit); restore after connect;
env wins; off under `NEOWON_SCRIPT`/`NEOWON_NO_STATE`; validate against caps.

**Done when:** `cargo test -p neowon-app --test state_persist -- --ignored`
launches, changes scale/tune/demod/window, exits, relaunches and asserts the
state restored (fixed-order); a run with `NEOWON_SCRIPT` set writes no state
file (exact); an out-of-caps saved frequency is dropped with a status line
(fixed-order).

### 10.13 — Channel-relative visualizations

Per D15: expose the channelised baseband (reuse the `demod::Receiver`'s
mixer/filter/decimation) to the app; add `sdr view wide|channel`; channel
spectrum and waterfall on the same canvas with the anchored 0 Hz axis; feed 3D
Terrain/Phase/Tunnel/XyTime from the channel baseband; remove scope-only views
from SDR (D12).

**Done when:** `cargo test -p neowon-app --test sdr_channel_view -- --ignored`
switches wide/channel, asserts `get sdr` reports the view, and pixel-checks
that a tone at the tuned offset lands at the channel axis centre. Class
fixed-order + pixel.

## Verification contract

Every "Done when" is **quantity · unit · criterion · determinism class (exact /
fixed-order / tolerance / statistical) · command**, with a fixture row (signal,
seed, N, SNR) where a signal is involved. A criterion with no command is not met.
Runs emit a JSON readout cited by the report. Crash points use `NEOWON_*_KILL`
failpoints, never timing.

## Testing strategy

- Sim-first for every DSP/measurement/classifier/decoder path, seeded PRNG only,
  no wall clock on the signal path.
- `neowon-dsp` is the oracle; GPU/ML variants match within tolerance.
- Catalog: round-trip, integrity, crash (failpoints), migration; decoders get
  independent reference vectors.
- One integration test: tune → detect → analyse → classify → decode → catalog →
  export.
- Hardware smoke (manual, never CI, dongle on hand), required to close the phase:
  `cargo run -p neowon-cli -- sdr smoke --freq <hz> --json-out audit/rtlsdr-smoke.json
  --doc docs/protocol-rtlsdr.md` writes JSON
  `{dongle_serial, tune_hz, peak_hz, peak_tol_hz, class, confidence, decode, snr_db}`.
  FAIL if no peak within ±2 RBW, class `unknown`, confidence < 0.70, or empty
  decode.

## Done when (phase)

- All sub-phases' criteria met; SDR-G1 and SDR-G2 passed and recorded.
- `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test`,
  `shaders`, `ui_pixels --ignored` clean; `cargo test --workspace --features
  neowon-ml/ort` passes in the feature-on CI job, and the default-off build works
  without `ort`.
- A JSON hardware-smoke readout committed to `docs/protocol-rtlsdr.md`.
- `PLAN.md` §4 status updated; deferrals in `## Backlog`; deviations here.

## Open questions / risks

1. **D1b blast radius** — spike-guarded; overrun stops the program (SDR-G1).
2. **Driver** — P0.1 prefers gap-free `rs-rtl`; else librtlsdr/libusb accepted
   cost; both-fail stops.
3. **ML runtime cost** — `ort` default-off; feature-on CI job.
4. **Wideband IQ memory** — an IQ sub-budget of the 2 GB ring; honest gaps.
5. **Domain shift** — reference features/classifiers are sample-rate-aware; retrain
   before trusting (D9).
6. **Validation honesty** — in-distribution accuracy is not a result.
7. **Reassess by:** SDR priority is re-read at the close of Phase 9, before SDR
   implementation starts; raise it if the scope phase lands early or an external
   signal-analysis request arrives.

## Review fixes

Applied rounds 1–3; the round-by-round merge/fix record is the gitignored scratch
under `.critic/phase10-sdr-plan/` (not a committed link). Findings were fixed as
classes: one id namespace (`DM*` features vs `D0–D9` decisions vs `SDR-G1/G2`
gates), one field-name home (`IqCal`), one complex-layout schema, one command per
criterion.

## Deviations (recorded per AGENTS.md)

- **D8, platform-exact arithmetic (2026-09-18).** CI runs `cargo test` on
  Linux, macOS and Windows, whose libms disagree in the last bits of
  `sin`/`cos`/`ln`, so a bit-exact fixture cannot use them. The generator
  (`neowon-sim/src/iq.rs`) uses IEEE-754 basic operations only: a local
  Taylor sin/cos (error < 1e-13, tested against libm) and Irwin–Hall normal
  noise (12 uniforms; tails end at ±6σ). The fixture pins
  `IqScene::reference()` (0.5 FS tone at +100 kHz, 2.048 MS/s, 0.05 FS
  noise), which is therefore a stable preset.
- **D8, `get iq` deferred to the views item.** The app has no IQ source until
  the SDR views land; `neowon sim iq` prints the same `{seed, n, layout,
  bytes_fnv}` readout now (FNV-1a in-tree, no hash crate).
- **D2 driver, ported surface (2026-09-18).** Not ported: E4000/FC0012/
  FC0013/FC2580 tuners, the Blog V4 upconverter, offset tuning (librtlsdr
  refuses it on R82xx anyway), test mode, EEPROM. The crystal-cap 20p/10p
  table columns were dropped, because librtlsdr always runs the high-cap
  0p setting. Additions: `set_direct_sampling(Off)` restores the tuner's
  bandwidth **and gain**, which upstream loses because the tuner `init`
  resets every register; `RtlSdr::tuner_if_hz()` is exposed because ppm
  maths needs it.
- **10.0 scope (2026-09-18).**
  - `get iq` also reports `start`, the frame's first sample index, so a
    test can re-derive `bytes_fnv` from the D8 generator. Without it the
    fingerprint could only be compared against a recording.
  - `sdr mode` and `sdr squelch` are deferred to 10.3. Both act on a
    demodulator, and there is none yet; a control with no effect would be
    dishonest.
  - SDR frames bypass the recorder. IQ needs its own sub-budget of the
    ring (risk 4), so SDR history is the waterfall only for now.
  - The menu bar and front panel are still the scope's in SDR mode
    (mode-aware chrome is 10.9). The SDR view and controls draw over the
    plot and dock.
  - `--sdr-sim` / `--rtl` select the instrument at launch; switching at
    run time is 10.9.
  - `script/mod.rs` was already over the hard budget; it grew by 3 lines
    (one `Action` variant and a one-line arm). `main.rs` shrank below its
    starting size because backend selection moved to `launch.rs`.
- **10.1 detection (2026-09-18).**
  - *Floor:* the lower quartile over a window a quarter of the band wide,
    not a rolling median. On the dongle a narrow median sat inside a
    200 kHz FM station and hid it. The quartile is corrected to the noise
    mean (Wilson–Hilferty), and floored at −120 dBFS/bin.
  - *Blocks:* 50% overlap, continuous across frames. Non-overlapping Hann
    blocks hid a 10 ms burst on a block edge.
  - *Fixture parameters the table leaves implicit:* fs = 8192 Hz (N is
    one second), nfft 256 × 4 blocks per 125 ms frame, 32 Hz bins,
    min_duration 0.25 s. SNR is signal over total noise power.
  - *"10 ms burst at 0.5 fs":* read as centred at half the capture's
    duration. Its "bandwidth ±1 bin" is measured against the burst's own
    99% bandwidth, computed from its exact energy spectrum. The burst has
    raised-cosine edges: a rectangular gate's 99% bandwidth is set by the
    noise floor, so it has no truth to compare against.
  - *Transient:* checked as never active at any frame, not merely absent
    at the end, and its timed span must match the true one within a block.
- **10.2 catalog (2026-09-19).**
  - Undo is per session (an in-memory stack of exact inverses). Merge and
    purge cannot be undone and clear the stack; persisting undo across
    restarts was not asked for.
  - "zstd blobs" (D4) are deferred. Nothing attaches binary payloads yet;
    the first will be IQ snippets on observations.
  - The single-writer actor is an exclusive file lock plus `&mut`
    ownership. The app commits on its main thread; commits are
    user-paced, fsync-bound, and a few ms.
  - "MCP mirrors" is one `catalog` tool taking any script verb, plus
    `catalog_list` and `catalog_history`, rather than fourteen tools.
  - The entity tag in JSON is `entity`, not `kind`: `Source` has a `kind`
    field, and the collision only showed on WAL replay.
  - Deleting a signal also drops redirects that pointed at it (the
    merged-away ids keep their own tombstones).
- **10.3 modulation lab (2026-09-19).**
  - C40 and C41 are orientation-dependent: the published +1 for QPSK is
    axis-aligned, and the shared Gray QPSK sits at 45° (C40 = −1).
    `get modmeas` therefore reports complex cumulants as magnitudes. The
    spec's C42 row is rotation-invariant and unaffected.
  - SNR in the lab rows is symbol Es/N0 at the matched-filter output. The
    simulator's digital `amplitude` is defined there, which makes the EVM
    closed form exact (10^(−SNR/20)).
  - The QPSK symbol-rate row (no N in the table) uses 65 536 samples at
    20 dB.
  - The app lab assumes roll-off 0.35, analyses 32 Ki pairs of every 8th
    frame, and does not decimate. Blind phase leaves the constellation's
    rotational ambiguity: EVM and cumulant magnitudes are unaffected, but
    bits need a reference (M6 slices to Gray labels; resolving the
    ambiguity waits on 10.6 framing).
  - Modulation "auto" is nearest-(C42, |C40|): parameter estimation (M5),
    not the 10.5 classifier, and it carries no confidence.
- **D8, CLI vs. the hardware rule.** AGENTS.md says "no `neowon-cli`" for
  automated runs; `neowon sim …` never opens a device (dispatch opens USB per
  hardware subcommand), so the spec's two-process `cmp` criterion is safe to
  automate. The fixture is a binary file, committed by this spec's D8.
- **10.8 dataset pipeline (2026-09-19).** `neowon_sim::dataset`,
  `neowon sim dataset [--recipe r.json] [--seed] [--examples] --out dir`.
  - *Export:* one SigMF recording (`cf32_le`, examples back to back, one
    annotation per signal with `core:freq_lower/upper_edge` and
    `core:label`); the recipe and each example's drawn values ride in
    `neowon:` fields. torchsig reads SigMF, so there is no separate
    torchsig writer.
  - *Occupancy by definition:* RRC signals `Rs·(1+β)`, AM `±tone`, CW a
    line. FM has no finite band; its rectangle is Carson's `2·(Δf + tone)`
    (≈98% of the power), and the test holds FM examples to 97%.
  - *Time:* recipe signals are continuous, so every rectangle spans its
    whole example. Bursty builders come with a use for them.
  - *Seeds:* example `i` is `splitmix64(recipe.seed, i)`; signal `j` in it
    renders from `splitmix64(example, 2³² + j)`, because two digital
    signals on one seed would share a symbol stream.
  - *Pinned bytes:* the 16-example test recipe's FNV-1a is asserted, like
    the D8 fixture, so a platform or code change that moves a dataset
    fails CI.
  - *QC panel deferred:* the app has no dataset view. The QC is the
    `dataset_recipe` test (metadata re-measured from the samples). A panel
    waits on IQ playback in the app, since there is no IQ recording yet.
- **10.9 UX parity (2026-09-19), partial.**
  - *Instrument switch:* `instrument scope|sdr` (also the app bar's
    Instrument menu and MCP `instrument`). `mode` was already the trace
    mode (a stable verb), so the switch has its own verb. It works within
    the launch's family: sim ↔ sim-SDR, VDS1022 ↔ RTL-SDR, audio → sim-SDR.
    The old supervisor is shut down and joined before the new one claims
    a device.
  - *Parity by construction:* `SdrAction` and `CatalogAction` implement
    `Display` as their script line. The `every_action_round_trips` unit
    tests parse every variant back, and an exhaustive `variant()` match
    fails to compile until a new variant has a sample. The UI injects only
    these actions, so this is the "every UI control and catalog op"
    criterion.
  - *The chain test* (`--test sdr_integration`) runs scope → `instrument
    sdr` → tune → detect → analyse → classify → catalog → export → scope →
    SDR, with settings kept. **Decode is missing**: it is 10.6, which is
    blocked on reference vectors.
  - *Mode-aware chrome:* the app bar is mode-aware (SDR run state, tuning
    and gain, IQ frame counter). The front panel is still the scope's.
    The tuning widget is the dock's centre field and step buttons, plus
    click-to-tune on the spectrum and the signal list; there is no
    dedicated dial.
  - A `shot window` (whole-window PNG through Bevy's `Screenshot`) was
    tried and backed out: it wrote all-black frames on macOS/Metal here.
    UI state is verified through `get …` queries instead.
- **10.12 (2026-09-19, operator: "it should save last settings").** Landed
  ahead of 10.11 because the operator asked for it. `neowon-app/src/autostate.rs`.
  - Not persisted, on purpose: the sim stimulus and seed (test fixtures, and
    the instrument switch resets them), run/stop (an instrument that comes up
    stopped looks broken), and the instrument (the launch flags choose it).
    The workspace is not saved yet, because it does not exist until 10.11.
  - `NEOWON_STATE=<path>` moves the file (tests need their own), and
    `neowon-mcp --spawn-sim` sets `NEOWON_NO_STATE`, so an agent-driven sim
    cannot overwrite the operator's setup. Every automated app spawn in the
    test suites sets it too.
  - The replay needed new script actions (parity gaps the emitter exposed):
    `dock a,b|none` (the dock allows several open sections, but `menu` opens
    exactly one), `windowpos X Y`, and `measwin on|off` (the measurements
    window had no action at all). The `layout` dump now reports `scale`.
  - Caps validation: the scope's sample rate is checked against
    `caps.sample_rates`, and SDR frequencies go through the existing `sdr`
    range refusal. Either way the rejection reaches the status line, and the
    other lines still apply.
