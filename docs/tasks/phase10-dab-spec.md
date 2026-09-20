# Phase 10.15 — DAB/DAB+ decoding

Sub-phase of the Phase 10 SDR program (`docs/tasks/phase10-sdr-spec.md`), added
2026-09-20 at the operator's request. Read that spec's Purpose, decisions (D0–D21)
and gates first; its decisions and hard rules apply here unless this file overrides
them explicitly. The research home for SDR features stays
`docs/sdr-feature-catalog.md`; this file is the contract for DAB.

## Purpose (D22)

The SDR mode answers *what is on the air* by measuring and classifying — it guesses.
DAB is the one broadcast standard in Band III that **tells** you: an ensemble
declares its identity and its service list in the Fast Information Channel, error
protected and CRC-checked, at no cost to the operator's signal budget. Decoding it
turns a peak on a band plan into a name, a bitrate and a protection profile.

That is the whole argument for the FIC tier, and it is deliberately the *only* claim
made here. DAB's Main Service Channel and audio codecs are separate tiers with
separate costs, and the phase is structured so the expensive tiers are not started
on the strength of the cheap one's story.

The counter-case, recorded because it is real: welle.io's backend — the chain
everything else is measured against — is ~10k lines of GPL C++ covering OFDM sync,
FIC, MSC, Viterbi, Reed–Solomon, de-interleaving, PRBS descrambling and two audio
codecs. A full DAB+ receiver is a project, not a feature. Tier 1 is ~1/5 of that and
is the part with no codec and no licensing question.

### Gates (a "stop when", not only a "done when")

- **DAB-G1 — closes (or stops) 10.15.1.** The FIC tier must lock and name a real
  ensemble on hardware: on the on-hand RTL-SDR V3, one Band III channel, ≥ 3
  services with their labels, the correct EId, and ≥ 90% of FIBs CRC-clean, filed
  as a JSON readout in `docs/protocol-dab.md`. If the front end has not locked on
  real air by the end of 10.15.1's budget, **the program stops there** — 10.15.2
  and 10.15.3 are not started on an unproven front end — and the failure is
  recorded here and in `PLAN.md`. Sim results do not pass this gate.
- **DAB-G2 — before 10.15.3 (audio).** The DAB+ audio path requires an
  HE-AAC v2 decoder; the crates landscape offers an fdk-aac C dependency (AAC
  licensing is administered as a patent pool) or nothing. This gate is a
  **decision the operator owns**, taken when 10.15.2 lands: approve the dependency,
  or close the program as "FIC + MSC + DLS text, no DAB+ audio". Sim results do
  not pass this gate either.

## Decisions

Proposed 2026-09-20 with this spec; each is marked as approved when the operator
confirms. Nothing in 10.15.1 depends on D23's tier-3 half or on D25's exact naming.

- **D22 — DAB is sub-phase 10.15, in-tree, tiered** (operator-approved 2026-09-20
  by authorising this spec). Tiers: **10.15.1** FIC (ensemble + service table,
  no audio) · **10.15.2** MSC (de-interleaving, energy dispersal, depuncture,
  Viterbi, subchannel bytes, DLS/PAD text) · **10.15.3** audio (DAB: MPEG-1
  Layer II; DAB+: HE-AAC v2). Decision **D5** of the parent spec stands and is
  load-bearing here: in-tree decoders only, so `welle.io`/`qt-dab`/`dabradio` as
  subprocesses are out.
- **D23 — no new dependencies in Tier 1.** `rustfft` is already a `neowon-dsp`
  dependency and nothing else is needed to reach a service table. Tier 2 needs
  none either. Tier 3's codec choice (`oxideav-mp2` for DAB's MPEG-1 Layer II,
  `fdk-aac` for DAB+ HE-AAC v2) is gate DAB-G2 and a separate operator decision.
- **D24 — the decoder is an engine-free, stateful receiver in
  `neowon_dsp::dab`.** It is fed `SampleLayout::Complex` frames at 2.048 MS/s and
  emits structure (`DabStatus`: lock state, EId, ensemble label, services,
  subchannel organisation, FIB CRC counters, sync metrics). It is **not** a
  `DemodMode` variant: `neowon_dsp::demod` is a narrowband channel model
  (`default_width_hz`, one audio stream out) and DAB consumes the whole 1.536 MHz
  ensemble. Audio, when it exists, is a sink off the MSC tier, not the receiver's
  product.
- **D25 — sim-first with a DAB Mode I encoder in `neowon-sim`**
  (`neowon_sim::dab`, `RfScene` preset `rf-dab`): deterministic, seeded, a chosen
  ensemble layout (EId, ensemble label, N services with labels, subchannel
  address/size/bitrate/protection) modulated into Mode I IQ at 2.048 MS/s, with
  optional AWGN. The decoder must recover exactly what the encoder was told to
  send. Preset names are a stable API (AGENTS.md); `rf-dab` joins the existing set.
- **D26 — licensing/provenance rule (this one is not negotiable).** Reference
  implementations are read for algorithm structure. Code or tables are copied into
  this workspace **only** from MIT/Apache-licensed sources, with the notice
  recorded in `docs/protocol-dab.md`; `dabradio` (MIT) is therefore usable as a
  porting reference and `dab-cmdline`/`qt-dab`/`welle.io` (GPL) are **read-only**.
  Constants taken from the standards are cited by clause in the code comment that
  uses them.
- **D27 — honest state, no guessing.** The ensemble table is published only from
  CRC-clean FIBs. `locked` requires a majority of clean FIBs over a window
  (`fib_crc_ok / fib_total` is always reported alongside), and an unlocked receiver
  reports *no* services rather than a stale or partial table. A wrong service name
  is worse than no service name — this is the same rule as `unknown` in the
  classifier.
- **D28 — protocol doc discipline.** 10.15.1 creates `docs/protocol-dab.md` with
  the Mode I parameters actually used, the Band III raster, and every fact verified
  on hardware. Same rule as `docs/protocol-vds1022.md` and
  `docs/protocol-rtlsdr.md`.

## Existing code (verified 2026-09-20)

- **Sample rate is already there.** `neowon_sdr::backend::SAMPLE_RATES` includes
  `2.048e6` (`crates/neowon-sdr/src/backend.rs:24`), which is exactly Mode I's rate.
  P0.1 already streamed 2.048 MS/s with 0 drops on the V3
  (`docs/protocol-rtlsdr.md`).
- **IQ frames are the input.** `CaptureFrame` with `SampleLayout::Complex`,
  interleaved I,Q `f32`, per-component `IqCal`
  (`crates/neowon-core/src/frame.rs`); `neowon_sim::iq` is the D8 deterministic IQ
  generator and `neowon_sim::sdr::RfScene` the preset scene source
  (`crates/neowon-sim/src/sdr.rs:96`), where `rf-dab` will live.
- **The band is already known.** `assets/bandplans/united-kingdom.json:429` labels
  174–230 MHz "DAB Radio"; `neowon-refdb` carries `Modulation::Dab`
  (`crates/neowon-refdb/src/station.rs:38`) and the FMLIST importer maps "DAB+". A
  decoded ensemble is therefore matchable against the station store, and a decoded
  ensemble/service is a `neowon-catalog` entity of the same shape as a detected
  signal.
- **Consumers that already exist:** the SDR workspace routes complex frames to
  `neowon-app/src/sdr/` (`sdr/mod.rs:340`: `frame.layout != SampleLayout::Complex`
  is the guard); the audio output sink is real since 10.10
  (`crates/neowon-audio/src/sink.rs`); the control socket, script grammar and MCP
  tool surface exist (10.9: parity by construction).
- **What does NOT exist:** any OFDM, Viterbi, de-interleaver or codec code
  anywhere in the workspace. `neowon_dsp::decode` is a *scope* digitizing layer
  (UART/I2C/SPI/1-Wire over threshold-crossing traces) and is not reusable here;
  `neowon_dsp::modlab` has symbol-rate/carrier recovery and a Gray slicer, but no
  differential demapping, no FEC, no OFDM.

## Scope fence

Work is confined to `crates/neowon-dsp` (new `dab` module), `crates/neowon-sim`
(new `dab` module + the `rf-dab` preset), `crates/neowon-app` (SDR dock section,
script actions, `get dab`), `crates/neowon-mcp` (one tool), `crates/neowon-refdb`
only if ensemble/service matching is wired, and docs: new `docs/protocol-dab.md`,
this file, `PLAN.md` §4/§7. Do not touch `neowon-vds1022`, the scope paths, the
recorder, or the phosphor engine. Tier 1 adds no UI beyond the dock readout.

## Hard rules

- **Sim first, and the sim is the primary oracle until hardware says otherwise.**
  Every criterion below is a command; a criterion with no command is not met.
- **No new dependency** (D23) — Tier 1 must build with the existing workspace
  dependency set.
- **Engine-free**: `neowon-dsp` and `neowon-sim` acquire no Bevy/GPU dependency.
- **File budgets** ~500 soft / 700 hard — the DAB module is expected to be several
  files along its real seams (front end / FEC / FIB / FIG / state), not one.
- **Determinism**: seeded PRNG only, no wall clock on the signal path, in the
  decoder and in the encoder alike. Decode results are a pure function of the frame
  sequence.
- **Script parity**: every control 10.15.4 adds gets a script action; the ensemble
  readout gets a `get` query and an MCP tool.
- **Never touch USB without explicit permission**; the hardware step of DAB-G1 is a
  separate, operator-authorised run (AGENTS.md).

## Technical basis (to be pinned by 10.15.1's first work item)

| Item | Value / source |
|---|---|
| Standard | **ETSI EN 300 401** (DAB system: framing, FIC/MSC, OFDM, FEC); **ETSI TS 102 563** (DAB+: RS(120,110), superframes, AAC) |
| Mode I | 2.048 MS/s; 2048-point FFT; 504-sample guard; 76 OFDM symbols + null symbol per 96 ms transmission frame; 1536 carriers |
| Modulation | frequency-differential QPSK (DQPSK) — differential along carriers, so **no absolute phase lock is needed**; the FIC and MSC are demapped differentially |
| FIC | 3 OFDM symbols per frame → 12 FIBs of 256 bits (30 data bytes + CRC) → punctured convolutional code → FIGs |
| FIGs used by Tier 1 | ensemble info (label, EId), service list, service labels, subchannel organisation (address/size/bitrate/protection), programme type, country, date/time |
| Band III raster | 174–240 MHz, 1.536 MHz channels, 16 blocks per channel label set |
| Porting reference (MIT) | `xoolive/desperado` → `dabradio` 0.5.0, 8 393 Rust LOC, `has_lib: false` (binary), MIT |
| Read-only reference (GPL) | `JvanKatwijk/dab-cmdline`, `JvanKatwijk/qt-dab`, `welle.io` |

**The tables are the risk.** PRS, puncturing vectors, the de-interleaver mapping and
the FIG layouts are where a from-scratch implementation is wrong in ways that look
like clean data: a sibling project (`Supermagnum/tpeg-rust`) ran a full gr-dab MSC
chain, saw statistically normal soft output, and got **zero** valid packets across
98k CRC checks, never isolating soft-bit polarity from a deeper sync fault — and
abandoned the toolchain over it. Check polarity and table indexing first, on a
synthetic encoder before hardware, and record the check.

## Sub-phases and work items

### 10.15.1 — OFDM front end + FIC → ensemble/service table (Tier 1)

Work items, in order (each lands with its test):

1. **Pin the standard.** Fetch EN 300 401 and record the exact clause numbers for
   the mode table, the PRS definition, the FIB CRC polynomial, the FIC puncturing
   vectors and the FIG layouts in `docs/protocol-dab.md`; create that file. Every
   constant in the code cites its clause. *(No constant enters the code from
   memory.)*
2. **`neowon-dsp::dab::fec`** — FIB CRC check, puncturing/depuncturing to the
   mother code, and a soft-decision Viterbi over the convolutional code, with
   table-vector unit tests and an explicit soft-bit **polarity** test.
3. **`neowon-dsp::dab::ofdm`** — null-symbol detection, PRS correlation for fine
   timing, coarse + residual frequency offset, per-symbol FFT, differential
   demapping of the FIC symbols.
4. **`neowon-dsp::dab::fic`** — FIC → FIBs → FIGs → accumulated ensemble state,
   with the D27 honesty rule (majority-clean window before `locked`).
5. **`neowon-dsp::dab::fig`** — the FIG parsers Tier 1 needs, refusing unknown
   FIGs cleanly (they are legal and expected on air; an unknown FIG must never
   corrupt the table).
6. **`neowon-sim::dab`** — the Mode I encoder (D25) and the `rf-dab` preset.
7. **`neowon-dsp::dab::receiver`** — the `DabReceiver` state machine: IQ frames in,
   `DabStatus` out, re-lock after dropped frames, no false lock on non-DAB input.

**Status 2026-09-20: tier 1 complete on the sim; the hardware run (row 7) is the
only thing outstanding.** *(updated as items land)*

| Item | State |
|---|---|
| 1. Pin the standard | **done** — `docs/protocol-dab.md`: EN 300 401 V2.1.1 clause map, mode I table 22, tables 12/13/23/24/25 pinned by clause. Note: the common `dabradio`/`dab-cmdline` lineage cites the 2001-era edition (`table 31`/`44`, `clause 12`); V2.1.1 numbers them `table 13`/`23`/`24` and `clause 10`. |
| 2. `dab::fec` | **done** — PRBS (table 12 test vector), mother code from the clause-11.1.1 equations, PI=16/15 + `V_T` depuncturing, soft Viterbi, round-trip and polarity tests. |
| 3. `dab::ofdm` | **done** — PRS from tables 23/24, frequency interleaver against the standard's table 25 worked example, differential demap. |
| 4. `dab::fic` | **done** — 9216 soft bits → 4 codewords → 12 FIBs → ensemble table, lock window (D27). |
| 5. `dab::fig` | **done for tier 1** — FIG 0/0, 0/1, 0/2, 1/0, 1/1; unknown FIGs skipped by length. |
| 6. Encoder (oracle) | **done** — FIC bits *and* Mode I IQ (null symbol, PRS, FIC symbols, pseudo-random MSC), in `neowon-dsp::dab::encoder` rather than `neowon-sim` (deviation 1). |
| 7. `dab::receiver` | **done** — `DabReceiver`: null-symbol search, PRS correlation gate, cyclic-prefix frequency offset removed on a continuous time base, three FIC symbols demapped per frame, streaming sample buffer. |

Criteria rows 1–6 all pass: `cargo test -p neowon-dsp --lib dab` (34 tests) and
`cargo test -p neowon-dsp --test dab_fic` (6 tests, ~1.4 s). Row 7 — one Band III
channel on the RTL-SDR, ≥ 3 named services, ≥ 90% FIB CRC, filed in
`docs/protocol-dab.md` — is the only unmet criterion, and it is the one that
passes DAB-G1.

Tables are generated, not typed: a one-off generator reads the standard's text
extraction and emits `dab/tables.rs` after asserting each table's invariants
(PI n keeps exactly 8+n of 32 bits; the PRS ranges tile `[-768,-1] ∪ [1,768]`
exactly once; h values in 0..=3). Values agree with the MIT reference
`dabradio` 0.5.0 value-for-value.


### 10.15.2 — MSC → subchannel bytes + DLS text

Out of scope until DAB-G1 passes: CIF assembly, frequency and time de-interleaving,
energy dispersal descrambling, UEP/EEP depuncturing, Viterbi per subchannel,
subchannel extraction, and the PAD decoder for DLS text (song/artist strings) —
which is the half of DAB+ that needs no audio codec at all.

### 10.15.3 — Audio (behind DAB-G2)

DAB: MPEG-1 Layer II (patent-free; a pure-Rust decoder exists). DAB+: HE-AAC v2
(SBR + PS), which no pure-Rust decoder covers — the operator decision at DAB-G2, and
the reason DAB+ audio is *last* in this program rather than assumed.

### 10.15.4 — App, script and MCP surface

Dock section with the ensemble/service table and the sync quality readout; script
actions `sdr dab on|off|reset` and a `dab` view verb; `get dab` JSON; one MCP tool
returning the ensemble and its services. Emitted after 10.15.1's decoder exists, so
that every displayed field is a field the receiver actually produces.

## Verification contract

Every "Done when" is **quantity · unit · criterion · determinism class · command**,
with a fixture row (signal, seed, N, SNR) where a signal is involved — the parent
spec's rule (phase10-sdr-spec.md §Verification contract).

| # | Quantity · unit | Criterion | Class | Command |
|---|---|---|---|---|
| 1 | EId, ensemble label, service labels, service count | exactly the values the sim encoder was told to send, over 200 frames at 15 dB SNR | exact (fixed-order) | `cargo test -p neowon-dsp --test dab_fic` |
| 2 | FIB CRC pass rate | ≥ 95% of FIBs in the 200-frame fixture | fixed-order | same |
| 3 | frames to re-lock | frame sync re-acquired on the first frame after a 3-frame hole in the stream (≥ 10 of 12 frames decode), table survives, post-gap CRC rate ≥ 0.95 | fixed-order | same |
| 4 | false locks | 0 services and `locked == false` over a 60-frame fixture of `rf-noise`, `rf-reference` and a bare tone (60 frames ≈ 5.8 s of signal; the count is a runtime trade-off, the criterion is that it never locks) | exact | same |
| 5 | table vectors | FIB CRC check value, PRS sequence values, puncture patterns per protection profile, convolutional generator parity — all match the clause cited | exact | `cargo test -p neowon-dsp --lib dab` |
| 6 | soft-bit polarity | a deliberately inverted soft-bit sign fails test 1 and no other test (the trap in `tpeg-rust` is a failing test, not a silent one) | exact | same |
| 7 | hardware lock (manual, dongle on hand, **this closes the sub-phase and is the only unmet criterion**) | ≥ 3 services with labels + correct EId + ≥ 90% FIB CRC on one Band III channel, filed in `docs/protocol-dab.md`; needs the operator's go-ahead and a channel, and the CLI surface it runs through lands with 10.15.4 | fixed-order | manual, `--sim` never |

Rows 1–6 run in CI on the sim; row 7 is never CI (AGENTS.md, hardware safety) and
is the only thing that passes DAB-G1.

**Budget (reviewed, not silently exceeded):** Tier 1 ≤ 12 files and ≤ 2 500 net
lines across `neowon-dsp`, `neowon-sim` and the docs; overrun is a dated review
against the reference size (welle.io's ~10k-line backend for the *whole* receiver).
Command: `git diff --stat $(git merge-base HEAD main)..HEAD -- crates/neowon-dsp
crates/neowon-sim docs/protocol-dab.md`.

*Budget review, 2026-09-20:* tier 1 landed at 9 module files totalling 2 596
lines plus 298 lines of end-to-end test — over the 2 500 figure. The reason is
that the budget did not separate code from tests: 34 of those tests are inline
(1 150 lines), and the code alone is ~1 450. No file approaches the workspace's
700-line hard budget (largest: `fec.rs` 382). Judged in range against the
reference and kept as is; the lesson recorded is that the next budget should
count tests separately, since a spec that demands a published-value test per
table cannot also be tight on lines.

## Testing strategy

- The sim encoder is the oracle for every falsifiable claim above; the decoder is
  tested against *constructed* ensembles (chosen EId, chosen labels, chosen
  protection profiles), never against a recorded snapshot.
- Table-level unit tests cite the standard's clause and the published vector, so a
  wrong table fails a test whose name is the clause — this is what makes "both
  sides written by the same author" survivable.
- **Known epistemic limitation, stated so it is not mistaken for coverage:** the
  encoder and the decoder are written from the same reading of the same standard,
  so a shared misreading passes the sim and fails on air. Only row 7 removes it.
  Do not close 10.15.1 on sim results.
- Impairments the fixture must cover: AWGN at 15 dB, dropped input frames (row 3),
  a carrier offset within ±50 ppm, and a non-DAB input (row 4).
- The hardware run is a manual, operator-authorised step; it records what it saw in
  `docs/protocol-dab.md` whether it passes or fails.

## Done when (10.15.1)

- Rows 1–6 pass in CI; row 7's readout is filed and DAB-G1 is recorded as passed.
- `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test` clean.
- `docs/protocol-dab.md` exists and carries the pinned clauses and the hardware
  facts; `PLAN.md` §4 status and §7 index updated; deviations recorded below.

## Open questions / risks

1. **Shared misreading (the real one).** Encoder and decoder share an author and a
   reading. Mitigated by clause-cited table tests and by DAB-G1, not by more sim.
2. **The GPL boundary.** The best-known implementations are GPL. D26 keeps the
   workspace clean; a lapse here is a licensing defect, not a style issue.
3. **FEC polarity/indexing.** The `tpeg-rust` failure mode (clean-looking soft
   data, zero valid packets) is a table/index/polarity bug that no amount of SNR
   fixes. Row 6 exists for exactly this.
4. **Clock and drops.** The RTL-SDR's crystal error needs residual frequency
   tracking even with ppm correction (`SdrConfig::ppm`, already implemented);
   2.048 MS/s is at the dongle's reliable edge, so dropped chunks are normal input,
   not an error path — the re-lock criterion (row 3) is the contract.
5. **Band III occupancy is location-dependent.** A capture may find an empty
   channel; DAB-G1 needs a channel chosen with the band map / band strip, which the
   RF reference already displays (10.14).
6. **Audio licensing is not a technical question** (DAB-G2) — fdk-aac, and the AAC
   patent pool, or no DAB+ audio. Do not discover this after writing the MSC tier.
7. **Scope priority.** AGENTS.md and PLAN.md §2 keep scope feature-parity ahead of
   the SDR program; 10.15 work must not preempt Phase 8/9 work, and this spec does
   not amend that ordering.

## Deviations (recorded per AGENTS.md)

1. **The oracle encoder lives in `neowon-dsp`, not `neowon-sim`** (amends D25's
   wording). `neowon-sim` does not depend on `neowon-dsp` — the dependency runs
   the other way, in tests — and adding that edge so the sim could synthesise a
   DAB ensemble would invert the workspace's layering. So `dab::encoder` sits
   next to the decoder it tests, and the `rf-dab` `RfScene` preset is deferred to
   10.15.4, where the app can drive the encoder from a dependency it already has.
2. **UEP sub-channels report an index, not a bit rate.** The FIC's short form
   carries a table index whose size and protection live in the standard's table
   8. That table is not transcribed yet (its two-column PDF layout does not
   extract cleanly), so `Protection::Uep` stores the index and `bitrate_kbps`
   stays `None` rather than a guess (D27). EEP is exact: 1 CU = 64 bits per
   24 ms. Table 8 lands with the MSC tier, where UEP sizes are actually needed.
3. **Service labels are exact only in ASCII.** Charset 0 (complete EBU Latin) is
   not transcribed (table 47), so bytes outside `0x20..=0x7E` are shown as `?`;
   charset 15 (UTF-8) decodes fully. Real service names are ASCII in practice,
   and a visible placeholder beats a silently wrong letter.
4. **Data services (`P/D = 1`, 32-bit SId) are counted, not tabled** — tier 1
   tables the 16-bit-`SId` programme services, which is what carries audio.
5. **FIC energy dispersal contradicts the parent spec's research note.** Tier 1
   applies it per 768-bit group with the PRBS restarting at index 0, per
   clause 10.2 — the standard is explicit ("the 3 FIBs corresponding to one CIF"
   form the vector), and it is what makes the CRC rate meaningful.
6. **Row 3's wording is amended to match what a lock window can do.** A majority
   over `LOCK_WINDOW_FRAMES` (8) cannot re-form faster than the window slides, so
   the criterion is: frame sync is re-acquired on the first frame after the gap
   (test: ≥ 10 of 12 frames right after a 3-frame hole), the table survives, and
   the CRC rate over the post-gap frames stays ≥ 0.95. Asserting "re-lock within
   3 frames" would have meant either a shorter window (more false locks) or
   pretending the counter is the indicator.
7. **The carrier-offset impairment is specified in baseband, not in ppm.** The
   cyclic prefix measures offsets within ±500 Hz at mode I
   (`fs / (2·Tu)`), i.e. ±0.19 rad of rotation per symbol — comfortably more
   than an RTL-SDR's residual error once `SdrConfig::ppm` is set, and far less
   than a 50 ppm *RF* error at 200 MHz (10 kHz ≈ 10 carrier spacings).
   **Consequence, recorded rather than hidden:** an offset of a whole carrier or
   more needs the integer-carrier coarse search (`dabradio` implements one;
   tier 1 does not), so tuning to a Band III block must be within ±500 Hz of the
   ensemble's centre. If the hardware run shows carriers landing one bin off,
   this is the first thing to add, and the first thing to check is that the ppm
   correction was actually applied.
8. **Sync costs a frame of lookahead.** Detecting the null symbol requires
   `T_NULL` samples beyond a frame's end, so with `n` frames pushed the receiver
   decodes `n - 1`. Tests account for it explicitly rather than pretending the
   latency is absent.

