//! Verification row 14: the committed HE-AAC v2 (LOAS) fixture decodes
//! through the adapter within the stated metric, and the DAB+-specific
//! 960-line limitation is surfaced.
//!
//! # The metric
//!
//! Encoder and decoder delays differ (Apple's MP4 carries priming; our
//! decode starts at frame 0), and two conforming HE-AAC decoders may
//! differ in the SBR/PS synthesis. So the test aligns first (normalised
//! cross-correlation, positive lag sweep to 8192 samples), then asserts:
//!
//! * peak normalised correlation ≥ 0.98 (measured 1.0000),
//! * relative RMS error at that lag ≤ 0.05 (measured ≤ 0.0006),
//! * the expected tone dominates its channel by ≥ 8×, and the tone
//!   amplitude is within 0.5–2× of the ffmpeg reference.
//!
//! # What this fixture does and does not prove
//!
//! The fixture is generic HE-AAC v2: Apple's encoder emits the
//! **1024**-line transform and signals SBR/PS in-band. DAB+ mandates the
//! **960**-line transform (TS 102 563 clause 5.1), and the pinned
//! `oxideav-aac` 0.1.7 rejects SBR on non-1024 families — the last test
//! here reproduces that error on a real SBR-bearing AU and records it.
//! The operator owns the fallback decision (DAB-G2).

mod common;

#[cfg(not(feature = "fdk-aac"))]
use neowon_codec::Error;
use neowon_codec::aac::{AacDecoder, AudioSpecificConfig, SbrSupport, demux_loas};
use neowon_codec::dabplus::{SuperframeDecoder, SuperframeEncoder, SuperframeHeader};

fn dabplus_header(sbr: bool, ps: bool) -> SuperframeHeader {
    SuperframeHeader {
        rfa: false,
        dac_rate: true, // 48 kHz DAC
        sbr_flag: sbr,
        aac_channel_mode: false, // mono core
        ps_flag: ps,
        mpeg_surround_config: 0,
    }
}

#[test]
fn fixture_asc_is_generic_he_aac_v2_1024() {
    let stream = demux_loas(&common::fixture("he_aac_v2.latm")).expect("demux");
    assert_eq!(stream.access_units.len(), 12, "0.4 s at 24 kHz core");
    let asc = stream.config;
    assert_eq!(asc.outer_aot, 2, "Apple signals SBR/PS in-band");
    assert_eq!(asc.aot, 2);
    assert_eq!(asc.core_sample_rate(), 24_000);
    assert_eq!(asc.channel_configuration, 1);
    assert!(
        !asc.frame_length_960,
        "the fixture is not a DAB+ 960 stream"
    );
    assert!(!asc.sbr_present && !asc.ps_present);

    // Our ASC writer round-trips through the pinned parser.
    let reparsed = AudioSpecificConfig::parse(&asc.to_bytes()).expect("parse");
    assert_eq!(reparsed, asc);
}

#[test]
fn he_aac_v2_fixture_decodes_within_the_stated_metric() {
    let stream = demux_loas(&common::fixture("he_aac_v2.latm")).expect("demux");
    let mut decoder = AacDecoder::new(stream.config).expect("decoder");
    let mut pcm = Vec::new();
    for (index, au) in stream.access_units.iter().enumerate() {
        let decoded = decoder
            .decode(au)
            .unwrap_or_else(|error| panic!("AU {index}: {error}"));
        // In-band SBR + PS: a mono core renders stereo at 48 kHz.
        assert_eq!(decoded.channels, 2, "AU {index}");
        assert_eq!(decoded.sample_rate, 48_000, "AU {index}");
        assert_eq!(decoded.sbr_support, SbrSupport::Ps, "AU {index}");
        pcm.extend_from_slice(&decoded.pcm);
    }

    let reference = common::s16_to_f32(&common::fixture("he_aac_v2_ref.s16"));
    let ours_channels = common::deinterleave(&pcm, 2);
    let reference_channels = common::deinterleave(&reference, 2);
    for (index, (ours, reference)) in ours_channels.iter().zip(&reference_channels).enumerate() {
        let expected = if index == 0 { 440.0 } else { 880.0 };
        let other = if index == 0 { 880.0 } else { 440.0 };

        let agreement = common::align_and_compare(ours, reference, 8192);
        assert!(
            agreement.correlation >= 0.98,
            "channel {index}: correlation {:.4} (lag {})",
            agreement.correlation,
            agreement.lag
        );
        assert!(
            agreement.relative_rms_error <= 0.05,
            "channel {index}: relative RMS error {:.4}",
            agreement.relative_rms_error
        );

        let tone = common::goertzel(ours, 48_000.0, expected);
        let leak = common::goertzel(ours, 48_000.0, other);
        assert!(
            tone > 8.0 * leak,
            "channel {index}: {expected} Hz should dominate (tone {tone:.5}, leak {leak:.5})"
        );
        let reference_tone = common::goertzel(reference, 48_000.0, expected);
        let amplitude_ratio = tone / reference_tone;
        assert!(
            (0.5..=2.0).contains(&amplitude_ratio),
            "channel {index}: tone amplitude ratio {amplitude_ratio:.3}"
        );
    }
}

#[test]
fn fixture_aus_survive_a_dabplus_superframe_round_trip() {
    // The AUs are not DAB+-shaped (1024-line), but the transport carries
    // bytes: packing them into superframes and back must be byte-exact,
    // and the unpacked AUs must decode to the same PCM.
    let stream = demux_loas(&common::fixture("he_aac_v2.latm")).expect("demux");
    let header = dabplus_header(true, false); // num_aus = 3
    assert_eq!(header.num_aus(), 3);
    let max_au = stream.access_units.iter().map(Vec::len).max().expect("AUs");
    let subchannel_index = (3 * (max_au + 2) + 8).div_ceil(110) as u8;
    assert!(subchannel_index <= 24, "sub-channel too small for the AUs");

    let encoder = SuperframeEncoder::new(subchannel_index).expect("encoder");
    let mut framed = Vec::new();
    for chunk in stream.access_units.chunks(3) {
        let refs: Vec<&[u8]> = chunk.iter().map(Vec::as_slice).collect();
        framed.push(encoder.encode(&header, &refs).expect("encode"));
    }

    let mut decoder = SuperframeDecoder::new(subchannel_index).expect("decoder");
    let mut decoded = Vec::new();
    for superframe in &framed {
        for chunk in superframe.chunks(13) {
            decoded.extend(decoder.push(chunk));
        }
    }
    assert_eq!(decoded.len(), framed.len());
    let mut unpacked = Vec::new();
    for superframe in &decoded {
        assert_eq!(superframe.header, Some(header));
        assert_eq!(superframe.rs_uncorrectable, 0);
        assert!(superframe.aus.iter().all(|au| au.crc_ok));
        for au in &superframe.aus {
            unpacked.push(au.data.clone());
        }
    }
    assert_eq!(unpacked.len(), stream.access_units.len());
    for (recovered, original) in unpacked.iter().zip(&stream.access_units) {
        assert!(recovered.starts_with(original), "AU bytes diverge");
    }

    // Direct decode and superframe-routed decode must agree exactly.
    let pcm_direct = decode_all(&stream.config, &stream.access_units);
    let pcm_routed = decode_all(&stream.config, &unpacked);
    assert_eq!(pcm_direct, pcm_routed);
}

fn decode_all(config: &AudioSpecificConfig, aus: &[Vec<u8>]) -> Vec<f32> {
    let mut decoder = AacDecoder::new(*config).expect("decoder");
    let mut pcm = Vec::new();
    for (index, au) in aus.iter().enumerate() {
        let decoded = decoder
            .decode(au)
            .unwrap_or_else(|error| panic!("AU {index}: {error}"));
        pcm.extend_from_slice(&decoded.pcm);
    }
    pcm
}

/// The DAB+ configuration boundary, default build: TS 102 563 clause
/// 5.1 mandates the 960 transform, HE-AAC v2 means SBR, and
/// `oxideav-aac` 0.1.7 cannot do SBR on 960. A real DAB+ AU therefore
/// surfaces [`Error::SbrUnsupportedFrameFamily`] — the typed boundary
/// that made libfdk-aac the `fdk-aac` feature (DAB-G2).
#[cfg(not(feature = "fdk-aac"))]
#[test]
fn dabplus_960_sbr_is_surfaced_not_silently_misdecoded() {
    assert_eq!(AacDecoder::BACKEND, "oxideav-aac");
    let asc = AudioSpecificConfig::for_dabplus(&dabplus_header(true, true));
    assert!(asc.frame_length_960);
    assert_eq!(asc.outer_aot, 29, "PS is signalled hierarchically");
    assert_eq!(asc.core_sample_rate(), 24_000);
    assert_eq!(asc.output_sample_rate(), 48_000);
    let reparsed = AudioSpecificConfig::parse(&asc.to_bytes()).expect("parse");
    assert_eq!(reparsed, asc);

    let mut decoder = AacDecoder::new(asc).expect("decoder");
    assert_eq!(decoder.sbr_support(), SbrSupport::Ps);
    let stream = demux_loas(&common::fixture("he_aac_v2.latm")).expect("demux");
    let error = decoder
        .decode(&stream.access_units[0])
        .expect_err("960 + SBR is the surfaced limitation");
    assert_eq!(error, Error::SbrUnsupportedFrameFamily);
}

/// The same boundary under the fallback: libfdk-aac accepts the DAB+
/// 960 + SBR + PS configuration outright.
///
/// The LOAS fixture's AUs are 1024-line, so they are *not* decoded here
/// under this config: libfdk-aac cannot see the transform mismatch and
/// conceals (silence) rather than erroring. The matching-config decode
/// of a real 960 stream is `tests/aac_960_sbr.rs`'s subject, with the
/// same caveat documented there.
#[cfg(feature = "fdk-aac")]
#[test]
fn dabplus_960_sbr_config_is_accepted_by_the_fdk_backend() {
    assert_eq!(AacDecoder::BACKEND, "fdk-aac");
    let asc = AudioSpecificConfig::for_dabplus(&dabplus_header(true, true));
    assert!(asc.frame_length_960);
    assert_eq!(asc.outer_aot, 29, "PS is signalled hierarchically");
    let reparsed = AudioSpecificConfig::parse(&asc.to_bytes()).expect("parse");
    assert_eq!(reparsed, asc);

    let decoder = AacDecoder::new(asc).expect("libfdk-aac configures 960 + SBR + PS");
    assert_eq!(decoder.sbr_support(), SbrSupport::Ps);
}

#[test]
fn dabplus_960_without_sbr_configures_cleanly() {
    // An SBR-less DAB+ stream (clause 5.2 Table 4) is plain AAC-LC at
    // 48 kHz with the 960 transform — a configuration the pinned codec
    // accepts.
    let asc = AudioSpecificConfig::for_dabplus(&SuperframeHeader {
        rfa: false,
        dac_rate: true,
        sbr_flag: false,
        aac_channel_mode: true,
        ps_flag: false,
        mpeg_surround_config: 0,
    });
    assert!(asc.frame_length_960);
    assert_eq!(asc.outer_aot, 2);
    assert_eq!(asc.core_sample_rate(), 48_000);
    assert_eq!(asc.sample_rate, asc.core_sample_rate());
    let reparsed = AudioSpecificConfig::parse(&asc.to_bytes()).expect("parse");
    assert_eq!(reparsed, asc);
    let decoder = AacDecoder::new(asc).expect("decoder");
    assert_eq!(decoder.sbr_support(), SbrSupport::None);
}
