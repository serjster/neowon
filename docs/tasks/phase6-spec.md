# Phase 6 task: advanced triggers, pass/fail, MULTI port

You are implementing Phase 6 of neowon, a Rust oscilloscope app for the OWON
VDS1022I. Phases 0-5 are complete and hardware-verified. Read `PLAN.md` and
`docs/protocol-vds1022.md` first for context.

## Hard rules

- Do NOT run the app or any CLI command that talks to the USB device (no
  `neowon probe/dump/...`, no `neowon-app`). The maintainer verifies on
  hardware afterward. `cargo build` and `cargo test` are fine and required.
- Do NOT `git commit`. Leave changes in the working tree.
- Match the existing code style: sparse comments only for protocol
  constraints, thiserror errors, unit tests in the same file.
- `cargo build && cargo test` must pass when you finish. Run
  `cargo test 2>&1 | tail -20` and fix everything.

## Codebase map

- `crates/neowon-core` — shared vocabulary (Coupling, Slope, Sweep, AcqMode,
  CaptureFrame). Add new trigger vocabulary here.
- `crates/neowon-backend` — `ScopeConfig`/`TriggerConfig`, `Backend` trait,
  supervisor thread. UI talks to backends only through this.
- `crates/neowon-vds1022` — the driver. `consts.rs` (register map),
  `device.rs` (register writes, `set_edge_trigger` shows the existing
  pattern), `backend.rs` (maps ScopeConfig onto driver calls, diff-based
  `apply`).
- `crates/neowon-app` — Bevy app. `ui.rs` egui panel (see the existing
  Trigger section), `derived.rs` per-frame computed state, `main.rs` systems.

## Protocol facts (from the vendor app; treat as authoritative)

Register writes are `send(addr, width_bytes, value)`; the register file is
byte-addressed and little-endian.

### Trigger word (`SET_TRIGGER` = 0x24, u16), single-trigger mode

- bit 0: source is external (always 0 for us — channel source)
- bit 8 = trigger-type code bit 0, bit 14 = trigger-type code bit 1
- bit 13: source channel (0 = CH1, 1 = CH2)
- Type codes (hardware): Edge = 0, Slope = 1, Video = 2, Pulse = 3.
  (The Python reference has Slope/Video swapped — do NOT copy it.)
- EDGE:  bit 12 = slope (0 rise, 1 fall), bits 10-11 = sweep
  (0 auto, 1 normal, 2 single), bit 9 = 0.
- PULSE and SLOPE: bits 5-7 = condition code, bits 10-11 = sweep.
  Condition codes: 0 = positive/rising >, 1 = positive/rising =,
  2 = positive/rising <, 3 = negative/falling >, 4 = negative/falling =,
  5 = negative/falling <.
- VIDEO: bits 10-12 = sync mode (0 Line, 1 Field, 2 OddField, 3 EvenField,
  4 LineNum). NTSC/PAL and module bits are not fully decoded; implement the
  packing, mark clearly as hardware-unverified in a comment, and expose it in
  the UI with a "(unverified)" label.

### Per-type auxiliary registers (existing constants in consts.rs where noted)

- Pulse level: same registers as edge level (`SET_EDGE_LEVEL_CH1/2`
  0x2E/0x30), same (hi, lo = hi-10) packing — reuse the existing helper path.
- Pulse/slope width (FPGA >= V3, which this unit has): let
  `m = round(width_seconds * 1e8)` (units of 10 ns);
  write u16 `m & 0xFFFF` to `trg_cdt_gl` (CH1 0x42, CH2 0x46) and u16
  `m >> 16` to `trg_cdt_hl` (CH1 0x44, CH2 0x48). Add these four registers to
  `consts::reg`.
- Slope thresholds (`slope_thred`, CH1 0x10, CH2 0x12, u16): value =
  `(upper_raw & 0xFF) | ((lower_raw & 0xFF) << 8)` where upper/lower are the
  two i8 threshold levels (upper > lower). Add the registers. The
  freq-meter reference (`SET_FREQREF_*`) for slope should be the mean of the
  two levels.
- Video line number: `SET_VIDEOLINE` = 0x32, u16 (only meaningful for
  sync = LineNum).
- MULTI port: `SET_MULTI` = 0x06 (u8) already exists: 0 = trigger out,
  1 = pass/fail out, 2 = trigger in. Pass/fail TTL level: `SET_PF` = 0x07
  (u8), 0 or 1 — add the constant.

## Work items

### 1. neowon-core: trigger vocabulary

Add (with docs):

```rust
pub enum PulseCondition { PositiveGreater, PositiveEqual, PositiveLess,
                          NegativeGreater, NegativeEqual, NegativeLess }
pub enum VideoSync { Line, Field, OddField, EvenField, LineNumber }
pub enum TriggerKind {
    Edge { slope: Slope },
    Pulse { condition: PulseCondition, width: f64 },
    Slope { condition: PulseCondition, width: f64, upper: f64, lower: f64 },
    Video { sync: VideoSync, line: u16 },
}
```

(upper/lower for Slope are in volts, like the edge level.)

### 2. neowon-backend

- `TriggerConfig`: replace `slope: Slope` with `kind: TriggerKind` (keep
  `source`, `level`, `sweep`, `holdoff`). `level` stays the edge/pulse level
  in volts. Update `Default` (Edge, Rising).
- `Backend` trait: add `fn set_multi(&mut self, mode: MultiMode) -> Result<(), BackendError>`
  and `fn set_pass_fail_output(&mut self, level: bool) -> Result<(), BackendError>`
  with no-op defaults, plus `pub enum MultiMode { TriggerOut, PassFailOut, TriggerIn }`.
- `Command`: add `Multi(MultiMode)` and `PassFail(bool)`; the supervisor
  routes them to the trait methods (follow the ForceTrigger pattern).

### 3. neowon-vds1022

- `device.rs`: generalize `set_edge_trigger` into
  `set_trigger(&mut self, ch: usize, kind: &TriggerKind, level_volts: f64, sweep: Sweep)`
  building the word per the layout above and writing the per-type aux
  registers (edge/pulse level, width, slope thresholds, video line). Keep a
  thin `set_edge_trigger` wrapper so existing callers/CLI compile, or update
  the callers. Level/threshold volts→raw conversion follows the existing
  code (`(v / (range*probe) + offset) * 250`, clamped).
- `set_multi(mode)` and `set_pf_level(bool)` methods writing 0x06 / 0x07.
- `backend.rs`: map the new `TriggerConfig` in `apply` (extend
  `part_trigger`), implement the two new trait methods.
- Unit tests: trigger-word packing for each kind (edge rise CH1 auto = 0,
  pulse >400µs CH2 normal, slope, video), width split (e.g. 1 ms →
  gl 0x86A0, hl 0x0001), slope threshold packing.

### 4. neowon-app: UI + pass/fail engine

- `ui.rs` Trigger section: a kind selector (Edge/Pulse/Slope/Video) that
  shows the relevant fields — slope buttons for Edge; condition combo +
  width DragValue (µs) for Pulse; condition + width + upper/lower V for
  Slope; sync combo + line DragValue for Video (labelled unverified).
- MULTI port combo (Trigger out / Pass-fail out / Trigger in) sending
  `Command::Multi`.
- Pass/fail: new `PfState` resource in `derived.rs`:
  `{ enabled, source_slot, h_div: f64 (time tolerance in divisions),
     v_div: f64 (voltage tolerance in divisions), mask: Option<PfMask>,
     pass: u64, fail: u64, stop_on_fail: bool, output_multi: bool }`
  where `PfMask { lo: Vec<i8>, hi: Vec<i8> }` built from a captured
  reference trace: dilate horizontally by `h_div/20 * len` samples (min/max
  over the window), then pad vertically by `v_div/10 * 250` raw counts
  (saturating). A "Capture reference" button snapshots the current trace of
  the source slot. Evaluation per new frame (in `compute_derived` or a new
  system): every sample within [lo, hi] → pass else fail; update counts; if
  `output_multi`, send `Command::PassFail(result)`; if `stop_on_fail` and
  fail, set `link.config.running = false` and `link.dirty = true`.
  UI section with enable, tolerances (DragValue in divisions), capture
  button, pass/fail/total counts, reset button.
- Draw the mask bounds as two gizmo polylines (dim green) when enabled —
  follow `draw_trace`-style mapping in `main.rs` (see `draw_graticule` for
  the PLOT_OFFSET convention).
- Unit test for the mask build + evaluation (pure functions in
  `derived.rs` or a new `pf.rs` module).

### 5. Keep working

- CLI `--sweep`/trigger flags keep compiling (edge only there is fine).
- `cargo test` green, `cargo build` green (the naga shader test must still
  pass — you are not touching shaders).
