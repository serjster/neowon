# RTL-SDR — hardware and driver facts

Our own record of what is verified on the attached dongle. Anything learned
on hardware lands here in the same session (AGENTS.md). Decisions live in
`docs/tasks/phase10-sdr-spec.md`; this file holds facts.

## The unit on the desk

| field | value | how known |
|---|---|---|
| USB id | `0bda:2838` | enumeration (ioreg, `rs-rtl`) |
| strings | manufacturer "Realtek", product "RTL2838UHIDIR", serial "00000001" | enumeration |
| tuner | R820T family (I2C 0x34; the R820T2 answers the same id) | `rs-rtl` probe |
| board variant | `Generic` (not Blog V4) | `rs-rtl` string match |

The unit is a genuine RTL-SDR Blog V3 (operator, 2026-09-18), although its
EEPROM carries the stock Realtek strings rather than "RTLSDRBlog"/"Blog V3".
Software therefore cannot tell it from a generic R820T dongle, and no driver
should gate V3 features on the USB strings. HF direct sampling (Q branch) is
verified below; bias-T is untried.

Discrete tuner gains (0.1 dB): 0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166,
197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402, 421, 434, 439,
445, 480, 496 — 29 steps, 0 to 49.6 dB.

## P0.1 driver spike (2026-09-18) — `rs-rtl` 0.5.0 (historical baseline)

Run from the first version of `crates/neowon-sdr/examples/p01.rs` (since
rewritten for the in-tree driver; `rs-rtl` is no longer a dependency).
Self-referential checks: survey 88–108 MHz for the strongest carrier, then
stream, retune live, and step gain.

Readout (first run; a second run matched within 0.02%):

```json
{"driver":"rs-rtl 0.5.0","serial":"00000001","tuner":"R820T","rate_set":2048000,"pairs_per_s":2051457,"dropped_chunks":0,"carrier_hz":99402500,"tune_shift_hz":300000,"tune_moved_hz":-299000,"tune_tol_hz":10000,"gain_delta_db":21.3,"stream_ok":true,"tune_ok":true,"gain_ok":true,"pass":true}
```

- **Open** takes ~1.41 s (baseband init, tuner probe, filter calibration).
  Reopen after a clean exit works; drop puts the tuner in standby.
- **Stream:** 2.048 MS/s requested and reported exactly. Wall-clock
  delivery measured 2.0515 M pairs/s over 2 s. The extra 0.17% is queue
  drain at the start of the window, not a rate error. 0 dropped chunks.
  Samples are u8 offset binary, interleaved I,Q; we map them as
  `(b − 127.5) / 127.5`. DC offset about −0.001 FS on both components, no
  clipping at 29.7 dB on local FM.
- **Tune:** a live retune through the stream's control handle moved the
  carrier by −299.0 / −300.0 kHz for +300 kHz (bin width 500 Hz).
- **Gain:** 0 → 29.7 dB raised total power from −44.8 to −23.6 dBFS (+21.3 dB,
  both runs). The step is less than 29.7 dB because the noise and the
  RTL2832's own stages do not scale with tuner gain.
- Strongest local carrier: FM at ~99.40 MHz, 30 dB over the median bin.

### `rs-rtl` gaps (not needed for SDR-G1)

The public API has no **frequency correction (ppm)**, no **RTL2832 digital
AGC** toggle, no **direct sampling** (the V3's HF path below 24 MHz), and no
offset tuning. Offset tuning does not matter here: librtlsdr refuses it
for R82xx tuners anyway. `RtlSdr` keeps its register-level `Device` private, so none of
these can be added from outside the crate. SDR++'s RTL-SDR source
(`tmp-inspiration/SDRPlusPlus/source_modules/rtl_sdr_source`) drives all of
them through librtlsdr (`rtlsdr_set_freq_correction`,
`rtlsdr_set_direct_sampling` I/Q branch, `rtlsdr_set_agc_mode`,
`rtlsdr_set_offset_tuning`), which is the reference for porting them.

## P0.1 comparison — `librtlsdr-rs` 0.3.0 (2026-09-18)

Checkout: `tmp-inspiration/librtlsdr-rs` (a faithful, audited pure-Rust port
of librtlsdr on `rusb`, i.e. libusb). Run from a scratch crate with a path
dependency; the carrier is the 99.4 MHz FM station found above. Tuned
offsets are power-weighted centroids within ±100 kHz, which are far steadier
on FM than the peak bin.

| check | result |
|---|---|
| open | 362 ms (`rs-rtl`: ~1.41 s) |
| stream, one synchronous bulk read at a time (256 KiB) | 2.048 MS/s → 2 050 031 pairs/s; 2.4 MS/s → 2 402 545 (both +0.1%, the same start-of-window artefact). No sign of FIFO loss on macOS even without transfers in flight |
| tune, live retune +300 kHz | centroid moved −300.22 kHz |
| **ppm** ±100 | centroid +10.33 / −9.47 kHz, i.e. 19.80 kHz apart against 19.88 predicted (2 × 99.4 MHz × 100 ppm). Correction works, and its sign raises the observed frequency for positive ppm. 0 ppm reads +0.83 kHz, which bounds this dongle's own error to about ±8 ppm (FM carriers are accurate) |
| **RTL AGC** (demod reg 0x19: 0x25 on / 0x05 off) | at 0 dB tuner gain, −44.8 → −29.9 dBFS (+14.9 dB) |
| gain 0 → 29.7 dB | −44.8 → −23.5 dBFS (+21.3 dB, same as `rs-rtl`) |
| **HF direct sampling, Q branch** (`set_direct_sampling(2)`) | 1.0 / 6.0 / 7.1 / 9.6 / 11.8 / 15.3 MHz: −45 to −27 dBFS; strongest peak 34.7 dB over the median at 11.8 MHz (the 25 m shortwave broadcast band). The same +89/+90 kHz peak at both 1.0 and 6.0 MHz is suspect (a spur or alias) and unverified |

**Hazard: leaving direct sampling at an HF frequency fails.**
`set_direct_sampling(0)` re-initialises the tuner and then retunes to the
*current* centre frequency. At 15.3 MHz that is below the R820T's range, so
the call errors with "PLL programming failed for 18870000 Hz (no valid VCO
divider)" (15.3 MHz + the 3.57 MHz IF), leaving direct sampling off and the
tuner untuned. `librtlsdr-rs` documents this function as a port of
upstream `rtlsdr_set_direct_sampling`, so the C library likely shares the
ordering (not checked against the C source). The fix is
to retune into the tuner's range first, or leave direct sampling with the
target VHF frequency in hand. The device recovered completely: a following
`rs-rtl` P0.1 run passed.

## In-tree driver `neowon-sdr::rtl` (2026-09-18) — the D2 decision

RTL2832U + R820T/R828D on `nusb`, ported from `librtlsdr-rs` (spec D2).
Command: `cargo run -p neowon-sdr --example p01 -- --json <path>` (hardware
only). Final readout (99.41 MHz, the strongest local carrier):

```json
{"driver":"neowon-sdr::rtl","serial":"00000001","tuner":"R820T","open_ms":355,"pairs_per_s":[[2400000, 2400036], [2048000, 2047999]],"overflows":0,"carrier_hz":99409000,"tune_moved_hz":-299981,"gain_sweep_dbfs":[-44.8, -37.1, -23.4, -6.1],"gain_0_to_297_db":21.4,"ppm_moved_hz":20141,"ppm_expect_hz":20207,"agc_delta_db":12.9,"agc_off_db":0.0,"hf_peak_hz":9740000,"hf_peak_db":30.3,"hf_exit_untuned":true,"stream_ok":true,"tune_ok":true,"gain_ok":true,"ppm_ok":true,"agc_ok":true,"hf_ok":true,"hf_exit_ok":true,"pass":true}
```

Earlier passing runs at 98.30 MHz and 106.60 MHz agree to within the
noted tolerances.

- **Open** 355 ms, the same as `librtlsdr-rs` (the ~1.4 s of `rs-rtl` is not
  inherent to the chip).
- **Stream:** 15 bulk transfers in flight, each sized to ~50 ms of samples
  for the set rate (200 KiB at 2.048 MS/s; the 256 KiB ceiling binds only
  from 2.62 MS/s, the 32 KiB floor below ~328 kS/s) so the
  one-frame-per-transfer display cadence stays ~15–20 rows/s at every rate
  instead of collapsing to ~2 rows/s at 250 kS/s. Timed from chunk
  arrivals, delivery is within ±0.002% of the set rate at 2.048 and 2.4
  MS/s, with 0 overflows. **Control-to-sample latency is one chunk**
  (~50 ms; ~65 ms at 250 kS/s when the floor binds): the first chunk after
  a change straddles it, the second is clean. Consumers measuring a setting
  must drop that chunk. At 29.7 dB its old samples outweigh a 0 dB window
  by ~100:1, which is how a first version of the check read the gain step
  as 11 dB.
- **ppm acts on the LO, which sits at centre + IF.** The IF depends on the
  bandwidth: 1.625 MHz at 2.048 MS/s, 1.815 MHz at 2.4 MS/s (the tuner's
  filter arithmetic). So ±100 ppm moves the band by 2·(f + IF)·1e-4:
  measured 20.14 kHz against 20.21 kHz, and 20.00 against 19.99 at 98.3 MHz.
  Positive ppm raises the band (the tuner takes its crystal as fast and
  programs the LO low). Traced: +100 ppm changes the PLL `sdm` word by −364,
  i.e. −10.0 kHz of LO at 99.9 MHz.
- **Gain** is instant in both directions and monotonic over the table:
  0 / 14.4 / 29.7 / 49.6 dB → −44.8 / −37.1 / −23.4 / −6.1 dBFS at 99.4 MHz,
  with no clipping at 49.6 dB. The size of a step depends on the scene: at
  98.3 MHz, 0 → 29.7 dB is +14.3 dB, and `librtlsdr-rs` measures the same
  (+14.5 dB) there. So only monotonicity is a driver property.
- **RTL AGC** (demod 0x19 = 0x25) adds ~13–15 dB at 0 dB tuner gain and
  releases exactly (0.0 dB residual) when turned off.
- **HF direct sampling** (Q branch) works through the in-tree path: the
  strongest spot is 9.74 MHz, in the 31 m shortwave broadcast band, at
  30–49 dB over the median across runs. The recurring +90 kHz peak at
  6.0 MHz is still unexplained.
- **HF exit is fixed:** leaving direct sampling at 15.3 MHz succeeds and
  leaves the device untuned (`center_freq() == 0`); the tuner's bandwidth
  and gain are restored. The next tune puts the carrier back (20–30 dB).
  librtlsdr's behaviour (retune and fail in the PLL) is documented above.

## The `Backend` through the `Supervisor` (2026-09-18)

`cargo run -p neowon-sdr --example backend` (hardware only) drives
`RtlBackend` exactly as the app does. At 2.048 MS/s and 29.7 dB, frames are
128 Ki pairs (64 ms), and timestamps advance by exactly each frame's
duration (no chunk dropped; supervisor drops 0). The 99.4 MHz station peaks
~29 dB over the floor. A +200 kHz retune moves it −196.5 kHz; the FM
peak bin wanders ±5 kHz with modulation. Tuning to 9.74 MHz switches to
direct sampling automatically (a peak 24–32 dB over the floor at
9.64 MHz), and tuning back to 99.4 MHz restores the tuner path.

## The app on the dongle (2026-09-18)

`NEOWON_CONTROL=<port> cargo run -p neowon-app -- --rtl` (hardware; run by
hand). `sdr tune 99.4M`, `sdr gain 29.7`, `sdr span 1M` show the 99.4 MHz
station as a ~200 kHz FM channel peaking at about −40 dBFS over a −72 dBFS
floor, with narrow carriers either side. `sdr tune 9.7M` crosses into
direct sampling on its own: the 31 m shortwave carriers appear, the
strongest at 9.600 MHz, −43 dBFS. At these levels the samples span only a
few of the 8-bit codes, so the IQ constellation auto-scales to the peak
(×8 here), where the FM ring and the u8 quantisation lattice are both
visible.

## The zero-IF DC spike at the tuned centre (2026-09-21)

The R820T/RTL2832U pair leaves a DC offset spike at the hardware centre
(LO leakage and IQ imbalance). It is a receiver artefact, not a signal and
not a measurement: the SDR display notches `DC_GUARD` bins either side of
the spectrum's DC bin — the same guard the signal list and the peak readout
use — and both the trace and the waterfall draw the one masked copy
(`crates/neowon-app/src/sdr/display.rs`). The demodulator, detector,
recorder, DAB receiver and the `neowon-dsp` oracle always see the raw
IQ/spectrum.

## Detection on the dongle (2026-09-18)

At 99 MHz centre, 2.048 MS/s, 29.7 dB, the in-app detector (Phase 10.1)
reports:
- the 99.4 MHz FM station: ~112–120 kHz occupied (99%), −25 dBFS
  integrated, 25 dB SNR;
- six narrow carriers (e.g. 98.302 MHz at −42 dBFS, 99.070 MHz at
  −54 dBFS), 0.5–1.5 kHz wide, 13–28 dB SNR.

A 31-bin rolling-median floor missed the station entirely: its window
sat inside the 400-bin channel. The detector's floor is now the lower
quartile of a quarter-band window.

## The DSP classifier on the air (2026-09-19)

With `sdr analyse on` on the dongle (Phase 10.5, C1):
- **Broadcast FM (99.4, 106.6 MHz) → `unknown`** (confidence 0.2–0.44).
  Its C42 wanders between −0.25 and +3.8, nowhere near a constellation.
  The detector reports the station as ~55 kHz fragments rather than its
  ~200 kHz channel, so the classifier's channel clips the ±75 kHz FM
  (envelope CV 0.3–0.7, not flat), and the stereo pilot and RDS put lines
  in the symbol-rate band. An earlier version, whose constellation weights
  were only relative, called this **64QAM at confidence 0.95**. Scoring
  digital by the best constellation's *absolute* fit, with a competing
  "none of these" score, made it `unknown`. This is the D9 domain-shift
  risk made concrete: nothing here is `validated`.
- **The weak 98.302 MHz carrier (7–12 dB in band) → `am`** at 0.63–0.83.
  At that SNR, noise alone gives a CW an envelope CV near 0.3, so CW and
  AM are not separable there. Ground truth is unknown (it may be a
  receiver spur).
Open: FM wants the detector to hold a broadcast channel together, or the
classifier to widen its channel for wideband candidates.
