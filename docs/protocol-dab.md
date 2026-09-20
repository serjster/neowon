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
| UEP sub-channel sizes by index (table 8) | clause 11.3.1 — **not yet transcribed** |
| Character set mapping (complete EBU Latin) | table 47 — **not yet transcribed** |

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
   `PRS_METRIC_MIN` = 0.5 sits between them. A rejected frame decodes *nothing* —
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

## Licensing / provenance (decision D26)

- Porting reference: **`dabradio` 0.5.0** (`xoolive/desperado`), **MIT**,
  8 393 Rust LOC, published as a binary (`has_lib: false`), so it is read and
  ported from, not depended on. Its MIT notice is honoured by this record:
  copyright © Xavier Olive and contributors, MIT licence.
- Read-only reference (GPL, **never copied into this workspace**):
  `JvanKatwijk/dab-cmdline`, `JvanKatwijk/qt-dab`, `welle.io` — including the
  copy vendored under `tmp-inspiration/SDRPlusPlus/decoder_modules/dab_decoder/`.
- Standard-derived constants (mode parameters, tables 12/13/23/24, CRC
  polynomial, tail vector) are cited to the clause that defines them, in the
  code that uses them.

## Verified on hardware

*(empty — this section is the record of the DAB-G1 hardware run: one Band III
channel, the RTL-SDR V3, ensemble identity, service labels, FIB CRC rate, and
whatever surprised us. Nothing in tier 1 is claimed to work on air until this
section has content.)*

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
195.936 "7A" (it is 8A) and 188.928 "9C" (it is 7A). The raster itself is
`174.928 + 1.712·n` MHz with 5A at `n = 0`, which puts 11C at 220.352 and 12C at
227.360 — both matching. Tune by frequency; the letters are administrative
(the "Wiesbaden" arrangement), not from EN 300 401.

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

- **Table 8 (UEP sub-channel sizes)** is not transcribed, so UEP sub-channels
  report their index and no bit rate. EEP sub-channels are exact (1 CU = 64 bits
  per 24 ms).
- **Table 47 (complete EBU Latin)** is not transcribed; labels decode exactly in
  the ASCII range and show `?` outside it. UTF-8 labels (charset 15) are decoded.
- **FIG type 2** extended labels (UTF-8/UCS-2, segmented) are not parsed.
- **Data services** (`P/D = 1`, 32-bit SId) are counted, not tabled.
- **MSC and audio** (tiers 2–3): time de-interleaving (clause 12), energy
  dispersal in the MSC (clause 10.3), UEP/EEP depuncturing (clause 11.3),
  sub-channel extraction, PAD/DLS text, and the codecs.
- The **OFDM front end** (null-symbol detection, PRS correlation, fine timing and
  residual frequency offset) is the next implementation step; its hardware
  behaviour (locking with an RTL-SDR's clock error) is not yet recorded here.
