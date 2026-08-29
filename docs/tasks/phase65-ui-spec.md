# Phase 6.5 Track C: scope-grade UI (Siglent SDS2000X Plus anatomy)

You are restructuring the neowon-app UI from the current left collapsible
panel into the screen anatomy of the Siglent SDS2000X Plus, as documented
in `docs/ui-ux-research.md` (verified against the vendor user manual,
chapters 7–9). Read `PLAN.md` (Phase 6.5) and the research doc first. You
work ONLY in `crates/neowon-app/` (src/, tests/). Do not touch other
crates — their APIs are stable.

## Hard rules

- Files outside `crates/neowon-app/` are OFF LIMITS for writes (read is
  fine). PLAN.md/docs updates happen in the supervising session, not here.
- Never run anything that touches USB: `--sim` only.
- Every UI control must be reachable by `NEOWON_SCRIPT` (AGENTS.md rule).
  Adding a control without a script action is a spec violation.
- File budgets: 500 lines soft / 700 hard. `ui.rs` (755 lines) is split,
  never grown.
- Finish with `cargo build`, `cargo test --workspace`,
  `cargo test -p neowon-app --test ui_pixels -- --ignored`,
  `cargo fmt --all` and `cargo clippy --workspace --all-targets` all green.

## Target anatomy (docs/ui-ux-research.md §1–2, manual-verified)

Fixed window 1520×820; plot texture 1000×500. Regions defined ONCE in
`ui/layout.rs`, used by egui panels, Bevy placement, and tests:

- `menu_bar` (top, h=36) — drop-down menus (Acquire, Display, Analysis,
  Utility) + status readouts on the right: run-state badge (yellow RUN /
  red STOP / amber WAIT when Normal starved or Single armed), trigger
  status (Trig'd/Ready), backend name+serial, frame counter.
- `plot` (100, 103, 1000, 500) — waveform area, **8 vertical × 10
  horizontal divisions** (research §6: row = (0.5 − raw/200)·(H−1); XY
  uses the same scale on x). Overlays: trigger-level indicator at the
  right edge, trigger-delay marker at the top edge, channel offset
  indicators at the left edge, measurement readout boxes along the bottom.
- `descriptors` (100, 603, 1000, 54) — under the grid: channel boxes
  (`C1 500mV/div DC ×10`, channel hue, dimmed when off), then the
  timebase box (`Main 200µs/div  250kS/s  5000pts`) and the trigger box
  (`C1 Edge ↗ 2.50V Normal`). Each box is a button opening its dialog.
- `dialog` (1200, 36, 320, 688) — right-side dialog box: collapsible
  title bar + the controls of the open function. Always reserved; the
  waveform never reflows.
- `front_panel` (0, 724, 1520, 96) — the virtual front panel (manual
  chapter 8), grouped: [CH1][CH2][Math] | [Auto][Normal][Single][Force] |
  [Run/Stop][Single-shot][AutoSetup] | [Measure][Cursor][Acquire]
  [Display][Clear][Utility]. Features we lack are omitted entirely
  (Search/Navigate/History/Decode/Ref/Zoom/Roll/Save).
- `MenuState` resource: `Option<Menu>` with `Menu { Channel(usize),
  Horizontal, Trigger, Acquire, Display, Measure, Math, Cursor, Utility,
  PassFail }`; exactly one dialog open at a time.

## Work items

### 1. `src/ui/layout.rs`

Geometry constants + `Roi` table + `PLOT_CENTER` (Bevy world) + unit
tests. `PLOT_OFFSET` in main.rs becomes `ui::layout::PLOT_CENTER`.

### 2. Display-geometry switch to 8×10 (research §6)

`waveform.wgsl` sample_row / XY scale: `/250.0` → `/200.0` (with the same
clamp). main.rs: `H_DIVS=10`, `V_DIVS=8`, per-axis division sizes
(100×62.5 px); trigger line, offset indicators, pass/fail mask, and
`cursors.rs` y-mapping all follow. Update `tests/ui_pixels.rs`
expectations to the new mapping (assertions unchanged otherwise).

### 3. Split `ui.rs` → `ui/` module

`ui/mod.rs` (panel entry = table of contents + MenuState), `ui/layout.rs`,
`ui/menubar.rs`, `ui/frontpanel.rs`, `ui/descriptors.rs` (descriptor
boxes + measurement overlay), `ui/dialog_channel.rs`,
`ui/dialog_horizontal.rs`, `ui/dialog_trigger.rs`, `ui/dialog_acquire.rs`,
`ui/dialog_display.rs`, `ui/dialog_measure.rs`, `ui/dialog_math.rs`,
`ui/dialog_cursor.rs`, `ui/dialog_utility.rs` (incl. MULTI + pass/fail +
FFT toggle/window), `ui/widgets.rs` (ladder_combo, condition_combo,
shared labels). Move existing widgets verbatim first, restyle second.

### 4. Script parity (extend `script.rs`)

New actions (parse + execute; documented in the module header):

```text
autoset ; force ; holdoff <seconds> ; triglevel50
trigpulse <ch> <pos|neg> <gt|eq|lt> <width_us> <auto|normal|single>
trigslope <ch> <pos|neg> <gt|eq|lt> <width_us> <upper_v> <lower_v> <sweep>
trigvideo <line|field|odd|even|linenum> <line#> <sweep>
multi <trigout|pfout|trigin> ; pfout <0|1>
pf <on|off> ; pfsrc <slot> ; pftol <h_div> <v_div> ; pfcapture ; pfreset
cursor <time|amp> <on|off> ; cursorpos <0..3> <frac>
stats <slot> ; statsreset
fft <on|off> ; fftsrc <slot> ; fftwnd <window>
menu <channel <ch>|horizontal|trigger|acquire|display|measure|math|cursor|utility|none>
layout <path.json>
```

Deviation (recorded at completion): the pass/fail controls live in the
Utility dialog (no separate `passfail` menu entry); the `menu` action maps
pass/fail access to `menu utility`. All pass/fail controls remain script-
reachable via `pf`/`pfsrc`/`pftol`/`pfcapture`/`pfreset`.

### 5. Layout test (`tests/ui_layout.rs`)

Run the app with a script that opens each dialog in turn (`menu …` then
`layout stepN.json`), asserting per step: every named ROI equals the
published constants; the recorded open menu matches; plot center matches
`layout.rs` math. Module header documents the manual full-window check:
`screencapture -R x,y,w,h` over the window for visual review (not
asserted).

### 6. Restyle to the reference visual language

Dark theme; channel hues CH1 (1.0, 0.85, 0.1) / CH2 (0.2, 0.75, 1.0) /
math (1.0, 0.35, 0.85) shared with `gpu.rs`; descriptor boxes drawn as
rounded chips with the channel hue; run badge yellow RUN / red STOP /
amber WAIT; monospace numerals for readouts.

## Done when

- The screen shows menu bar / grid + indicators / descriptor boxes /
  dialog / front panel with the reference anatomy;
- no control exists that a script cannot drive; `ui_pixels` still green;
  `ui_layout` green; no file over budget; clippy/fmt clean.
