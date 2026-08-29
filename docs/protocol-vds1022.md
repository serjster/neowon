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

## Still to verify

- FPGA upload handshake on a cold (power-cycled) device.
- `HTP_ERR = 11` horizontal correction (need a fast edge + timebase sweep).
- Trigger word bits for Normal/Single sweep gating (`GET_DATAFINISHED`,
  `GET_TRIGGERED`).
- Slope/Video trigger type codes (Java says Slope=1/Video=2; Python has them
  swapped).
- Roll-mode cursor arithmetic (DM=5120, circular).
- Keep-alive: how quickly the link actually drops when idle (>3 s claimed).
