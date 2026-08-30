# neowon

[![CI](https://github.com/serjster/neowon/actions/workflows/ci.yml/badge.svg)](https://github.com/serjster/neowon/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

A high-performance, scope-grade oscilloscope application in Rust, built on
[Bevy](https://bevy.org) with a GPU digital-phosphor rendering pipeline and a
modular acquisition-backend architecture. The first supported instrument is
the **OWON VDS1022 / VDS1022I** USB oscilloscope, with a deterministic
simulated backend for development and testing; SDR and other sources are on
the roadmap.

![The neowon UI](docs/media/ui.png)

## Features

- **GPU digital-phosphor display**: compute-shader rasterization with
  intensity grading, persistence (off → infinite), vectors/dots/XY modes,
  optional CRT styling (phosphor halo, scanlines, vignette), and thermal /
  green-CRT palettes.
- **Full acquisition control**: edge, pulse-width, and slope triggers
  (hardware-verified; video trigger implemented but unverified), Auto /
  Normal / Single sweeps, holdoff, peak-detect, host-side averaging, roll
  mode, auto-set.
- **Measurements**: 18 automatic measurements with running statistics
  (mean/min/max/σ/n), draggable time & amplitude cursors, on-graph
  measurement guides, math channel (+, −, ×, ÷, d/dt, ∫) rendered as a
  first-class trace.
- **FFT spectrum** with six windows, amplitude-correct scaling, and
  zoom/pan.
- **Pass/fail testing** against a captured reference envelope, with the
  MULTI port TTL output.
- **Recording, history & export**: capture the record stream, scrub back
  through it frame by frame (history browser), save/reload lossless
  `.nwc` capture files (zstd), import the vendor app's `.cap` recordings,
  and export as WAV (16-bit PCM at the acquisition rate — an XY capture
  is directly replayable oscilloscope music), CSV, raw i8, or a PNG of
  the display.
- **Reference traces & sessions**: freeze a channel as a ghost trace for
  visual comparison; save/restore the full instrument setup — a session
  file is itself a neowon automation script, readable and editable.
- **Touch-scope interaction on a desktop**: drag the trigger level and
  position, drag traces to move offsets, scroll to change volts/div,
  shift+scroll for the timebase.
- **Fully scriptable**: every control is reachable from a plain-text
  automation script (`NEOWON_SCRIPT`), including plot-texture screenshots
  with regions of interest — the same mechanism the test suite uses.
- **Virtual testbench**: a deterministic signal engine (sine/square/
  trapezoid/chirp/AM/FM sums, XY figures, WAV playback, simulated
  triggering) verifies every DSP and render path in CI-friendly tests.

![Oscilloscope Quake](docs/media/quake-demo.png)

## Hardware

| Instrument | Status |
| --- | --- |
| OWON VDS1022 / VDS1022I | Working, hardware-verified (25 MHz, 2 ch, 100 MS/s) |
| OWON VDS2052 | Untested; the driver's register-table design should make it a small port |
| RTL-SDR, Flipper Zero | Planned (see `PLAN.md`) |

The protocol implementation was ported from the community
[OWON-VDS1022](https://github.com/florentbr/OWON-VDS1022) Python reference
and the decompiled vendor app, then verified against real hardware —
including a few places where this repo's findings *correct* the reference
(see `docs/protocol-vds1022.md`).

## Building

Requires stable Rust (edition 2024; 1.95+).

```sh
cargo build --release
```

- **macOS**: works out of the box (pure-Rust USB via `nusb`; no driver).
- **Linux**: install Bevy's system deps (`libasound2-dev libudev-dev` on
  Debian/Ubuntu) and install the shipped udev rules:

  ```sh
  sudo cp scripts/99-vds1022.rules /etc/udev/rules.d/
  sudo udevadm control --reload   # then replug the scope
  ```

  They grant USB access *and* stop the kernel's `usb_serial_simple` driver
  from claiming the scope's interface (which otherwise makes
  `claim_interface` fail with `EBUSY`). If a session is already wedged:
  `echo <bus>-<port>:1.0 | sudo tee
  /sys/bus/usb/drivers/usb_serial_simple/unbind` (see
  `docs/protocol-vds1022.md`).
- **Windows**: untested; contributions welcome.

### FPGA bitstreams (hardware only)

The VDS1022 needs an FPGA bitstream uploaded at every cold start. The
OWON bitstreams are vendored in [`3rdparty/fw/`](3rdparty/fw/) (see its
README for provenance — they are OWON's, not covered by this repo's
license), so a repo checkout works out of the box. neowon looks in
`$NEOWON_FPGA_DIR`, `./fwr`, `./3rdparty/fw`, then
`../OWON-VDS1022/fwr`.

## Running

```sh
cargo run --release -p neowon-app            # real hardware
cargo run --release -p neowon-app -- --sim   # simulated backend
cargo run --release -p neowon-app -- --demo  # Oscilloscope Quake (see below)
```

Only one process may use the scope at a time — close the vendor app first.

### The Quake demo

`--demo` plays back the *Oscilloscope Quake* stereo WAVs from
[lofibucket.com](https://www.lofibucket.com/articles/oscilloscope_quake.html)
in XY mode (left = X, right = Y) on a green CRT. Fetch the files first:

```sh
scripts/fetch-demo.sh
```

### Headless CLI

```sh
cargo run -p neowon-cli --                    # `neowon` binary
  probe | dump | stream | smoke | autoset
```

`neowon smoke` verifies the whole stack against the scope's own 1 kHz
probe-compensation signal.

### Scripting

Set `NEOWON_SCRIPT=path.txt` to drive the app from a plain-text action list
(stimulus selection, every control, screenshots, exports…). The full
grammar is documented at the top of `crates/neowon-app/src/script.rs`.

## Testing

```sh
cargo test                                        # unit + virtual testbench
cargo test -p neowon-app --test ui_pixels  -- --ignored  # render geometry (opens a window)
cargo test -p neowon-app --test ui_layout  -- --ignored  # layout invariants
cargo run -p neowon-vds1022 --example trigtest    # trigger matrix (needs hardware)
cargo test -p neowon-app --test shaders           # naga-validate all WGSL
```

## Repository layout

| Crate | Role |
| --- | --- |
| `neowon-core` | Engine-free shared types, WAV I/O |
| `neowon-backend` | Backend trait, config model, supervisor thread |
| `neowon-sim` | Deterministic signal engine / virtual testbench source |
| `neowon-vds1022` | VDS1022 USB driver (nusb), protocol constants |
| `neowon-dsp` | Measurements, statistics, FFT, math — the CPU oracle |
| `neowon-cli` | Headless bring-up and debugging tool |
| `neowon-app` | Bevy application: GPU pipeline, UI, scripting |

`PLAN.md` holds the roadmap and phase status;
`docs/protocol-vds1022.md` records every hardware-verified protocol fact.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The demo WAVs and FPGA
bitstreams are third-party content and are not covered.
