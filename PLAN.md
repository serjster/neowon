# Neowon

A high-performance, fully-featured oscilloscope application in Rust + Bevy, with GPU-offloaded rendering and DSP, built around a modular acquisition-backend abstraction. First backend: OWON VDS1022I. Planned backends: simulated source, RTL-SDR (realtime IQ decoding), Flipper Zero.

---

## 1. Research findings (2026-08-29)

### Why owowon never worked

`owowon` targets the **OWON HDS handheld series** (HDS2102S etc.), not the VDS1022:

- It speaks ASCII SCPI (`:DATa:WAVe:SCReen:CH1?`) — the VDS1022 has **no SCPI parser at all**; it uses a binary register protocol.
- It never uploads the FPGA bitstream the VDS1022 requires on every cold start.
- It expects a JSON header + 300-sample screen frames; the VDS1022 returns raw 5211-byte binary frames.
- Its USB layer is Windows-only WinRT (`windows::Devices::Usb`) — it doesn't even link on macOS.
- The cruel part: HDS and VDS share USB IDs (`5345:1234`), so it *enumerates* the VDS1022I and then hangs on the first command.

Conclusion: nothing to salvage as a driver. Its egui plot/side-panel structure is a mild UI reference at best.

### What we do have

- **`OWON-VDS1022/api/python/vds1022/vds1022.py`** (florentbr's community repo, 2664 lines) — a complete, commented reference implementation of the entire device backend: connect, FPGA upload, flash calibration, all registers, frame parsing, roll mode, auto-set, auto-calibration. **This is the porting bible.** Also `decoder.py` (I2C/UART/1-Wire) and `generator.py` (synthetic signals).
- **The vendor Java app** (`lib/owon-vds-tiny-1.1.5-cf19.jar`) — decompiled and cross-validated against the Python. Full register map, trigger word bit layout, calibration DAC math, `.cap` record format, and machine parameter files all extracted (see §3 and §10).
- **`~/projects/GoL`** — Bevy **0.19** patterns: compute-shader plugins (physarum: 4 WGSL passes, storage textures, texture→sprite display), custom instanced render pipelines, egui integration on `EguiPrimaryContextPass`, GPU readback screenshots, naga-based WGSL validation tests, and written tutorials in `GoL/docs/bevy/`. Also the dev-profile trick (`opt-level=1` workspace, `opt-level=3` deps + hot crates).
- **Hardware confirmed present**: the scope enumerates on this Mac as VID `0x5345` / PID `0x1234` (strings "ZHBI2.0"/"ZPRO2.0"). CH1 is on the 1 kHz 5 V test signal — perfect bring-up target.
- **Ecosystem** (verified current, late 2026): Bevy 0.19.1 stable; `bevy_egui` 0.42 (egui 0.34) is the standard tool-UI pairing; `nusb` is the pure-Rust USB stack (no libusb, no drivers needed on macOS); no existing usable VDS1022 crate (one early-stage Windows-only GitHub project, `Atmel2005/ATMELOWON`, useful only as a reference); SDR: pure-Rust `rs-rtl`/`librtlsdr-rs` (nusb-based) or `seify` for multi-hardware; Flipper: `flipper-rpc` crate speaks the official protobuf RPC over USB CDC.

### Device essentials (VDS1022I)

- 2 channels, 8-bit ADC (samples are `i8`, ±125 = 10 vertical divs), 100 MS/s max, **5 K memory per channel**. USB bulk only, one OUT + one IN endpoint, interface 0.
- Command = `u32 LE address` + `u8 size (1|2|4)` + LE value. Response = 5 bytes: status char (`'S'`=ok, `'E'`=busy/empty, …) + `u32 LE` value. Register file is **byte-addressed** — multi-byte writes spill into consecutive addresses.
- Host must upload the FPGA bitstream (from `OWON-VDS1022/fwr/*.bin`, selected by hardware version string in flash) on every cold start: `QUERY_FPGA 0x0223` → if ≠1, `LOAD_FPGA 0x4000` with total size → device replies chunk size → send `u32 index` + payload chunks, each acked.
- Factory calibration lives in a 2002-byte flash blob (`READ_FLASH 0x01b0`): 6 × `u16[10]` arrays (gain/amplitude/compensation × 2 channels, per voltage range), hardware version + serial strings, phasefine.
- A frame (`GET_DATA 0x1000`) is 5211 bytes per enabled channel: channel byte, freq-meter counters (`time_sum`, `period_num`), `u16 cursor`, 100-byte trigger buffer, then 5100 `i8` samples (50 pre + 5000 + 50 post).
- Sample rate = `100 MHz / prescaler` (`SET_TIMEBASE 0x52`), ladder 2.5 S/s → 100 MS/s. Voltage ranges 50 mV → 50 V full-scale (10 div), attenuation relay at ≥1 V range. Roll mode below 2.5 kS/s.
- Trigger: edge/pulse/slope/video + alternate, packed into a u16 at `0x24` plus per-channel level/holdoff/width registers (mantissa·10^exp encoding in 0.1 ns units). Auto sweep is host-side; Normal/Single gate on `GET_DATAFINISHED`/`GET_TRIGGERED`.
- **Keep-alive is mandatory**: send `RUNSTOP=1` at least every ~3 s when idle or the link drops.

Full register map and bit layouts: §10 references + `vds1022.py` lines cited there.

---

## 2. Goals & non-goals

**Goals**

- Feature parity with advanced bench scopes where the hardware allows, and beyond it where software can compensate (digital-phosphor rendering, deep measurement statistics, protocol decoding, segmented history, FFT quality).
- GPU offload for everything per-sample: waveform rasterization, persistence/intensity grading, FFT, decimation for display, XY, waterfall.
- Clean backend abstraction so a new instrument = one crate implementing one trait set.
- Cross-platform (macOS first, Linux next, Windows eventually — nusb supports all three).

**Non-goals (for now)**

- Driving the AWG of other OWON models, VDS2052 support (kept cheap by the address-table design, but not a milestone).
- Replacing the vendor app's SCPI server.
- On-device Flipper apps (host-side RPC only).

---

## 3. Architecture

### Workspace layout

```
neowon/
├── Cargo.toml                 # workspace; Bevy pinned =0.19, edition 2024
├── crates/
│   ├── neowon-core/           # engine-free: units, frames, ring buffers, calibration model,
│   │                          #   device-capability descriptors, config types
│   ├── neowon-backend/        # the AcquisitionBackend trait + control-plane types
│   ├── neowon-sim/            # simulated backend (port of generator.py + noise/jitter models)
│   ├── neowon-vds1022/        # nusb driver: codec, FPGA upload, flash cal, submitor, frames
│   ├── neowon-dsp/            # measurements, FFT, filters, math channels, interpolation,
│   │                          #   protocol decoders (engine-free, rustfft; GPU variants live in app)
│   ├── neowon-cli/            # headless bring-up & debugging tool (probe/dump/stream/cal)
│   └── neowon-app/            # Bevy 0.19 + bevy_egui application, GPU pipelines (WGSL)
├── shaders/                   # WGSL (validated by naga in tests)
└── docs/
    ├── protocol-vds1022.md    # our own protocol doc, grown as we verify on hardware
    └── ddr/                   # decision records (pattern borrowed from GoL)
```

Later: `neowon-sdr/` (rs-rtl or seify source + demodulators), `neowon-flipper/` (flipper-rpc source).

### The backend abstraction

Designed from **two concrete implementations from day one** (sim + vds1022) so the trait is shaped by reality, not speculation:

- `Capabilities` — static descriptor: channel count, sample-rate ladder, voltage ranges, probe factors, coupling options, trigger types, memory depth, streaming vs framed, has-calibration, has-multi-port. The UI builds itself from this (a backend with no video trigger simply shows none).
- `ScopeConfig` — desired state: per-channel (enabled, range index, coupling, probe, offset), timebase index, trigger spec, sweep mode, acquisition mode (sample/peak/average), pre/post trigger position, roll.
- `AcquisitionBackend` (runs on its own thread, owned by a supervisor):
  - `apply(&ConfigDelta)` — backends coalesce writes (the VDS1022 needs ordered, rate-limited register submission; gain/channel writes clear the sample buffer).
  - frame stream out: `crossbeam` channel of `Arc<CaptureFrame>` (immutable, timestamped, carries the config snapshot it was captured under + calibration-applied scale/offset so consumers never guess).
  - control queries: triggered status, real sample rate, frequency-meter reading.
- Two stream shapes, one enum: `Framed` (scope: discrete 5000-sample records) and `Continuous` (SDR/roll: unbounded sample/IQ stream feeding a ring buffer). Roll mode on the VDS1022 is exposed as `Continuous` — same path SDR will use later. This is the key decision that makes SDR a backend rather than a second app.

### Threading & data flow

```
[USB thread (nusb, blocking)] --frames--> [SPSC channel] --> [Bevy ECS ingest system]
        ^ pending-command map (coalesced, ordered, keep-alive timer)
[Bevy main world]  UI (egui) -> ConfigDelta -> backend; frames -> History ring + LatestFrame
[Render world]     extract LatestFrame/history -> GPU buffers -> compute passes -> screen
[DSP pool]         rayon: measurements, decoders, CPU-FFT on latest frame (results as ECS resources)
```

- Frames are `Arc`-shared, never copied per consumer. History is a bounded ring (segmented memory: thousands of frames — at 5211 B/frame, 100 MB holds ~19 k frames; make depth configurable).
- The device thread owns reconnection: hot-unplug → supervisor re-enumerates, re-uploads FPGA if needed, replays last config.

### GPU strategy (Bevy 0.19)

Follow the physarum plugin skeleton from GoL (`RenderStartup` pipeline init, `Render` prepare sets, dispatch system in the `RenderGraph` schedule before `camera_driver`):

1. **Waveform rasterizer (compute)**: sample buffer (SSBO) → line-segment accumulation into an `r32float` intensity texture with anti-aliased coverage. One dispatch per visible trace; supports dots and vectors, sin(x)/x interpolated (interpolation kernel also on GPU) when < ~1 screen sample per pixel column.
2. **Persistence pass (compute)**: exponential decay of the intensity texture each frame (digital-phosphor look; decay constant = persistence setting, ∞ for infinite persistence). New frames add; nothing is redrawn from history — this is what lets us blend thousands of acquisitions like a DPO.
3. **Colormap/compose pass**: intensity → color LUT (per-channel hue, intensity grading), composited with grid/graticule; output storage texture displayed via `Sprite::from_image` (physarum pattern).
4. **GPU FFT** (phase 5+): radix-2 Stockham compute FFT for 4096/8192 points with window pre-multiply; powers spectrum view and (later) SDR waterfall. CPU `rustfft` is the correctness oracle and fallback.
5. XY mode = same accumulation shader with (ch1, ch2) as coordinates. Waterfall = scrolling storage texture fed a row per FFT.

Gotchas already known from GoL: uniform struct field order must match WGSL; `NoAutomaticBatching` + explicit `Aabb` for per-entity instance buffers; use `Readback::texture` for headless screenshots; validate every WGSL file with naga in unit tests.

### UI

`bevy_egui` 0.42 on `EguiPrimaryContextPass`: side control panel (channels / timebase / trigger / acquisition), top status bar (run state, sample rate, freq meter), bottom measurement strip, floating dialogs (measure config, decode setup, cal). The waveform view is the Bevy-rendered texture; egui draws overlays only where cheap (cursor readouts, decode tables). Keyboard-first bindings from day one. Persist app state as RON/JSON per device serial.

---

## 4. Incremental phases

Each phase ends with something runnable and testable — most against the real scope on your desk (CH1 = 1 kHz 5 Vpp probe-comp signal).

> **Status 2026-08-29:** Phases 0–5 are DONE and hardware-verified (unit
> VDS1022I2324259, hw V5.0.1). The app is now a working oscilloscope: live
> phosphor display (GPU decay/raster/compose, dots/XY, persistence), full
> acquisition control (Normal/Single gating, peak, roll, holdoff, averaging,
> force trigger, auto-set), egui control panel, 18 auto-measurements with
> running statistics (σ(freq) = 1.1 mHz on the probe-comp signal), draggable
> time/amplitude cursors, math channel (+,−,×,÷,d/dt,∫ as a third phosphor
> layer), and a windowed FFT spectrum view. Verified facts live in
> `docs/protocol-vds1022.md`. Still open: FPGA upload untested until a
> power-cycle; roll-mode incremental streaming, sin(x)/x interpolation, GPU
> FFT deferred. Next: Phase 6 (advanced triggers, pass/fail, MULTI port) or
> Phase 7 (history/recording/export).

### Phase 0 — Scaffold + simulated trace (½ day)

Workspace, git init, Bevy 0.19 pinned (submodule optional; reuse GoL's profile settings), CI-less `cargo test` culture. `neowon-sim` produces sine/square/noise frames at a configurable rate. Bevy window renders the trace as a simple polyline (CPU mesh — GPU pipeline comes in Phase 4) with a graticule.
**Done when:** window shows a live scrolling simulated 1 kHz square wave; `cargo test` runs sim golden tests.

### Phase 1 — VDS1022 bring-up, headless (the critical de-risk)

`neowon-vds1022` + `neowon-cli`. Port from `vds1022.py` in this order:

1. Codec: command pack, 5-byte response parse, single 6000-byte read buffer, retry policy (3× with backoff on USB error; 60 ms retry on `'E'`).
2. Connect: enumerate `5345:1234`, claim interface 0, discover bulk endpoints, `MACHINE_TYPE 0x4001='V'` probe (expect value 1).
3. Flash read + calibration parse (2002 bytes, header `AA 55`, version 2, six `u16[10]` arrays, version/serial strings, phasefine).
4. FPGA upload with progress (bitstreams referenced from `OWON-VDS1022/fwr/` via config path — vendor blobs stay out of our repo).
5. Init register sequence (per §1.4 of the research: CHL_ON, PHASE_FINE, DM=5100, PRE/SUF_TRG, TRG=0, holdoff, edge levels, timebase) + **keep-alive thread**.
6. `GET_DATA` frame parse; voltage conversion `v = raw × range × probe / 250`; frequency meter `period_num/time_sum × 100e6`.

CLI verbs: `neowon probe` (IDs, version, serial, cal dump), `neowon dump --ch1 --rate 5M` (N frames to stdout/CSV), `neowon stream` (continuous with live Vpp/freq).
**Done when:** `neowon dump` against CH1 measures ~5 Vpp and ~1.000 kHz from the probe-comp signal, repeatedly, without wedging the device (survives ctrl-C + reconnect).

### Phase 2 — Backend trait + live app on real hardware

Extract `neowon-backend` from the two implementations. Device supervisor thread with pending-command coalescing (ordered map keyed by register, flushed before each `GET_DATA`; write order `zero_off → volt_gain → channel`). App gains a backend selector (sim / vds1022), ingest system, and shows the live real waveform. Run/Stop. Auto sweep only.
**Done when:** app displays the live 1 kHz test signal; unplugging and replugging the scope recovers automatically.

### Phase 3 — Full acquisition control

- Timebase ladder (all 32 steps), roll mode below 100 ms/div (continuous stream shape, cursor-delta reassembly modulo 5120).
- Per channel: range (10 steps incl. attenuation-relay bit), coupling AC/DC/GND, probe ×1…×1000, vertical offset with calibrated DAC math (`zero = comp − pos0·ampl/100`, `gain = gain_cal`).
- Edge trigger: source, slope, level (hi/lo pair with 10-LSB hysteresis), holdoff (mantissa/exponent, byte-swapped), auto/normal/single sweep with `GET_DATAFINISHED`/`GET_TRIGGERED` gating, force trigger, single-shot re-arm.
- Horizontal trigger position (pre/suf registers, `HTP_ERR=11` correction — verify empirically on this unit and record in `docs/protocol-vds1022.md`).
- Peak-detect (odd=max/even=min unpack), software averaging (2…128, running).
- Auto-set (port `autoset` from vds1022.py).

**Done when:** every control in the vendor app's acquisition domain has an equivalent, validated against the test signal + a second source if available.

### Phase 4 — GPU waveform engine

The phases-1–3 CPU polyline is replaced by the compute pipeline (§3 GPU strategy): accumulation, persistence, colormap, interpolation, dots/vectors, XY mode, configurable graticule. Persistence settings: off / 50 ms…10 s / ∞. Intensity-graded rendering of the full history ring (each incoming frame is one accumulation dispatch, so thousands of wfms blend like a DPO).
**Done when:** 60 fps with persistence at max frame rate from the device; naga tests pass; screenshot readback works headless; visual A/B vs Phase 3 shows no geometry error.

### Phase 5 — Measurements, cursors, math, FFT

- All 18 vendor measurements (Period, Freq, Rise/Fall time, ±Width, ±Duty, Vpp, Vmax, Vmin, Vamp, Vtop, Vbase, Over/Preshoot, Vavg, Vrms) + delay measurements + **statistics** (current/mean/min/max/σ/count — beyond the vendor app), gated by cursor region. CPU (rayon), on the latest frame; measurement engine unit-tested against `neowon-sim` golden signals.
- Cursors: time pair, amplitude pair per trace, FFT freq/amp cursors, plus the fork's extras (pulse-width-to-trigger cursor, duty/phase readout).
- Math channels: ch1±ch2, ×, ÷, plus derivative, integral, low/high-pass filter (biquad), inversion — math output is a first-class trace (measurable, decodable).
- FFT: rustfft CPU first (windows: Rectangle, Hamming, Hann, Blackman, Flattop, Triangular, Kaiser; dBV/dBm/Vrms scales; averaging; peak-hold; peak markers), GPU compute FFT once outputs match CPU to tolerance.

### Phase 6 — Advanced triggers, pass/fail, MULTI port

Pulse/slope/video triggers and alternate mode (bit layouts from the Java app — note the Slope/Video code swap bug in vds1022.py: use Java's `Edge=0, Slope=1, Video=2, Pulse=3`). Width encoding per FPGA version (V3+: `t×1e8` split u16/u16). Pass/fail rule engine (up to 8 rules, per-channel h/v tolerance masks, GPU point-in-mask test is trivial in the rasterizer), MULTI port modes (trigger out / pass-fail out / trigger in).

### Phase 7 — Capture workflows

- Segmented history browser (scrub through the frame ring, like modern scope history mode).
- Recording/playback to our own format (frame ring → file, zstd); **import** of vendor `.cap` (big-endian format documented in research).
- Export: CSV, binary, PNG of the view, reference waveforms (load/save, rendered as ghost traces).
- Full session save/restore.

### Phase 8 — Protocol decoders

Decoder framework over any trace (analog threshold → bitstream → decoder): UART, I2C, SPI, 1-Wire (port/verify against `decoder.py`), then CAN. Output: overlay annotations in the waveform view + a searchable table with export. Decoders run on the DSP pool over history, not just the latest frame.

### Phase 9 — Calibration

Auto-cal port (compensation pass descending ranges @ DC, amplitude pass ascending @ AC, adaptive convergence, probes-off interlock), manual fine-tune dialog, per-serial JSON cal store, and — explicitly guarded, double-confirm — flash write-back.

### Phase 10 — SDR backend

`neowon-sdr`: RTL-SDR via `rs-rtl` (pure Rust/nusb) first, `seify` if/when more hardware is wanted. IQ enters as a `Continuous` stream (same path as roll mode). New views: spectrum (GPU FFT), waterfall (scrolling storage texture). Demodulators (AM/NFM/WFM/SSB) produce ordinary traces → existing measurements/decoders apply. Realtime decode targets: start with things the existing decoder framework nearly covers (OOK/ASK keyfobs via threshold→UART-ish framing), grow toward POCSAG/ADS-B as separate decoder plugins.

### Phase 11 — Flipper Zero (exploratory)

`neowon-flipper` using the `flipper-rpc` crate (protobuf RPC over USB CDC). First target: pull raw Sub-GHz captures as a `Framed` source for offline decode; later, a live raw-CDC streaming companion app if RPC throughput disappoints. Explicitly a spike — scope it after Phase 10 learnings.

---

## 5. Testing strategy

- **Sim-first**: every DSP/measurement/decoder feature lands with golden tests against `neowon-sim` (deterministic PRNG, per GoL's DDR-0003 spirit).
- **Codec tests**: byte-exact command/response/frame fixtures captured from the real device in Phase 1 (`neowon-cli record-fixtures`), replayed in unit tests forever after.
- **Shader tests**: naga parse+validate every WGSL (GoL `tests/shaders.rs` pattern); GPU-vs-CPU FFT/rasterizer comparison tests via `Readback`.
- **Hardware smoke script**: `neowon smoke` — probe, cal read, FPGA state, 100 frames, assert 1 kHz/5 Vpp within tolerance. Run before merging anything touching the driver.
- **Protocol doc discipline**: anything we verify or discover on hardware (e.g. `HTP_ERR`, trigger-code swap) goes into `docs/protocol-vds1022.md` immediately.

## 6. Risks & gotchas (carry into implementation)

1. Keep-alive (`RUNSTOP=1` ≤3 s) or the link drops silently.
2. Register writes clear the sample buffer (gain/channel) — coalesce + order writes, flush before `GET_DATA`.
3. `vds1022.py` Slope/Video trigger-code swap — trust the Java constants, verify on hardware.
4. `HTP_ERR = 11` horizontal correction is empirical — verify per-unit.
5. FPGA bitstreams are vendor blobs — reference from the OWON-VDS1022 checkout or a user config path; don't redistribute.
6. macOS + nusb needs no driver, but only one process can claim interface 0 — the vendor Java app and neowon are mutually exclusive.
7. 8-bit ADC clips at ±125; flag `|raw| ≥ 125` as clipped in the UI.
8. Bevy 0.19 API era differs from most online docs — GoL's `docs/bevy/*.md` and vendored source are the authority.

## 7. Reference index

| What | Where |
|---|---|
| Protocol reference impl (bible) | `OWON-VDS1022/api/python/vds1022/vds1022.py` |
| Decoders / signal generator | `OWON-VDS1022/api/python/vds1022/{decoder,generator}.py` |
| Register map (Java, authoritative) | jar: `com.owon.uppersoft.vds.device.interpret.DeviceAddressTable` (`javap -p -c -constants -cp lib/owon-vds-tiny-1.1.5-cf19.jar …`) |
| Trigger bit layout | jar: `…comm.ext.TinyTrgSubmitHandler`, `…ClockTimeAdjuster` |
| Machine params (rates, ranges, timebases) | jar resource: `com/owon/uppersoft/dso/model/machine/params/VDS1022ONE.txt` |
| `.cap` record format | jar: `com.owon.uppersoft.dso.function.record.RecordFileIO` |
| FPGA bitstreams | `OWON-VDS1022/fwr/VDS1022_FPGAV*.bin` |
| Bevy 0.19 compute pattern | `GoL/tools/physarum/src/gpu.rs`, `GoL/docs/bevy/compute_shaders_wgsl.md` |
| Custom render pipeline pattern | `GoL/tools/plife3d/src/draw.rs`, `GoL/docs/troubleshooting/bevy.md` |
| egui integration pattern | `GoL/tools/physarum/src/ui.rs` |
| SDR crates | `rs-rtl`, `librtlsdr-rs`, `seify`, FutureSDR (framework, not chosen) |
| Flipper host RPC | `flipper-rpc` crate + `flipperdevices/flipperzero-protobuf` |

---

**Next action:** Phase 0 scaffold, then Phase 1 bring-up against the connected scope (CH1 @ 1 kHz 5 V) — the single riskiest and most valuable step.
