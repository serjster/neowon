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

**Workspace** — which of the two instruments the app is: the oscilloscope
or the SDR. The **SCOPE | SDR** switch, first in the app bar (⌘/Ctrl+1,
⌘/Ctrl+2), and the `instrument scope|sdr` script verb switch it at run
time within the
launch's family: the simulators swap for each other, and the VDS1022 swaps
for the RTL-SDR. Each keeps its settings across a switch. (Plain `mode` is
the scope's *trace* mode: vectors, dots or XY.)

In SDR mode the regions keep their names and change their content; no
scope control is on screen (D12):

- **App bar** — the workspace switch, the menus (View lists the RF map,
  band strip, minimap, band plan and Catalog), RUN/STOP, then the tuned
  frequency, the IQ rate, the gain (or AGC) and the **band** the tuned
  frequency is in, coloured by kind (the narrowest allocation; hover for
  all of them); on the right, the SDR's name and the **IQ frame counter**.
- **Minimap** (`ui/sdr_bands.rs`, `bandmap mini`) — across the top of the
  canvas: every band of the active plan on a log axis over the tuner's
  whole range, the IQ window as a white bracket, the tuned frequency as a
  red tick. Click anywhere on it to tune there.
- **Spectrum**, **band strip** and **waterfall** (`ui/sdr_view.rs`) — drawn
  where the grid and descriptor bar sit: the spectrum on top (round MHz
  ticks, levels in dBFS), the band strip (`bandmap strip`: the plan's bands
  in view on the same frequency axis, nested allocations stacked; click a
  band to tune to its centre, double-click to fit the span to it), the
  waterfall below, newest row at the top. The waterfall's black sits 5 dB
  under the measured noise floor and white at the reference level. A red vertical bar with a triangle tip is the **tuned
  cursor**, labelled with its frequency at the top of the waterfall; the
  translucent band around it is the **channel width**, with solid filter
  edges. The mouse works as in the scope's Spectrum window: *left-click*
  tunes to the frequency under the pointer (the window moves only for a
  target outside the IQ band),
  *left-drag on a filter edge* resizes the width, *left-drag* elsewhere pans
  the view inside the IQ band (vertically it moves the reference level),
  *right-drag* moves the **hardware window** (the band follows the pointer),
  *scroll* zooms the span at the pointer, *shift+scroll* (or a 2-D wheel's
  x axis) zooms the dB range, and *double-click* resets the view. A tuned
  frequency outside the view shows an edge arrow with its frequency; the
  hardware centre is a faint amber line when it differs from the tuned
  frequency.
- **SDR dock** (`ui/sdr_dock.rs`) — collapsible sections like the scope's,
  labels in the left column: **Tuning** (the Tuned frequency in large type,
  the band it is in, Centre, Follow, Width), **Receiver** (rate, gain, RTL
  AGC, ppm), **Display** (span, FFT, reference and range, peak and floor),
  **Audio** (demod, volume, mute, squelch, state), **Signals** (Detect,
  threshold and the resizable signal list), **Analysis** (the modulation
  lab and the constellation; open while the lab runs) and, on the
  simulator, **Simulator** (the scene). Hovering "Tuned" lists the canvas
  mouse gestures.
- **RF map window** (`ui/bandmap_window.rs`, `bandmap window`) — the whole
  tunable range as a band chart: one row per decade, log frequency within
  it, nested allocations stacked, a legend of kinds, the plan selector, the
  IQ window and the tuned frequency marked. Click tunes; double-click a
  band fits the span to it.
- **Catalog window** (`ui/catalog_window.rs`) — the persistent signal
  catalog.
- **Station overlay** (`ui/station_overlay.rs`, `stations overlay`) — on the
  spectrum: a tick and a `name · WFM [· km]` label (up to three staggered
  rows; a label that does not fit becomes a tick) for every known station
  in view. Filtering is the per-service radius (broadcast 150 km, aviation
  100 km; `stations radius` overrides both) plus "on air now" for scheduled
  rows. Click tunes there and selects the fitting demodulator.
- **Stations window** (`ui/stations_window.rs`, `stations window`) — the
  RF reference, not the catalog (D20): search, filters (source, service,
  modulation, scope all/in view/near me, on air), sort by frequency or
  distance, columns frequency · name · mod · service · km · source; click
  tunes, `+ cat` copies the row into the catalog with `refdb:<source>:<id>`
  provenance. The **Sources** tab lists each source's snapshot (count,
  fetched date, origin, licence) with Fetch (a no-location source disables
  it with the reason), Import and Clear; the **Location** tab holds the
  manual fix (lat/lon or Maidenhead locator) and the consent-gated
  `Locate me`.
- **Front panel** (`ui/sdr_panel.rs`) — the radio's keys: TUNE (±1 MHz,
  ±100 kHz, Follow), SPAN presets, DEMOD (Off/AM/NFM/WFM, Mute), RUN, and
  VIEW (RF map, band strip, minimap, Detect, Analyse, Catalog).

SDR vocabulary:

- **Centre** — the hardware window's centre: the DC bin of the IQ stream,
  which sets what RF the band covers (`sdr centre`). It is not where you
  listen.
- **Tuned** — the channel the operator monitors, an absolute frequency
  inside (or beyond) the IQ band (`sdr tune`, `sdr step`). Tuning inside
  the band moves this and leaves the window alone, so two in-band signals
  stay visible; a target outside the band recentres the window on it, and a
  target outside a zoomed view pans the view.
- **Follow** — when on, the hardware window keeps the tuned frequency at
  its centre (`sdr follow`); off by default. Right-drag moves the window
  by hand.
- **Width** — the channel bandwidth around the tuned frequency (`sdr
  width`, or the nearest detection's measured OBW with `sdr width auto`).
  Distinct from a track's **OBW**; it is what a demodulator would filter.
- **Demod** — the audio demodulator (`sdr demod am|nfm|wfm|off`), applied
  to the tuned channel at the current Width. Host-side, not hardware.
- **Squelch** — mutes the demodulated audio when the channel power falls
  below the threshold (`sdr squelch`); `off` leaves the gate open.
- **Audio state** — `playing`, `muted`, `squelched`, `no device`,
  `starting` or `off`. All but the first mean silence, so the dock names
  which one applies.
- **Span** — how much of the IQ band is displayed. *Full* means the whole
  sample rate.
- **Pan** — where the span sits, as an offset from the hardware centre
  (`sdr pan`). It stays inside the IQ band and is display only.
- **Frame** — one block of IQ pairs from the SDR (the scope's *record*).
- **Floor** — the noise level the detector measures each bin against: the
  lower quartile over a quarter of the band.
- **Detection / track** — a *detection* is one frame's signal above the
  floor. A *track* is the same signal followed across frames under a stable
  id. It becomes active after 0.25 s and is forgotten after 1 s unseen.
- **OBW** — occupied bandwidth: the band holding 99% of the signal's power.
- **Lab** — the modulation lab. It measures the signal nearest the tuned
  frequency: symbol rate, EVM/MER, cumulants, recovered constellation.
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
- **Reference store** — `~/.neowon/refdb` (`NEOWON_REFDB`): one snapshot
  per station source (Wikidata, EiBi, OurAirports, FCC, FMLIST), replaced
  wholesale by `refdb fetch|import` and never written by the catalog. The
  only bridge is `Add to catalog`, which copies a row (D20).
- **Location** — the operator's fix for distance filters and ranking:
  `location <lat> <lon>` | `location <locator>` | `location ip` |
  `location clear`, stored in `~/.neowon/location.json`
  (`NEOWON_LOCATION`). `ip` is one request to ipapi.co after an explicit
  consent, never a background lookup (D18).

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

## UI element tree

`get uitree` (control socket), `uitree <path>` (script) and the MCP
`ui_tree` tool export the UI as a tree, like a browser's DOM inspector:
every egui widget (role, label, value, toggled/disabled, clickable) and
every custom-painted element registered with `uitree::node` (the SDR
canvas, spectrum, waterfall, band strip and each band segment, minimap,
dock sections, RF map rows), each with its rect `[x, y, w, h]` in logical
window pixels, plus the painted regions of the layout dump. It is egui's
AccessKit tree, built each frame while a control socket is open. Assert
layout and presence from it rather than from screenshots; a new
custom-painted element should call `uitree::node` so it shows up.

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
