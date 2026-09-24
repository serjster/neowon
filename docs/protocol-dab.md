# DAB (Eureka-147) — protocol and parameters

Our own record of what we implement and what we verified, in the same spirit as
`docs/protocol-vds1022.md` and `docs/protocol-rtlsdr.md`. Created 2026-09-20 with
`docs/tasks/phase10-dab-spec.md` (phase 10.15, tier 1).

**The authority is the standard, not a reference implementation.** Everything
below was read out of **ETSI EN 300 401 V2.1.1 (2017-01)** (downloaded from ETSI;
text extraction used for the tables), except where a hardware note says
otherwise. Where a widely-used reference implementation cites *different* clause
numbers, that is noted: the common `dabradio`/`dab-cmdline` lineage cites the
2001-era edition, whose tables are numbered differently.

## Clause map (V2.1.1)

| Subject | Clause / table |
|---|---|
| Transmission modes, mode I parameters | clause 14.2, **table 22** |
| Transmission frame, FIB/FIC/CIF structure | clauses 5.1, 5.2, 12.3, 13 |
| FIB = 30 data bytes + 16-bit CRC, MSb first | clause 5.2.1, figure 6 |
| FIB CRC: `X^16 + X^12 + X^5 + 1`, init all ones, complemented | clause 5.2.1, annex E |
| FIG type 0 flags (`C/N \| OE \| P/D \| extension`) | clause 5.2.2.1 |
| FIG type 1 data field (charset, identifier, 16-byte character field, 16-bit character flag) | clause 5.2.2.2 |
| FIG type 2 / extended labels (UTF-8 or UCS-2) | clause 5.2.2.3 — not implemented |
| Which FIG/extension carries which label | **table 4** |
| FIG 0/0 ensemble information (`EId`) | clause 6.4.1 |
| FIG 0/1 sub-channel organization (short = UEP index, long = explicit size + EEP) | clause 6.2.1 |
| FIG 0/2 service organization and component description | clause 6.3.1 |
| FIG 1/0 ensemble label | clause 8.1.13 |
| FIG 1/1 programme service label | clause 8.1.14.1 |
| `TMId` / `ASCTy` assignments (ASCTy 0 = MPEG-1 Layer II, 63 = DAB+) | clause 8.1.14, table 33 |
| Energy dispersal PRBS `P(X) = X^9 + X^5 + 1`, init all ones | clause 10.1, **table 12** |
| Energy dispersal in the FIC: per-768-bit group, PRBS restarts at index 0 | clause 10.2 |
| Convolutional mother code: constraint length 7, rate 1/4, generators 133/171/145/133 (octal), six tail bits | clause 11.1.1 |
| Puncturing procedure: 128-bit blocks = four 32-bit sub-blocks, one vector each | clause 11.1.2 |
| Puncturing vectors (24 of them) | **table 13** |
| Tail vector `V_T = 1100 1100 1100 1100 1100 1100` (12 of 24 bits kept) | clause 11.1.2 |
| FIC coding in mode I: 768 bits → 3096 mother bits → 2304 transmitted | clause 11.2.1 |
| Null symbol, phase reference symbol (PRS), time reference | clauses 14.3.1–14.3.3 |
| Frequency interleaving worked example | **table 25** |
| PRS phase values (`phi_k = pi/2 * (h[i, k-k'] + n)`), mode I ranges | clause 14.3.2, **tables 23 and 24** |
| FIC block partitioning, mode I (4 codewords → 3 symbols) | clause 14.4.1.1 |
| QPSK symbol mapper (first `K` bits = I, next `K` = Q) | clause 14.5 |
| Frequency interleaving (`Pi(i) = 13*Pi(i-1) + 511 mod 2048`) | clause 14.6.1 |
| Differential modulation: symbol to symbol on each carrier | clause 14.7 |
| UEP sub-channel sizes by index (table 8) | clause 11.3.1 — transcribed in `dab/fec/uep_table.rs` |
| Character set mapping (complete EBU Latin) | table 47 — transcribed in `dab/charset.rs` |

## Mode I parameters

`T = 1/2 048 000 s`, from table 22 (all values are the standard's):

| Parameter | Value |
|---|---|
| Sample rate | 2 048 000 samples/s |
| `Tu` (useful symbol) | 2 048 samples (1 ms) |
| Guard interval `Delta` | 504 samples (246 µs) |
| `Ts` (total symbol) | 2 552 samples |
| `TNULL` (null symbol) | 2 656 samples |
| `L` (OFDM symbols per frame) | 76: one PRS + 75 data |
| `K` (active carriers) | 1 536 |
| Carrier spacing | 1 kHz |
| Frame `TF` | 196 608 samples = 96 ms |
| FIC symbols per frame | 3 (symbols 2, 3, 4 — after the null symbol and the PRS) |
| FIC capacity | 12 FIBs per frame (4 CIFs × 3 FIBs) |
| CIF | 86 400 bits (864 capacity units, 24 ms); 1 CU = 64 bits |

Active carriers occupy 1.536 MHz of the 2.048 MHz window. In the natural DFT
ordering a carrier `k` sits in bin `k` for `k > 0` and bin `2048 + k` for `k < 0`
(clause 14.2/14.4), so the band is DC-centred and 1536 carriers wide.

## The FIC decode chain, in the order we implement it

1. **Soft bits** — 3 symbols × 1536 carriers × 2 bits = 9216 per frame, in
   symbol order (clause 14.4.1.1). Carriers are de-interleaved with the
   clause-14.6.1 permutation and demapped differentially against the previous
   symbol (clause 14.7), the first data symbol against the PRS (clause 14.3.2).
2. **Four codewords of 2304 bits** — the 9216 bits are four consecutive
   2304-bit groups, one per CIF.
3. **Depuncture to 3096** — 21 blocks of 128 bits with `PI = 16`, 3 with
   `PI = 15`, then the 24-bit tail with `V_T` (clause 11.2.1). Punctured
   positions become erasures.
4. **Viterbi** — 64 states, rate 1/4, tail-terminated: 774 steps → 768 data
   bits + 6 tail bits (clause 11.1.1).
5. **Energy dispersal** — the 768 bits are scrambled with the PRBS starting at
   index 0 (clause 10.2); it is applied at the transmitter *before* coding, so
   the receiver undoes it *after* decoding.
6. **Three FIBs of 256 bits** — 30 data bytes + CRC, MSb first; only CRC-clean
   FIBs are parsed (clause 5.2.1).
7. **FIGs** — walked by type and length, unknowns skipped by length
   (clause 5.2.2.x, table 4).
8. **Ensemble table** — EId, ensemble label, sub-channels, services and their
   labels; published only when the CRC rate over a window of frames says the FIC
   is real (decision D27 in the spec).

### When the table expires (the D27 boundary)

The table is a claim about the *current* signal, so it must be discarded when
the signal is gone. Three boundaries, ordered shortest to longest:

- **A splice keeps it.** `DabReceiver::discard_buffer` (a gap or overlap in the
  frame stream, up to ~4 transmission frames ≈ 384 ms) drops the samples and
  the MSC chain but keeps the FIC window and the table: the receiver re-locks
  as the window slides, and the labels that took ~12 s to collect are not
  thrown away for a USB drop. This is capture-proven behaviour.
- **Seconds of undecodable input expire it.** The receiver counts consecutive
  attempts that accept no FIC — open-loop searches, noise on the predicted
  grid, fades — and after `TABLE_EXPIRY_FRAMES` = 48 (≈ 4.6 s at Mode I)
  empties the FIC window and the raw table. On-air acceptance is about one
  attempt in three, so a fading ensemble cannot reach 48; a retune onto noise
  reaches it in a few seconds even if nothing else notices.
- **No input at all expires it at the owner.** With no samples there are no
  attempts to count, so the app reports the stop (`DabReceiver::no_input`)
  after `NO_INPUT_TIMEOUT_S` = 2 s without an IQ frame — a stopped backend, a
  disconnect, a stalled link. Retunes, rate changes, instrument switches and
  `sdr dab reset` call the full `reset()` at once, because the new signal is
  known to be a different one.

A selection (`sdr dab service`) is part of the same claim: when the table goes,
the app drops the selection, its PAD/DLS parsers and the playback transport in
the same step.

Soft-bit convention in our decoder: positive = likely 1, negative = likely 0.
The Viterbi maximizes correlation, so a sign error is a systematic failure, and
there is a test whose only job is to fail if the convention inverts.

## The OFDM front end, as implemented

`dab::ofdm` + `dab::receiver`. The order of operations per frame:

1. **Null symbol.** The transmitter is silent for `T_NULL` = 2656 samples
   (clause 14.3.1), so the frame start is the deepest sustained power dip. Found
   on an 8-sample grid using a prefix sum of `|x|²`, not a per-offset scan.
2. **PRS gate.** The phase reference symbol must follow the null. The
   normalized correlation between the received spectrum and the known 1536-carrier
   reference scores ≈ 1 for DAB and ≈ `1/sqrt(1536)` ≈ 0.026 for noise;
   `PRS_METRIC_MIN` = 0.35 sits between them. A rejected frame decodes *nothing* —
   this is what stops the receiver naming an ensemble that is not on the air.
3. **Carrier offset.** The cyclic prefix of the PRS measures the offset from the
   phase between the guard and its copy one useful period later:
   `df = -arg(sum guard·conj(copy))·fs/(2·pi·Tu)`. Unambiguous range ±500 Hz at
   mode I. The measured values in the test suite land within 5 Hz of the injected
   offsets, sign included. **A frame is de-rotated on one continuous time base**:
   restarting the phasor per symbol would re-introduce the very rotation being
   removed (the rotation is `2·pi·df·Ts` per symbol, which is 0.19 rad at 150 Hz —
   enough to break DQPSK on its own).
4. **Demap three symbols** differentially against the PRS and each other, in
   interleaved symbol order (first `K` soft bits in-phase, next `K` quadrature),
   with a silent carrier producing an erasure rather than a decision.

Two impairments cancel by construction and need no estimation: a
carrier-independent channel phase, and a fixed sub-guard timing offset (the same
relative window is taken for every symbol, so its phase ramp appears in both
factors of the differential product).

Known limitations of this front end, recorded for the hardware run:

- **No integer-carrier coarse search.** An offset of ≥ 1 carrier spacing
  (1 kHz) lands the ensemble in the wrong bins. The operators are the dongle's
  ppm correction and the operator tuning a block from the band map; if carriers
  come out one bin off, this is the gap, not the FEC.
- **Timing is not refined inside the guard.** Unnecessary for the FIC (see above),
  but the MSC's time de-interleaving may want a real symbol boundary.
- **Sync costs one frame of lookahead**: with `n` frames of samples pushed, `n-1`
  can be decoded.

## Test vectors we hold the code to

- **table 12**: the first 16 PRBS bits, `0000011110111110` — pinned as
  `tables::PRBS_FIRST_16` and asserted against our PRBS generator.
- **table 13**: all 24 puncturing vectors, machine-transcribed; the generator
  asserts each `PI` keeps exactly `8 + PI` of 32 bits.
- **tables 23/24**: the PRS ranges and h values, machine-transcribed; the
  generator asserts the ranges tile `[-768, -1]` and `[1, 768]` exactly once.
- **clause 11.1.1**: the four parity equations, checked against a hand-computed
  codeword in `fec::tests::mother_code_matches_the_clause`.
- **`V_T`**: 24 bits, 12 kept — the arithmetic only closes to 2304 transmitted
  bits if this is right (21×96 + 3×92 + 12 = 2304).
- **table 25**: the standard's worked example of the frequency interleaver
  (`F(0..10) = -513, -14, 329, 692, -733, 13, 680, 273, -36, 43, 85`), which pins
  both the permutation and the direction of the `n → k` mapping. Note the
  standard's own row `i = 13` prints `1076` where the recursion gives `1067`
  (`13·988 + 511 mod 2048`); the `d_n = 1067` beside it is the consistent value
  and the one we implement, matching the published `k = 43`.

The tables are generated, not typed: a one-off script reads the standard's text
extraction and emits `crates/neowon-dsp/src/dab/tables.rs`, asserting the
invariants above before emitting. Table values were cross-checked against the
MIT reference `dabradio` 0.5.0 and agree value-for-value.

## Verified on hardware

**DAB-G1 passed, 2026-09-20.** RTL-SDR V3 (R820T), 2.048 MS/s, gain auto, ppm 0,
hardware centre 220.352 MHz (block 11C), app driven over the control socket.

| Fact | Value |
|---|---|
| Lock | `locked: true` on the first ensemble tried |
| EId | `0x8008` |
| Ensemble label | `"DAB+"` — the multiplex's own label as transmitted, apparently a placeholder |
| Services | **14**, all `ASCTy 63` (DAB+ / HE-AAC v2), each with a sub-channel |
| Service labels | SLAM!, YOURSAFE, BNR BusinessBeat, Sky Radio, 538, Qmusic, BNR Nieuwsradio, Radio 10, 100% NL, Veronica, 538 NONSTOP, Qmusic Non-stop, JOE, Sky Radio Hits |
| Sub-channels | 14, `EEP 3-A` except one `EEP 2-A`, 128–256 kbit/s |
| FIB CRC | 1306/1320 = **98.9%** (110 frames, first run); 453/456 = **99.3%** (38 frames, after the feed fix) |
| Carrier offset | **−12 to −15 Hz** (≈ −0.06 ppm) with a contiguous feed |
| Labels acquired | all 14 within ~12 s of switching on |
| Data services | 0 on this ensemble |

The labels are independent confirmation that the FIG parsers work: they are the
stations the external listing attributes to this ensemble (100% NL, BNR, Qmusic,
Radio 10, 538, Veronica, Sky, SLAM!), including two the listing does not have
(JOE, YOURSAFE) and one it names differently (54 → "538 NONSTOP").

### Two defects the session found — both were ours, both fixed

1. **The receiver was fed from the display path, which is latest-wins.** The
   spectrum view only needs the newest frame to paint, so it consumes one frame
   per render tick and overwrites the rest; a decoder fed from there sees a
   stream full of holes. Evidence: with a holed feed the carrier estimate was
   **−223 Hz** and labels took 40 s; with every frame fed from `ingest` the
   estimate settled at **−12 Hz** and labels arrived in ~12 s. The estimate was
   the giveaway — a cyclic-prefix measurement straddling a splice is simply
   wrong, and nothing about it looks wrong in isolation. `feed_dab` now runs in
   `ingest`, where every frame arrives.
2. **`prs_metric` reported the last *attempt*, not the accepted frame.** On air
   it read ~0.03 while the receiver was decoding 98.9% of its FIBs — a readout
   that makes a working receiver look broken, and one an operator would use to
   stop believing the screen. The accepted frames' score is now reported
   separately (`get dab` carries `prs_metric`, `last_attempt_metric`,
   `frames_decoded`, `frames_rejected`).

### The tier-2 failure, root-caused offline (2026-09-21)

The symptom: on 11C the FIC was clean (EId `0x8008`, 14 services, 100% FIB
CRC) but `neowon_codec::dabplus` never saw a Fire-clean super frame — "not the
signalled 11 × 8 kbit/s shape". The MSC byte rate was right, so extraction,
FEC and the CU mapping looked innocent; the first suspect was a shared
encoder/decoder misreading of a table. **It was not.** Every suspect table was
re-checked against the standard and is correct: the MSC PRBS restarts per
sub-channel logical frame (clause 10.3), the puncturing vectors are table 13
with `PI n` keeping 8+n of 32 bits (clause 11.1.2), the EEP profiles are tables
18/20 (`3-A` 88 kbit/s = `L1 63, L2 3, PI 8/7`, 66 CUs), and the clause-12
delay map is table 21. The archived capture then settled it: a receiver that
re-derives its frame start from the null-symbol power dip every frame — no
predicted grid, no soft-miss continuation — produces **0 Fire-clean super
frames and 0 AU CRCs on the capture while the FIC still reports 180/180 CRCs
and EId `0x8008`**. The mechanism: on air most frames score below the PRS
acceptance gate (15 of 147 attempts in that run), and the reject path
discarded the MSC de-interleaver, so a sub-channel never held the 16 logical
frames of clause-12 continuity. The FIC needs no inter-frame memory, which is
why it stayed clean while the MSC was starved. The fix (already in the working
tree, pinned this session): predict the next frame start from the accepted
grid, refine it to the PRS peak, treat a weak frame at the predicted start as
a fade (keep demapping, keep the delay line), and give the super-frame search
a 20-super-frame shape budget that a stream which ever aligned can never trip.

**Re-measured 2026-09-22 at rev `4579cfb3d9d5beca49b4591d13ed486b4c645fdb`.**
Capture: `tmp-inspiration/dab-11c.f32`, 30 720 000 complex samples (15.00 s),
2.048 MS/s, Band III 11C, recorded 2026-09-21 (file mtime), SHA-256
`93293363ba09a35e4737fb72d0e7bd3e20dcd233ef9c4f70642cc37421d462a8`. Run from
the repo root (`$PWD` expands before cargo runs the test binary, whose CWD is
`crates/neowon-app`):

```bash
NEOWON_IQ_CAPTURE=$PWD/tmp-inspiration/dab-11c.f32 \
  cargo test -p neowon-app --release --test dab_air_capture -- --ignored --nocapture
```

**525 Fire-clean super frames, 1280 AUs, 1100 AU CRCs ok across all 14 services**
(indexes 8–12, dac 32/48 kHz, SBR true, one PS mono service — all plausible),
62 frames accepted against 119 rejected, FIB CRC 444/444. The same capture
through the app's real transport:

```bash
NEOWON_IQ_CAPTURE=$PWD/tmp-inspiration/dab-11c.f32 \
  cargo test -p neowon-app --release --bin neowon-app -- --ignored air_capture --nocapture
```

**1110 AU CRCs ok, 0 shape mismatches**, 200 MSC logical frames on every
sub-channel.

The counterfactual is a switch, not prose: `NEOWON_DAB_NO_PREDICTION=1`
(`crates/neowon-dsp/src/dab/receiver.rs`; unset on the production path)
disables only the frame-grid prediction. Either command with it set reproduces
the root cause: **0 Fire-clean super frames, 0 AU CRCs, 0 shape mismatches —
while the FIC still locks with 180/180 clean FIBs and EId `0x8008`** (15 frames
decoded against 132 rejected). Both harnesses accept that mode as their
expected result, so the claim above is a command, not a sentence. The earlier
record's **346 super frames / 843 AUs / 754 AU CRCs and 707 AU CRCs** came
from an older working tree of the 2026-09-21 session, before the fix settled;
they do not regenerate at this revision and are superseded by the numbers
above.

The capture is **operator-local scratch**: `tmp-inspiration/` is gitignored
(`.gitignore`) and the ~234 MB file is not committed, so these numbers are
regenerable only with it — the SHA-256 above is the check that it is the same
file. The harnesses panic (clear message, non-zero exit) when
`NEOWON_IQ_CAPTURE` is unset or the file is absent, and assert capture-derived
floors (400 super frames / 1000 AUs / 800 AU CRCs; 800 AU CRCs in the transport
harness), so a collapse cannot pass as a measurement.

Two bounds defects in the same search path were found and fixed while
reproducing this: the refinement could move the start past the buffered frame
(index out of bounds), and its scan guard omitted `T_G`. Regression tests:
`multipath_echo_keeps_the_clause_12_chain_contiguous` and
`frame_sized_feeds_never_overrun_the_sample_buffer` in
`crates/neowon-dsp/tests/dab_msc.rs`; the shape-verdict policy tests live in
`crates/neowon-app/src/sdr/dab_audio/transport.rs`.

### Open items from the session

- **Sync yield is ~13% of attempts.** The receiver attempts a frame per six
  16 ms input frames and accepts about one in eight: 38 accepted against 262
  rejected in the measured run. The rejected attempts' PRS score is ~0.04, i.e.
  they are genuinely misaligned, not marginal. **Likely cause: splices from USB
  drops**, because the null symbol then stops being the unique power dip and the
  sync wanders. It still locks in seconds and holds, so this is a yield problem,
  not a correctness one — but it is the first thing to fix. *(Update 2026-09-21:
  with the frame-grid prediction the archived capture's yield rose to 56 of 168
  attempts, and its residual rejections are what the `next_frame` grid is
  tolerant of. Splice detection is still the fix for the dropped-sample case.)*
- **Splice detection needs a real sequence indicator — CLOSED 2026-09-23.**
  `CaptureFrame::t_start` is derived from *arrival* time ("biased late by up to
  one poll"), so it cannot distinguish a jitter of one poll from a dropped
  chunk. The check used to be a coarse safety net (four frame durations); a
  tight bound fired on ordinary jitter during this session and, before it was
  loosened, cost ~87% of attempts. The fix was the one named here — a
  dropped-sample counter from the backend — and it landed: `CaptureFrame`
  carries `dropped_before()`, filled by `RtlBackend::poll_frame` from
  `Stream::overflows()` and carried forward by the supervisor when it cannot
  hand a frame over; `sdr::dab::feed` splices on `dropped > 0`, and the
  timestamp check survives only as the net for what a counter cannot see (a
  producer that does not count, a restarted stream, an instrument swap).
  Reported by `get sdr` as `dropped_pairs` / `drop_events` and shown in the SDR
  dock's Receiver section. See `docs/tasks/phase10-dab-spec.md` deviation 19.
- **The ppm conclusion from earlier in this session is void.** The −223 Hz
  reading that suggested ≈ −1 ppm was an artifact of the holed feed, and the
  attempt to "correct" it produced two readings at the same setting that
  disagreed by 435 Hz. With a contiguous feed the dongle measures ≈ −12 Hz
  (≈ −0.06 ppm), i.e. it is well calibrated as it stands. Whether the R820T also
  quantizes small ppm nudges is still open and no longer urgent.
- **The ensemble label `"DAB+"`** is recorded as received. It is what the
  multiplex declares in FIG 1/0, but it reads like an operator placeholder rather
  than an ensemble name, so treat it as a fact about this ensemble, not a
  property of the label parser.


## Band III targets for the first hardware run

**External data, not verified by us.** Listed so the run has a destination; each
line becomes a verified fact only when it appears in the section above.

For the Eindhoven area, two national ensembles are the obvious targets, because
tier 1 needs only a well-filled FIC and these carry 12–15 named services:

| Block | Centre | Ensemble (external) | Services (external) |
|---|---|---|---|
| **11C** | **220.352 MHz** | Dutch commercial national, from Gemert (De Mortel) | 100% NL, BNR, Groot Nieuws, Q Music, Qmusic non-stop, Radio 10 (+60s&70s), 538 (+Top 40), Radio Maria, Veronica, Sky Radio (+Hits), Slam FM, SubLime |
| **12C** | **227.360 MHz** | NPO public national, from Tilburg (Loon op Zand) | Radio 1, Radio 2, 3FM, Radio 4 (+Concerten), Radio 5, FunX (+Dance, Slow Jamz), NPO Soul & Jazz, NPO SterrenNL, 3FM Alternative / KX |
| 8A | 195.936 MHz | regional Brabant/Limburg, from Eindhoven (Daalakkersweg) and Gemert | Omroep Brabant, Radio 8FM, RadioNL, Radio Continu, Q Music |
| 7A | 188.928 MHz | regional, from Eindhoven and Tilburg | Radio JND, Mexico FM |

Sources: `frequentie.fm/eindhoven` for the service lists, cross-checked against
the Band III block table at `wiki.opendigitalradio.org/Band_3_Channels`.

**Do not trust the block letters in that station listing.** Its frequencies match
the standard raster, but two labels are shifted by a block group: it calls
195.936 "7A" (it is 8A) and 188.928 "9C" (it is 7A). The raster is **a table,
not a formula**: the nominal 1.712 MHz step breaks at group boundaries
(1.872 MHz before 6A–10A, 1.856 MHz before 11A/12A, 1.712 MHz for 12D→13A and
1.568 MHz for 13C→13D), so a generated raster would be wrong. The full 38-entry
5A–13F table is transcribed in `neowon-refdb::dab` from `dabradio` 0.5.0 (MIT;
notice below); the four centres this project has recorded — 7A 188.928,
8A 195.936, 11C 220.352, 12C 227.360 — match it value-for-value. Tune by label
or by frequency; the letters are administrative (the "Wiesbaden" arrangement),
not from EN 300 401.

Two consequences for the run:

- **Both national ensembles are DAB+** (`ASCTy` 63), which tier 1 handles
  *without* any codec: the FIC carries the names regardless of what the audio
  sub-channels contain. This is why DAB-G1 does not wait on the DAB-G2 decision.
- **Set `sdr ppm` before concluding anything about the front end.** At 220 MHz a
  20 ppm dongle error is 4.4 kHz, i.e. four carrier spacings — outside the
  ±500 Hz the cyclic prefix measures, and squarely in the "no integer-carrier
  coarse search" limitation. If `prs_metric` stays near noise, stepping the
  centre by whole kHz should recover it, and that observation is itself the
  evidence for whether the coarse search has to be built.


## Open items

- **FIG type 2** extended labels (UTF-8/UCS-2, segmented) are not parsed.
- **Data services** (`P/D = 1`, 32-bit SId) are counted, not tabled.
- **DAB+ audio** (tier 3) landed: `neowon-codec` carries the DAB+ transport and
  the codec adapters (clause map and the HE-AAC v2 decoder situation below),
  and the app plays a DAB+ and an MP2 programme from the `rf-dab` scene
  (`crates/neowon-app/src/sdr/dab_scene.rs`). The on-air listening run (spec
  row 17) is still open, and real 960-transform SBR+PS access units are
  exercised only there.

## Licensing / provenance (decision D26)

Porting reference: **`dabradio` 0.5.0** (`xoolive/desperado`), **MIT**,
8 393 Rust LOC, published as a binary (`has_lib: false`), so it is read and
ported from, not depended on — copyright © Xavier Olive and contributors, MIT
licence. Tier 1 ports from it, and tier 2 (MSC, DLS) ports under the same
notice: the UEP profile table (`dab/fec/uep_table.rs`, from `src/fec/uep.rs`),
the EEP profile formulas (`dab/fec/eep.rs`), the clause-12 time-interleaving
order and delay map (`dab/msc.rs`, from `src/msc/mod.rs`), the DLS segment
structure and reassembly rules (`dab/pad.rs`, from `src/pad/mod.rs`), the
complete EBU Latin repertoire (`dab/charset.rs`, from `src/charsets.rs`), and
the Band III channel catalogue (`crates/neowon-refdb/src/dab.rs`, from
`src/constants.rs`) that the DAB dock's channel selector and `sdr dab channel`
tune by. Every ported file carries the comment
`// Ported from dabradio 0.5.0 (MIT); notice in docs/protocol-dab.md`, and the
tables were re-checked value-for-value against EN 300 401 V2.1.1 (tables 8,
13, 15, 18, 20); the Band III table has no EN 300 401 source (it is the
Wiesbaden arrangement) and was checked against the four centres recorded
above.

Read-only reference (GPL, **never copied into this workspace**):
`JvanKatwijk/dab-cmdline`, `JvanKatwijk/qt-dab`, `welle.io` — including the
copy vendored under `tmp-inspiration/SDRPlusPlus/decoder_modules/dab_decoder/`
— were read for structure only; no code or table was copied from them.
Standard-derived constants (mode parameters, tables 12/13/23/24, CRC
polynomial, tail vector) are cited to the clause that defines them, in the
code that uses them.

Tier-1 clause-map rows now closed: **table 8** (UEP sizes and profiles) is
transcribed as one 64-entry table in `dab/fec/uep_table.rs` (clause 11.3.1);
**tables 18/20** (EEP profiles) and **9/10** (sizes) are formulas in
`dab/fec/eep.rs` (clause 11.3.2); the clause-12 time interleaver
(`r' = r − D(ir mod 16)`, table 21) is implemented in `dab/msc.rs`; **table 47**
(complete EBU Latin) is transcribed in `dab/charset.rs`. MSC energy dispersal
is clause 10.3; the standard defines no MSC payload CRC (only the FIB CRC and
the X-PAD/DLS CRC), so tier 2 checks a CRC only in oracle mode.

## DAB+ audio transport and codec adapters (Phase 10.15.3)

Standards used: **ETSI TS 102 563 V2.1.1** (DAB+ audio) and **ETSI EN 300 401
V2.1.1** (DAB system), both downloaded free from ETSI on 2026-09-20. Crate:
`crates/neowon-codec` (engine-free; `oxideav-aac` 0.1.7 and `oxideav-mp2`
0.0.10 only).

### TS 102 563 clause map used by neowon-codec

| Constant / rule | Clause |
|---|---|
| super frame = 120 ms; size = `subchannel_index × 110` bytes; carried in five consecutive DAB logical frames; index 1..=24 | 5.1 |
| header syntax: Fire code (16), rfa/dac_rate/sbr_flag/aac_channel_mode/ps_flag/mpeg_surround_config (8), `au_start[1..]` (12 each), alignment (4) | 5.2 Table 2 |
| `num_aus` from (dac_rate, sbr_flag) = 2 / 3 / 4 / 6 | 5.2 Table 2 |
| dac_rate = 32/48 kHz; SBR halves the core rate | 5.2 Tables 3, 4 |
| channel mode / PS / MPEG Surround parameter meanings | 5.2 Tables 5, 6, 7 |
| `au_start[0]` = 5/6/8/11; `au_start[n] = au_start[n-1] + au_size[n-1] + 2`; terminal offset = super frame size | 5.2 Table 8 |
| per-AU CRC: `G(x)=x^16+x^12+x^5+1`, init all ones, complemented; procedure per EN 300 401 annex E | 5.2 |
| header Fire code: `G(x)=(x^11+1)(x^5+x^3+x^2+x+1)` = 0x1782F (mask 0x782F), init all zeros, over super frame bytes 2..10 | 5.2 |
| PAD in a leading `data_stream_element()`; F-PAD (2 bytes) + X-PAD Ind (none / short 4 / variable); PAD in `au[n]` belongs to `au[n+1]`; invalid length ⇒ no PAD | 5.4.0–5.4.3 |
| RS(120,110,t=5): GF(2^8), α=2, `P(x)=x^8+x^4+x^3+x^2+1` (0x11D), `G(x)=∏_{i=0}^{9}(x+α^i)`, shortened from RS(255,245) by 135 leading zeros | 6.0, 6.1 |
| virtual interleaver: `C[i][j] = A[i + j·s]`; parity per row; output = 110·s data then 10·s parity in s-byte columns | 6.2–6.5 |
| audio parameters signalled in the super frame header; the ASC is derived from them (not transmitted, not in PAD) | 7.2 |
| receiver-side error concealment / super frame sync / processing (informative) | annexes A, C, D |

Additional (EN 300 401 V2.1.1): F-PAD is "contained in the last two bytes of the
DAB audio frame" (clause 3.1) — the MP2 ancillary tail — and the F-PAD/X-PAD
structure is clause 7.4 (shared by both audio codings; the Layer II coding
itself is ETSI TS 103 466). CRC-16 procedure: annex E.

TS 102 563 publishes no numeric Fire-code, AU-CRC or RS vectors, so the tests
assert the defining algebraic properties (polynomial expansion, linearity,
single-bit-flip detection, generator-root closure, 5-error and 10-erasure
correction) and the published CRC-16 check value `0xD64E` for `"123456789"`.

### The 960 transform / HE-AAC v2 state (important)

DAB+ mandates the 960-line transform (TS 102 563 clause 5.1), and HE-AAC v2 is
SBR + PS. `oxideav-aac` 0.1.7 decodes 960 for plain AAC-LC but rejects SBR on
any non-1024 family (`Error::SbrUnsupportedFrameFamily`), so the pure-Rust
primary cannot play real DAB+. The `fdk-aac` feature (operator decision,
2026-09-20) swaps the backend to libfdk-aac behind the same adapter API;
`AacDecoder::BACKEND` names the compiled-in one (`"oxideav-aac"` default,
`"fdk-aac"` with the feature). libfdk-aac configures the 960 + SBR + PS
AudioSpecificConfig DAB+ derives, asserted in `tests/aac_fixture.rs`.

The same library's *encoder* cannot emit 960: `AACENC_GRANULE_LENGTH` accepts
1024/512/480/256/240/128/120 and rejects everything else, so no available open
encoder produces a conformant 960 DAB+ bitstream for CI. The committed
`dabplus_heaacv2.sf` fixture is therefore 1024-line — libfdk-aac HE-AAC v2
(AOT 29, 64 kbit/s CBR, 48 kHz), framed by `SuperframeEncoder` into DAB+
super frames whose header is 48 kHz + SBR + mono core + PS. It proves the
transport + adapter + SBR + PS chain against the source PCM (correlation
0.999915 / 0.999920 L/R, rel. RMS 0.0202/0.0130 — the table below); it does
**not** prove the 960 transform. That
stays a hardware item: when the 11C feed reaches the codec, real 960 access
units are the first thing to decode and file here; CI has no 960 vector until
one can be obtained.

Two libfdk-aac behaviours the adapter handles, recorded because they recur:

* Its raw transport (`TT_MP4_RAW`) is a packet transport and requires the AU to
  be bit-exact — any bit the `raw_data_block()` did not consume is a parse
  error. DAB+ zero-stuffs the last AU of a super frame inside its CRC-covered
  region (clause 5.2), so `aac::fdk` trims trailing zero bytes before the fill;
  safe because `ID_END` plus byte alignment makes a valid block's last byte
  non-zero.
* It cannot see that an ASC does not describe the AUs: a 1024 bitstream under a
  960 ASC decodes to concealed silence and returns OK. The adapter's contract
  is that the caller pairs the ASC with its AUs; on air the ASC is derived from
  the header (clause 7.2) and per-AU CRCs gate corruption.

### fdk-aac licensing (Phase 10.15.3 fallback)

`fdk-aac` 0.8.0 is MIT (the Rust binding). `fdk-aac-sys` 0.5.0 is MIT (binding
and build) but vendors the Fraunhofer FDK AAC Codec Library source
(`fdk-aac-sys/aac/`, from `haileys/fdk-aac-rs`), under the "Software License for
The Fraunhofer FDK AAC Codec Library for Android": BSD-3-Clause-style terms
plus extra conditions (retain the notice; make source available with binary
redistribution; no endorsement; no fees) and, in section 3, **no patent licence
of any kind**. AAC/HE-AAC patent licences are administered separately (Via
Licensing / Fraunhofer) and are the user's responsibility, for encoding as well
as decoding. The C source is not vendored in this repository; the dependency is
declared in `neowon-codec`'s `Cargo.toml` and fetched by Cargo.

### Acceptance fixtures and measured agreement

`crates/neowon-codec/tests/fixtures/` (249 KB across the seven fixtures and
their README): synthetic two-tone stereo
(440 Hz L / 880 Hz R), ffmpeg 9.0.2 + afconvert 2.0, commands and SHA-256 in
the fixture README. Metric: best-lag normalised cross-correlation + relative
RMS error at that lag + Goertzel tone check.

Each suite prints its measured row as JSON under `-- --nocapture`; these are
the commands the table's numbers come from (repo root):

1. `cargo test -p neowon-codec --test aac_fixture -- --nocapture`
2. `cargo test -p neowon-codec --features fdk-aac --test aac_fixture -- --nocapture`
3. `cargo test -p neowon-codec --features fdk-aac --test aac_960_sbr -- --nocapture`
4. `cargo test -p neowon-codec --test mp2_fixture -- --nocapture`

Measured 2026-09-22 on this tree (channel L / R); the correlation is the
best-lag normalised value at the stated lag:

| Fixture (build) | lag | correlation L / R | rel. RMS err L / R | tolerances | command |
|---|---:|---|---:|---|---|
| `he_aac_v2.latm` (12 AUs), `oxideav-aac` | 4224 | 0.999999875 / 0.999999847 | 5.0e-4 / 5.5e-4 | ≥0.98, ≤0.05 | 1 |
| `he_aac_v2.latm` (12 AUs), `fdk-aac` | 6992 | 0.999999957 / 0.999999960 | 3.0e-4 / 2.9e-4 | ≥0.98, ≤0.05 | 2 |
| `dabplus_heaacv2.sf` (9 AUs, 3 super frames), `fdk-aac` only | 10528 | 0.999914863 / 0.999919757 | 0.0202 / 0.0130 | ≥0.99, ≤0.05 | 3 |
| `tone.mp2` (17 frames), both builds | 0 | 0.999999990 / 0.999999989 | 1.4e-4 / 1.5e-4 | ≥0.999, ≤0.01 | 4 |
