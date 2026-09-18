# Screen anatomy — what everything is called

The names here are the ones used in the code, in commit messages and in
these docs. If you and I use different words for the same strip of screen,
every bug report costs a round trip.

```
┌──────────────────────────────────────────────────────┬──────────────┐
│ APP BAR   File View Settings │ RUN │ 2 ms/div 250 kS/s│             │
├──────────────────────────────────────────────────────┤              │
│                                                      │              │
│                    GRID                              │     DOCK     │
│              (the waveform area)                     │  ┌────────┐  │
│   markers: trigger level ▸ right edge                │  │ view    │ │
│            trigger position ▾ top edge               │  │ toolbar │ │
│            channel offsets ▸ left edge               │  └────────┘  │
│            cursors, decode annotations               │  ▸ Trigger   │
│                                                      │  ▾ Horizontal│
│  ┌──────────────────────────────────┐                │    …section  │
│  │ READOUT BADGES (per source)      │                │      body    │
├──┴──────────────────────────────────┴────────────────┤  ▸ Acquire   │
│ DESCRIPTOR BAR  [C1][C2] [timebase] [trigger]        │  ▸ Channel 1 │
├──────────────────────────────────────────────────────┤  …           │
│ FRONT PANEL  VERTICAL │ HORIZONTAL │ TRIGGER │ RUN │ PANELS         │
└──────────────────────────────────────────────────────┴──────────────┘
```

## The regions

**App bar** (`ui/menubar.rs`) — the top strip. Drop-down menus on the left
(File, View, Instrument, Settings) for things about the *application*; ambient status on
the right: run state, time base, sample rate, ROLL and TIMELINE badges, the
instrument's name and serial, and the **acquisition counter** (`#1234`). That
counter is the number of records captured since launch — it should climb
steadily, and a stalled one means the trigger is starving or the instrument
has stopped.

**Grid** (`ui/layout.rs::Roi::Plot`) — the waveform area, 10 × 8 divisions.
Everything drawn on it belongs to one of: the trace itself, the graticule,
draggable **markers** (trigger level at the right edge, trigger position at
the top, per-channel offset at the left), **cursors**, decode annotations
along the bottom, and the timeline's gap markers.

**Readout badges** — the small per-source boxes inside the grid's bottom-left
showing frequency and amplitude at a glance.

**Descriptor bar** (`ui/descriptors.rs`) — under the grid: one chip per
source, then the time base and trigger chips. Clicking a chip **reveals** the
matching dock section (opens it *and* scrolls it into view).

**Dock** (`ui/menu.rs`) — the always-visible right-hand rail. This is the
name for that whole area; the collapsible parts inside it are **sections**
(Trigger, Horizontal, Acquire, Channel 1…), each with a **header** and a
**body**. At the top of the rail is the **view toolbar** — zoom, pan and
home. The dock scrolls; a section too wide for the rail scrolls sideways
rather than spilling over the grid.

**Front panel** (`ui/frontpanel.rs`) — the bottom strip of hardware-style
keys, grouped VERTICAL / HORIZONTAL / TRIGGER / RUN / PANELS. Two kinds of
key live here and the grouping says which is which:

- keys that **do something to the instrument** — CH1/CH2 switch a channel on
  and off, Auto/Normal/Single set the sweep, Force, AutoSetup;
- keys in **PANELS**, which are only shortcuts: they reveal a dock section.

**Windows** — floating, movable, closable: Measurements, Spectrum,
Waterfall, 3D View, Settings. Anything with more content than the rail can
show gets one of these rather than being crammed into a section.

## SDR mode

**Instrument** — which of the two instruments the app is: the
oscilloscope or the SDR. The app bar's **Instrument** menu and the
`instrument scope|sdr` script verb switch it at run time within the
launch's family: the simulators swap for each other, and the VDS1022 swaps
for the RTL-SDR. Each keeps its settings across a switch. (Plain `mode` is
the scope's *trace* mode: vectors, dots or XY.)

In SDR mode the regions keep their names and change their content:

- **App bar** — RUN/STOP, then the tuned frequency, the IQ rate and the
  gain (or AGC); on the right, the SDR's name and the **IQ frame counter**.
- **Spectrum** and **waterfall** (`ui/sdr_view.rs`) — drawn where the grid
  and descriptor bar sit: the spectrum on top, the waterfall below, newest
  row at the top. Clicking tunes to that frequency.
- **SDR dock** (`ui/sdr_dock.rs`) — drawn over the dock: tuning, rate, gain,
  ppm, span, FFT size, level; **Detect** and the **signal list**; the
  **modulation lab**; the **constellation**.
- **Catalog window** (`ui/catalog_window.rs`) — the persistent signal
  catalog.
- **Front panel** — still the scope's. While the SDR is the instrument,
  scope edits change its settings and reach the scope when it is the
  instrument again.

SDR vocabulary:

- **Centre** — the tuned frequency, the middle of the displayed band.
- **Span** — how much of the IQ band is displayed, centred on the centre.
  *Full* means the whole sample rate.
- **Frame** — one block of IQ pairs from the SDR (the scope's *record*).
- **Floor** — the noise level the detector measures each bin against: the
  lower quartile over a quarter of the band.
- **Detection / track** — a *detection* is one frame's signal above the
  floor. A *track* is the same signal followed across frames under a stable
  id. It becomes active after 0.25 s and is forgotten after 1 s unseen.
- **OBW** — occupied bandwidth: the band holding 99% of the signal's power.
- **Lab** — the modulation lab. It measures the signal nearest the centre:
  symbol rate, EVM/MER, cumulants, recovered constellation.
- **Class / trust** — the classifier's verdict and whether it has been
  proven on over-the-air data. It is `unproven` until the SDR-G2 evaluation.
- **Survey** — a sweep of a frequency range in tuning **steps**. Its
  **coverage** records what each step could see: whether it was scanned,
  whether it was truncated at the peak cap, and the kept-power floor. A
  **diff** between surveys calls a signal *unknown* where a survey could not
  have seen it.
- **Signal / observation** — in the catalog, a *signal* is an entity with
  an id, a name, aliases and tags. An *observation* is one sighting of it,
  with time, power and SNR.

## Vocabulary that matters

- **Record** — one acquisition from the instrument. Fixed at 5000 samples on
  the VDS1022.
- **Time base** — s/div. The *acquisition* control: it picks the sample rate.
- **Zoom window** — a magnified view *inside one record* (delayed sweep).
- **Follow mode** — how the timeline tracks live data. **Page** fills a fixed
  slice of the clock and then turns over, so nothing moves while it fills;
  **slide** keeps the newest data at the right edge and the trace marches
  left. Page is the default because a sliding window at a short time base
  shifts the whole trace by many columns per record.
- **Timeline** — the display spanning recorded *history* rather than one
  record, at the acquisition's own sample rate, with the time the instrument
  was not acquiring drawn as marked gaps.
- **Scrollback** — the ring of recorded frames, bounded by a memory budget in
  Settings. This is what the timeline and the history scrub read back through.
- **Source / slot** — CH1, CH2, or the math trace: the three things that can
  be measured.
- **Reveal** — open a dock section *and* scroll it into view. What a
  front-panel PANELS key or a descriptor chip does.

## Conventions

- A control that changes the instrument marks the config dirty and is sent on
  the next flush; a control that only changes the display does not.
- Every control is reachable from the script grammar (`crates/neowon-app/src/
  script/grammar.rs`). A control with no script action is a bug. SDR and
  catalog controls inject typed actions whose `Display` is their script
  line. The `every_action_round_trips` tests check that each line parses
  back to the same action.
- Widgets in the dock do not respond to the scroll wheel. The dock is a
  scrolling rail, and a widget that reacts to the wheel changes its value
  whenever the pointer crosses it mid-scroll.
- Icons and disclosure carets are painted vector shapes, never font glyphs:
  egui's bundled fonts are subset per platform and glyphs like ▶ rendered as
  tofu on some of them.
