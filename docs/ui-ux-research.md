# Phase 6.5 pillar 4 — UI/UX research: scope-grade screen

Reference instrument: **Siglent SDS2000X Plus** (user choice, 2026-08-29).
Goal from PLAN.md: the app screen carries the *anatomy* of a high-end bench
scope — same layout, same buttons; features we lack are omitted, features we
add follow the same visual language. Everything stays script-reachable
(`NEOWON_SCRIPT`) and the layout geometry is published as named ROIs for
tests.

Confidence key: **(V)** vendor-documented / widely reproduced; **(C)**
common to this scope family, verified against screenshots where possible;
**(N)** general instrument-UI convention. Items marked **(T)** must be
verified against the real instrument/manual before pixel-exact work.

---

## 1. Screen anatomy of the reference (SDS2000X Plus)

The SDS2000X Plus is a 4-channel bench scope with a 10.1" touch display.
Its on-screen layout (what we reproduce):

```
+------------------------------------------------------------------+
| STATUS BAR: run state | trig status | acq mode | rate | clock    |  <- (V)
+----+-----------------------------------------------------+-------+
| CH |                                                     | MENU  |
|badg|                 WAVEFORM AREA                       | PANEL |  <- (V)
| es |              (graticule + traces)                   | + 5   |
|    |                                                     |softkey|
+----+-----------------------------------------------------+-------+
| MEASUREMENT STRIP (dboxes: value + source color)         |       |  <- (C)
+------------------------------------------------------------------+
| BOTTOM BAR: timebase (Main/Zoom s/div) | trig position marker    |  <- (C)
+------------------------------------------------------------------+
```

Physical control surface mapped onto screen regions:

- **Channel section** — one colored button per channel; selecting it opens
  the vertical menu (V/div, offset, coupling, probe, invert, bandwidth
  limit) in the right panel and drives the channel badges. (V)
- **Horizontal section** — timebase (s/div), trigger delay/position, zoom
  (Zoom on/off). (V)
- **Trigger section** — source, type (edge/pulse/slope/video/…), slope,
  level, holdoff, sweep mode (auto/normal/single), force. (V)
- **Acquire** — sample mode (normal/peak/average), memory depth, sample
  rate readout. (V)
- **Measure** — measurement boxes, statistics table, source selection. (V)
- **Cursor** — manual time/amplitude cursors with readouts. (V)
- **Math** — math operator trace + its own vertical scale. (V)
- **Decode** — protocol decode (we omit until Phase 8). (V)
- **Display** — persistence, intensity, graticule, trace mode
  (vectors/dots), XY. (V)
- **Utility** — configuration, calibration, system info. (V)
- **Run control** — Run/Stop (with colored state), Single, Force,
  AutoSetup. (V)

Key structural rules we adopt:

1. **One context menu at a time**, in a fixed right-hand panel next to a
   column of 5 softkeys. The waveform never gets pushed around by menus.
2. **Badges over dialogs**: continuous state (per-channel scale, coupling,
   probe, trigger level, timebase) is always visible as small colored
   badges at the plot edge — never hidden inside a menu. (C/N)
3. **Run state is ambient**: the status bar and the Run control change
   color with state (green running, red stopped, amber waiting-for-trigger
   / single armed). (N)
4. **Color discipline**: each channel owns one hue used everywhere —
   trace, badge, measurements, cursors, trigger marker when that channel
   is the source. (N)
5. **The graticule is the coordinate system**: 10 vertical divisions;
   horizontal divisions fill the plot width; center axes marked. Readouts
   are per-division (V/div, s/div), never per-pixel. (N)

## 2. General UI/UX best practices applied

- **Glanceability first**: a user should read run state, both channels'
  scales, timebase, and trigger condition without opening any menu — this
  is the entire reason badges + status bars exist on real scopes.
- **Mode awareness**: when acquisition is starved (Normal sweep, no
  trigger) the screen must say *Waiting* rather than imply a frozen
  waveform. Our sim already starves correctly (Phase 6.5 pillar 1); the UI
  must surface it.
- **Direct manipulation where cheap**: egui sliders/drag-values for
  offset/level/width; discrete ladder buttons (selectable labels) for
  V/div, s/div, probe — scopes are ladder-stepped, not continuous.
- **No destructive defaults**: nothing in the UI can silently change the
  device beyond the config model; auto-set announces itself (it arrives as
  `Event::ConfigUpdated`).
- **Keyboard parity**: every panel control also has a key binding or a
  script action (AGENTS.md rule: a UI control with no script action is a
  bug).
- **Fixed geometry**: the layout never reflows; regions are constants so
  tests can assert on them (named ROIs below).

## 3. Feature mapping (reference → neowon)

Keep / add (ours follow the reference visual language):

| Reference section | neowon today | Disposition |
|---|---|---|
| Channel menu | vdiv/coupling/probe/offset/enable | keep, as badge + menu |
| Horizontal | rate + trig position | keep (rate shown as s/div) |
| Trigger | edge/pulse/slope/video, level, sweep, force | keep |
| Acquire | sample/peak/average | keep |
| Measure | 18 metrics + statistics | keep |
| Cursor | time/amp cursors | keep |
| Math | +,−,×,÷,d/dt,∫ | keep |
| Display | persistence, vectors/dots/XY, intensity gain | keep (ours: extend) |
| Utility | backend status, MULTI port mode, pass/fail setup | keep (ours) |
| Run/Stop/Single/Force/Autoset | present | keep, into status bar |
| Stimulus selection | sim presets | **add** (Display/Utility menu; sim-only, hidden on hardware) |
| Decode | not built | **omit** until Phase 8 |
| Zoom, history, mask-test HW | not built | **omit** (host pass/fail mask stays in Utility) |

Omitted controls must not appear grayed-out; they simply don't exist
(reference rule: "if missing features then remove").

## 4. Named ROI map (published geometry)

Window 1520×820 (existing). Plot texture 1000×500 at world offset
`PLOT_OFFSET = (120, 90)`. The restructure fixes these screen-space
regions (top-left origin, pixels) and publishes them as constants in the
app for tests:

| ROI name | rect (x, y, w, h) | content |
|---|---|---|
| `status_bar` | (0, 0, W, 36) | run state, trig status, rate, serial |
| `plot` | plot texture screen rect | waveform only (texture readback = signal truth) |
| `ch_badges` | strip left of plot | CH1/CH2 badges |
| `trig_badge` | strip right of plot | trigger level/source badge |
| `meas_strip` | (0, below plot, W, 28) | measurement boxes |
| `bottom_bar` | (0, H-32, W, 32) | timebase, trig position marker |
| `menu_panel` | right panel rect | open context menu |

Layout tests grab full-window ROIs via `screencapture` (PLAN pillar 3) and
assert region presence/geometry; signal tests keep using the plot-texture
readback (immune to chrome).

## 5. Open questions for implementation

- Channel hue set: adopt Siglent-like CH1 yellow / CH2 cyan (current app
  colors already close: (1.0,0.85,0.1) / (0.2,0.75,1.0)). (T)
- Exact SDS2000X Plus status-bar wording and menu item order: verify
  against manual/unit before claiming pixel-identity. (T)
- s/div display: derive from `rate` and record length (20 div ⇒
  s/div = record_time/20).
