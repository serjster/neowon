# Phase 7.7: horizontal buffer view + instrument widget kit

User-approved 2026-08-30 (conversation): shift+scroll should change the
horizontal *division* (a zoom window into the acquired record), not the
sample rate; UI needs a slider for it; icons/symbols should be drawn
vector graphics instead of font glyphs; rotary controls should back the
view parameters as the mouse-scroll substitute; and test coverage should
exercise every UI control's effect through the simulator.

## Hard rules

- Sim-first; deterministic; no new dependencies (Bevy + egui + WGSL only;
  icons are egui-painter vector paths, not a font or image asset).
- Every new control gets a script action and rides in sessions (script-
  parity rule). The existing `rate`/`trigpos` actions keep their meaning —
  acquisition parameters are untouched by this phase.
- Phosphor pixel tests keep passing; the accumulation buffer is cleared on
  view change (no stale phosphor at the wrong zoom).
- File budgets apply: widget kit lives in `src/ui/widgets.rs` extensions
  or small new `src/ui/*.rs` modules.

## Work items

### 1. Horizontal buffer view (the "zoom window")

The record (5000 samples on VDS1022/sim) is acquired once; the horizontal
division becomes a *display* window into it, like a scope's zoom mode —
instant, no re-acquire. Sample rate stays the hardware timebase control
(horizontal dialog, keyboard arrows).

- State: `Phosphor.hview: (center: f64, span: f64)` — span is a fraction
  of the record (default (0.5, 1.0); min span 0.01 = 100×; center clamped
  so the window stays inside the record).
- Rasterizer: `Params` gains `view_start`/`view_span` (f32). `raster` maps
  sample index through the window and skips samples outside it; segments
  crossing the edge clamp to the boundary. XY mode ignores the view (no
  time axis). Roll/WAV short records work unchanged (fraction of n).
- View change clears accumulation: render-side detects hview change and
  one-shots decay 0 (same path persistence-off uses).
- Gestures (`ui/touch.rs`):
  - shift+scroll = horizontal zoom of the view **at the pointer** (the
    spectrum window's anchor pattern) — replaces today's rate stepping;
  - waveform drag x = pan the view (instant; y stays channel offset);
  - trigger-position marker drag stays the acquisition `trigpos` control;
  - double-click on empty plot = reset the view (spectrum precedent).
- Keyboard: `H` home also resets the view; left/right arrows stay rate.
- Readouts: horizontal dialog shows the effective zoom s/div
  (`record_s × span / 10`) next to the Main s/div; menubar unchanged.
- Cursors map time→x through the view (both time-cursor lines and the
  delta readout stay record-relative; their screen positions follow the
  window).

### 2. Instrument widget kit (icons + knobs)

- `ui/icons.rs`: egui-painter vector icons (house, arrows, magnifier ±,
  knob pointer/caret), stroke-based, sized to the button. The dock
  toolbar's text glyphs (⌂ ← → ↑ ↓ V± H±) become icons.
- Rotary knob widget (new `ui/knob.rs` or widgets.rs): circular egui
  widget over a ladder or range — vertical/rotational drag moves it,
  scroll over the knob steps one rung, double-click restores its default.
  Lands on: V/div + offset (channel dialog), rate + trig position
  (horizontal dialog), and the new zoom-window span/center. Knobs are the
  front-panel substitute for mouse scrolling.
- Toolbar/dock: zoom-window slider (center when zoomed) + s/div readout
  joins the existing toolbar; home icon resets view AND acquisition view
  state (today's `home`).

### 3. Control-socket test harness for UI effects

The Phase 7.5 socket is the verification harness: one app launch per test
binary, drive via script lines / `get config` / `get measure`, assert
effects, `shot` for pixels.

- New integration test (extends `tests/control_socket.rs` or a sibling):
  - each view op (`hview`, `hzoom`, `pan`, `zoom`, `home`) changes `get
    config` state as documented;
  - pixel assertions through socket `shot`: pan shifts a DC trace, hzoom
    widens/narrows a square wave's visible periods, home restores;
  - knob/slider UI parity is covered at the unit level (widget fns are
    pure state mutations shared with script actions — one code path).
- Script additions: `hview <center> <span>`, `hzoom <in|out>`; sessions
  emit `hview`.

## Done when

Workspace + ignored suites green; shader naga-validates; every new
control script-reachable and session-persisted; PLAN.md §4 updated.

## Objective (user request, 2026-08-30)

- shift+scroll must change the **horizontal division** — a zoom window
  into the acquired record — *not* the sample rate; the record is the
  buffer that makes this instant ("it means we need a buffer").
- UI controllers/sliders for the window; vector-graphic (or baked-font)
  icons instead of text glyphs, which render as tofu on some platforms.
- Rotary controls as the front-panel substitute for mouse scrolls.
- Wider sim-based test coverage: every UI control's effect verified
  through the simulator, ideally via the Phase 7.5 control socket.

## Challenges hit (recorded per AGENTS.md)

- **Timebase vs zoom window.** The VDS1022 has a fixed 5000-sample
  record and its native timebase *is* the sample rate, so horizontal
  zoom was previously a re-acquire. The division is now a display window
  (`Phosphor.hview = (center, span)`); the trigger *position* stays the
  acquisition control (top-edge marker, `trigpos`), and horizontal
  drag/wheel/`pan` move the window instead.
- **Phosphor ghosts.** A view change must clear the accumulation buffer
  (old energy lives at the old zoom); reuses the persistence-off
  decay-0 one-shot in `prepare_buffers`.
- **Shader window mapping.** `view_start`/`view_span` uniforms replace
  the old `_pad2` slot (layout changed on both sides of the FFI — the
  Rust `ShaderType` and the WGSL struct must stay in lockstep); samples
  outside the window cull, edge-crossing segments clamp to the boundary
  column; XY mode ignores the window (no time axis).
- **Cursors** map record-fraction → screen through the window and hide
  when outside it.
- **Icons.** egui's bundled fonts subset per-platform; the dock toolbar
  now draws stroke-based vector icons (`ui/icons.rs`) — no glyph risk.
- **Compositor throttling.** Bevy's default `unfocused_mode` is
  reactive-low-power: an unfocused/obscured window stops updating, so
  automated test windows sat blank forever (and KWin throttles hard).
  Fixed with `WinitSettings { Continuous, Continuous }` — a scope keeps
  sweeping while you look away anyway.
- **Test races.** Socket shots can land before the first raster; the
  view test retries shots until pixels are lit. And `home` restores the
  startup 0.2 V/div, where the test's 1 V DC is outside the ±4-div
  window — the honest off-screen suppression hides it, so the test
  re-applies its scale before asserting the restored centring.

## Progress (2026-08-30, branch `phase77-view`)

Done and green:

- Horizontal buffer view end-to-end: `Phosphor.hview`, shader window,
  clear-on-change, `hview`/`hzoom` script actions, session emission,
  `hview` in `get config` (control socket), horizontal-dialog zoom
  group (s/div readout, window/centre sliders, zoom icons).
- Gestures remapped: shift+scroll zooms the window at the pointer,
  waveform-drag x pans the window (y stays offset), 2-D wheel x pans,
  double-click resets the window; `H`/home resets window + startup view.
- Widget kit: `ui/icons.rs` (Home, arrows, zoom ±, recenter),
  `ui/knob.rs` rotary widget (drag/scroll/double-click-default) on
  V/div, offset, and trigger position; dock toolbar is all icons.
- Cursors follow the window; `Pan` left/right now moves the window
  (content follows the arrow), up/down moves the offset.
- Tests: unit (view ops, knob rungs, hit-testing), socket integration
  `view_controls` (config JSON + pixel-level pan/home through `shot`),
  `ui_pixels` pan/home test, naga shader validation.
- Carried from the checkpoint commit (360bdd3, on local main): sim
  offset parity, graceful `AppExit` quit (NVIDIA teardown segfault).

Remaining:

- Full ignored-suite re-run on this machine (ui_pixels/ui_layout/
  control_socket/effects_pixels) after the view change.
- PLAN.md §4 status entry; README feature line.
