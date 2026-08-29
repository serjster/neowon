# Phase 6.5 pillar 4 — UI/UX research: scope-grade screen

Reference instrument: **Siglent SDS2000X Plus** (user choice, 2026-08-29).
Authority: `docs/SDS 2000 X Plus User Manual.pdf` (UM0102XP-E01A),
chapters 7 (Touch Screen Display), 8 (Front Panel), 9 (Function Recall).
Goal from PLAN.md: the app screen carries the *anatomy* of the reference —
same layout, same buttons; features we lack are omitted, features we add
follow the same visual language. Everything stays script-reachable
(`NEOWON_SCRIPT`) and the layout geometry is published as named ROIs.

Confidence key: **(M)** manual-verified; **(N)** general instrument-UI
convention adopted where the manual is silent.

---

## 1. Screen anatomy of the reference (manual chapter 7) (M)

```
+------------------------------------------------------------------+
| MENU BAR (drop-down menus; all functions reachable here)         |
+-------------------------------------------------------+----------+
|                                                       |          |
|   GRID AREA  (8 vertical x 10 horizontal divisions)   |  DIALOG  |
|   - trigger level indicator  (vertical, right edge)   |   BOX    |
|   - trigger delay indicator  (horizontal, top edge;   | (right   |
|     triangle flips when off-screen)                   |  side;   |
|   - channel offset indicators (channel #, left edge)  | collaps- |
|   - cursors                                           | ible     |
|                                                       | title)   |
+-------------------------------------------------------+          |
| DESCRIPTOR BOXES (touch -> opens the dialog):         |          |
| [C1][C2]...[D][F][Ref]  [timebase box] [trigger box]  |          |
+-------------------------------------------------------+----------+
```

- **Menu bar** (7.2): drop-down menus reach every function. Menu-only
  entries on the real unit: Utility > Help/Reboot, **Acquire > XY Mode**,
  Analysis > Mask Test / Bode / Power / Counter.
- **Grid area** (7.3): **8 vertical × 10 horizontal divisions** (we adopt
  this: display window shows ±4 div; the i8 encoding stays ±125 = ±5 div
  so traces can drive 1 div off-screen like the real unit). Indicators:
  trigger level marker (right edge), trigger delay marker (top edge),
  per-channel offset indicators (left edge, channel-colored).
- **Channel descriptor boxes** (7.4): located **under the grid area**, one
  per trace (C1…C4, D, F1-F2, Ref); content: channel index, coupling +
  input impedance, vertical scale, vertical offset, bandwidth-limit glyph,
  probe attenuation, invert glyph. Touching a box opens its dialog.
- **Timebase descriptor box** (7.5): resolution, horizontal scale
  (timebase), sample rate, samples, trigger delay.
- **Trigger descriptor box** (7.5): source, coupling, mode, level, type,
  slope.
- **Dialog box** (7.6): right side of the screen, main parameter area for
  the selected function; title bar touch collapses/expands; scrollbar when
  overflowing. Input widgets: Switch (2-state), List (popup), Virtual
  keypad/knob (numeric).
- Descriptor boxes + menu bar + front panel are three parallel ways to
  recall the same function (chapter 9).

## 2. Front panel anatomy (manual chapter 8) → virtual buttons (M)

The bottom strip of our screen reproduces the front panel in software
(same grouping, same button semantics):

- **Vertical** (8.2): channel buttons — press cycles disabled → enabled →
  (active) → disabled; Math button (press = toggle math + open dialog);
  Ref button (we omit Reference until Phase 7); shared vertical-scale and
  offset knobs (our ladder buttons / sliders in dialogs are the analog).
- **Horizontal** (8.3): timebase knob, Zoom button (omit — no zoom yet),
  Roll button (omit — roll not yet exposed), trigger-delay knob
  (press = zero → our trigger-position control).
- **Trigger** (8.4): trigger-menu button, **Auto / Single / Normal mode
  buttons**, level knob (press = 50% → we add a "50%" action), trigger
  status light: **Ready / Trig'd** (we render WAIT when Normal is starved).
- **Run/Stop** (8.5): yellow when Run, red when Stop. (M — note: Siglent
  uses yellow, not green, for Run.)
- **Auto Setup** (8.6).
- **Common function** (8.7): Search / Navigate / History / Decode — all
  omitted (not built yet; Phase 7/8).
- **Cursors** (8.8): cursors button + cursor knob.
- **Other buttons** (8.10): **Measure**, Save/Print (omit), Touch-lock
  (omit), Default, **Clear** (clears persistence + statistics),
  **Acquire**, **Display** (second press toggles Persist), **Utility**.
  AWG button → we omit (no AWG; the sim *stimulus* selector takes its
  place in the Display dialog, sim-only).

## 3. General UI/UX best practices applied (N)

- **Glanceability first**: run state, per-channel scales, timebase, and
  full trigger condition are visible without opening any dialog — that is
  exactly what the descriptor boxes + indicators provide.
- **Mode awareness**: starved Normal sweep shows WAIT, Single armed shows
  the armed state — never imply a live waveform on a frozen record.
- **One dialog at a time**, fixed right-hand zone; the waveform never
  reflows when dialogs open/close.
- **Color discipline**: each channel owns one hue across trace,
  descriptor box, offset indicator, measurements, cursors, and the
  trigger marker when it is the source.
- **Ladder-stepped scales**: V/div and s/div are discrete ladders
  (selectable labels), continuous widgets only for offset/level/width.
- **Keyboard + script parity**: every control has a script action
  (AGENTS.md rule); existing key bindings kept.
- **Fixed geometry**: all regions are compile-time constants; tests assert
  against the published ROI map.

## 4. Feature mapping (reference → neowon)

| Reference | neowon today | Disposition |
|---|---|---|
| Channel dialogs (scale/coupling/probe/offset/invert) | vdiv/coupling/probe/offset/enable | keep (invert omitted) |
| Timebase + trigger delay | rate + trigger position | keep, shown as s/div |
| Trigger dialog (edge/pulse/slope/video, level, holdoff, modes) | all present | keep |
| Acquire (sample/peak/average, XY mode) | acq modes; XY via trace mode | keep |
| Measure + statistics | 18 metrics + statistics | keep |
| Cursors | time/amp cursors | keep |
| Math dialog | +,−,×,÷,d/dt,∫ | keep |
| Display dialog (persist, intensity, graticule type) | persistence, intensity, trace mode | keep |
| Analysis > Mask Test | host-side pass/fail engine | keep (ours) |
| Utility | backend status, MULTI port, stimulus (sim-only) | keep/adapt |
| Search / Navigate / History / Decode / Ref / Zoom / Roll / AWG | not built | **omit** (until their phases) |

## 5. Named ROI map (published geometry, window 1520×820)

Screen-space rects (top-left origin), defined once in `ui/layout.rs`:

| ROI name | rect (x, y, w, h) | content |
|---|---|---|
| `menu_bar` | (0, 0, 1520, 36) | drop-down menus + status readouts |
| `plot` | (100, 103, 1000, 500) | waveform (texture readback = signal truth) |
| `descriptors` | (100, 603, 1000, 54) | channel + timebase + trigger boxes |
| `dialog` | (1200, 36, 320, 688) | the open dialog box |
| `front_panel` | (0, 724, 1520, 96) | virtual front-panel buttons |
| `trig_badge` | overlaid at plot right edge | trigger level indicator |
| `meas_overlay` | overlaid at plot bottom | measurement readouts |

(Signal pixel tests keep using the plot-texture readback, immune to
chrome; layout tests use the `layout` script action dumping this table.)

## 6. Display-geometry consequence (decision, 2026-08-29)

Adopting the reference's 8×10 graticule changes the pixels-per-count
mapping: the display window is ±4 div = ±100 counts, so
`row = (0.5 − raw/200)·(H−1)` (was `raw/250`); counts beyond ±100 pin at
the plot edge like a real scope overdriving the graticule. XY mode uses
the same scale on x. Encoding invariant unchanged: i8 ±125 = ±5 div of
ADC range. Affected: `waveform.wgsl`, graticule/trigger/cursor/mask
geometry in `main.rs`/`cursors.rs`, and the `ui_pixels` expectations
(updated accordingly).
