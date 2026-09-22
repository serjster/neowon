# Acceptance fixtures — Phase 10.15.3 (neowon-codec)

All audio here is **our own synthetic two-tone signal**: a 440 Hz sine in
the left channel and an 880 Hz sine in the right channel, generated with
ffmpeg's `lavfi` sources. No third-party content is committed.

The files were generated on 2026-09-20 on this machine with:

* `ffmpeg version 9.0.2` (Apple clang build)
* `afconvert` (Audio File Convert **Version 2.0**, macOS)

## HE-AAC v2 (DAB-G2 primary path)

```sh
# 1. deterministic 0.4 s stereo tone (L = 440 Hz, R = 880 Hz)
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.4" \
       -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=0.4" \
       -filter_complex "[0:a][1:a]join=inputs=2:channel_layout=stereo[a]" \
       -map "[a]" -c:a pcm_s16le tones.wav

# 2. Apple HE-AAC v2 (core 1 ch @ 24 kHz, SBR + PS, 48 kHz stereo out)
afconvert -f m4af -d aacp -b 32000 tones.wav tones.m4a

# 3. LOAS/LATM carriage with the AudioSpecificConfig in the StreamMuxConfig
ffmpeg -i tones.m4a -c copy -f latm he_aac_v2.latm

# 4. independent reference PCM (ffmpeg's HE-AAC decoder, MP4 edit list honoured)
ffmpeg -i tones.m4a -f s16le -ac 2 -ar 48000 he_aac_v2_ref.s16
```

`ffprobe he_aac_v2.latm` reports `aac_latm (HE-AACv2), 48000 Hz, stereo`.
Its `StreamMuxConfig` ASC is `audioObjectType = 2` (AAC-LC), core 24 kHz,
mono, `frameLengthFlag = 0` (the **1024**-line family — Apple's encoder
does not emit the 960-line transform DAB+ mandates); SBR and PS are
signalled in-band in the FIL extension payloads. The acceptance test
documents exactly what that does and does not prove.

## MPEG-1 Layer II (DAB classic)

```sh
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.4" \
       -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=0.4" \
       -filter_complex "[0:a][1:a]join=inputs=2:channel_layout=stereo[a]" \
       -map "[a]" -c:a mp2 -b:a 128k tone.mp2
ffmpeg -i tone.mp2 -f s16le -ac 2 -ar 48000 tone_ref.s16
```

## DAB+-framed HE-AAC v2 (the `fdk-aac` fallback path)

The `he_aac_v2.latm` fixture above is Apple-encoded generic HE-AAC v2
(1024-line, in-band SBR/PS) and needs no C dependency. This second
fixture exercises the whole DAB+ chain — super frame framing, RS,
per-AU CRC, in-band PAD carriage and the fdk adapter — with audio
encoded by libfdk-aac, the backend that decodes what DAB+ actually
carries.

```sh
# 1. the same deterministic 0.4 s stereo tone as a raw source (this is
#    the reference the decode is compared against, not a decode):
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.4" \
       -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=0.4" \
       -filter_complex "[0:a][1:a]join=inputs=2:channel_layout=stereo[a]" \
       -map "[a]" -f s16le -ac 2 -ar 48000 dabplus_heaacv2_ref.s16

# 2. encode + frame + write the fixture (libfdk-aac's own encoder via
#    the fdk-aac crate; rewrites the three files committed here):
cargo test -p neowon-codec --features fdk-aac \
  --test gen_dabplus_fixture -- --ignored --nocapture
```

The generator (see `tests/gen_dabplus_fixture.rs`) encodes with
`fdk_aac::enc` as `Mpeg4HeAacV2` (AOT 29), 48 kHz input, 64 kbit/s CBR,
`Transport::Raw`, and frames the resulting `raw_data_block()` AUs three
per super frame with `neowon_codec::dabplus::SuperframeEncoder`
(`subchannel_index` 10, i.e. 1 200-byte protected super frames). The
super frame header it writes is the stream's parameters: 48 kHz DAC,
SBR on, mono core, PS on. `dabplus_heaacv2.asc` is the encoder's own
`AudioSpecificConfig` (`EB 09 88 00`).

**What this proves:** the fdk backend decodes libfdk-aac HE-AAC v2 from
DAB+ super frames to 48 kHz stereo against the *source* tone within the
fixture metric (measured correlation 0.9999, relative RMS 0.020 L /
0.013 R), with PS producing stereo from the mono core.

**What it does not prove:** the 960-line transform TS 102 563 clause 5.1
mandates. libfdk-aac's encoder accepts `AACENC_GRANULE_LENGTH`
1024/512/480/256/240/128/120 only — there is no 960 — so this fixture is
1024-line and its encoder ASC says so. The standard-derived DAB+ ASC
(`frame_length_960 == true`) is therefore asserted in
`tests/aac_fixture.rs` at the *configuration* level for libfdk-aac, and
in `tests/aac_960_sbr.rs` only as the default backend's typed
`SbrUnsupportedFrameFamily` boundary.

## Files

| File | Bytes | What |
|---|---:|---|
| `he_aac_v2.latm` | 1302 | 12 HE-AAC v2 AUs in LOAS (48 kHz stereo) |
| `he_aac_v2_ref.s16` | 76800 | ffmpeg decode of the m4a, interleaved s16le 48 kHz stereo |
| `tone.mp2` | 6528 | 17 MPEG-1 Layer II frames @ 128 kbit/s, 48 kHz stereo |
| `tone_ref.s16` | 78336 | ffmpeg decode of `tone.mp2`, interleaved s16le 48 kHz stereo |
| `dabplus_heaacv2.sf` | 3600 | 3 DAB+ super frames (`subchannel_index` 10) carrying 9 libfdk-aac HE-AAC v2 AUs |
| `dabplus_heaacv2.asc` | 4 | the encoder's own AudioSpecificConfig: `EB 09 88 00` (AOT 29, 24 kHz core, 1024, SBR + PS) |
| `dabplus_heaacv2_ref.s16` | 76800 | the ffmpeg source tone the encoder was fed, interleaved s16le 48 kHz stereo |

Total ≈ 240 KB, well under the ~300 KB budget. Reference PCM is s16
rather than f32 to keep the set small.

SHA-256:

```
450168fb91981d13ab187b6264f060e7a5503153f3afad8ce4c60881c98a84df  dabplus_heaacv2_ref.s16
6d45cc4aa220b34ae69eed1d56f44fb37aff64ad6cead8c6af4560aa5ab8a4c3  dabplus_heaacv2.sf
e6abbe756c1313b7a04e8d68b8e62707f7e34e4cc431e276353c7623434bc6b1  dabplus_heaacv2.asc
bbb0e99418195002045c7595fde7ad26060b0b54c290da0c3b0484eb91858ba0  he_aac_v2_ref.s16
6376740aefb9bf545cfdda687b4355dad84e721c32ba4179eec750764d1663de  he_aac_v2.latm
b5f92bdbc18892c4a0d70a7c6578e6ee410f0a4d5f0cbf14d344647a22cab3e3  tone_ref.s16
75512b8eee3b41ab10da5891c9b45dc05eed16b9679ddc07e7b99acfdfc7e414  tone.mp2
```
