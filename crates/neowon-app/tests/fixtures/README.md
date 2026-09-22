# `rf-dab` audio fixtures (Phase 10.15.3, app)

Copies of three committed `neowon-codec` acceptance fixtures, byte-identical
to `crates/neowon-codec/tests/fixtures/` (SHA-256 checked on copy). The
`rf-dab` sim scene embeds them with `include_bytes!` so the ensemble carries
a real DAB+ stream and a real MPEG-1 Layer II stream, and the windowed
`sdr_dab_audio` test can play them through IQ → FIC → MSC → codec → sink
without a hardware run.

Origin, generation commands and the measured codec agreement live in
`crates/neowon-codec/tests/fixtures/README.md` — one home per fact. In
short: ffmpeg 9.0.2 generated a deterministic 0.4 s two-tone stereo signal
(440 Hz left, 880 Hz right); `dabplus_heaacv2.sf` is three DAB+
`subchannel_index` 10 super frames carrying libfdk-aac HE-AAC v2 AUs with
`dabplus_heaacv2.asc` as the encoder's own AudioSpecificConfig, and
`tone.mp2` is 17 MPEG-1 Layer II frames at 128 kbit/s.

| File | Bytes | SHA-256 (copy) |
|---|---:|---|
| `dabplus_heaacv2.sf` | 3600 | `6d45cc4aa220b34ae69eed1d56f44fb37aff64ad6cead8c6af4560aa5ab8a4c3` |
| `dabplus_heaacv2.asc` | 4 | `e6abbe756c1313b7a04e8d68b8e62707f7e34e4cc431e276353c7623434bc6b1` |
| `tone.mp2` | 6528 | `75512b8eee3b41ab10da5891c9b45dc05eed16b9679ddc07e7b99acfdfc7e414` |

**The 1024-vs-960 honesty note, inherited:** DAB+ mandates the 960-line
transform (TS 102 563 clause 5.1) and no available open encoder emits it, so
the fixture's super frames are 1024-line and its own ASC says so. The app
carries this ASC as an explicit per-programme override in the sim scene
(`dab_scene::asc_override`); a real stream takes the standard-derived
`AudioSpecificConfig::for_dabplus(header)` and, on the default `oxideav-aac`
backend, surfaces `SbrUnsupportedFrameFamily` instead of decoding. The
override exists to prove the playback pipeline, not to claim the fixture is
a conformant DAB+ bitstream.
