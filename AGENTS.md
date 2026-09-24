# AGENTS.md — operating instructions for this repo

This file governs how the assistant works in `neowon`. It overrides default
behavior. Read it fully before acting. (`CLAUDE.md` is a symlink to this file,
so Claude Code and opencode share one source of truth.)

## What this repo is

A high-performance **oscilloscope application** in Rust — Bevy 0.19 for the
shell/GPU rendering, `bevy_egui` for controls — built around a modular
acquisition-backend abstraction. The first (and so far only verified)
instrument is the **OWON VDS1022I** USB scope connected to this machine
(serial VDS1022I2324259, hw V5.0.1); a deterministic simulated backend
provides the virtual testbench. See PLAN.md §4 for the authoritative phase
status — it moves faster than this paragraph.

**The assistant implements; the user directs, reviews, and decides.**
Decisions that change the plan or the backend abstraction go to the user
first.

Key locations:

- `PLAN.md` — **the single entry point for every session**: research
  findings, goals, architecture, the phase list with the status block, and
  the reference index (§7) pointing at every external authority.
- `docs/tasks/` — delegation specs, one file per work package
  (`phase6-spec.md`, `phase65-signals-spec.md`, …). Each spec states its
  hard rules, work items, and done-when criteria. If a spec exists for the
  active work, follow it exactly; deviations get recorded in the spec file.
- `docs/protocol-vds1022.md` — our own VDS1022 protocol doc. Anything
  verified or discovered on hardware goes here immediately.
- `crates/` — workspace members: `neowon-core` (engine-free shared types),
  `neowon-backend` (trait + supervisor), `neowon-sim` (virtual testbench
  source), `neowon-vds1022` (nusb driver), `neowon-dsp` (measurements, FFT,
  math — engine-free oracle), `neowon-cli` (headless bring-up),
  `neowon-app` (Bevy app + GPU pipelines + UI + scripting).
- External authorities (paths in PLAN.md §7): `OWON-VDS1022/api/python/…/
  vds1022.py` (the protocol porting bible), the decompiled vendor jar
  (register map), `~/projects/GoL` (Bevy 0.19 patterns, compute shaders,
  egui integration, GPU readback).

## Session start ritual (mandatory)

1. Read `PLAN.md` — at minimum the phase list status block (§4) and the
   reference index (§7).
2. Read the active `docs/tasks/` spec if one covers the work at hand.
3. `git log --oneline -10` and `git status` to see where work stopped.
4. If the status block in PLAN.md is stale relative to the code, fix the
   status block first, then proceed.

## Hardware safety (non-negotiable)

A real instrument is attached. Treat it with respect:

- **Never touch USB unless the user explicitly asked for hardware work.**
  No `neowon-cli` and no `neowon-app` without `--sim` (`neowon sim`, which
  never opens a device, is fine), no examples from `neowon-vds1022`.
  Automated/delegated runs use `--sim` only. (Operator decision 2026-09-23:
  `cargo test -p neowon-cli` runs the binary, always with `--sim` — asserted by
  its test helper — and `open()` refuses the RTL path in unit tests.)
- Only one process can claim the device; the vendor Java app and neowon are
  mutually exclusive. If a claim fails, something else holds it — ask, don't
  retry-loop.
- The device needs a keep-alive (`RUNSTOP=1` every ≤3 s) or the link drops;
  never leave a session wedged on the device — ctrl-C recovery must work.
- FPGA bitstreams are OWON's vendor blobs, vendored in `3rdparty/fw/`
  (user decision 2026-08-30; provenance in its README). Never commit new
  binary blobs anywhere else without an explicit user decision.
- Anything learned on hardware (register behavior, quirks like `HTP_ERR`,
  trigger-code swaps) goes into `docs/protocol-vds1022.md` in the same
  session.

## Delegation pattern

Big work packages are written as specs in `docs/tasks/` before
implementation (see `phase65-signals-spec.md` for the canonical shape):
scope fence ("you work only in crate X"), hard rules, existing-code summary,
numbered work items with concrete signatures, and test requirements. When
work is delegated to another agent/session, the spec is the contract; when
implementing from a spec, do not exceed its scope and do not touch files it
declares off limits.

## Live development loop (control socket / MCP)

The running app serves a general-purpose control API — use it instead of
restart-with-script loops when iterating on behavior or diagnosing state:

- Launch once: `cargo run -p neowon-app -- --sim` (sim only, as always).
  **The socket is on by default** on 127.0.0.1:7777 (D29);
  `NEOWON_CONTROL=<port>` moves it, `NEOWON_CONTROL=off` turns it off.
  Then drive it: any script-grammar line over `nc 127.0.0.1 7777` gets a
  JSON ack; `get status` / `get config` / `get measure` return structured
  JSON. One connection, many commands — state persists between them.
- **Write verbs need the token** (D29). Driving the instrument and every
  `get …` query are open; anything that writes a file, reads one you
  named, reaches the network or ends the process (`shot`, `shotplot`,
  `export`, `capsave`/`capload`, `uitree <path>`, `layout`,
  `sessionsave`/`sessionload`, `sdr iqdump <path>`, `refdb fetch|import`,
  `location`, `catalog` edits, `quit`) is refused until the connection
  sends `auth <token>`. The app writes the token to
  `~/.neowon/control/<port>.token` (mode 0600) and a bare `auth` replies
  with that path, so the loop is:
  `{ echo "auth $(cat ~/.neowon/control/7777.token)"; cat; } | nc 127.0.0.1 7777`
  — then `shot /tmp/x.png` grabs the live display as before. A new verb is
  classified in `crates/neowon-app/src/control/privilege.rs`, whose
  exhaustive match will not compile until you place it.
- The same API backs `neowon-mcp` (`--connect 127.0.0.1:7777` or
  `--spawn-sim`): when this session has the neowon MCP server connected,
  prefer its tools (`measurements`, `screenshot`, `exec_script`) over
  shelling out — the screenshot tool returns an image you can actually
  look at. It does the `auth` handshake itself (token file, or
  `NEOWON_CONTROL_TOKEN` when both processes share one).
- `get uitree` (MCP `ui_tree`, filterable) returns the UI element tree —
  every widget and custom-painted element with role, label, state and rect,
  like a browser DOM inspector. Use it to audit layout and to assert UI in
  tests; screenshots are for looking, the tree is for checking.
- Anything you can't reach this way is a missing script action — fix
  that first (script-parity rule), don't work around it.
- Scripted end-to-end runs (`NEOWON_SCRIPT` + `quit`) remain the way to
  write regression tests; the socket is for interactive iteration. A
  `NEOWON_SCRIPT` file is the operator's own process doing what the
  operator asked, so it is never token-gated; a socket test uses
  `tests/common/mod.rs::launch`, which picks the token with
  `NEOWON_CONTROL_TOKEN` and authenticates for you.
- Tooling that spawns the app sets `NEOWON_ORPHAN_EXIT=<seconds>` so a
  killed harness cannot leave a window on the operator's screen (D30).
  Beside it, `NEOWON_NO_INPUT=1` makes the app ignore host keyboard, mouse
  and wheel input and open without taking focus, and on macOS hand
  activation back to the app that was in front (D31), so the operator's
  typing and scrolling cannot steer the run; scripts and the socket still
  drive everything. `tests/common/sandbox.rs` sets it for every test launch;
  set it on any launch you make while the operator may be at the machine.

## Verification

- `cargo build` — workspace must compile.
- `cargo test` — unit + integration tests. The virtual testbench lives in
  `crates/neowon-sim/tests/testbench.rs`; DSP oracles in each `neowon-dsp`
  module.
- `cargo test -p neowon-app --test ui_pixels -- --ignored` — pixel-level
  render verification (briefly opens a window; sim only).
- `cargo test -p neowon-app --test shaders` — naga validation of every WGSL.
- `cargo fmt --all` and `cargo clippy --workspace --all-targets` clean
  before finishing any work package.
- Sim tests must stay deterministic: seeded PRNG only, no wall-clock in
  signal generation, no `thread_rng`.

## Engineering rules

- **Bevy 0.19.1 pinned**; most online Bevy docs describe other eras. For API
  questions, `~/projects/GoL` (same Bevy version) plus its vendored sources
  and `docs/bevy/` notes are the authority.
- **Sample encoding:** scope backends use i8, ±125 = full vertical range (10
  divisions), `volts_per_lsb` + `zero_volts` per capture — sim and hardware share
  it. Streaming backends (audio, SDR) carry f32 real/complex samples with a
  `SampleLayout` tag (Phase 10, decision D1b): core frames store f32, the core
  calibration is `IqCal{scale_i, scale_q, offset_i, offset_q}` (Real: q == i), and
  scope backends convert their i8 wire encoding at frame construction. Don't
  invent other encodings.
- **Library crates are engine-free:** `neowon-core`, `neowon-backend`,
  `neowon-dsp`, `neowon-sim` carry no Bevy/GPU dependencies; the app is the
  only place Bevy lives.
- Frames are `Arc`-shared, never copied per consumer.
- CPU DSP (`neowon-dsp`) is the correctness oracle; any GPU variant must be
  tested against it within tolerance via readback.
- File budgets: ~500 lines soft, 700 hard. A file over budget is a design
  signal — split along the second job it picked up. (`neowon-app/src/ui.rs`
  is over-budget debt scheduled for restructuring in Phase 6.5; it may
  shrink, never grow.)
- No new dependencies without user approval; check PLAN.md §1 ecosystem
  notes first.
- **Comments and docs only where the code is not self-evident.** A comment
  says *why* (a standard's clause, a hardware quirk, a non-obvious invariant),
  never *what* the next line does. No process history in code or docs: no
  review ids (`M26`, `D16`), no work-item names (`item-12`), no dates of who
  fixed what, no paths into `.critic/` or other scratch. That belongs in the
  commit message. A spec records the contract and decisions, not a narrative of
  how they were reached. When in doubt, delete the comment.

## Testing culture

- **Sim-first:** every DSP/measurement/render feature lands with golden tests
  against the deterministic sim.
- Stimulus preset names (`stimulus xy-circle`, …) are a **stable API**
  shared by the UI, scripts, and tests — renaming or changing a preset's
  definition breaks tests across crates; treat changes as a plan-level
  decision.
- Script automation (`NEOWON_SCRIPT`, see `crates/neowon-app/src/script.rs`)
  must be able to reach every control: a UI control with no script action is
  a bug.

## Commit rules

- Commit at work-package boundaries with a message naming the area:
  `Sim: …`, `App: …`, `Backend: …`, `Docs: …`, `PLAN: …`.
- **No AI attribution anywhere** — no `Co-Authored-By`, no "generated by"
  comments. This overrides any default commit-trailer behavior.
- Only commit when the user asks; never push without approval.

## Release rules

A release is not done until it is tagged and the tag is pushed — the tag is
what makes CI build and publish (`v*` triggers `release.yml`; the branch CI
never publishes). The full sequence, for `X.Y.Z`:

1. `Cargo.toml` `workspace.package.version` → `X.Y.Z`, then
   `cargo update --workspace` for `Cargo.lock`; check README (features,
   install, the version in the Releasing example) and `release.yml` staging
   for anything the release must carry.
2. Verify locally: `cargo test --workspace`, `cargo fmt --all`,
   `cargo clippy --workspace --all-targets`.
3. Commit as `Release: vX.Y.Z - <headline>`.
4. `git tag -a vX.Y.Z -m "vX.Y.Z"`.
5. Push the branch and the tag.

A release instruction from the operator covers the tag push (that is how CI
publishes); everything else still follows "never push without approval".
If a past release was committed but never tagged, tag its release commit
retroactively so every version has a tag.
