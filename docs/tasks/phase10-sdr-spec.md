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
upgrade. Ordered index `(signal_id, time, observation id)` serves
history/sparkline (observations carry no WAL seq, so the observation id is
the tie-break).

Migrations: manifest `"format":"neowon-catalog","schema":1`; v0 = no `schema` key,
`Emission` read into `Transmission`; version > current refused.

Provenance: `{kind ∈ {user,decoder,classifier,db,import,merge,fingerprint}, tool,
tool_version, timestamp: RFC3339, input_ref}`; `input_ref` is plain text and is
never resolved (M27, 2026-09-23: an imported `import:#N` names another
catalog's entity, so resolving it through this catalog's redirects would be the
M15 cross-wire);
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
(class tolerance): 16-QAM EVM at 30 dB = closed form ±0.05 percentage points;
QPSK symbol rate 100 ksym/s within 0.01%; BPSK/QPSK C42 = published values
±0.02 (N=8192). *(Tolerances tightened 2026-09-22 per M21: the old ±0.5 pp /
1 % were two orders looser than the measured agreement.)*

**Status 2026-09-19: 10.3 DONE.** `cargo test -p neowon-dsp --test
mod_estimators -- --nocapture`:
- 16QAM EVM 3.156% against the 3.162% closed form (measured agreement 0.006 pp;
  the test's bound is ±0.05 pp);
- QPSK Rs 100 001.9 Hz (+0.0019%; the test's bound is 0.01%);
- C42 −1.9992 (BPSK) and −1.0000 (QPSK) over 8192 recovered symbols. These are
  the recovered-symbol measurements the command prints; −2.0000/−1.0000 are the
  ideal-constellation constants, asserted as such by `modlab::cumulants`' table
  test, not this row's output.
In-app: `cargo test -p neowon-app --test sdr_modlab -- --ignored`.

### 10.4 — Scanning & survey

`Survey.coverage: Vec<BandCoverage>` with `BandCoverage{band, scanned: bool, bins,
threshold, truncated, peak_cap, selection_rule, retained_power_floor}`; an
unscanned or pruned peak is `unknown`, never `gone`.

**Done when:** `cargo test -p neowon-dsp --test survey_diff` (class fixed-order):
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
  --nocapture` (statistical, seed 42, N 500/class) gives, on the simulator,
  macro precision / recall / unknown-rate of **0.9995 / 0.9567 / 4.29 %** at
  10 dB and **0.9994 / 0.9580 / 4.16 %** at 20 dB (measured 2026-09-23;
  both rows are asserted — the earlier "macro precision 1.0" quoted
  precision alone and rounded it).
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

**Done when:** `cargo test -p neowon-app --test sdr_integration -- --ignored` —
every UI control and catalog op has a script action, and a scripted sim run
drives tune → detect → analyse → classify → decode → catalog → export. Class
fixed-order. (The test opens a window, hence `-- --ignored`; sim only.)

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
  *Definitions (operator-accepted 2026-09-23, M9):* `peak_hz` is the centre of
  the detected signal's 99 % occupied band (an FM signal's strongest bin is a
  sideband, not its carrier); `decode` is the demodulated audio for AM/NFM/WFM or
  the recovered symbol labels for a digital signal, because 10.6's protocol
  decoders do not exist yet. The readout also carries `source` (`sim|rtl`),
  `centre_hz`, `pass` and `failures`, so a sim readout can never pass for the
  hardware one.
  FAIL if no peak within ±2 RBW, class `unknown`, confidence < 0.70, or empty
  decode.

## Done when (phase)

- All sub-phases' criteria met; SDR-G1 and SDR-G2 passed and recorded.
- `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test`,
  `shaders`, `ui_pixels --ignored`, and the Phase 10 gate lines in `harness.md`
  clean. (`cargo test --workspace --features neowon-ml/ort` was struck
  2026-09-22: `neowon-ml` is not a workspace member yet, so the command errored
  rather than testing anything; it returns with 10.7/10.8.)
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
(not a committed link). Findings were fixed as
classes: one id namespace (`DM*` features vs `D0–D9` decisions vs `SDR-G1/G2`
gates), one field-name home (`IqCal`), one complex-layout schema, one command per
criterion.

### Implementation round 1 (`phase10-implementation`, 2026-09-20)

One round over the Phase 10 build (6 seats, mean 6.85, 15 errors; scratch
`panel.md`/`triage.md`, not a committed link). Checklist names the merged finding ids (M-numbers).

**How to read it.** State is **per id**: every `M`-id has its own box under the
bullet that names it, ticked only when that id is wholly true in the tree, with
the date it was last checked and one clause of evidence (a command or a
file:line). A parent bullet is ticked only when every id beneath it is — so an
unticked parent over ticked children means *partly landed*, not *nothing done*.
An id whose box is unticked is open: either not fixed, or fixed only in part and
said so. Last full per-id pass against the tree: **2026-09-22**.
Landed so far: M1, M2, M3, M4, M5, M6, M7, M8, M10, M11, M12, M13, M14, M15, M16,
M17, M18, M19, M20, M21, M22, M23, M24, M26, M27, M28, M29, M30, M31, M32, M33, M34,
M35 (33 of 35; M25 is deferred by decision, not open). The
Evidence-honesty, Seams, Performance, port-race, Parity, Test-surface and Budgets
bullets are closed.

- [x] Catalog durability: a non-current headerless WAL segment must not brick
  `open` (+ `segment-create` failpoint); a CRC-bad mid-WAL frame is corruption,
  not a torn tail (M1, M2).
  - [x] **M1** — fixed 2026-09-23. A segment whose header is torn
    holds no record: `wal::read` returns `Ok(None)` and `Catalog::open` sets it
    aside (not current) or recreates it (current); checkpoint numbers past every
    segment on disk instead of deleting a leftover before the manifest swap, and
    its sweep deletes only its own names. New failpoint `segment-create`,
    exercised in a checkpoint and before a fresh `open`. Evidence:
    `cargo test -p neowon-catalog --test crash -- --nocapture` → 5 cases
    `"recovered":true`; before the fix both `segment-create` cases failed
    `no valid header`.
  - [x] **M2** — fixed 2026-09-23. Only a stretch with no valid
    frame after it is a torn tail; a damaged frame (bad CRC or bad length)
    with a valid frame behind it is `Corrupt` and nothing is truncated.
    Evidence: `cargo test -p neowon-catalog --test recovery` → 6 passed; before
    the fix probe E opened with 2 of 5 signals.
- [x] Cascade `delete` is not undoable — `undoable()` must say so, or the inverse
  lands; the unrelated-edit rewind probe gets a test (M3).
  - [x] **M3** — fixed 2026-09-23. `undoable()` is false for
    `Delete{cascade:true}`; an op with no exact inverse (also a pinned insert and
    a merge-target delete) clears the undo stack instead of skipping the entry.
    Evidence: `cargo test -p neowon-catalog --test undo_exact -- --nocapture`
    → 3 passed (probes B, B3, and every op kind); before the fix B failed
    `NotFound(#2)` and B3 rewound `"keep me"`.
- [x] The gate: `sdr_integration` gets `-- --ignored` in `harness.md` and 10.9's
  Done-when; the `neowon-ml/ort` line is removed or marked "member absent" (M4).
  - [x] **M4** — landed 2026-09-22, re-verified 2026-09-22:
    `harness.md`'s Phase 10 block runs
    `cargo test -p neowon-app --test sdr_integration -- --ignored`, 10.9's
    Done-when carries the same flag, and the `neowon-ml/ort` line is struck with
    a dated note (member absent until 10.7/10.8) in both `harness.md` and this
    spec's phase Done-when. Evidence:
    `rg -n "sdr_integration|neowon-ml/ort" harness.md docs/tasks/phase10-sdr-spec.md`.
- [x] Indexed history `(signal_id,time,seq)` and bounded refdb queries; no
  per-frame full scans in the catalog/Stations windows (M5).
  - [x] **M5** — fixed 2026-09-23 (catalog/refdb half, then Stations
    half). `State::history` is a range over a derived `(signal, t_start,
    observation id)` index (not stored); refdb `Index::query` visits only
    `within(lo,hi)`; the Catalog window's listing, counts and integrity are
    rebuilt once per commit and it draws a 500-row page. The Stations window keeps
    its rows as positions into the index keyed on `(Index::generation(), filters)`
    (`crates/neowon-app/src/refmap/rows.rs`), so `all`/`near` no longer query the
    set per frame. Evidence:
    `cargo test -p neowon-app --bin neowon-app refmap::rows` — 60 frames × 1 000 /
    10 000 / 100 000 stations, one build per change, 500 stations read per frame;
    with the cache disabled `All 100000: frame 1 read 100500 stations`.
- [x] Evidence honesty: C42 −1.9992 recorded as measured; PRS false-lock asserted
  on attempts; bare-tone case reaches an attempt; the classifier asserts its 10 dB
  row and reports recall/unknown beside precision; EVM/Rs tolerances tightened;
  FIB CRC known-answer vector; golden JSON readouts filed; IQ-fixture limits
  stated (M6, M19, M20, M21, M32, M33).
  - [x] **M6** — fixed 2026-09-22. The record carries the measured
    C42 (−1.9992 BPSK, −1.0000 QPSK) beside the command that prints it, and
    labels −2.0000/−1.0000 as the ideal constants: this spec's §10.3 status and
    `PLAN.md:482`. Evidence:
    `cargo test -p neowon-dsp --test mod_estimators -- --nocapture` prints
    `"c42":-1.9992` for BPSK.
  - [x] **M19** — fixed 2026-09-23. The false-lock half landed
    earlier; the bare-tone case now reaches an attempt and asserts it.
    `the_reference_scene_is_a_tone_not_an_ensemble` feeds
    `2 · (FRAME_SAMPLES + T_NULL)` — one attempt is guaranteed only there,
    because the null search ranges over the first `FRAME_SAMPLES` and a start
    it picks must still have a whole frame behind it — then asserts
    `frames_rejected == 1` and `last_attempt_metric() < PRS_METRIC_MIN`
    (measured 0.0282 against the 0.35 gate)
    (`crates/neowon-dsp/tests/dab_fic.rs:366-411`). Evidence:
    `cargo test -p neowon-dsp --test dab_fic -- --nocapture` → 8 passed; with
    the pre-fix `FRAME_SAMPLES` feed the same assert fails
    (`left: 0, right: 1`).
  - [x] **M20** — fixed 2026-09-23. The `if snr >= 20.0` guard is
    gone: both rows assert macro precision ≥ 0.99, macro recall ≥ 0.94,
    unknown rate ≤ 0.06 and, per class, precision ≥ 0.98 / recall ≥ 0.70
    (`crates/neowon-dsp/tests/classify_golden.rs:100-232`). Each bound is read
    off the run, not recorded — measured 0.9995/0.9567/4.29 % at 10 dB and
    0.9994/0.9580/4.16 % at 20 dB — and the record now carries recall and
    unknown beside precision (`PLAN.md:495-501`). Evidence:
    `cargo test -p neowon-dsp --test classify_golden -- --nocapture` →
    2 passed; a per-class recall floor of 0.80 (past the measured 0.776) fails
    **on the 10 dB row** — `64qam recall 0.7760 at 10 dB (floor 0.8)`.
  - [x] **M21** — fixed 2026-09-22. EVM/Rs bounds tightened to
    ±0.05 pp / 0.01 % (`crates/neowon-dsp/tests/mod_estimators.rs:76-100`); the
    measured agreement is 0.006 pp / +0.0019 %, the tightened run is green, and
    a deliberately tighter 0.001 pp / 1e-6 bound fails
    (`EVM 3.156252221849834 vs 3.162277660168379`). The id's other half, the
    FIB CRC known-answer vector for tier-1 row 5, is in the tree:
    `crc16(b"123456789") == 0xD64E` plus a single-bit-flip probe
    (`crates/neowon-dsp/src/dab/fec/mod.rs:448-452`). Evidence:
    `cargo test -p neowon-dsp --test mod_estimators` and
    `cargo test -p neowon-dsp --lib dab::fec`.
  - [x] **M32** — fixed 2026-09-23. Every golden run in
    `crates/neowon-dsp/tests/` files its readout as JSON through
    `common::file_readout` (`tests/common/mod.rs`), under
    `target/tmp/readouts/`, and prints the path as `readout: <path>`:
    `classify.json`, `detect-*.json` (6), `demod-*.json` (3),
    `modest-*.json` (4), `iq-*.json` (2). The filing is itself checked — the
    helper rejects an unbalanced document and re-reads what it wrote — so a
    readout that does not reach the disk fails its run. Evidence:
    `cargo test -p neowon-dsp --test classify_golden --test detect_golden
    --test demod_golden --test mod_estimators --test iq_fixture --
    --nocapture`, then `ls target/tmp/readouts/` → 16 files; a one-byte
    truncated write fails with `did not survive the round trip`.
  - [x] **M33** — fixed 2026-09-23.
    `crates/neowon-dsp/tests/iq_fixture.rs` states the D8 tone fixture's three
    limits (it proves determinism and portability, not correctness; it touches
    no byte of the `Digital`/RRC path the goldens run on; 1024 pairs supports
    no statistical claim) and closes the first two for that path with a
    closed-form property — the matched filter returns `amplitude ·` the
    transmitted symbol, worst error 0.00157 at amplitude 0.5 against a 0.002
    bound — and a cross-platform byte pin (FNV-1a 64 `0x2969C92EA83F481B`
    over 8 192 bytes plus the first pair's exact `f32` bits). Evidence:
    `cargo test -p neowon-dsp --test iq_fixture -- --nocapture` → 3 passed;
    roll-off 0.35 → 0.36 moves the pinned bytes and a symbol grid off by one
    takes the worst error to 1.00158. **Placed out of its natural
    home:** it belongs beside `crates/neowon-sim/tests/iq_determinism.rs`,
    which was outside that work item's scope fence; that file still carries no pointer
    to these limits.
- [x] `state_persist`'s control-port race classed (reserved port or echoed nonce)
  — M7.
  - [x] **M7** — fixed 2026-09-23. The token was *not* already the
    nonce: `launch` derived it from the port (`test-token-<port>`), so two
    launches on one port computed the same token and the second authenticated
    against the first's app. It is now unique per launch (pid + counter,
    `crates/neowon-app/tests/common/sandbox.rs::unique`), and `launch_on`
    proves the listener is its own by authenticating a probe connection
    before it hands back any connection — `launch_raw`'s unauthenticated one
    included, so reads are covered too; a stranger answers anything but
    `"authed":true` and the launch retries on a new port (3 attempts). The
    six suites that had their own `free_port` + connect loop (`accuracy`,
    `deep_view`, `ui_geometry`, `view_controls`, `decode_flow`, `sdr_mode`)
    now use `common::launch`, and `mcp_e2e` gives the MCP server and its app
    a shared per-run token. Evidence:
    `cargo test -p neowon-app --test isolation -- --ignored` → 2 passed; with
    the port-derived token restored,
    `a_launch_never_talks_to_an_app_it_did_not_start` fails with `a launch
    accepted port … held by another app`.
- [x] Parity: sample `Dab` and assert one count in `sdr/actions.rs`; a count
  guard in the refmap parity check (M8, M35).
  - [x] **M8** — fixed 2026-09-22, re-verified 2026-09-22: every
    `Dab` verb has a round-trip sample and the variant count is asserted
    (`assert_eq!(seen.len(), 28, …)`,
    `crates/neowon-app/src/sdr/actions.rs:607-622,638`); the spec's "fails to
    compile" claim is corrected in the 10.9 deviation below. Evidence:
    `cargo test -p neowon-app --bin neowon-app sdr::actions`.
  - [x] **M35** — fixed 2026-09-23. 10.9's decode deviation is
    recorded in the 10.9 entry below ("**Decode is missing**: it is 10.6 …"),
    and the refmap round-trip now has M8's guard: an exhaustive `variant()`
    match and `assert_eq!(seen.len(), 21, …)` over the samples
    (`crates/neowon-app/src/refmap/actions.rs`, `every_action_round_trips`).
    Evidence: `cargo test -p neowon-app --bin neowon-app refmap::actions` →
    2 passed; with the `CatalogStation` sample removed it fails `left: 20,
    right: 21`.
- [ ] Closure: `neowon-cli sdr smoke --json-out/--doc` exists and files the
  readout, or triage #22 is un-recorded and closure re-planned (M9).
  - [ ] **M9** — built 2026-09-23. **Hardware run done 2026-09-23 at
    99.4 MHz WFM: FAIL on all four rules** (peak 14.76 kHz off, `unknown` 0.066,
    no decode; filed in `docs/protocol-rtlsdr.md` with the analysis). **Open:**
    a passing readout needs a station the contract can pass on (AM, or a narrow
    digital carrier) or an operator decision on the contract for WFM. `neowon sdr smoke` (`crates/neowon-cli/src/sdr/`) tunes
    `--freq` − `--lo-offset` (default 250 kHz, off the DC spike), captures
    256 K pairs, detects (4096-bin RBW, so ±2 RBW = ±1 kHz at 2.048 MS/s),
    classifies and decodes through one pipeline fed by the RTL dongle or, with
    `--sim <scene>`, by `neowon-sim`; it writes the JSON atomically, appends a
    dated `## SDR smoke readout` section with a fenced JSON block to `--doc`,
    and exits non-zero naming each tripped rule. Sim evidence:
    `cargo test -p neowon-cli` → 14 + 3 passed (rf-am, rf-digital and a
    ±75 kHz FM pass; `no-peak`, `class-unknown`, `low-confidence` and
    `empty-decode` each trip on their own scene, and each rule's test fails
    with that rule disabled).
    Deviations from the contract: the JSON adds `source` (`sim`|`rtl`, so a
    sim readout is never the hardware one), `centre_hz`, `pass` and
    `failures`; `peak_hz` is the middle of the detected signal's 99 %
    occupied band (an FM signal's strongest bin and its power centroid both
    sit kHz off its carrier); `decode` is the demodulated audio (AM, NFM,
    WFM) or the recovered symbol labels (digital), since 10.6's protocol
    decoders do not exist, so a bare carrier (`cw`) FAILs `empty-decode`.
    To close it, with the dongle attached, on a station that is AM, FM or
    digital: `cargo run -p neowon-cli -- sdr smoke --freq <hz> --json-out
    audit/rtlsdr-smoke.json --doc docs/protocol-rtlsdr.md`.
- [x] Tracker and record: PLAN's Phase 10 block, the DAB budget figures and the
  README/ui-anatomy/spec claims all resolve to the tree (M10, M11, M34).
  - [x] **M10** — fixed 2026-09-22, re-verified 2026-09-22: PLAN
    records DAB-G1 passed (11C, 2026-09-20, `PLAN.md:568`) and the tiers 2–3
    landing (`PLAN.md:565-589`); the status block is re-dated 2026-09-22
    (`PLAN.md:428`). Evidence: `rg -n "DAB-G1|Status 2026-09-22" PLAN.md`.
  - [x] **M11** — fixed 2026-09-22, re-verified 2026-09-22: the
    tier-1 budget figures are re-derived (2 726 total / 765 inline test / 1 961
    code) and the no-op `git diff --stat` command is replaced by one that runs,
    in `docs/tasks/phase10-dab-spec.md`'s budget review. The panel's "refdb 40
    tests" did not reproduce — the tree has 45, so the record was left as is.
    Evidence: `for f in encoder fec fib fic fig mod ofdm receiver tables; do git
    show 3842782:crates/neowon-dsp/src/dab/$f.rs; done | wc -l` → `2726`;
    `cargo test -p neowon-refdb` → 45 passed.
  - [x] **M34** — fixed 2026-09-22, re-verified 2026-09-22: the
    Instrument-menu claims are the **SCOPE | SDR** switch (`README.md:215-219`,
    `docs/ui-anatomy.md:30-33`, and the 10.9 deviation below); the librtlsdr
    claims are the in-tree driver (`PLAN.md:28,87,598`); the next action has one
    home (§10.15 states what remains, `PLAN.md:590-592`; the bottom line is the
    single next-action statement, `PLAN.md:688`); the phase-close criteria
    include 10.14 and DAB-G1/G2 (`PLAN.md:612-618`); the DAB spec's deviations
    are renumbered 1–19 with no number used twice. Evidence:
    `rg -n "Instrument menu" README.md docs/ PLAN.md` (only `PLAN.md:538`, which
    is history) and `rg -n "librtlsdr bindings" PLAN.md README.md` (no matches).
- [x] Seams: supervisor clamp from `Capabilities`; layout×acq private fields +
  `new`; `sdr_config`; one `Option<Capabilities>`; `survey` out of the driver
  crate; ladders in one home; sim `Emitter` kind enum (M13, M14, M28, M29, M30).
  - [x] **M13** — fixed 2026-09-23. The sample grid comes from the
    instrument: `Capabilities::count_range()`
    (`crates/neowon-core/src/instrument.rs:140-153`) answers `Some((-128,127))`
    for a scope's i8 counts and `None` for a streaming SDR's full-scale f32,
    and the supervisor's `Averager` takes it at connect and rounds/clamps only
    when there is a grid
    (`crates/neowon-backend/src/supervisor.rs:108-155,195`). Evidence:
    `cargo test -p neowon-backend supervisor` →
    `averaged_samples_follow_the_instrument_range` (full-scale 0.4/0.9 survive;
    scope values round and hold at ±128/127) and `complex_frames_are_not_averaged`.
  - [x] **M14** — fixed 2026-09-23. `acq` and `layout` are private
    with `acq()`/`layout()` readers, so `CaptureFrame::new` is the only door
    (`crates/neowon-core/src/frame.rs:90-97,102-163`); the one mutation the
    supervisor needs goes through `with_acq`, which re-checks the matrix. All
    nine producers and the `.nwc` loader now construct through `new` — a file
    claiming `Complex × Average` is `InvalidData`, not a frame
    (`crates/neowon-core/src/nwc.rs:212-231`). Evidence:
    `rg -n "pub acq|pub layout" crates/neowon-core/src/frame.rs` (no matches)
    and `cargo test -p neowon-core` → 17 passed, including
    `frame::tests::with_acq_keeps_the_matrix`.
  - [x] **M28** — fixed 2026-09-23. `survey` is `neowon-dsp`'s
    (`crates/neowon-dsp/src/survey.rs`), the coverage record has an engine-free
    home in core (`neowon_core::BandCoverage`) with the catalog wrapping it as
    `CoverageRecord` exactly as it wraps `SignalObservation`, and
    `neowon-catalog`/`neowon-dsp` are gone from the driver crate's
    `[dependencies]`. `feed` guards `channels.first()` instead of indexing.
    Evidence: `sed -n '/^\[dependencies\]/,/^\[dev/p' crates/neowon-sdr/Cargo.toml`
    (core + backend + nusb only) and `cargo test -p neowon-dsp --test survey_diff`
    → 1 passed, plus `survey::tests::a_channel_less_frame_is_ignored_not_a_panic`.
    Note for 10.4's Done-when: that suite is now
    `cargo test -p neowon-dsp --test survey_diff` (the `-p neowon-sdr` spelling
    at §10.4 and `PLAN.md:486`'s `neowon_sdr::survey` are stale — left for the
    doc lane, not edited here).
  - [x] **M29** — fixed 2026-09-23. The app carries one
    `Option<Capabilities>` — `Link.caps`
    (`crates/neowon-app/src/main.rs:66-70`), read as its two halves by
    `Link::scope_caps()`/`sdr_caps()`
    (`crates/neowon-app/src/sdr/instrument.rs:12-28`); `SdrState.caps` is gone
    and its readers take `Option<&SdrCaps>` from the link, so no pair of fields
    has to be kept exclusive by hand. `neowon_backend::sdr_config` sits beside
    `scope_config` and both SDR backends use it
    (`crates/neowon-backend/src/lib.rs:45-52`). The stale "the app has no SDR
    mode yet" line is gone. Evidence:
    `rg -n "caps: Option<(Scope|Sdr)Caps>|no SDR mode yet" crates/neowon-app/src`
    (no matches) and `cargo test -p neowon-backend` → 5 passed, including
    `sdr_backends_refuse_scope_config`.
  - [x] **M30** — fixed 2026-09-23. Every advertised ladder has one
    home in `crates/neowon-core/src/ladders.rs` — the scope's rate and
    volts/div ladders and the RTL rate and R82xx gain ladders — read by the
    driver crates, both sims, the `.cap` importer and the UI fallbacks; the
    VDS1022's register tables stay in its own crate with a test tying them to
    the ladder (`crates/neowon-vds1022/src/consts.rs`,
    `voltbase_registers_match_the_shared_ladder`). The sim `Emitter` is one
    `EmitterKind` enum, so no emitter can be digital *and* analogue with
    `baseband()` silently preferring one (`crates/neowon-sim/src/sdr.rs:28-99`).
    Evidence:
    `rg -n "250e3, 1.024e6|2.5, 5.0, 12.5|0.005, 0.01, 0.02|0, 9, 14, 27" crates/ -g '*.rs'`
    → only `ladders.rs` (plus one unrelated test list in `instrument.rs`), and
    `cargo test -p neowon-core ladders` → 2 passed.
- [x] Budgets: split `sdr/actions.rs` (the parser is the second job) — M12.
  - [x] **M12** — fixed 2026-09-23 (parser split 2026-09-22).
    `sdr/parse.rs` holds the SDR parser (`actions.rs` 714 → 637). `main.rs`
    (952 → 233) keeps only the app wiring: the instrument link is `link.rs`,
    the plot texture/phosphor hand-off `plot.rs`, the window fit/layout/title
    `window.rs`, the gizmo overlays `overlays.rs`. `script/mod.rs`
    (741 → 142) keeps the queue and the `NEOWON_SCRIPT` loader: the `Action`
    vocabulary is `script/action.rs`, execution `script/run.rs`. Evidence:
    `wc -l crates/neowon-app/src/{main,link,plot,window,overlays}.rs crates/neowon-app/src/script/*.rs crates/neowon-app/src/sdr/{actions,parse}.rs`
    → every file under 700 (`actions.rs` 637; the rest under 500).
- [x] Import/export: absent-ref refusal, atomic export, `format` validation,
  refdb pair-write (M15, M26, M27).
  - [x] **M15** — fixed 2026-09-23. `exchange::problems` is the
    document's own integrity (repeated id, a reference absent from the document
    or of the wrong kind); `import` refuses by name before allocating an id, and
    the `unwrap_or(id)` local fallback is gone. Evidence:
    `cargo test -p neowon-catalog --test import_refs` → 5 passed; before, an
    import missing its signal bound the observation to local entities with
    `integrity []`.
  - [x] **M26** — fixed 2026-09-23.
    `import` refuses a `format` other than `neowon-catalog-export`; refdb
    `Store::load` loads each source on its own and names a corrupt one, and
    `meta.json` entries carry a digest of their snapshot (optional field; older
    files still load) so metadata of another fetch is reported, not shown; a
    refdb document every row of which is rejected no longer replaces the
    snapshot. Evidence: `cargo test -p neowon-refdb --lib store` → 7 passed;
    before, all four pair-write crash cases loaded stale metadata. The digest field was
    ratified by the operator 2026-09-23 (`PLAN.md` D32).
  - [x] **M27** — fixed 2026-09-23 by dropping the claim:
    `crates/neowon-catalog/src/model.rs` `Provenance` now says `input_ref` is
    opaque and never resolved (no producer writes a catalog id into it; the
    `import:#N` id belongs to another catalog's document, so resolving it would
    cross-wire). Evidence: `rg -n "never resolves" crates/neowon-catalog/src/model.rs`.
- [x] Performance: `--example frame_cost` (headless sim) priced against the 32 ms
  period; non-blocking audio spawn; overflows surfaced in `get sdr`; batch WAL
  sync; the straddling post-retune frame dropped (M16, M17, M22, M23, M24).
  - [x] **M16** — fixed 2026-09-23:
    `crates/neowon-dsp/examples/frame_cost.rs` prices the engine-free per-frame
    stages (`iq_spectrum`, `detect`+tracker, `survey::feed`, demod
    `Receiver::process`, DAB `push_iq`, plus the `FftPlanner` M25 names) on a
    deterministic sim scene, headless, against both the frame's own period and
    the 32 ms reference. Measured release, 102 400 pairs at 2.048 MS/s: 0.76 /
    1.39 / 0.87 / 1.08 / 0.71 ms, 3.02 ms display path, 4.81 ms with every
    consumer on — 6.6x inside 32 ms. Evidence:
    `cargo run --release -p neowon-dsp --example frame_cost`. Not priced (stated
    in the example's own header): the app's `mask_dc`/`columns`/waterfall map and
    texture upload, because `neowon-app` is a binary crate with no library
    target for an example to link against.
  - [x] **M17** — fixed 2026-09-22, re-verified 2026-09-22:
    `AudioOut::spawn` opens the device on its own thread and returns at once,
    and the opening thread's report is polled non-blockingly from every accessor
    (`crates/neowon-audio/src/sink.rs:107-187`, `poll()` at `:191-209`); it is
    used per frame with no wait (`crates/neowon-app/src/sdr/audio.rs:118`; the
    device now has one owner) and
    no `recv_timeout` remains on the audio/frame path. Evidence:
    `rg -n "recv_timeout" crates/neowon-audio crates/neowon-app/src/sdr` → no
    matches (the one left in the app is the control socket's command reply,
    `crates/neowon-app/src/control/mod.rs:115`, not the frame loop).
  - [x] **M22** — fixed 2026-09-23: the drop travels on the frame.
    `CaptureFrame::dropped_before()` (private, set through
    `with_dropped_before`) carries the units a producer knows it lost;
    `RtlBackend::poll_frame` fills it from `Stream::overflows()`, the
    `Supervisor` adds a frame it could not hand over (`carry_loss`), and
    `SdrState::note_frame` accumulates it into `dropped_pairs` /
    `drop_events`, which `get sdr` reports and the SDR dock shows as its
    *Drops* row. `sdr::dab::feed` splices on that count, with the coarse
    timestamp check kept only as the net for what a counter cannot see (DAB
    spec deviation 19, now closed). Evidence:
    `cargo test -p neowon-app --bin neowon-app sdr::dab::tests::a_reported_drop`
    → 1 passed; it fails both ways under mutation (splice ignoring the count:
    `left: 24576 right: 8192`; `get sdr` without the fields: "get sdr must
    report the gap").
  - [x] **M23** — fixed 2026-09-23: `wal::Writer::append_all` writes
    the batch's frames and fsyncs **once**; `Catalog::commit_many` is the batch
    path (`commit` is now one op through it), used by `catalog bulk …`
    (`crates/neowon-app/src/catalog/mod.rs`) and
    `exchange::import`. `Catalog::wal_syncs()` reports the fsync count, so the
    saving is measured rather than asserted in prose. Evidence:
    `cargo test -p neowon-catalog --test batch_commit` → 2 passed (three ops in
    one batch cost 1 fsync, the same three through `commit` cost 3; a refused op
    still leaves the prefix durable in one sync).
  - [x] **M24** — fixed 2026-09-23: `RtlBackend::apply` marks
    `straddling` when a centre/gain/AGC/ppm change lands under a running
    stream, and `plan_chunk` discards that chunk in `poll_frame` — counting its
    pairs into the next frame's `dropped_before`, so a discarded chunk is a
    reported hole rather than a silent one. Evidence (pure policy, no dongle):
    `cargo test -p neowon-sdr backend::tests` → 7 passed, including
    `the_straddling_chunk_after_a_setting_change_is_dropped_and_counted` and
    `a_usb_overflow_and_a_discard_add_up_in_one_report`.
- [x] Test surface: automated DAB control tests; `ui_pixels` sets its own
  catalog (M18, M31); 10.9's decode deviation recorded (M35, above).
  - [x] **M18** — fixed 2026-09-22, re-verified 2026-09-22:
    `crates/neowon-app/tests/sdr_dab.rs` drives the scene, `get dab`, the
    service/channel verbs and the DLS line over the control socket, negative
    paths included (4 tests, run by the gate), and `sdr_dab_audio` covers the
    unplayable-service error. Evidence:
    `cargo test -p neowon-app --test sdr_dab -- --ignored` → 4 passed.
  - [x] **M31** — fixed 2026-09-23, as its class rather than the
    catalog alone: every app a test spawns goes through
    `common::Sandbox::command` (`crates/neowon-app/tests/common/sandbox.rs`),
    which gives it a private `HOME` — so the catalog, `state.nws`, refdb,
    `location.json`, bandplans, the control token file and
    `~/neowon-captures` are all the sandbox's — drops every inherited
    `NEOWON_*`, and turns the control socket and saved state off unless the
    test sets them. `launch` and all eleven scripted/own-launcher suites use
    it; the two unit tests that called `RefMap::load()` (and created
    `~/.neowon/refdb`) use `RefMap::shipped_only()`. Evidence: with
    `HOME` pointed at a stand-in directory, `ui_pixels` at HEAD logs
    `catalog: <stand-in>/.neowon/catalog`, three of its four apps find it
    `open in another process`, and it writes `control/7777.token` there; after the fix the same run
    logs four private `neowon-home-ui-pixels-*` catalogs and leaves the
    stand-in empty (`m31-after.log`); `cargo test -p neowon-app --test
    isolation` checks the rule itself.
  - [x] **M35** — see the Parity bullet above (fixed 2026-09-23).
- [ ] Deferred: per-frame DSP churn (M25) — `PLAN.md` `## Backlog`, behind the
  `frame_cost` rig.
  - [ ] **M25** — **deferred** by operator decision (2026-09-20 triage), not
    open work: recorded in `PLAN.md` `## Backlog`. **Re-deferred with the
    measurement 2026-09-23**, now that M16's rig exists: the
    `FftPlanner` half is *closed* — measured 0.017 ms, 2.2% of the
    `iq_spectrum` call and 0.03% of the frame period, so caching the plan buys
    nothing — and what stays deferred is the waterfall texture re-upload
    (~25 MiB/s, D25's fix), whose GPU half is not measurable headless. The
    numbers and their command are in `PLAN.md` `## Backlog`.

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
    **SCOPE | SDR** toggle, `ui::menubar::mode_toggle`, and MCP
    `instrument`). `mode` was already the trace
    mode (a stable verb), so the switch has its own verb. It works within
    the launch's family: sim ↔ sim-SDR, VDS1022 ↔ RTL-SDR, audio → sim-SDR.
    The old supervisor is shut down and joined before the new one claims
    a device.
  - *Parity by construction:* `SdrAction` and `CatalogAction` implement
    `Display` as their script line. The `every_action_round_trips` unit
    tests parse every variant back; an exhaustive `variant()` match fails to
    compile until a new variant has an arm, and the `seen.len()` count
    assertion (`28` in `sdr/actions.rs`, `18` in `catalog/grammar.rs`) fails
    until it has a sample. The UI injects only
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
  - *Screenshots (2026-09-21):* `shot <path> [x y w h]` is now the whole
    window (egui included) through Bevy 0.19's `Screenshot`; `shotplot
    <path> [x y w h]` keeps the raw 1000x500 plot-texture readback the
    pixel tests assert on (the per-capture PNG button in the record dialog
    moved to it). The earlier `shot window` attempt wrote all-zero PNGs on
    macOS/Metal: Bevy's capture path reports success with a zeroed buffer
    when the window's view did not render into the capture texture that
    frame, and that attempt wrote the first image it got. The new path
    refuses an all-zero capture, retries it (bounded, with a watchdog for
    a capture that never returns), and reports a failed shot on the status
    line instead of writing black; `shot` also names the file it wrote in
    `get status`. Guarded by `cargo test -p neowon-app --test ui_capture
    -- --ignored` (SDR mode, asserts the dock fill, waterfall content and a
    non-black window).
- **10.12 (2026-09-19, operator: "it should save last settings").** Landed
  ahead of 10.11 because the operator asked for it. `neowon-app/src/autostate.rs`.
  - Not persisted, on purpose: the sim stimulus and seed (test fixtures, and
    the instrument switch resets them) and run/stop (an instrument that comes
    up stopped looks broken).
  - **Amendment (operator, 2026-09-19): the workspace mode is persisted.**
    The file ends with `instrument scope|sdr`; the launch flags still pick
    the family (simulators ↔ simulators, hardware ↔ hardware, `--audio` ↔
    the simulated SDR), and the saved mode picks which instrument of that
    family comes up. Asserted by `state_persist::the_workspace_mode_survives_a_restart`.
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
- **10.0 stream frame sizing (operator bug report, 2026-09-21).** Selecting
  250 kS/s made the spectrum/waterfall crawl (~2 rows/s) while 2.048 MS/s
  ran at ~16: the RTL stream used a fixed 256 KiB transfer (131 072 pairs)
  and the app draws one row per frame, so the cadence was
  `transfer_pairs / rate`.
  - The transfer length is now time-based: one frame per transfer at ~50 ms
    of samples, clamped to 32 KiB…256 KiB and aligned to 64 bytes
    (`neowon_core::stream_chunk_pairs`, `neowon_sdr::rtl::transfer_len`).
    Computed: 250 kS/s → 32 768 B (the floor binds, 65.5 ms, ~15 rows/s);
    1.024 MS/s → 102 400 B (50 ms); 2.048 MS/s → 204 800 B (50 ms, not the
    old 256 KiB ceiling). The brief predicted ~25 000 B at 250 kS/s and the
    ceiling at 2.048 MS/s, but its own stated 32 KiB floor makes the first
    impossible and its 50 ms formula the second; the formula and clamps as
    written are what landed.
  - `apply` restarts the stream when the sample rate changes: transfers in
    flight keep their old length, so without that a live rate change would
    keep the old cadence until a stop/start.
  - `Stream::chunk_pairs()` exposes the real chunk, and overflow accounting
    in `poll_frame` uses it instead of the removed `CHUNK_PAIRS` constant,
    so timestamps after dropped chunks stay true. `SdrCaps::acquisition` is
    recomputed on every apply; each frame's own `acq` remains authoritative.
  - `neowon-sim`'s SDR backend had the same rate-dependent cadence with its
    fixed 64 Ki-pair frames (262 ms per frame at 250 kS/s); it now uses the
    same shared helper, so sim and hardware show the same rows/s at any rate.
