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
  `neowon-app/src/sdr/` (`sdr/mod.rs:376`: `frame.layout() != SampleLayout::Complex`
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

**Status 2026-09-20: tier 1 complete, and row 7 passed on air — DAB-G1 is met.**
*(updated as items land)*

| Item | State |
|---|---|
| 1. Pin the standard | **done** — `docs/protocol-dab.md`: EN 300 401 V2.1.1 clause map, mode I table 22, tables 12/13/23/24/25 pinned by clause. Note: the common `dabradio`/`dab-cmdline` lineage cites the 2001-era edition (`table 31`/`44`, `clause 12`); V2.1.1 numbers them `table 13`/`23`/`24` and `clause 10`. |
| 2. `dab::fec` | **done** — PRBS (table 12 test vector), mother code from the clause-11.1.1 equations, PI=16/15 + `V_T` depuncturing, soft Viterbi, round-trip and polarity tests. |
| 3. `dab::ofdm` | **done** — PRS from tables 23/24, frequency interleaver against the standard's table 25 worked example, differential demap. |
| 4. `dab::fic` | **done** — 9216 soft bits → 4 codewords → 12 FIBs → ensemble table, lock window (D27). |
| 5. `dab::fig` | **done for tier 1** — FIG 0/0, 0/1, 0/2, 1/0, 1/1; unknown FIGs skipped by length. |
| 6. Encoder (oracle) | **done** — FIC bits *and* Mode I IQ (null symbol, PRS, FIC symbols, pseudo-random MSC), in `neowon-dsp::dab::encoder` rather than `neowon-sim` (deviation 1). |
| 7. `dab::receiver` | **done** — `DabReceiver`: null-symbol search, PRS correlation gate, cyclic-prefix frequency offset removed on a continuous time base, three FIC symbols demapped per frame, streaming sample buffer. |

Criteria rows 1–6 all pass: `cargo test -p neowon-dsp --lib dab` (74 tests) and
`cargo test -p neowon-dsp --test dab_fic` (8 tests, ~4 s).
*(Counts re-derived 2026-09-22; the earlier 34/6 predated the tier-2/3 tests
and the capture-fix cases.)*

**Row 7 passed on air, 2026-09-20** (11C, 220.352 MHz, RTL-SDR V3, 2.048 MS/s):
`locked`, EId `0x8008`, **14 services with labels** (SLAM!, YOURSAFE, BNR
BusinessBeat, Sky Radio, 538, Qmusic, BNR Nieuwsradio, Radio 10, 100% NL,
Veronica, 538 NONSTOP, Qmusic Non-stop, JOE, Sky Radio Hits) and **98.9–99.3%**
FIB CRC over 110 and 38 frames, against a floor of 3 services and 90%. The
readout, the two defects it exposed, and the open yield question are in
`docs/protocol-dab.md`. **DAB-G1 is met** — the front end is not merely
sim-plausible, it locks and names a real ensemble.

Tables are generated, not typed: a one-off generator reads the standard's text
extraction and emits `dab/tables.rs` after asserting each table's invariants
(PI n keeps exactly 8+n of 32 bits; the PRS ranges tile `[-768,-1] ∪ [1,768]`
exactly once; h values in 0..=3). Values agree with the MIT reference
`dabradio` 0.5.0 value-for-value.


### 10.15.2 — MSC → subchannel bytes + DLS text

**Status 2026-09-20: work items specified below, implementation in progress.**
DAB-G1 passed, so this tier is unblocked. It adds no dependency (D23) and is
pure Rust.

Work items, in order (each lands with its test):

1. **All-symbol demapping in the receiver.** The FIC path demaps 3 of the 76
   symbols per frame; MSC needs the other 72 (72 symbols × 2K bits = 4 CIFs ×
   55 296 bits). The receiver demaps every data symbol with the *same*
   timing/carrier state the FIC path already tracks, hands the FIC's three
   symbols to `fic` as today, and assembles the MSC symbols into four CIFs of
   soft bits. The FIC path is hardware-proven: do not change its arithmetic,
   only its inputs.
2. **`dab::msc` — sub-channel handlers.** One `SubChannelDecoder` per
   `SubChId`, created/reset when the FIC's sub-channel table (0/1) changes.
   Each consumes the CIF soft bits and reverses, in order: capacity-unit
   extraction (`start_cu`, `size_cu`; 1 CU = 64 bits), **time de-interleaving**
   (clause 12 and table 21: 16 delay branches on bit positions,
   `r' = r − D(ir mod 16)` with `D = [0,8,4,12,2,10,6,14,1,9,5,13,3,11,7,15]`;
   per sub-channel state),
   **energy dispersal descrambling** (clause 10.3; the MSC PRBS is not the
   FIC's), **depuncturing** to the mother code (clause 11.3; EEP PI = 24 with
   the sub-channel's option/level, UEP by the table-8 profile), a
   **terminated soft Viterbi** (clause 11.3, not the FIC's tail-biting one),
   and the EEP/UEP CRC check. Output: sub-channel bytes.

   **Table 8 (UEP profiles, clause 11.3.1) lands here** — 64 profiles (bitrate ×
   UEP level 1–5, with the two-level short form's "3-A" style entries), each
   carrying its usable bit rate, puncturing vector and CU size, so UEP
   sub-channels stop reporting an index with no bit rate (deviation 2 of tier 1
   closes).
3. **Porting reference and clause discipline.** The MIT-licensed `dabradio`
   0.5.0 (`fec/eep.rs`, `fec/uep.rs`, `fec/viterbi.rs`,
   `fec/energy_dispersal.rs`, `msc/mod.rs`) is the porting reference; the
   notice goes in `docs/protocol-dab.md`. Every constant cites its EN 300 401
   clause in the code that uses it. The reference's MSC order of operations is
   checked against the standard, not assumed (the `tpeg-rust` failure mode is
   exactly an ordering/polarity slip that looks like clean data).
4. **`dab::pad` — PAD, F-PAD, X-PAD, DLS.** The 2-byte F-PAD + X-PAD carried by
   the audio stream (for DAB MP2: the PAD region at the end of each MPEG frame;
   for DAB+: the in-band PAD at the start of each AAC access unit — the same
   parser serves both, fed from the two transports). DLS is user application
   type 2: segment reassembly (a string may span frames), the update flag and
   the charset. **Table 47 (EBU Latin) is transcribed** here so DLS text is not
   `?`-mangled (deviation 3 of tier 1 was written for service labels; DLS is
   mostly Latin text, so this is where it matters). Unknown applications are
   skipped by their length: never guessed (D27).
5. **Status and counters.** `DabStatus` gains per-sub-channel decode counters
   (`frames`, `crc_failures`) and the DLS state; an unlocked receiver still
   reports no table. The honesty rule of D27 applies to text too: publish a DLS
   string only when its reassembly is complete and CRC-clean, never a partial.
6. **Encoder: the MSC mirror.** `dab::encoder` gains the encode side of every
   step above — UEP/EEP encoding (at least one EEP profile and one UEP profile
   for the tests), CRC, energy dispersal, time interleaving, CU mapping — and
   can carry a chosen set of sub-channels. The sim oracle then covers tier 2
   the same way it covers the FIC: the decoder must recover exactly what the
   encoder was told to send.

**Status 2026-09-20: landed, sim-proven, wired into the app.** `dab::fec` gained
the EEP/UEP tables and the terminated Viterbi (`fec/` split for budget),
`dab::msc` the clause-12 interleaver and per-sub-channel decoders, `dab::pad`
the PAD/DLS parser and `dab::charset` table 47. Rows 8–12 are green
(`--test dab_msc`, 200 frames at 15 dB, EEP 3-A + UEP 3 bit-exact; multi-frame
DLS). The app's `rf-dab` scene carries the ensemble (five services, EEP+UEP,
DLS — `crates/neowon-app/src/sdr/dab_scene.rs`); `--test sdr_dab` proves
FIC → MSC → DLS through the control socket.

### 10.15.3 — Audio (DAB-G2 decided 2026-09-20: oxideav-aac, fdk-aac fallback)

DAB: MPEG-1 Layer II; DAB+: HE-AAC v2 (SBR + PS). **DAB-G2 is decided**
(operator, 2026-09-20, after the crates-landscape research): the primary codec
is the **pure-Rust MIT `oxideav-aac`** (0.1.7, claims the full AAC-LC + SBR +
PS chain with staged-fixture evidence and the mandatory 960 transform), with
the **`fdk-aac` C binding as the documented fallback** if the primary fails its
acceptance fixtures. `oxideav-mp2` (pure Rust, MIT, passes the official
ISO/IEC 13818-4 suite) decodes DAB classic. Both live behind one adapter so the
fallback is a feature flip, not a rewrite. New crate `neowon-codec` (engine-free,
no Bevy/GPU) owns the DAB+ transport framing, the codec adapters and PAD/DLS,
so `neowon-dsp` stays codec-free.

Work items, in order:

1. **`neowon-codec::dabplus` — DAB+ transport.** From TS 102 563: the audio
   superframe (120 ms; `subchannel_index × 110` bytes of data plus RS parity),
   the superframe header (Fire code, parameters, `au_start`), the per-AU CRC,
   the access units (2/3/4/6 per superframe per table 2 — not five; five is the
   number of DAB logical frames that carry one superframe), the in-band PAD in
   a leading `data_stream_element()`, and the `AudioSpecificConfig` **derived
   from the superframe header** (clause 7.2 — it is not transmitted, and not in
   the PAD); RS(120,110) error correction (erasure-aware), then AU assembly.
   Constants cite TS 102 563 clauses.
2. **`neowon-codec::aac` — the adapter.** `AacDecoder::decode(au, asc) -> PCM`
   wrapping `oxideav-aac` (LOAS/LATM carriage carrying the ASC, or the crate's
   raw-AU entry point if one exists — the adapter hides which). Output: f32
   interleaved, decoder rate (HE-AAC v2 = 48 kHz stereo), with `SbrSupport`
   surfaced rather than assumed.
3. **`neowon-codec::mp2` — DAB classic.** MPEG-1 Layer II frames out of the
   sub-channel byte stream (PAD at the frame end), decoded with `oxideav-mp2`;
   PAD bytes are returned alongside PCM for the DLS parser.
4. **Acceptance fixtures.** The codec is the one part of this program whose
   sim oracle cannot be our own encoder: an HE-AAC v2 fixture (a few frames,
   DAB+ framed) is committed with reference PCM generated by an independent
   decoder offline, and the acceptance test asserts our decode matches within
   tolerance (energy/spectral, not bit-exact — the standard allows decoder
   differences). The same for one MP2 frame. If `oxideav-aac` fails these, stop
   and report to the operator before writing the fdk-aac feature.
5. **PCM delivery.** Decoded PCM goes to the existing `neowon-audio` sink. If a
   stream's rate differs from the sink's, resample with the crate's existing
   windowed-sinc resampler (10.10) rather than opening a second device.

**Status 2026-09-20: landed; playback works with the `fdk-aac` feature.** The
primary failed its real use case (deviation 10), so the fallback is the
playback backend: `AacDecoder::BACKEND` reports which is compiled in. The
transport, MP2 and the HE-AAC v2 fixture suites pass (`neowon-codec`, rows
13–15 and 14b under the `fdk-aac` feature); the app plays both codings from the
sim scene (`--test sdr_dab_audio`,
`get dab.audio` with backend/rate/channels/peak). Row 17 (on-air listening) is
the operator's and is the only available proof for real 960/SBR AUs.

### 10.15.4 — App, script and MCP surface

Dock section with the ensemble/service table and the sync quality readout; script
actions `sdr dab on|off|reset`; `get dab` JSON; MCP tools returning the ensemble
and its services. Emitted after 10.15.1's decoder exists, so that every displayed
field is a field the receiver actually produces.

**Status 2026-09-20: control plane done, verified live.** `sdr dab on|off|reset`
(`SdrAction::Dab`, with its `Display` arm so the script-parity rule holds by
construction and a persistence priority), `get dab` (JSON: lock state, FIB CRC
counters, carrier offset, PRS metric, and the ensemble with services,
sub-channels, bit rates, protection and coding), the MCP tools `dab_ensemble`
and `dab_control`, and a `DAB` dock section whose readout always shows the sync
quality line — an unlocked receiver must not look like an absent one. The
receiver is fed the **raw IQ frames**, beside the demodulator rather than as a
mode of it: DAB wants the whole 1.536 MHz ensemble, not a 12.5 kHz channel.

Verified over the control socket in `--sim` (no USB): `sdr dab on` then `get dab`
returns `locked:false` with `prs_metric` ≈ 0.02 and an empty table on the
reference tone, and `get uitree` shows `dock section DAB` drawing
`not locked  FIB CRC -  0 frames  PRS 0.02`.

**Still missing for a positive demo:** the `rf-dab` `RfScene` preset, i.e. a sim
scene carrying a real ensemble, so the app can be seen locking without hardware.
It is a small piece (the sim gains a buffer-backed `IqComponent`; the app builds
the frame from `dab::encoder`, which is the layering deviation 1 already
records), and the hardware run is the stronger evidence anyway.

**Audio playback (lands with 10.15.3):** service selection by index or SId
(`sdr dab service <n|sid>`), `sdr dab play|stop`, a DLS line (song/artist from
the selected service's PAD) in the DAB dock section and the `get dab` JSON,
which also gains `service`, `dls`, and `audio` (decoder state, rate, channels,
underruns). MCP: `dab_control` gains the selected service and transport state,
`dab_ensemble` carries DLS. Volume and mute reuse `sdr volume|mute` and the
existing sink. The `rf-dab` sim scene is then built in the app from
`dab::encoder` through a buffer-backed sim component (tier-1 deviation 1), so
the positive demo needs no hardware.

**Status 2026-09-21: the Band III channel selector answers "how do I tune to
DAB without knowing the frequency by heart".** The DAB dock's block row and
`sdr dab channel next|prev|<label>` land the hardware centre exactly on a
block's centre — the front end measures ±500 Hz of carrier offset, so a
band-plan click (which lands on the allocation's centre, 18 MHz off in Band
III) locks nothing. `get dab` reports `channel` (label, centre, and whether
the active plan allocates it) whether or not the receiver runs; MCP
`dab_control` gains `channel`. The block catalogue is a table in
`neowon-refdb::dab` (deviation 16); the picker draws its list from the active
plan's DAB-named allocation, and a plan without one says so instead of
inventing frequencies. Tests: `--test sdr_dab`'s
`dab_channel_selects_a_band_iii_block_over_the_control_socket`, the refdb
lookup tests, and the `sdr dab channel` round-trip.

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
| 7 | hardware lock (**passed 2026-09-20**) | 11C at 220.352 MHz: locked, EId `0x8008`, 14 services with labels, 98.9–99.3% FIB CRC — filed in `docs/protocol-dab.md` | fixed-order | manual, `--sim` never; run through the app's control socket (`sdr dab on`, `get dab`) |

Rows 1–6 run in CI on the sim; row 7 is never CI (AGENTS.md, hardware safety) and
is the only thing that passes DAB-G1.

**Budget (reviewed, not silently exceeded):** Tier 1 ≤ 12 files and ≤ 2 500 net
lines across `neowon-dsp`, `neowon-sim` and the docs; overrun is a dated review
against the reference size (welle.io's ~10k-line backend for the *whole* receiver).
Command (the landing tree, rev `3842782`): `for f in encoder fec fib fic fig mod
ofdm receiver tables; do git show 3842782:crates/neowon-dsp/src/dab/$f.rs; done
| wc -l` — 2 726.

*Budget review, 2026-09-20 (figures re-derived 2026-09-22):* tier 1 landed at 9
module files totalling **2 726** lines plus 298 lines of end-to-end test — over
the 2 500 figure. The reason is that the budget did not separate code from
tests: **765** of those lines are inline tests (counted from the first
`#[cfg(test)]` to EOF) and the code alone is **1 961**. No file approaches the
workspace's 700-line hard budget (largest: `fec.rs` 382 at that rev). Judged in
range against the reference and kept as is; the lesson recorded is that the next
budget should count tests separately, since a spec that demands a published-value
test per table cannot also be tight on lines.

### Verification contract (tiers 2–3)

Every row is quantity · unit · criterion · class · command, with the same rule
as tier 1: a row with no command is not met. Rows 8–15 (14b included) are
sim/CI; row 16 is a windowed app test (`-- --ignored`, sim only); row 17 is
manual.

| # | Quantity · unit | Criterion | Class | Command |
|---|---|---|---|---|
| 8 | sub-channel bytes | bit-exact against the sim encoder for EEP (option A, level 3) and UEP (level 3) over 200 frames at 15 dB | exact (fixed-order) | `cargo test -p neowon-dsp --test dab_msc` |
| 9 | MSC CRC pass rate | ≥ 95% over the same fixture | fixed-order | same |
| 10 | time de-interleaver | the clause-12 CU permutation reproduced for a known input vector | exact | `cargo test -p neowon-dsp --lib dab` |
| 11 | UEP table 8 | every profile's usable bit rate and CU size match clause 11.3.1 | exact | same |
| 12 | DLS text | exact round-trip of a multi-segment string, including one spanning two frames | exact | `cargo test -p neowon-dsp --test dab_msc` |
| 13 | DAB+ superframe | RS(120,110) parity and CRC match TS 102 563 vectors; 2/3/4/6 AUs per superframe (table 2 — five is the number of DAB logical frames, deviation 11); a damaged superframe is recovered by erasure correction | exact | `cargo test -p neowon-codec --test dabplus` |
| 14 | HE-AAC v2 fixture | committed DAB+ fixture decodes to PCM within the stated tolerance of independently generated reference PCM | tolerance | `cargo test -p neowon-codec --test aac_fixture` |
| 14b | DAB+ HE-AAC v2 super frame fixture | the committed `dabplus_heaacv2.sf` decodes to PCM within the stated tolerance of its source-tone reference, with correlation / lag / relative RMS printed under `--nocapture`; compiles only under `--features fdk-aac` | tolerance | `cargo test -p neowon-codec --features fdk-aac --test aac_960_sbr -- --nocapture` |
| 15 | MP2 fixture | committed MP2 frame decodes to PCM within the stated tolerance | tolerance | `cargo test -p neowon-codec --test mp2_fixture` |
| 16 | playback state · sim scene | the `rf-dab` DAB+ and MP2 programmes decode and play over the control socket (state `playing`, 48 kHz, 2 channels, a non-zero moving peak, the compiled backend reported); stop returns to `off` with no peak; a sub-channel that is not a codec stream surfaces a typed `error`, never silence | fixed-order | `cargo test -p neowon-app --test sdr_dab_audio -- --ignored` |
| 17 | hardware listening | the operator hears a named 11C service and the DLS line tracks it — filed in `docs/protocol-dab.md` | manual | app, `--rtl`, never CI |

**Budget (tiers 2–3):** ≤ 14 new files and ≤ 5 000 net lines across
`neowon-dsp`, `neowon-codec`, `neowon-sim` and the app; tests counted
separately (the tier-1 lesson). Overrun is a dated review against the
references (`dabradio`'s MSC+PAD is ~2 400 lines; welle.io's whole backend
~10k).

*Budget review, 2026-09-22; re-counted 2026-09-23 (round-1 fix pass).* Counted
from the tree, the DAB files added after tier 1 total **7 499** lines by the
command below, now **25 files / 7 881** lines once `sdr/dab_state.rs` (382, the
one-owner DAB state) is included — it is DAB code and belongs in
this count, but is left out of the quoted command so the command and its printed
number stay in step. The 2026-09-22 figure was **7 353**; the growth is the
round-1 fix pass (`sdr/dab_state.rs`, and `dab_audio/decode.rs` gaining the
`SinkFeed` pre-buffer). `sdr/audio.rs` is deliberately **not** counted: it owns
the audio device for every instrument, not DAB. The original count said 24 files
total **7 353** lines — 23 inside the budget's four crates: 8 in
`neowon-dsp` (`charset`, `msc`, `pad`, `encoder/msc`,
`fec/{eep,fic,uep,uep_table}`), 9 in `neowon-codec` (`lib`, `error`, `mp2`,
`aac/*`, `dabplus/*`), 6 in the app (`sdr/dab.rs`, `sdr/dab_scene.rs`,
`sdr/dab_audio/{mod,decode,transport}.rs`, `ui/dab_dock.rs`) — plus the Band III
catalogue (`neowon-refdb/src/dab.rs`, outside the four). Code alone is **5 996**
lines — over the 5 000 cap by ~1 000, and 24 files against 14. The inline tests
in those files (first `#[cfg(test)]` to EOF) are 1 357 lines and the `tests/`
and `tests.rs` suites 3 087 more. **Judged justified and kept as is:** the cap
was written before tier 3's shape was known — a new engine-free crate for the
DAB+ transport (superframe, RS(120,110), Fire code, AU assembly) and the two
codec adapters, the app's audio/transport/scene path, and the MSC
de-interleave → descramble → depuncture → Viterbi chain — and the result is
still inside the references (`dabradio`'s MSC+PAD ~2 400 lines; welle.io's whole
backend ~10k). The limit itself is not changed; if the codec and app tiers are
meant to fit under the same cap, that is a plan-level re-scope.
Command (repo root): `wc -l crates/neowon-dsp/src/dab/{charset,msc,pad}.rs
crates/neowon-dsp/src/dab/encoder/msc.rs
crates/neowon-dsp/src/dab/fec/{eep,fic,uep,uep_table}.rs
crates/neowon-codec/src/{lib,error,mp2}.rs crates/neowon-codec/src/aac/*.rs
crates/neowon-codec/src/dabplus/{mod,crc,rs}.rs crates/neowon-app/src/sdr/dab.rs
crates/neowon-app/src/sdr/dab_scene.rs
crates/neowon-app/src/sdr/dab_audio/{mod,decode,transport}.rs
crates/neowon-app/src/ui/dab_dock.rs crates/neowon-refdb/src/dab.rs | tail -1`
→ `7499 total` (2026-09-23; `7353` on 2026-09-22).

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
- The receiver→transport→codec path runs headless too (D20, D21):
  `dab_scene::tests::the_rf_dab_scene_decodes_to_the_known_tone` drives the
  `rf-dab` scene IQ to decoded PCM and asserts the fixture tone's spectrum, and
  `dab_scene::tests::the_embedded_audio_fixtures_match_the_codec_originals`
  fails on any drift of the app's fixture copies.
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

## Review fixes

### Implementation round 1 (`phase10-dab-implementation`, 2026-09-22)

One round over the uncommitted DAB tiers 2–3 session (6 seats, mean 6.2, 23
errors; scratch `panel.md` / `triage.md` / `panel-plan.md`, not a committed
link). Operator decision
(2026-09-22): fix everything — every error and finding — and sweep the still-open
`M`-items in `phase10-sdr-spec.md` `## Review fixes` in the same pass. Checklist
names the round's merged ids (`D`-numbers):

- [x] **Evidence regenerates.** Re-record the 11C capture as 525/1280/1100 and
  1110/0 with path, rev, date and SHA-256; the recorded command is root-relative
  and panics on a missing file; `dab_air_capture` asserts capture-derived floors;
  the "prediction off" counterfactual becomes a switch or is deleted (D1).
- [x] **Every criterion can fail.** `harness.md` and the Done-when rows get an
  `-- --ignored` app sweep (`sdr_dab`, `sdr_dab_audio`, `sdr_integration`);
  `neowon-ml/ort` removed or annotated; `fdk-aac` added to the gate and CI; row 13
  → 2/3/4/6 AUs; row 16 gains its spectral assertion or is restated; continuity
  counters asserted; M19's vacuous assert replaced; a headless receiver→codec
  test; a bounded-tracker test; a fixture-drift test; the expiry test's guard no
  longer scales with its constant (D2, D3, D5, D16, D19, D20, D21, D24).
- [x] **The record against the tree.** `protocol-dab.md` refreshed
  (`PRS_METRIC_MIN` 0.35, superseded statements, "not yet transcribed" tables,
  duplicate `D26`); numbers re-derived (73/8, one MP2 value, 249 KB, C42,
  EVM/Rs); codec correlation output by a named `--features fdk-aac` command;
  `aac_fixture.rs` contradiction fixed; M34 finished (librtlsdr, README /
  ui-anatomy / spec Instrument-menu claims, phase-close criteria, one
  next-action); "three services"
  → five; deviations renumbered; the tiers-2–3 budget review filed; the
  `M`-checklist ticked (M8, M10, M11, M17, M20, M29) and the half-landed
  annotated (M12, M19, M21, M34) (D4, D5, D6, D7, D26, D27).
  *Closed 2026-09-23. Every clause re-verified
  against the tree by its own command; the count is **74/8**, not the
  73/8 written above, because tier-2/3 tests landed after this bullet was
  written (`cargo test -p neowon-dsp --lib dab`). M20 and M29 were **false when
  the triage assumed them green** — M20's 10 dB row was guarded away by
  `if snr >= 20.0` and M29's two `Option<...Caps>` were still split — and were
  fixed rather than ticked. Gate green on all 11 lines.*
- [x] **One owner per state.** `DabState` with `reset()`/`no_input()`; one owner
  of the audio device; the default-on socket and `NEOWON_ORPHAN_EXIT` recorded in
  PLAN with a date, write verbs token-gated, `AGENTS.md:85` / `PLAN.md:167`
  corrected (D8, D9, D10).
  *Closed 2026-09-23. D8's sweep found a site the finding did not
  name — a hand-rolled clear in `sdr/dab.rs::channel()` cleared 5 of 7 fields,
  leaving a stale splice grid after a block change; the clears are now written
  `*self = Self { rx, ..Self::default() }` so a later field is cleared by
  construction. D10's verb classification is structural
  (`control/privilege.rs`, exhaustive match — a new verb will not compile until
  it is classified) rather than a hand-kept list. **Gate debt:** 10 of 11 lines
  green (`cargo test` 519/0/40); `ui_capture` is **undecided** — the operator's
  screen re-locked, the environmental failure `harness.md` documents. The token
  gate is not implicated: both `ui_capture` logs fail inside `script::shot`'s
  own `got only blank frames` path with zero auth-refusal text, i.e. `shot` was
  authorised and ran. **Debt cleared 2026-09-23:** `ui_capture` passed 2/2 on an
  unlocked screen, exercising the newly gated `shot` verb end to
  end.*
- [x] **Budgets.** Split `main.rs`, `receiver.rs`, `control/mod.rs`,
  `dab_scene.rs`, `dab_audio/mod.rs`; `ui.rs` shrinks again; partial waterfall row
  upload or the byte rate recorded (D11, D25).
  *Closed 2026-09-23.* Every file named here is under 500 lines; the waterfall
  byte rate (~25 MiB/s) is recorded in `PLAN.md`'s Backlog. The one file still
  over 700 is `neowon-vds1022/src/device.rs` (884), left alone because a split
  of driver code cannot be verified without the scope attached.
- [x] **The interface a user meets.** DAB readout fits the rail; expired
  diagnostic visible; an unplayable service is not offered; DAB entry point and
  gesture hint; expiry age, cumulative-CRC label, SDR error home, notch label
  (D12, D13, D17, D18).
  *Closed 2026-09-23. Every geometry/visibility fix is pinned by a
  `get uitree` test with revert-and-fail evidence, not by a screenshot. The
  locked section went from 668.9 px wide in a 456 px rail, with Play and DLS
  below the rail's bottom, to 456 × 620.5 with both on screen; the rail test
  asserts no element crosses the rail edge in eight states. An unplayable
  service is judged by the same `sdr::dab_audio::service_spec` that refuses
  `sdr dab play`, so the dock, `get dab` (`playable`/`why_not`) and the verb
  cannot disagree. The View entry reuses `sdr dab on|off` (script parity with
  no new verb). **Left open, recorded:** the SDR dock's collapsible sections
  have no script action — the `dock` verb reaches only the scope's — which is
  a script-parity gap for the carry-over sweep. Gate green on all 11 lines,
  `cargo test` 531/0/42, `sdr_dab` 6.*
- [x] **Drops and pricing.** M22 dropped-pairs surfaced and used by splice
  detection; M16 `--example frame_cost` landed; M25 re-deferred with the measured
  number (D14, D15).
  *Closed 2026-09-23, with M23 and M24 swept in from the sdr-spec's
  Performance bullet. The drop now travels **on the frame**
  (`CaptureFrame::dropped_before()`, filled from `Stream::overflows()` and
  carried forward by the supervisor when it cannot hand a frame over), so
  `sdr::dab::feed` splices on a counted drop and the timestamp check survives
  only as the net for what a counter cannot see — which closes **deviation 19**
  and the matching paragraph in `docs/protocol-dab.md`. `frame_cost` prices the
  loop at 3.116 ms display path / 5.036 ms all consumers against the 32 ms
  reference (6.4x margin), which retired M25's stale `FftPlanner` reason
  (0.018 ms, 0.04 % of the period); what stays deferred is the waterfall
  re-upload at ~25 MiB/s, now written down. Gate green on all 11 lines,
  `cargo test` 529/0/40.*
- [ ] **Carry-over sweep.** Every open `M`-item in `phase10-sdr-spec.md`
  `## Review fixes` (M1–M3, M5, M6, M13–M15, M21, M23, M24, M26–M28, M30, M33,
  M35, plus the open halves listed above) is fixed in this pass (operator,
  2026-09-22).

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
9. **Tier-2 standard corrections (2026-09-20).** The spec's "M = 17 CUs" for
   clause 12 does not exist in the standard: it is 16 delay branches on bit
   positions (`D(ir mod 16)`, table 21). And EN 300 401 defines no MSC payload
   CRC (only the FIB CRC and the X-PAD/DLS CRC), so the "MSC CRC pass rate"
   row is met by an oracle-only CRC used when the encoder and decoder are
   paired (`enable_msc_payload_crc`); on air the counters report zero checks
   rather than an invented rate.
10. **`oxideav-aac` 0.1.7 cannot decode DAB+ HE-AAC v2 (2026-09-20).** It
    rejects SBR on non-1024 frame families, and DAB+ mandates the 960
    transform with SBR (TS 102 563 clause 5.1). The committed 1024-line Apple
    HE-AAC v2 fixture passes, so the adapter, SBR and PS paths are proven; a
    real 960/SBR AU returns `Error::SbrUnsupportedFrameFamily`, surfaced, never
    mis-decoded. The DAB-G2 fallback (`fdk-aac`, feature-gated) is therefore
    being implemented as the playback backend.
11. **DAB+ contract wording corrected from the standard (2026-09-20).** The
    superframe carries 2/3/4/6 access units per table 2 (five is the number of
    DAB logical frames per superframe), and the `AudioSpecificConfig` is
    derived from the superframe header (clause 7.2), not carried in the PAD.
    Code and tests follow the clause; the tier-2 PAD parser remains the DLS
    path for both codings, and PAD is the only metadata transport.
12. **PAD split across the seam (scope note).** The F-PAD/X-PAD and DLS parser
    lives in `neowon-dsp::dab::pad` as §10.15.2 specifies; `neowon-codec` owns
    the DAB+ transport framing and extracts the raw in-band PAD bytes
    (`dabplus::extract_pad`) that the app feeds to that parser. MP2's PAD
    (the ancillary tail) is returned by `neowon-codec::mp2` the same way. No
    second DLS implementation.
13. **The sim scene's loop seam is an interleaver continuity rule (2026-09-20,
    found by a failing test).** Replaying a buffer-backed `rf-dab` scene is a
    valid continuation of the transmitter only when (a) the buffer starts after
    the clause-12 transient (`WARMUP_FRAMES` are generated and discarded) and
    (b) every chosen stream's cycle *divides* the buffer's logical-frame count.
    The MP2 fixture's 17-frame cycle did not divide the 60-frame buffer, so the
    receiver's de-interleaver mixed two payload phases across each wrap and
    corrupted the frames after it (`--test sdr_dab_audio` hard-failed; the
    DAB+ stream, whose 3-superframe cycle divides 60, had hidden it). The scene
    now cycles 15 MP2 frames and asserts the divisibility for every chosen
    stream.
14. **Playback carries a documented non-conformant sim stimulus.** The DAB+
    programme in `rf-dab` is the 1024-line libfdk fixture with its own ASC
    (`asc_override`), because no available open encoder emits the 960 transform
    (deviation 10). The app labels it as a test stimulus; real 960/SBR AUs are
    exercised only by the on-air run (row 17), where the ASC is derived from
    the superframe header as the standard requires.

15. **Tier-2's on-air failure was the receiver's frame-grid policy, not a
    table (2026-09-21).** FIC clean at 100% FIB CRC while the DAB+ transport
    never validated read like a shared encoder/decoder misreading — but every
    suspect table checks against EN 300 401 (MSC PRBS clause 10.3, table 13
    puncturing, tables 18/20 EEP, table 21 de-interleaver). The defect: the
    frame start was re-derived from the null-symbol power dip every frame, and
    on air most attempts fell below the PRS gate; each rejection discarded the
    clause-12 delay line, so no sub-channel ever held 16 logical frames. The
     fix predicts the grid from the last accepted frame, treats a weak frame at
     the predicted start as a fade, and gives the super-frame search a
     20-super-frame shape budget (a stream that ever aligned can never be
     declared not-DAB+). Pinned on the archived 11C capture by the test-only
     switch `NEOWON_DAB_NO_PREDICTION=1` (`dab::receiver`): FIC 180/180 CRCs
     and EId 0x8008 with **0** Fire-clean super frames and 0 AU CRCs.
     Re-measured 2026-09-22 at rev `4579cfb` on the capture (sha256
     `93293363ba09a35e4737fb72d0e7bd3e20dcd233ef9c4f70642cc37421d462a8`), with
     the prediction enabled: 525 Fire-clean super
     frames / 1280 AUs / 1100 AU CRCs through `dab_air_capture`, and 1110 AU
     CRCs / 0 shape mismatches through the app transport. The deviation's
     earlier figures (346/843/754 and 707) were recorded on an older tree of
     the 2026-09-21 session and no longer regenerate. Two bounds defects in the
     same search path (refinement moving the start past the buffered frame; a
     scan guard missing `T_G`) are fixed with tests. Regression tests:
     `multipath_echo_keeps_the_clause_12_chain_contiguous` and
     `frame_sized_feeds_never_overrun_the_sample_buffer` in
     `crates/neowon-dsp/tests/dab_msc.rs`. Full evidence, paths and commands in
     `docs/protocol-dab.md`.

16. **The Band III block catalogue is a table, not a formula (2026-09-21).**
    The channel selector was briefed to "use the refdb band-plan data — do
    not hardcode a table". The plans carry DAB allocations as ranges, not
    blocks, and the 5A–13F raster is not uniform (steps of 1.568, 1.712,
    1.856 and 1.872 MHz), so its centres cannot be generated: a uniform
    `174.928 + 1.712·n` formula puts 11C at 221.152 MHz, not 220.352. The
    catalogue is therefore a 38-entry table in `neowon-refdb::dab`,
    transcribed from `dabradio` 0.5.0 (MIT, the tier-1 porting reference;
    notice in `docs/protocol-dab.md`) and checked against the four centres
    this project has recorded. The **offer** is still plan-derived, not
    hardcoded: `BandPlan::dab_blocks()` intersects the raster with the
    plan's DAB-named allocation, and a plan that declares none (the default
    `general`) yields an empty list — the dock and `get dab` say so instead
    of inventing per-country frequencies. The earlier protocol-doc sentence
    claiming a uniform raster formula is corrected in `docs/protocol-dab.md`.

17. **Two defects found on air, both ours, both fixed** (details and evidence in
    `docs/protocol-dab.md`): the receiver was fed from the display path, which is
    latest-wins, so it saw a holed stream — the carrier estimate read −223 Hz
    instead of −12 Hz and labels took 40 s instead of 12; and `prs_metric`
    reported the last *attempt* rather than the accepted frame, reading ~0.03 on a
    receiver decoding 98.9% of its FIBs. Both were invisible in sim: the sim feeds
    whole contiguous frames at exactly 2.048 MS/s and never splices them.
    **The lesson for the remaining tiers: a criterion that only holds because the
    sim's frame stream is ideal is not a criterion.** The in-app path is where
    that assumption gets tested, and it took a hardware session to expose it.
18. **Sync yield on air is ~13% of attempts** (38 accepted against 262 rejected in
    the measured run), with rejected attempts scoring ~0.04 PRS — genuinely
    misaligned rather than marginal. Most likely splices from USB drops, since the
    null symbol then stops being the unique power dip. It locks and holds, so this
    is a yield question and not a correctness one, but it is the first item of
    10.15.1 follow-up work.
19. **Splice detection was inexact; closed 2026-09-23 (D14/M22).** `CaptureFrame::t_start`
    is arrival-time derived, so a tight gap check fires on jitter (it cost ~87% of
    one session's attempts before being loosened to a coarse safety net). The fix
    named here — a dropped-sample counter in the frame, from the backend — has
    landed: `CaptureFrame::dropped_before()` carries the pairs the producer knows
    were lost (`neowon-sdr` reports USB overflows and the chunk it discards after
    a retune; `neowon-audio` reports its ring overruns), and `sdr::dab::feed`
    splices on that count. The coarse timestamp check is kept as the safety net
    for what a counter cannot see — a producer that does not count, a stopped and
    restarted stream, an instrument swap. The count is also reported by `get sdr`
    (`dropped_pairs`, `drop_events`) and the SDR dock's *Drops* row, so the
    live-continuity claim is falsifiable. Test:
    `cargo test -p neowon-app --bin neowon-app sdr::dab::tests::a_reported_drop`.
