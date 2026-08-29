# Phase 6.5 Track D: responsive layout, pointer control, measurement guides

You are fixing the scope-grade UI's biggest usability gaps. Read `PLAN.md`,
`docs/ui-ux-research.md`, and `docs/tasks/phase65-ui-spec.md` for context on
the existing SDS2000X-Plus-style layout.

## Hard rules

- You may edit only `crates/neowon-app/` (src + tests). Read anything.
- NEVER read files outside the project directory (auto-rejected, may kill
  your session) — learn APIs from the existing code and cargo errors.
- Never run anything that touches USB (no hardware CLI, and launch the app
  only with `--sim` if you must — prefer not launching it at all).
- No `git commit`.
- Finish with `cargo build`, `cargo test`, `cargo fmt --all`, and
  `cargo clippy --workspace --all-targets` clean.

## Fact you must respect

The plot's visible vertical window is ±100 counts (±4 divisions of the 8-div
graticule). Volts→screen mapping for channel c:
`frac = 0.5 - raw / 200.0` (top of plot = +100 counts), where
`raw = (volts - zero_volts) / volts_per_lsb` and full scale (10 div worth of
counts = 250) corresponds to `volts_div * 10 * probe`.

## Work item 1: responsive layout (the window must be resizable)

`crates/neowon-app/src/ui/layout.rs` currently hardcodes WINDOW_W/H = 1520 x
820 and derives everything as consts; the plot sprite is placed once at
startup. Convert to a runtime layout:

- New `#[derive(Resource, Clone, Copy)] pub struct Layout { ... }` holding
  the same regions as today (menu bar, plot rect, descriptors, dialog,
  front panel, plus plot_center world-space Vec2 and div sizes), built by
  `Layout::compute(win_w: f32, win_h: f32) -> Layout`, a pure function:
  - menu bar: full width, fixed 36 px.
  - front panel: full width, fixed 96 px at the bottom.
  - dialog: fixed 320 px on the right (reserved whether or not open, as
    today).
  - plot: fills the remaining middle area minus the descriptor strip
    (54 px + 4 px gap under the plot) and an 8 px margin on each side —
    it STRETCHES (no fixed aspect; a scope grid's divisions are just
    rectangles). The plot texture stays 1000x500; scale the sprite with
    `Sprite.custom_size = Some(plot_size)` and reposition its Transform
    every frame (a small Update system reading `Res<Layout>`).
  - keep `Roi` and `dump_json` working off the runtime Layout.
- A system early in Update recomputes Layout from the primary window's
  logical size (only on change). Set
  `Window { resize_constraints: WindowResizeConstraints { min_width: 1100.0,
  min_height: 700.0, .. } }` in main.
- Replace every use of the old consts (`PLOT_CENTER`, `PLOT_OFFSET` in
  main.rs, cursors.rs, gizmo draw functions, egui panels in ui/*) with
  `Res<Layout>`. Delete `main.rs`'s `PLOT_OFFSET` const.
- Descriptor strip: the four descriptor boxes (C1, C2, timebase, trigger)
  currently collide at some sizes. Lay them out as a fraction of the plot
  width: C1/C2 20% each, timebase 32%, trigger 28%, with 4 px gaps, never
  overlapping.
- `tests/ui_layout.rs`: rework the geometry assertions to call
  `Layout::compute` at (1100, 700), (1520, 820), (1920, 1080) and assert:
  regions on-screen, no overlaps, plot at least 600 x 320, descriptors under
  the plot, dialog flush right. Keep the dialog-opening script test working
  (the `layout` script action should now dump the runtime layout).

## Work item 2: touchscreen-style pointer control (new `ui/touch.rs`)

Modern scopes are touch-first; map those gestures to the mouse. All input
gated on `Res<EguiWantsInput>` (see `cursors.rs`) and must not fight cursor
dragging (cursors take priority when the pointer is within 12 px of an
active cursor — reuse/extend the existing hit logic ordering:
cursor drag > trigger-level drag > waveform drag).

- Add `pub selected: usize` (0 or 1) to `Link` — the "active" channel.
  Clicking the C1/C2 descriptor box selects it (and still opens its dialog);
  the selected descriptor gets a brighter border.
- Drag starting within 12 px of the trigger-level line: drag the trigger
  level (convert dy with the ±4-div mapping for the trigger source
  channel), live-updating `link.config.trigger.level` (+ dirty).
- Otherwise, drag on the plot: vertical component adjusts the selected
  channel's `offset` by `dy / plot_h * 0.8` fractions (0.8 = 8 divisions of
  the 10-div encoding; the offset field is a fraction of FULL scale, ±0.5),
  clamped; horizontal component adjusts `config.position` by
  `-dx / plot_w`, clamped 0..1. Both live (dirty each change is fine — the
  supervisor coalesces).
- Scroll wheel over the plot: step the selected channel's volts_div down the
  ladder (scroll up = fewer volts/div = zoom in), using
  `caps.volts_div` when connected. Shift+scroll (or horizontal wheel):
  step `sample_rate` along `caps.sample_rates` (right/up = faster).
- Script actions `select <ch>` (sets Link.selected) so tests can reach it.

## Work item 3: measurement guides

When the Measure dialog is open OR a new `guides` flag is on
(`MeasureState.guides: bool`, default true, checkbox in the Measure
dialog, script action `guides <0|1>`): overlay gizmo guides for the
`stats_slot` trace on the plot:

- Horizontal lines at Vtop, Vbase, Vavg (channel color, dimmed alpha ~0.5),
  and fainter lines (alpha ~0.3) at the 10% and 90% levels between
  base and top (the rise-time thresholds).
- Draw them dashed: short segments every 12 px (gizmos have no dash style —
  loop x in steps, draw 6 px on / 6 px off).
- Skip lines outside the visible ±4-div window.
- Label each line (\"top\", \"base\", \"avg\") in the egui pass: tiny text
  right-aligned just inside the plot's right edge at the line's y, using the
  egui painter over the plot area (see how descriptors paint) — skip labels
  whose y would collide (within 10 px of a previous label).
- The measurement values come from `MeasureState.latest[stats_slot]`; volts
  →y via the mapping above (you need the trace's volts_per_lsb/zero_volts —
  take them from the latest frame's matching channel, slot 2 = math trace in
  `MathState.trace`).

## Work item 4: Display dialog additions

- Checkbox "CRT screen" bound to `Phosphor.crt` (already plumbed to the
  shader; script action `crt 0/1` exists).

## Keep working

- All existing tests (`cargo test`), including `ui_layout` (reworked, not
  deleted) and the shader naga test.
- The pixel tests read the plot texture and are layout-independent — do not
  touch `tests/ui_pixels.rs` or the shaders.
- `script.rs`: append the new `select` and `guides` actions following the
  existing match style; do not restructure the file.
- Use `crate::derived::fmt_si` for any numbers you print.
