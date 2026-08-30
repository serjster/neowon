# Phase 7.8: lab-scope control semantics audit + UI geometry guarantees

Branch audit requested 2026-08-30: *"audit every 'official' UI control and
make sure it behaves like a lab scope in terms of the logical operation — I
don't want users accustomed to a control on a lab scope to find it means
something completely different in our app."* Plus three concrete defects:
horizontal zoom that cannot zoom out, dock sections that cover the plot, and
an unreadable UI on a 4K screen.

Evidence base for "how a lab scope behaves": the vendor user manuals for the
Rigol MSO5000, Keysight InfiniiVision 4000X/3000T, R&S RTM3000, and
Tektronix 4/5/6 Series MSO, plus the Siglent SDS2000X Plus notes already in
`docs/ui-ux-research.md`. Claims below cite the manual they come from.

## 0. Verdict

The branch's *rendering* work (zoom window, shader mapping, cursors through
the window, icon/knob kit) is sound. What was wrong is the **control model**:
Phase 7.7 made a display-only zoom window the primary horizontal control and
left the actual time base as a sample-rate combo box. That inverts the
front-panel relationship every scope shares, and it is the direct cause of
"I can't zoom out past 2 ms at 250 kS/s".

Three defects confirmed on the attached instrument (VDS1022I2324259, CH1 on
the 1 kHz probe-comp output), all fixed in this phase:

| # | Defect | Evidence |
|---|---|---|
| 1 | Horizontal zoom capped at the record | at 250 kS/s, `hview` span clamps to 1.0 = 20 ms = 2 ms/div; no control reached slower |
| 2 | Dock covers the plot when a section expands | dock's left border measured at x=1175 with Channel 1 open vs x=1234 with only Trigger — 59 px of the grid, including the trigger-level marker, painted over |
| 3 | UI unreadable on a 4K panel | monitor reports 3840x2160 at OS scale 1.0; the app had no scale concept |

## 1. Horizontal: the control model (defect 1)

### What lab scopes do

- **Horizontal SCALE = s/div is the primary control, and it changes the
  sample rate.** "Scale sets the time per major horizontal graticule division
  *and samples/second parameters*" (Tek). "When running, adjusting the
  horizontal scale knob changes the sample rate. When stopped, adjusting the
  horizontal scale knob lets you zoom into acquired data." (Keysight)
- **Zooming out past a full memory just lowers the sample rate.** "At slower
  time/div settings the effective sample rate drops … If the acquisition time
  is 100 ms (10 ms/div), only 1 of every 100 samples is needed to fill
  memory." (Keysight) There is no wall at the record length.
- **The governing identity**, printed outright by Rigol: `MDepth = SRate x
  TScale x HDivs`. Our record is fixed at 5000 points and the graticule at 10
  divisions, so `s/div = 5000 / (rate x 10)`.
- **Horizontal POSITION is trigger delay in seconds**, a different control:
  "the time distance from the trigger point to the reference point" (R&S),
  with push-to-zero (Rigol, Tek).
- **Zoom / delayed sweep is secondary and explicit**: a dedicated key, a
  split screen, "a horizontally expanded version of the normal display"
  (Keysight), and while it is on the scale and position knobs re-target to
  the zoom window. It never re-acquires.
- **Slow time bases enter roll**, they are not clamped: Rigol auto-rolls from
  200 ms/div, Keysight from 50 ms/div, Tek from 40 ms/div. In roll "there is
  no trigger … no pre-trigger information is available" (Keysight) and the
  trace scrolls right-to-left with the newest sample at the right edge.

### What the branch did

`Phosphor.hview = (centre, span)` — a window into the acquired record, span
clamped to ≤ 1.0 — was wired to *every* horizontal gesture: shift+scroll, the
dock's H± buttons, waveform drag, `hzoom`. The sample rate (the real time
base) was reachable only from a combo box labelled "Rate" and the arrow keys.
Consequences:

- zoom-out stopped dead at the record, exactly as reported;
- `pan left/right` was a no-op whenever the window was un-zoomed (span 1.0
  clamps), so the primary horizontal drag did nothing;
- the s/div readout moved when the *rate* changed but the zoom control
  claimed to be the "horizontal zoom", so two different things called
  themselves the horizontal control.

### What it does now

- `view::timebase{,_ladder,_step}` / `set_timebase` express the time base in
  s/div over the instrument's real rate ladder (VDS1022: 2.5 S/s … 100 MS/s,
  24 rungs = **200 s/div … 50 µs/div** on a 5000-point record).
- `view::hzoom` owns the mode split: zoom window off → step the time base;
  zoom window on → scale the window. Stopped acquisitions zoom the stored
  record instead of re-acquiring (the InfiniiVision rule above).
- `view::hposition` likewise: zoom off → trigger delay (`config.position`);
  zoom on → move the window. Waveform drag, the arrow buttons, and 2-D wheel
  x all go through it, so horizontal drag now does something at every zoom.
- The horizontal dialog is three groups in front-panel order — **Time base**
  (s/div ladder + knob + rate/span readout), **Position** (delay in seconds,
  "Set to 50%"), **Zoom** (an explicit "Zoom window (delayed sweep)"
  checkbox, window/centre sliders, magnification readout).
- A **zoom band** along the plot's top edge shows which slice of the record
  is on screen while zoomed — the compact stand-in for the split-screen zoom
  box, since the app has one grid.
- A **ROLL** badge lights below 2.5 kS/s with a tooltip saying the trigger is
  not used. 2.5 kS/s on 5000 points is 200 ms/div — the same threshold Rigol
  documents.
- The simulator's rate ladder was extended from 6 rungs to the hardware's 24
  so sim-based tests exercise the same time-base range.
- New script actions `timebase <s/div>` and `zoomwin <on|off>`; `hzoom`,
  `zoom h`, and `pan` keep their names and gain the mode split.

Verified on hardware: from 2 ms/div, ten `hzoom out` steps reach **125 S/s =
4 s/div** (40 s across the screen) with the zoom window untouched, ROLL lit,
and frames still streaming (see `docs/media/`-adjacent session shots).

### Known gaps (recorded, not fixed here)

**G1 — roll scrolls the wrong way.** In roll the app still draws each record
whole, so the display fills left-to-right rather than scrolling right-to-left
with the newest sample pinned at the right edge, which is what every manual
describes. Closing it needs the device's roll write-cursor — the "gapless
roll-mode streaming" item already parked in PLAN.md §4.

**G2 — no deep memory: zooming out costs sample rate.** Raised by the user
2026-08-30 after this phase landed: *"when I mentioned the time base I mean
purely a way to zoom, completely separate from sample rate … set 1 s and the
signal transforms into a straight plotted line."*

Reproduced on hardware and it is two separate things.

*The straight line is aliasing, and scopes have a standard cure.* At 1 s/div
the rate is 500 S/s, so the 1 kHz probe-comp signal aliases to a flat 16 mV
trace. Switching to **peak detect** at the same time base restores the full
616 mV envelope band — the hardware min/max pairs survive decimation. This is
exactly what the manuals promise ("signal aliasing can be prevented", Rigol;
"at slower time/div settings, the maximum and minimum samples in the
effective sample period are stored", Keysight). The app already has peak
detect and does not suggest it when the time base makes aliasing certain.
**Follow-up:** offer/auto-engage peak detect at slow time bases and say why.

*Decoupling zoom from sample rate needs memory we do not have.* `MDepth =
SRate × TScale × HDivs` is an identity, not a policy: with a **fixed
5000-point record**, span and rate trade against each other, always. 1 s/div
at 250 kS/s would be 2.5 M samples — 500x the instrument's memory. Real
scopes buy the decoupling with deep memory (Rigol offers 1 kpts…200 Mpts);
the VDS1022 has one depth and no setting for it. So the zoom window really is
of limited use today: it magnifies within one 5000-point record, which is
what the user noticed.

*Can we not just stream and keep the memory on the PC?* Asked by the user,
and tested on hardware — `cargo run -p neowon-vds1022 --example rolltest`,
findings in docs/protocol-vds1022.md. Host memory was never the constraint;
the device's ability to hand over a continuous stream is. Roll mode
(`SET_ROLLMODE`) is the streaming primitive and it is accepted at *every*
rate, but the frame `cursor` — the write position a host stream must follow —
only advances at or below 2.5 kS/s (752…5100, 232 distinct values in 3 s). At
25 kS/s and above it pins at 5100: complete buffers, no progressive fill. So
gapless capture exists, but only at a rate too slow to render a 1 kHz signal.
One useful surprise: a tight `GET_DATA` loop sustains **131 reads/s (7.6 ms
round trip)** against the app's ~36 fps, and at 250 kS/s a fresh buffer
appears every 20 ms — so coverage could be pushed from ~71 % toward ~100 %
just by reading faster. Bandwidth is not the limit here (683 kB/s of frames);
the acquisition model is.

*What we could actually build:* a **segmented / deep view** assembled
host-side from the scrollback ring, which already stores consecutive records.
That keeps the true sample rate and extends the time axis — the same trade
Keysight sells as Segmented Memory. Measured on hardware at 250 kS/s: 35.7
frames/s x 20 ms records = **71 % wall-clock coverage**, so ~29 % of the time
axis is dead time between acquisitions and must be drawn as gaps, not
interpolated. Two prerequisites: `CaptureFrame` carries only `seq`, so it
needs a **capture timestamp** to place records on a real time axis; and
trigger-aligned records are not successive in time, so an honest deep view
wants free-running acquisition (or must label what it is showing). This is a
plan-level decision — not started.

## 2. Every official control, audited

Legend: **OK** = matches the manuals; **fixed** = corrected in this phase;
**gap** = deliberate omission or recorded follow-up.

### Vertical (channel)

| Control | Lab-scope meaning | Status |
|---|---|---|
| V/div ladder + knob | volts per division, 1-2-5 steps (Rigol, Keysight) | OK — ladder from device caps, knob detents on rungs |
| Offset knob | R&S: *offset* is a voltage, *position* is divisions | **fixed** — read out in volts (was "+0.00 FS"); storage stays a full-scale fraction |
| Coupling DC/AC/GND | standard | OK |
| Probe attenuation | scales the vertical readout | OK |
| CH key | toggles the channel on/off | OK — configuring is the descriptor box/dock, as on touch scopes |
| Offset marker drag | drags the channel's ground reference | OK |
| Expansion centre (ground vs screen centre) | Rigol offers both | **gap** — we always expand about ground, Rigol's default |

### Horizontal

| Control | Lab-scope meaning | Status |
|---|---|---|
| s/div | primary time base; changes sample rate; no floor before roll | **fixed** (§1) |
| Position / delay | trigger point vs reference, in seconds, push-to-zero | **fixed** — labelled "delay" in seconds, knob double-click and "Set to 50%" reset it |
| Zoom / delayed sweep | explicit secondary magnified window into the record | **fixed** — explicit toggle, zoom band indicator |
| Arrow keys ←/→ | horizontal scale | **fixed** — now step the time base (were raw rate steps) |
| Roll indication | untriggered strip-chart mode at slow time bases | **fixed** (badge); scroll direction is the recorded gap |

### Trigger

| Control | Lab-scope meaning | Status |
|---|---|---|
| Level (+ marker drag) | voltage threshold with on-screen line | OK |
| "Lvl 50%" | Tek: `(Top + Bottom) / 2` | OK — uses vtop/vbase |
| Auto / Normal / Single | force after timeout / only on trigger / one shot then stop | OK — WAIT badge shows a starved Normal/Single |
| Force | generate a trigger in Normal/Single | OK |
| Edge / Pulse / Slope / Video | standard type set | OK — hardware-verified in Phase 6 |
| Holdoff | standard | OK |

### Acquire

| Control | Lab-scope meaning | Status |
|---|---|---|
| Sample | plain decimated sampling | OK |
| Peak detect | min/max per interval, catches narrow pulses | OK — hardware-verified |
| Average 4/16/64 | needs a stable trigger; powers of two | OK |
| High-Res | within-acquisition filtering for extra bits | **gap** — not implemented, so not shown |

### Measure / cursors

| Control | Lab-scope meaning | Status |
|---|---|---|
| Measurement table | value plus mean/min/max/σ/count statistics (Keysight, R&S) | OK — statistics on hover, per-slot reset |
| Time cursors | Δt with 1/Δt (frequency) | OK |
| Amplitude cursors | ΔV | OK |
| Track / XY cursor modes | Rigol/Keysight offer them | **gap** — manual cursors only |

### Display / utility

| Control | Meaning | Status |
|---|---|---|
| Persistence, palette, dots/vectors/XY | display-only | OK |
| UI scale | not a scope control — a desktop-app concern | **fixed** — new (§3) |

## 3. UI geometry: nothing may cover the grid (defects 2, 3)

### Cause of the occlusion

The dock is an `egui::Area` pinned at the layout's dialog rect with
`.constrain(true)`. When a section's content is wider than the 320 px rail,
egui keeps the area on screen by sliding it **left** — over the plot. The
channel dialog overflowed because `knob()` laid its value/label text out in
the parent `Ui`, so each knob claimed the remaining rail width and a row of
two knobs did not fit.

### Fixes

- `knob()` is now a fixed-size cell (`KNOB_W` wide) that *paints* its labels
  instead of laying them out, so a row of knobs has a predictable width.
- The dock sets `constrain(false)` and clips to its rail: overflow can no
  longer reach the waveform, by construction.
- The dock scrolls in both axes, so an over-wide section scrolls rather than
  spilling — what the reference manual specifies for the dialog box ("a
  scrollbar when overflowing", SDS2000X Plus 7.6).
- The Measure header, found by the new test to be 341 px wide in a 320 px
  rail, wraps onto two rows.

### UI scale for hi-DPI

`ui::UiScale` is a single egui zoom factor. `Layout` carries the same number
and scales the chrome (dock width, menu/front-panel heights, margins) in
logical pixels, converting to egui points via `Layout::points` and back via
`Layout::pixels`; Bevy-side geometry (plot sprite, gizmos, pointer hit tests)
stays in logical pixels and is unchanged. At startup `fit_display` picks the
factor from the monitor (`auto_scale`: a panel ≥ 2000 px tall that the OS is
not already scaling gets 2.0) and sizes the window to ~70 % of it.
`NEOWON_UI_SCALE`, the `uiscale <factor>` script action, and a Utility slider
all override it. `NEOWON_WINDOW` still pins the size for tests.

### How this is tested

`tests/ui_geometry.rs` (ignored by default; needs a window). The app already
dumps its named-ROI map via the `layout` action; that dump now also carries
**`painted`** — the rects egui actually returned for the dock, menu bar,
descriptors, measurement overlay, and front panel — and **`floating`** for
the movable spectrum/waterfall/3D windows. The test walks 4 window-size x
UI-scale combinations x all 11 dock sections and asserts:

1. no painted region overlaps the plot ROI (in px², with the offender named);
2. the dock stays inside its own rail.

Geometry-as-JSON was chosen over pixel comparison deliberately: the assertion
is exact, it reports *which* region broke the rule and by how much, and it
survives restyling. Pixel tests keep their job — what the trace looks like
(`ui_pixels`, `effects_pixels`) — and `Layout`'s unit tests now cover 1080p,
1440p, and 4K at scales 1.0/1.5/2.0.

## Done when

Workspace + ignored suites green; every new control script-reachable;
PLAN.md §4 and README updated; the three defects reproduced-then-verified on
the attached VDS1022.
