# SDR feature catalog

The home for the research behind Phase 10's signal-intelligence program. It is the
record a cold session resumes from, so the phase spec and `PLAN.md` link here rather
than restating it.

**Status:** researched 2026-09-18, user-reviewed. Analysis input only — the reference
repositories are local checkouts under `tmp-inspiration/` (untracked) and are **not
authorities**; where they disagree, the tree wins.

## Reference projects

| repo | what it is | features taken |
|---|---|---|
| `ravenSDR` | RTL-SDR console on a Pi with Hailo NPU | arbiter/coalescing; adaptive noise-floor VAD; classifier trust states + held-out-frequency validation; preset labels as ground truth; class-balanced corpus rotation; observation log; SEI embeddings; ADS-B/APRS/POCSAG/AIS/ACARS/ISM decoders + correlation; survey/persist/diff; two-process split |
| `rtlsdrAI` | OpenWebRX+ "signal detective" plugin | rolling-median noise floor; excess-threshold peak detection + bin clustering; duration-debounce tracking (its rounded-centre-freq id is **rejected** — spec 10.2 uses opaque ids); frequency allocation DB + confidence; survey band plans |
| `rtl-ml` | 7-class RF classifier on an ARM SBC | 17-feature vector + StandardScaler + Random Forest; capture quality gates; SNR→accuracy curves; cross-frequency generalisation test; dataset audit |
| `modulation-classification` | GNU Radio AMC over 12 classes | STFT 128×128 log-magnitude CNN (94.1% test, the only reference shipping a reproducible recipe); constellation visualiser; dataset recipe |
| `RF-Classification-ML` | RTL-SDR → ONNX live classification | confusion analysis (analog SSB/DSB hardest); 32-bit vs 1.5-bit quantisation robustness; live ONNX inference loop |
| `CNN-BiLSTM-AMC` | RadioML AMC notebook | CNN+BiLSTM and channel-attention architectures; SNR methodology; temporal-correlation/ACF analysis. **Accuracy numbers untrusted** — the notebook itself documents fabrication and broken checkpoints |
| `torchsig` | Python signal-dataset library | modulation/protocol builders; three impairment levels; wideband time×frequency compositing; hierarchical metadata; dataset recipes + HDF5 layouts; SigMF interop |
| `gnuradio_llm` | LLM-controlled GNU Radio | LLM as a control surface over a validated schema (schema = prompt = validator = label); retry-with-feedback; trace→dataset harvesting |
| `holohub` | (empty broken checkout) | nothing — recorded so the count of nine directories is honest |

Gaps these projects leave open, which neowon would fill: **none has a persistent,
runtime-mutable catalog** with management operations; none uses classical lab
features (EVM, higher-order cumulants, cyclostationary/SCD) — all AMC repos use raw
IQ or an STFT only; confidence provenance is thin (a hard-coded `"medium"`, or an LLM
label with no calibration). These are negatives scoped to the nine local checkouts;
no wider search is claimed, so "none" means "none of these", not "none exists".

## The feature program

Grouped as in the phase spec; ids (S1, M3, C2 …) are stable names for reference.

### 1. SDR backend & hardware (S)
S1 `neowon-sdr` backend (tuner range, rate/gain ladder, AGC, ppm, direct sampling,
bias tee, stream chunk). S2 retune arbitration/coalescing **in the existing
supervisor** (see spec D6). S3 hot-plug reconnect-and-replay. S4 multi-source, per-
serial config. S5 deterministic simulated SDR source. S6 IQ recording. S7
preemptible retune scheduler.

### 2. Live views (V)
V1 spectrum (averaging, peak/min hold, dBm/dBFS, RBW). V2 waterfall (span, cursors,
click-to-tune, LUTs). V3 IQ time scope + magnitude/phase. V4 constellation
(persistence, carrier/timing recovery). V5 eye diagram. V6 instantaneous A/P/F
traces + histograms. V7 zero-span/channel monitor. V8 demodulators (AM/NFM/WFM/SSB/
CW/OOK/FSK/AFSK) → audio and baseband traces. V9 bandplan overlay.

### 3. Modulation lab (M)
M1 constellation with carrier + symbol recovery and reference constellations.
M2 EVM/MER (RMS, peak, per-symbol). M3 higher-order cumulants/moments (C20, C21,
C40, C41, C42, C63 …) with theoretical references. M4 cyclostationary features
(cyclic autocorrelation, spectral correlation density, symbol-rate and carrier-offset
estimation). M5 parameter estimation (symbol rate, roll-off, modulation index, AM
depth, FM deviation, IQ imbalance, DC offset). M6 synchroniser + slicer → bitstream.
M7 occupied bandwidth (99%/OBW), channel power, ACPR, spectral flatness. M8 phase
noise. M9 reference constellation library.

### 4. Detection & measurement (DM)
DM1 rolling-median noise floor. DM2 excess-threshold peak detection + gap clustering.
DM3 stable identity + tracking (first/last seen, duration, debounce). DM4 quality gates
(SNR, peak-to-median, duration). DM5 power/RSSI timeline. DM6 durable observation log.
(These are feature names in the `DM` namespace; spec decisions use `D0`–`D9`.)

### 5. Classification & recognition (C)
C1 classical feature classifier (17-feature vector + per-scheme cumulants) — the
realtime oracle. C2 learned classifier plugin (spectrogram CNN and/or raw-IQ CNN)
behind a `Classifier` trait. C3 `unknown` class + top-2 margin + trust states
(validated/unproven/unprovable). C4 confusion-aware candidate sets (ASK↔DSB-SC,
FSK↔MSK, QAM16↔64, SSB↔DSB, BPSK↔8PSK, GMSK→FM). C5 SNR-conditioned confidence.
C6 cross-frequency validation. C7 operator/preset labels as ground truth. C8
embedding + nearest-neighbour retrieval. C9 class-balanced corpus rotation.

### 6. Protocol & source (P)
P1 frequency-allocation database + ITU emission designators. P2 bookmark import. P3
in-tree decoder plugins (APRS/AX.25, POCSAG/FLEX, ADS-B, AIS, ACARS, ISM/rtl_433,
DMR, P25, LoRa, NOAA APT, WEFAX). P4 demod→decode pipeline. P5 cross-source
correlation. P6 source records with provenance. P7 searchable decode table + export.
(spec D5 means subprocess decoders are out of scope.)

### 7. Scanning & survey (SC)
SC1 band plans. SC2 wideband power sweep. SC3 band-median peak finder. SC4 survey
persistence + diff (new/gone/stronger/weaker). SC5 peak→classify. SC6 scheduled
scans. SC7 watchlist alerts. SC8 frequency hop/channel list. SC9 scan history on the
timeline.

### 8. Catalog (K)
K1 engine-free `neowon-catalog`. K2 entity model (identity, referential rules —
see spec 10.2; record owner D4/D7). K3 per-field provenance + confidence. K4 full
management (create/rename/edit/tag/alias/delete/bulk/merge/purge/undo). K5 query.
K6 persistence (single-writer WAL + index). K7 import/export (CSV/JSON, SigMF). K8
retention/purge policies. K9 catalog UI. K10 script/MCP parity.

### 9. RF fingerprinting (F)
F1 emitter embeddings (1D-CNN + attention). F2 cheap physical fingerprints (IQ
imbalance, DC offset, carrier/clock offsets). F3 protocol-truth bootstrap labels.
F4 transient/startup analysis. F5 emitter DB. F6 enrolment workflow. F7 drift
tracking. F8 privacy guardrails (off by default).

### 10. Dataset & training (T)
T1 sim modulation/protocol builders. T2 impairment environments (perfect/cabled/
wireless). T3 wideband compositing. T4 hierarchical metadata. T5 dataset recipes +
SigMF/torchsig export. T6 golden testbench. T7 corpus from real + sim.

### 11. UX (X)
X1 Scope/SDR modes. X2 SDR workspace. X3 tuning widget. X4 follow-the-radio on
explicit tune only. X5 mode-aware controls. X6 shared phosphor/measurement engine.
X7 single settings surface.

### 12. Control plane (Z)
Z1 script parity. Z2 MCP tools. Z3 live-dev loop. Z4 sim regression runs. Z5
calibration/hold-out tests.

## Counter-case (kept with the sources, per the red-team finding)

The program's premise is not self-evidently right, and these are the strongest
objections found — recorded so the plan argues with them rather than around them:

- **Modulation classification generalises poorly off-distribution.** Deep-learning
  AMC is documented as degrading under noise/channel shift and across receivers; the
  reference accuracies are in-distribution. neowon's value must not rest on a
  headline accuracy from a synthetic corpus. *(Citation to add: the DL-AMC
  generalisation study, ScienceDirect S0952197625007961, 2025 — cited by the round-2
  red-team seat; unverified by us.)*
- **RF fingerprinting is an open problem across receivers** — models overfit the
  channel and the receiver, not only the emitter. A "known emitter" is meaningful
  only on the same receiver chain that enrolled it, which the UI must say.
  *(Verified citation: Pan et al., arXiv:2510.09405, v2 2026-05-26 — cross-receiver
  RFFI degradation; checked by the round-2 red-team seat.)*
- **The reference accuracies are partly untrustworthy** (`CNN-BiLSTM-AMC` documents
  fabricated results); treat every number as unverified until reproduced in-tree.
  "Verified" in this catalog means reproduced in neowon, never reproduced by a
  reference.
- **Scope creep is a real risk**: the seven sub-phases beyond the backend are each
  their own project. This is why the phase carries dated go/no-go gates with numeric
  bars (see the spec's Purpose and gates).
- **Reversing the project's "no libusb on macOS" advantage** to gain V3 compatibility
  is a real cost, and the P0.1 spike must price it rather than assume it.
