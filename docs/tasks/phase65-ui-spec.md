# Phase 6.5 Track C: scope-grade UI (Siglent SDS2000X Plus anatomy)

You are restructuring the neowon-app UI from the current left collapsible
panel into the screen anatomy of a high-end bench scope, modeled on the
Siglent SDS2000X Plus. Read `PLAN.md` (Phase 6.5) and
`docs/ui-ux-research.md` first. You work ONLY in `crates/neowon-app/`
(src/, tests/). Do not touch other crates — their APIs are stable.

## Hard rules

- Files outside `crates/neowon-app/` are OFF LIMITS for writes (read is
  fine). PLAN.md/docs updates happen in the supervising session, not here.
- Never run anything that touches USB: `--sim` only.
- Every UI control must be reachable by `NEOWON_SCRIPT` (AGENTS.md rule).
  Adding a control without a script action is a spec violation.
- File budgets: 400 lines soft / 600 hard. `ui.rs` (755 lines) is split,
  never grown.
- Finish with `cargo build`, `cargo test --workspace`,
  `cargo test -p neowon-app --test ui_pixels -- --ignored`,
  `cargo fmt --all` and `cargo clippy --workspace --all-targets` all green.

## Target anatomy (from docs/ui-ux-research.md §1)

Fixed window 1520×820; plot texture 1000×500 (unchanged). Regions, in
screen pixels (top-left origin), defined ONCE in a new `ui/layout.rs` and
used by both the egui panels and the Bevy-side placement (sprite offset,
graticule, trigger line, cursors, pass/fail mask):

- `status_bar` — top strip h=36: run-state badge (green RUN / red STOP /
  amber WAIT when Normal-sweep starved or Single armed), trigger status,
  backend name+serial, sample rate, record length, frame counter.
- `plot` — waveform texture; centered in the area left of the menu zone.
  Signal rendering is unchanged; only its screen placement moves.
- `ch_badges` — left strip w=56: one badge per channel in the channel hue:
  `CH1 500 mV/div DC ×10` + offset; dimmed when disabled; click opens the
  Channel menu for that channel.
- `trig_badge` — right of plot: trigger source, level, slope glyph.
- `meas_strip` — h=28 directly below the plot: latest measurement readouts
  (Freq/Vpp per enabled channel + math), source-colored.
- `bottom_bar` — bottom strip h=32: timebase as s/div
  (= record_time/20, derived from rate + record length), trigger position,
  acquisition mode, trace mode, persistence.
- `menu_rail` — right column w=72: one button per context menu:
  Channel, Horizontal, Trigger, Acquire, Display, Measure, Math, Cursor,
  Utility. Exactly ONE menu open at a time (`MenuState` resource; clicking
  the open button closes it).
- `menu_panel` — fixed zone w=320 right of the plot (always reserved so the
  waveform never reflows): the open menu's controls.

Features we lack are OMITTED, not grayed out: no Decode button (Phase 8),
no Zoom/History. Sim-only controls (stimulus selection) live in the
Display menu and are hidden when the backend reports no stimuli.

## Work items

### 1. `src/ui/layout.rs`

Geometry constants (`STATUS_H: 36`, `BOTTOM_H: 32`, `MEAS_H: 28`,
`RAIL_W: 72`, `MENU_W: 320`, `BADGE_W: 56`), the named ROI table, and pure
functions `plot_center(window: Vec2) -> Vec2` (world coords for the Bevy
sprite/gizmos) + `roi(name, window) -> egui::Rect`. Unit-test the math.
`PLOT_OFFSET` (main.rs) becomes a value computed from `plot_center` at
startup for the current window size.

### 2. Split `ui.rs` → `ui/` module

`ui/mod.rs` (panel entry = table of contents only), `ui/layout.rs`,
`ui/status.rs` (status bar + bottom bar), `ui/badges.rs` (channel +
trigger badges, measurement strip), `ui/menus.rs` (rail + dispatch),
`ui/menu_channel.rs`, `ui/menu_horizontal.rs`, `ui/menu_trigger.rs`,
`ui/menu_acquire.rs`, `ui/menu_display.rs`, `ui/menu_measure.rs`,
`ui/menu_math.rs`, `ui/menu_cursor.rs`, `ui/menu_utility.rs`. Move the
existing widgets verbatim first, restyle second — behavior must not change
mid-split. The floating egui windows (Measurements table, Spectrum,
Pass/Fail) become the Measure/Math/Utility menu panels.

### 3. `MenuState` + rail

`#[derive(Resource)] pub struct MenuState { pub open: Option<Menu> }` with
`enum Menu { Channel(usize), Horizontal, Trigger, Acquire, Display,
Measure, Math, Cursor, Utility }`. Rail buttons toggle it; the menu panel
renders the matching section. Channel badges set `Menu::Channel(ch)`.

### 4. Script parity (extend `script.rs`)

New actions (parse + execute; document in the module header):

```text
autoset
force
holdoff <seconds>
trigpulse <ch> <pos|neg> <gt|eq|lt> <width_us> <auto|normal|single>
trigslope <ch> <pos|neg> <gt|eq|lt> <width_us> <upper_v> <lower_v> <sweep>
trigvideo <line|field|odd|even|linenum> <line> <sweep>
multi <trigout|pfout|trigin>
pfout <0|1>
pf <on|off> / pfsrc <slot> / pftol <h> <v>
cursor <time|amp> <on|off> ; cursorpos <idx> <frac>
meas <metric...> selection for the strip ; stats <slot> ; statsreset
fft <window> / fftsrc <slot> / fft <open|close>
menu <channel [ch]|horizontal|trigger|acquire|display|measure|math|cursor|utility|none>
layout <path.json>      # dump named-ROI rects + open menu as JSON
```

(`menu` and `layout` are the automation hooks for UI tests.)

### 5. Layout test (`tests/ui_layout.rs`)

Non-pixel UI verification: run the app with a script that opens EACH menu
in turn (via `menu …`) and ends with `layout out.json`; the test asserts:

- every named ROI rect equals the published constants (deterministic);
- each menu was openable (JSON records the open menu per step — add
  `menu <name>` checkpoints to the layout dump, e.g. the script runs
  `layout` after every `menu`);
- the plot center from JSON matches `layout.rs` math.

Also keep a scripted full-window `screencapture` recipe documented in the
test module header for manual/CI visual checks (not asserted).

### 6. Restyle to the reference visual language

Dark background (egui dark theme / custom colors), channel hues CH1
(1.0, 0.85, 0.1) / CH2 (0.2, 0.75, 1.0) / math (1.0, 0.35, 0.85) shared
between badges, strip, and the GPU colors in `gpu.rs`, monospace numerals
for readouts, badge chips drawn as rounded rects with the channel hue
border. Status-bar run badge colors: green RUN, red STOP, amber WAIT/SINGLE.

## Done when

- The screen shows status bar / badges / plot / measurement strip / bottom
  bar / rail + one context menu, with the reference scope's anatomy;
- no control exists that a script cannot drive; `ui_pixels` still green;
  `ui_layout` green; no file over budget; clippy/fmt clean.
