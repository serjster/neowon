# Phase 7: capture workflows

History browser, our own capture file format, reference waveforms, PNG
export, session save/restore, vendor `.cap` import. Read `PLAN.md` §4
Phase 7 first. Implemented in-session (not delegated); this spec is the
contract and the record of decisions.

## Decisions (user-approved 2026-08-30)

- **zstd** compression for our capture format — new workspace dependency
  `zstd = "0.13"`, lives in `neowon-core` (engine-free is about Bevy/GPU,
  not about compression).
- **Sessions are scripts**: a session file is a `NEOWON_SCRIPT` text file;
  save emits one action per setting, restore replays it through the
  existing executor. No serde.
- **PNG** via the `png` crate already in Bevy's tree — direct dependency
  pinned to the tree's version (`0.18`), zero new compiled code.

## Hard rules

- Sim only; nothing may touch USB.
- Stimulus preset names and existing script actions are a stable API —
  extend, never rename.
- Every new UI control gets a script action; every new format gets a
  round-trip test against deterministic sim data.
- Library crates stay Bevy-free. File budgets apply (~500 soft/700 hard).

## Work items

### 1. Capture file format `.nwc` (`neowon-core/src/nwc.rs`)

Little-endian, zstd-compressed frame stream:

```
magic  b"NWCAP1\0\0"          8 bytes
flags  u32                     bit0 = payload is a zstd stream (always set)
payload (zstd):
  per frame:  seq u64 · sample_rate f64 · acq u8 (0/1/2) · avg u8
              n_channels u8
    per channel: ch u8 · volts_per_lsb f64 · zero_volts f64 · clipped u8
                 freq_flag u8 · freq f64 · n u32 · raw [i8; n]
read until decompressed EOF
```

```rust
pub fn write(path: &Path, frames: &[SharedFrame]) -> io::Result<()>;
pub fn read(path: &Path) -> io::Result<Vec<SharedFrame>>;
```

Round-trip test: write deterministic frames, read, compare field-exact.

### 2. History browser (`neowon-app`)

`HistoryState { active: Option<usize> }` resource. Scrubbing stops
acquisition (`running = false`, dirty) and assigns
`link.latest = Some(rec.frames[i].clone())` — every consumer
(GPU raster, measurements, exports) follows `link.latest` already, so no
new plumbing. `record_frames` skips while history is active. "Live"
clears history and sets `running = true`. UI: History rows in the Record
dock section (slider + prev/next/live + `i/N` readout, fixed-width).
Script: `history <idx> | prev | next | live`.

### 3. Capture save/load (UI + script)

Record section buttons `Save .nwc` / `Load .nwc` (load enters history at
frame 0). Script: `capsave <path>` / `capload <path>`. Loading replaces
the recorder ring.

### 4. PNG export of the plot view

`script.rs::write_shot` writes PNG when the path ends `.png` (PPM
otherwise), via the `png` crate. UI: "PNG" button beside the other
exports → `~/neowon-captures/<stem>.png` from the next readback; script
`shot <path>` unchanged (extension picks the format).

### 5. Reference waveforms (ghost traces)

`RefState { traces: [Option<ChannelCapture>; 2], show: bool }`. Saving
copies the current frame's channel capture; ghost drawn as a dim
downsampled egui polyline over the plot in raw-count space (the shape as
captured, like real budget scopes). Script: `refsave <ch>`, `ref on|off`,
`refclear`. References ride along in sessions? No — in-memory only this
phase (file format hook left for later).

### 6. Session save/restore (scripts)

`session.rs`: `emit(state…) -> String` writes one script line per
setting (channels, trigger, horizontal, acquire, display, math, FFT,
cursors, pass/fail, menus). Restore = feed the file's actions into the
script executor queue at runtime. Script: `sessionsave <path>`,
`sessionload <path>`; UI buttons in the Utility section. Round-trip
test: configure → save → reset → load → configs equal.

### 7. Vendor `.cap` import (`neowon-core/src/owon_cap.rs`)

Big-endian format from the decompiled
`com.owon.uppersoft.dso.function.record.RecordFileIO` (+ the frame
payload classes it calls). Header: ASCII machine header, machine type
i32, record version (4), byte 3, three zero bytes, then per-frame blocks.
Document the decoded layout in `docs/protocol-vds1022.md` as it is
established. `read(path) -> io::Result<Vec<SharedFrame>>`, surfaced as
`capload file.cap`. Tested against a fixture built from the documented
layout (no vendor file is committed).

## Done when

- `cargo test` green including new round-trips; `fmt` + `clippy -D
  warnings` clean; ui_pixels still green.
- Every new control reachable by script; PLAN.md §4 status block updated.

## Deviations (recorded per AGENTS.md)

- Sessions do NOT emit dock-section state (work item 6 said "menus"):
  sessions capture instrument/display/analysis settings like real scope
  setup files; window arrangement stays out. `menu` remains available to
  scripts directly.
- `.cap` channel blocklen is `40 + datalen` (the vendor's patch-back
  `endPtr - lenPos - 4`), not the 44 the first draft of the format notes
  said; the importer walks fields and skips any tail, so it tolerates
  either.
- Session `sessionload` splices actions at the FRONT of the script queue
  (later queue entries must observe the loaded state — caught by the
  round-trip integration test).
- Added `trigpos <frac>` script action (horizontal trigger position had
  no script coverage; sessions need it).
