# Harness — how this project is built

The harness reads this file. Keep it short and true; everything here is something an agent will
act on.

## Gate

All of these must pass before a work item is ticked (sim only — never touch real USB hardware
from automated work; see `AGENTS.md`):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo build
cargo test
cargo test -p neowon-app --test shaders
cargo test -p neowon-app --test ui_pixels -- --ignored
```

`ui_pixels` briefly opens a window; it is sim-only and safe to run. If the session cannot open
a window, note it in the work item rather than silently skipping the check.

Phase 10 adds two gate lines when it lands (see `docs/tasks/phase10-sdr-spec.md`):

```bash
cargo test --workspace --features neowon-ml/ort   # the ML feature is default-off
cargo test -p neowon-app --test sdr_integration   # tune -> detect -> classify -> catalog
```

A spec's testing section is not a substitute for these lines once the code exists.

## Layout

- **Tracker:** `PLAN.md` (§4 phase status block is the live state; fix it first if stale)
- **Briefs:** `docs/tasks/` — one spec file per work package; follow it exactly, record
  deviations in the spec file
- **Critic rounds:** `.critic/` (gitignored scratch)
- **Decisions:** `PLAN.md` (architecture, ecosystem, phase decisions) and
  `docs/protocol-vds1022.md` (hardware-verified protocol facts); user decisions are recorded
  with dates
- **Troubleshooting:** `docs/protocol-vds1022.md` for hardware/link behavior; nothing else yet

## Conventions an agent must follow

`AGENTS.md` holds them — one home per fact, do not copy. Non-negotiables it points at: Bevy
0.19.1 pinned; library crates stay engine-free; i8 sample encoding; sim tests deterministic;
no new dependencies without user approval; no AI attribution in commits.
