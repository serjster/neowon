# Phase 7.6: visualization playground

Realtime spectrogram, a 3D signal viewport, and user-loadable display
shaders. The 2D phosphor pipeline stays the precision instrument; these
are parallel views driven by the same frames. User approved 2026-08-30.

## Hard rules

- Sim-first; deterministic tests; no new dependencies (Bevy + WGSL only).
- Every new control gets a script action (and rides in sessions where it
  is instrument/display state).
- The phosphor plot texture and its pixel tests stay untouched.
- File budgets apply: new code goes in `src/viz/` and `src/effects.rs`.

## Work items

### 1. Realtime spectrogram (waterfall)

- `viz/waterfall.rs`: `WaterfallState { on, rows: ring of palettized
  RGBA rows, image: Handle<Image> }`. Each new FFT spectrum (already
  computed per record in `compute_derived` when FFT is on) appends one
  row (log-magnitude → thermal palette, 1024 bins × 512 rows); the CPU
  image is rewritten and Bevy re-uploads it.
- Shown in a resizable egui window via `EguiUserTextures::add_image`;
  horizontal crop follows the spectrum view's zoom (`fft.view` as UV).
- Turning the waterfall on forces FFT on. Script: `waterfall on|off`;
  emitted in sessions.
- Unit test: feeding synthetic spectra produces the expected row count,
  ring wrap, and palette endpoints.

### 2. 3D viewport (`viz/three_d.rs`)

Render-to-texture `Camera3d` (768×512 offscreen image) shown in an egui
window; drag = orbit, scroll = dolly. `Viz3d` modes, each a CPU-built
mesh updated per record (line strips / triangle grids, unlit
vertex-colored):

- `terrain` — spectrogram heightfield flying under the camera.
- `tunnel` — each record extruded as a ring in Z; fly through history.
- `phase` — delay embedding: (x(t), x(t−τ), x(t−2τ)) 3D curve.
- `xytime` — CH1 vs CH2 vs time (the Lissajous/Quake cube).

Script: `viz off|terrain|tunnel|phase|xytime`; in sessions. Test: mesh
builders are pure functions — golden tests on vertex counts/bounds for
deterministic sim records.

### 3. User display shaders (`effects.rs` + `shaders/effect.wgsl` contract)

A final full-screen compute pass over the composed display texture:
reads `display_image` + params (time, record metadata) and writes
`effect_image`; the plot sprite swaps to `effect_image` while an effect
is active (ping-pong, no in-place hazard).

- User WGSL lives in `assets/shaders/user/*.wgsl` (also `$NEOWON_SHADER_DIR`),
  scanned at startup; files are naga-validated BEFORE being installed —
  a broken shader reports in the UI, never crashes.
- Contract (documented in the shipped examples): storage texture out,
  display texture in, uniform `{ time, w, h, … }`.
- Shipped examples double as documentation and tests: `invert` (pixel
  test oracle), `kaleido` (mirror kaleidoscope), `ripple` (signal-driven
  wave distortion), `crt-warp` (barrel + chroma fringe).
- Script: `effect <name>|off`, `effectreload`. Display-section dropdown.
- Tests: naga-validates every shipped example; pixel test applies
  `invert` and asserts the display readback inverts.

## 4. Always-on scrollback (user request, added mid-phase)

The Recorder ring captures continuously by default (terminal-scrollback
model): `record 0|1` becomes pause/resume of the capture (script name
unchanged — stable API), overflow drops the oldest chunk instead of
stopping, the History slider is the scrub timeline, and Live resumes.
Recording-to-disk stays what it was: an explicit export/`capsave` of the
ring. Deviation from the terminal analogy: scrubbing stops acquisition
(scope history-mode semantics) rather than capturing behind the frozen
view — capturing-while-scrubbed needs display/`link.latest` decoupling,
deferred.

## Done when

Workspace + ignored suites green; examples validate; README features
updated; PLAN.md §4 status updated; every control script-reachable.

## Deviations (recorded per AGENTS.md)

- The 3D viewport draws immediate-mode gizmos (layer-1 group) instead of
  Mesh assets — regenerating meshes per frame trips Bevy's mesh slab
  allocator ("use-after-free" spam). Terrain is a wireframe, which suits
  the aesthetic anyway.
- The second camera required pinning bevy_egui explicitly:
  `auto_create_primary_context = false` + `PrimaryEguiContext` on the 2D
  camera — otherwise egui attaches to whichever camera it sees first and
  the whole UI vanishes into the offscreen target.
- User shaders are NOT naga-validated at runtime (the spec said so):
  Bevy's pipeline cache logs compile errors and the effect just never
  activates; the app keeps running. The shipped examples ARE
  naga-validated in `tests/shaders.rs`, and `tests/effects_pixels.rs`
  proves the whole pass end-to-end with `invert`.
- `shot` captures the effect output while an effect is active (WYSIWYG).
