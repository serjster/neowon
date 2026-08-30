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
provides the virtual testbench. Phases 0–6 are done and hardware-verified;
the current phase (6.5) is the virtual testbench + scope-grade UI.

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
  No `neowon-cli`, no `neowon-app` without `--sim`, no examples from
  `neowon-vds1022`. Automated/delegated runs use `--sim` only.
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
- **Sample encoding:** i8, ±125 = full vertical range (10 divisions),
  `volts_per_lsb` + `zero_volts` per capture. Sim produces the same encoding
  as hardware — one code path for every consumer. Don't invent new encodings.
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
