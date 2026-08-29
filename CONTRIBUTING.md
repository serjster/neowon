# Contributing to neowon

Thanks for considering it. This project has a few firm conventions — they
exist because an attached instrument and a GPU pipeline are both easy to
break silently.

## Ground rules

- **Sim-first.** Every DSP, measurement, or rendering feature lands with a
  test against the deterministic simulator (`neowon-sim`). If it can't be
  verified against a synthetic signal, say so in the PR and explain how it
  was verified instead.
- **Hardware discoveries get written down.** Anything learned on a real
  VDS1022 (register behavior, timing quirks, corrections to the reference
  implementations) goes into `docs/protocol-vds1022.md` in the same PR.
- **Stimulus preset names are a stable API.** They're shared by the UI,
  scripts, and tests; renaming one is a breaking change across crates.
- **Every UI control must be script-reachable.** If you add a control to
  the panel, add the matching action to
  `crates/neowon-app/src/script.rs` — the test suite drives the app that
  way.
- **Library crates stay engine-free.** `neowon-core`, `neowon-backend`,
  `neowon-dsp`, and `neowon-sim` must not grow Bevy/GPU dependencies; the
  app is the only place Bevy lives. New dependencies anywhere need a good
  reason in the PR description.
- **CPU is the oracle.** Any GPU implementation of a DSP operation must be
  tested against the CPU version within tolerance via readback.

## Before you open a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets   # zero warnings
cargo test                               # unit + virtual testbench
cargo test -p neowon-app --test ui_pixels -- --ignored   # needs a display
cargo test -p neowon-app --test ui_layout -- --ignored   # needs a display
```

If your change touches the driver and you have the hardware:

```sh
cargo run -p neowon-cli -- smoke                  # 1 kHz probe-comp check
cargo run -p neowon-vds1022 --example trigtest    # trigger matrix
```

Sim tests must stay deterministic: seeded PRNG only, no wall-clock time in
signal generation.

## Commit style

Prefix the area: `App:`, `Sim:`, `Backend:`, `Vds1022:`, `Dsp:`, `Docs:`,
`PLAN:`. First line says what changed; the body says why when it isn't
obvious.

## Larger work

Bigger work packages are written as specs in `docs/tasks/` before
implementation (see `docs/tasks/phase65-signals-spec.md` for the shape):
scope fence, hard rules, numbered work items, and test requirements. If
you're planning something substantial, open an issue first and we'll shape
a spec together. The roadmap lives in `PLAN.md`.

## Hardware safety notes

- Only one process can claim the device; the vendor app and neowon are
  mutually exclusive. (On macOS the OS won't stop a second claim — don't
  run two neowon processes against one scope.)
- The device drops its link without a keep-alive (`RUNSTOP` write every
  ≤3 s); the driver handles this, but keep it in mind when adding long
  blocking paths.
- FPGA bitstreams are vendor blobs referenced by path; never commit them.
