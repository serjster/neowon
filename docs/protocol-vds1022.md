# VDS1022 protocol — hardware-verified notes

Facts confirmed against the real unit (serial `VDS1022I2324259`, hw `V5.0.1`,
macOS, nusb). The full register map lives in
`crates/neowon-vds1022/src/consts.rs`; primary references are
`OWON-VDS1022/api/python/vds1022/vds1022.py` and the decompiled vendor jar.

## Verified 2026-08-29 (Phase 1 bring-up)

- **Enumeration**: VID `0x5345` PID `0x1234`, product string "ZPRO2.0",
  manufacturer "ZHBI2.0". Interface 0, bulk OUT `0x03`-ish / bulk IN — both
  discovered dynamically; nusb claims without any driver on macOS.
- **Machine probe** (`0x4001` = `'V'`, then 50 ms pause, then read): reply
  value 1. The pause matters; the vendor app does the same.
- **Flash read** (`0x01B0`): exactly 2002 bytes in one bulk IN transfer,
  header `AA 55`, version 2. Serial + hw version + full calibration tables
  parsed cleanly (this unit: phasefine 0, OEM 1).
- **Hardware version `V5.0.1`** maps to FPGA generation 5
  (`VDS1022_FPGAV5_gaoyun.bin`) via the `V<n>.` rule. This unit held its FPGA
  across USB reconnects (query `0x0223` → 1), so upload only happens after
  power-cycle. Upload path not yet exercised on hardware — first power-cycle
  will test it.
- **Acquisition**: `GET_DATA 0x1000` with u16 `0x0405` (CH1 on, CH2 off)
  returns one 5211-byte frame per enabled channel in a single bulk transfer
  (terminates on the 91-byte short packet). Not-ready is a 5-byte `'E'`
  response; 60 ms retry works. Sustained ~36 frames/s at 250 kS/s with
  single-transfer reads, no coalescing yet.
- **Frame contents**: layout exactly as documented (channel byte, freq-meter
  counters, cursor, 100-byte trigger buffer at 11, 5100 samples at 111).
  Measured the 1 kHz probe-comp signal at 999.99–1000.00 Hz (software zero
  crossing) and the calibrated voltage scale is correct: 0–5 V square reads
  5.0 Vpp + probe overshoot.
- **Frequency meter** (`time_sum`/`period_num` × 100 MHz): agrees to 1.0000
  kHz — but only when the freq-ref level (`0x4A`) actually sits within the
  signal swing. With a nonsense reference it happily reports garbage
  (e.g. 50 MHz). Set it from the trigger level, and ignore readings when the
  trigger level is off-screen.
- **Calibration DAC math**: `zero = comp - pos0*ampl/100`, `gain = gain[vb]`
  produces correct absolute voltages at both relay settings (tested 1 V/div
  relay-on and 0.2 V/div relay-off). Write order zero→gain→channel-byte, as
  documented.
- **Gain table shape**: this unit's gain cal is two descending runs with a
  jump between vb 5 and 6 — the attenuation-relay boundary
  (`[1269 … 555 | 989 … 560]`). A useful sanity check when parsing flash.

- **macOS + nusb does NOT give exclusive access**: two processes can claim
  interface 0 and stream simultaneously (verified: CLI + app both pulled ~36
  frames/s at once). Their command/response pairs can interleave — treat this
  as a footgun, not a feature. Don't run two neowon processes against one
  scope; a lock file may be worth adding.

## Verified 2026-08-29 (Phase 3)

- **Normal/Single sweep gating**: polling `GET_DATAFINISHED (0x7A)` and
  `GET_TRIGGERED (0x01)` (masked by trigger-source bit) before `GET_DATA`
  works exactly as documented — frames flow when the edge level is inside the
  signal, starve (clean `'E'`/not-ready) when it isn't. Single = one gated
  record then host stop.
- **Peak-detect (`0x09`)**: hardware-confirmed active — the min/max pair
  interleave doubles apparent zero crossings (software freq reads 2 kHz on a
  1 kHz square while the hardware meter stays at 1 kHz). Consumers must
  unpack odd=max/even=min pairs.
- **Roll mode (`0x0A`)**: engages below 2.5 kS/s; frames keep flowing with
  progressive content. (Incremental cursor-based streaming still not
  implemented.)
- **Holdoff encoding**: mantissa/exponent formula reproduces the documented
  power-on default `0x8002` for 100 ns.
- **Auto-set** (range sweep → amplitude fit → freq-based rate pick → trigger
  at midpoint) converges on the probe-comp signal in ~6 captures.

## Verified 2026-08-29 (Phase 6 — pulse/slope triggers, live)

- **Trigger type codes**: Java's mapping confirmed on hardware — Slope = 1
  triggers correctly (both `<` and `>` width conditions behave as expected on
  the probe-comp edge), so Edge=0 / Slope=1 / Video=2 / Pulse=3 stands.
- **Pulse/slope condition codes (trigger word bits 5-7)**: bit 2 = polarity,
  bits 0-1 = comparator. Positive `>`/`=`/`<` = 0/1/2, negative = **4/5/6**.
  The Python reference's 3/4/5 is WRONG — negative conditions silently starve
  with it. (This matches the truncated Java decompile `{0,1,2,4,5,...}`.)
- **Pulse level polarity**: the edge-level pair for a pulse trigger must be
  packed with the slope matching the pulse polarity (falling packing for
  negative pulses), or negative conditions starve.
- **Width registers** (FPGA >= V3): `m = seconds × 1e8` split u16 gl/hl at
  0x42/0x44 (CH1), 0x46/0x48 (CH2) — verified by `>`/`<` behavior at 400 µs
  and 600 µs against the 500 µs probe-comp half-periods.
- **Slope thresholds** (0x10/0x12, `(upper&0xFF)|((lower&0xFF)<<8)`) verified.
- `SET_MULTI` (0x06) and `SET_PF` (0x07) writes are acked; electrical
  behavior of the MULTI port not yet scoped.
- Test harness: `cargo run -p neowon-vds1022 --example trigtest` (requires
  the probe-comp signal on CH1).
- **Post-reconnect arm transient**: the first Normal-gated capture in a
  session that starts immediately after another session used the device can
  take >2 s to arm (observed once: edge control starved at a 2 s deadline
  right after a `smoke` run, then passed standalone repeatedly). Allow ~4 s
  for the first gated capture after reconnect.

## Verified 2026-08-30 (Linux host bring-up)

- **Permissions**: `/dev/bus/usb/<bus>/<dev>` is root-owned `0660` by
  default; without a udev rule granting access, `open()` fails with
  `Permission denied` and the app retries silently (error only in the window
  title). Same symptom class as "nothing happens".
- **Kernel driver squat**: mainline Linux matches VID `0x5345` / PID `0x1234`
  in `usb_serial_simple`, which binds interface 0 and makes nusb's
  `claim_interface` fail with `EBUSY` ("interface is busy (errno 16)").
  The scope is not a serial device; the driver must be unbound. Fix in the
  repo README (udev rule with a `RUN+=` unbind on plug-in). Neither problem
  exists on macOS.

## Still to verify

- FPGA upload handshake on a cold (power-cycled) device.
- `HTP_ERR = 11` horizontal correction (need a fast edge + timebase sweep).
- Video trigger (word packing implemented from the decompile; unverified).
- MULTI port electrical behavior (trigger out/in, pass-fail out).
- Roll-mode incremental cursor arithmetic (DM=5120, circular).
- Keep-alive: how quickly the link actually drops when idle (>3 s claimed).

## Capture files (vendor `.cap` format)

Reverse-engineered from the vendor jar (`RecordFileIO`, `RecordControlTiny`,
reader path `readHeader`/`readFrame`; all multi-byte values BIG-endian).
Importer: `neowon-core/src/owon_cap.rs`.

- **Header (34 bytes):** 10 ASCII `"SPB"+machine` (`SPBVDS1022`; readers
  check the `SPBVDS` prefix), machine type i32 (VDS1022 = 100, VDS2052 =
  102), record version i32 (current = 4), extend i32 (`extend >> 24` must
  be 3 for a record file, version ≥ 2), file size i32, timegap-ms i32,
  frame count i32. The last three are written 0 and patched when recording
  stops — a crashed recording leaves zeros; walk the frame chain to EOF.
- **Frame:** framelen i32 (bytes after the field), timebase index i32,
  horizontal trigger position i32 (units unverified), peak-detect u8
  (v ≥ 3), deep-memory length i32 (v ≥ 4), then channel blocks until
  `framelen` is consumed.
- **Channel block:** ch u8 (0-based), blocklen i32 = bytes after the field
  (40 metadata + datalen; the vendor patches `endPtr - lenPos - 4`),
  inverse i32 (v ≥ 1; 1 = inverted), initPos i32 (always 0),
  screendatalen i32, datalen i32 (writers duplicate screendatalen —
  readers must trust datalen), slowMove i32, pos0 i32 (vertical offset in
  ADC counts), voltbase index i32, probe index i32, freq f32 (Hz, may be
  0), cycle f32 (1/freq; +Inf when freq = 0), then `datalen` raw i8
  samples in the standard ±125 = 10 div encoding (25 LSB/div).
- **Index tables** (`params/VDS1022ONE.txt` in the jar): voltbase 0–9 =
  5 mV…5 V/div; probe 0–6 = ×1…×1000; timebase 0–31 = 5 ns…100 s/div
  (roll from 100 ms/div).
- **Sample rate** is not stored; inferred as
  `min(100 MS/s, 5000 / (20 div × timebase))` — reproduces both anchors
  (100 MS/s at ≤ 2.5 µs/div; 2500 S/s roll threshold at 100 ms/div).
  INFERRED, not hardware-verified.
- Full field-level spec with javap evidence: session scratch notes → this
  section; the importer's unit tests carry byte-exact fixtures.

## Verified 2026-08-30 (roll mode: can the device stream?)

Harness: `cargo run -p neowon-vds1022 --example rolltest` (CH1 on the 1 kHz
probe-comp signal). Question: is there a gapless sample stream the host can
accumulate into a long record at full rate (phase78 spec, gap G2)?

- **`SET_ROLLMODE (0x0A)` is accepted at every sample rate**, not just below
  the 2500 S/s threshold `set_sample_rate` uses — matching the vendor Python,
  which exposes `roll` as an explicit override (`set_sampling(rate, roll=…)`).
  Frames keep flowing at all rates with roll forced on.
- **But the write cursor only advances at or below the threshold.** Frame
  header `cursor` (u16 at offset 9, already parsed into `RawFrame::cursor`)
  behaves as:
  - roll off, any rate → constant **5108** (> SAMPLES; the Python asserts
    exactly this: `assert self.rollmode or cursor >= SAMPLES`);
  - roll on at 2.5 kS/s → **moves, 752…5100, 232 distinct values over 3 s**,
    i.e. a genuine progressive fill the host can follow;
  - roll on at 25 kS/s, 250 kS/s, 2.5 MS/s → constant **5100**. The buffer is
    always reported complete, so there is no write position to track even
    though the buffer wraps 26x slower than we poll at 25 kS/s.

  So progressive streaming is real, and it stops where the vendor threshold
  is. The threshold is not arbitrary policy — it is where the device actually
  fills progressively.
- **Read throughput is far higher than the app uses.** Polling `GET_DATA` in
  a tight loop serviced **~131 reads/s (7.6 ms round trip)** at every rate,
  versus the ~36 frames/s the app's loop achieves. At 250 kS/s a fresh
  5000-sample buffer exists every 20 ms (50/s), so a faster reader could take
  every buffer instead of ~71 % of them — but the frames carry no timestamp
  and, outside roll, no cursor, so contiguity between consecutive buffers is
  unproven and would have to be established by the host (timestamping, or
  correlating one record's tail against the next one's head).
- Bandwidth is not the constraint in this range: 5211-byte frames at 131/s is
  ~683 kB/s, and the samples themselves at 250 kS/s x 2 channels are 500 kB/s
  — both far inside USB 2.0. The limit is the acquisition model and the
  per-read round trip, not the link. (At the top of the ladder it would be:
  100 MS/s x 2 ch = 200 MB/s, well beyond USB 2.0.)

**Consequence for a host-side deep record:** truly gapless capture is
available only at <= 2.5 kS/s, which is too slow to render a 1 kHz signal
properly. Above that the honest option is a segmented record — many complete
buffers placed on a time axis with explicit gaps — whose coverage can be
pushed from today's ~71 % toward ~100 % by reading faster, but which needs a
capture timestamp on `CaptureFrame` to be placed truthfully.

## Verified 2026-08-30 (peak detect: the min/max pair phase is not fixed)

Peak-detect records interleave a minimum and a maximum per decimation
interval, which `AcqMode`'s doc comment described as "odd = max, even = min".
Measured on the probe-comp signal at 500 S/s with peak engaged:

- One exported record had `even` mean -5.99 (min -6, max -5) and `odd` mean
  70.50 (min 70, max 72) — every pair had `even < odd`, i.e. even = min.
- Consecutive live records reported `Vmax` and `Vmin` **swapped**, i.e. the
  same code read even = max.

So the pairing phase shifts between records; it is not a stable convention.
Anything that de-interleaves must **detect** which series is which (compare
the two series' means) rather than assume an ordering, and anything that only
needs extrema should take them over the whole record, where the phase cannot
matter. `neowon_dsp::measure_envelope` does both; the naive assumption
produces a *negative* peak-to-peak on the shifted records, which is how this
was noticed.

## From external sources (2026-08-30, not our own measurements)

Read at the user's request: Elmue's Oszi-Waveform-Analyzer (repo + the manual
at netcult.ch/elmue/Oszi-Waveform-Analyzer), the EEVblog VDS1022I teardown
thread, and github.com/Atmel2005/ATMELOWON. Second-hand unless noted; flagged
where it contradicts or confirms something we measured.

**Silicon and front end** (EEVblog teardown): SiLabs SiM3U156 Cortex-M3 plus a
Xilinx Spartan-3 on the early boards; ADC is an **AD9288** (100 MS/s dual, or
the MXT2088 clone), driven through LMH6551 differential amps with AD603 VGAs
for gain switching; video trigger uses a dedicated **BA7046** sync separator.
A single 100 MHz clock feeds ADC and FPGA. **There is no external RAM on the
board** — the 5000-sample record lives in FPGA block RAM, which is the
physical reason the record cannot be deepened.

**Measured analogue performance** (thread, two independent measurements):
CH1 is −3 dB at **34.7 MHz** and CH2 at **31.2 MHz** against a 25 MHz spec,
and the response is still only ~10 dB down **at 100 MHz** with **no
anti-alias filter**. Input rise time ~14 ns; trigger-out lags the edge by
~290 ns. Noise floor ~2–3 mVpp at 5 mV/div. The missing anti-alias filter is
a direct argument for auto peak-detect: at reduced sample rates the front end
passes energy far above Nyquist, so peak detect is the only thing standing
between a narrow event and an invisible one.

**Calibration is stored on the host, not the instrument** — `flash_text/
flashmemory.txt` plus `configuration/com.owon.uppersoft.dso/pref.properties`
in the vendor install; copying both to a new machine restores zero offsets.
The sigrok driver author says the same: these scopes "don't even handle the
calibration internally".

**The device resets itself if the host goes quiet** for more than a few
seconds — independent corroboration of our keep-alive rule. OWON added a 2 s
heartbeat in vendor v1.0.29 specifically to stop USB drops in stop mode.

**FPGA**: vendor software shipped V3.5, then V3.7 from v1.0.28; our vendored
V3.9/V4.1 and the `gaoyun` set are newer. **"gaoyun" is GOWIN (高云)** — so
the V3/V4/V5 boards (including our V5.0.1 unit) carry a Gowin FPGA rather
than the Spartan-3 of the 2016 teardown, which matches the V3/V4/V5
bitstreams all being 221368 bytes against V1/V2's 149554/169216. **There is
no model interlock on upload**: a VDS2052 bitstream loads onto a VDS1022
without complaint and simply plots wrong data, so our version-matched
selection is load-bearing.

**Roll mode, independently confirmed** (thread, 2017): the firmware "just
continuously write[s] on the 5K data buffer in loop **without any indication
of the current cursor position**. This make[s] impossible to do continuous
data recording." That matches what we measured above rate 2.5 kS/s exactly.
The vendor jar contains a dormant `InfiniteGetData` class that looks like a
streaming path, but someone tried its commands and the device did not respond.

**Throughput**: the vendor application manages ~71 waveforms/s; our tight
read loop sustains ~131.

**Equivalent-time sampling is not real.** The vendor app shows 1 ns dot
spacing at 5 ns/div from a 100 MS/s ADC even in single-shot; the thread's
conclusion is that it interpolates to a fixed 5000 points. There is no
hardware path for ETS (triggering is post-ADC in the FPGA off one clock), so
vendor dot spacing is not evidence of a sample rate.

**`.cap` files**: the header's `timeGap` — which our importer also currently
discards — is the vendor's own record of inter-frame dead time. Oszi reads it
and throws it away, then butts frames together and draws a red separator, so
any measurement spanning a boundary is wrong by the unknown gap. Worth
knowing that OWON's format models the gaps and the best third-party tool does
not. Unverified suspicion worth testing on hardware before we write a `.cap`
importer: Oszi derives `.cap` sample spacing from `dataLen` (4000) rather
than the ladder's 250 samples/div, which would stretch its time axis by 25 %;
the `lengthDM = 5000` field alongside suggests 4000 is a screen subset
(1000 px x 4 samples/px, which is also how the vendor renders).

**ATMELOWON** is not firmware and has nothing to do with Atmel/AVR — it is a
prebuilt unsigned Windows binary that bundles a copy of Oszi, with a 166-byte
`main()` as its only Rust source. Its `release/firmware/` bitstreams are
byte-identical to the ones we vendor (sha256 of `VDS1022_FPGAV5_gaoyun.bin`
matches). It carries no protocol, register or calibration information. Of
note only as a parallel effort: it also concluded that a long scrollable
history (it offers a 1–600 s memory window) is what this instrument is
missing.

## Verified 2026-08-30 (correction: reads/s is not records/s)

Earlier in this session the "~131 reads/s in a tight `GET_DATA` loop" figure
was read as headroom to raise the capture duty cycle. That was wrong, and the
correction matters for any host-side deep record.

`rolltest` now counts **distinct** acquisitions as well as successful reads
(payload compared against the previous one). Every read turns out to be
distinct — the device never hands back the same buffer twice — but the rate
at which it will produce them depends on the mode:

| rate | roll | distinct/reads over 3 s | interval |
|---|---|---|---|
| 250 kS/s | off | 109/109 | 27.8 ms |
| 250 kS/s | on | 397/397 | 7.6 ms |
| 2.5 MS/s | off | 323/323 | 9.3 ms |
| 2.5 MS/s | on | 399/399 | 7.5 ms |

So with roll off at 250 kS/s the instrument yields ~36 records/s no matter how
fast it is polled — 36 x 20 ms = **the 71 % coverage is a device limit, not a
polling limit**. The earlier 131/s came from roll being forced on.

What the adaptive backoff *does* buy, measured end to end in the app after
replacing the flat 60 ms wait with `record_duration / 4` (clamped 1–60 ms):

| time base | rate | before | after |
|---|---|---|---|
| 2 ms/div | 250 kS/s | 35.7 fps | 35.7 fps (device-limited) |
| 200 us/div | 2.5 MS/s | ~16 fps | **100 fps** |
| 20 us/div | 25 MS/s | ~16 fps | **125 fps** |

At fast time bases the old flat wait was throwing away 6–8 records out of
every 7 — that is waveform update rate, and it is what a phosphor display
lives on.

**Where gapless capture might still come from:** in roll mode the device
serves fresh content every 7.6 ms while the 5000-sample buffer wraps every
20 ms at 250 kS/s, so consecutive reads overlap by roughly 60 %. There is no
cursor to place them with, but overlapping windows can in principle be
stitched by correlating one read's tail against the next one's head. That is
a real route to a gapless high-rate record, and it is not implemented.
