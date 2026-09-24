//! The `rf-dab` scene's oracle tests: fixture copies, lock, table, DLS and
//! both audio programmes round-trip, and the DAB+ tone decodes.

use super::*;
use neowon_codec::aac::{AacDecoder, AudioSpecificConfig};
use neowon_codec::dabplus::{SuperframeDecoder, SuperframeHeader};
use neowon_codec::mp2::Mp2Decoder;
use neowon_dsp::dab::pad::PadParser;
use neowon_dsp::dab::receiver::DabReceiver;
use std::collections::BTreeMap;

/// Single-bin Goertzel magnitude (normalised by length), as the codec
/// acceptance tests use for the same two-tone fixture.
fn goertzel(samples: &[f32], sample_rate: f64, frequency: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0f64;
    let mut previous2 = 0.0f64;
    for &sample in samples {
        let current = f64::from(sample) + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    let power = previous * previous + previous2 * previous2 - coefficient * previous * previous2;
    power.max(0.0).sqrt() / samples.len() as f64
}

/// The README's byte-identical-copy claim, tested: the three fixture
/// files this scene embeds with `include_bytes!` must match the
/// `neowon-codec` acceptance fixtures byte for byte. Regenerating one copy
/// without the other fails here instead of silently playing an old stream.
#[test]
fn the_embedded_audio_fixtures_match_the_codec_originals() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../neowon-codec/tests/fixtures");
    for (name, embedded) in [
        ("dabplus_heaacv2.sf", DABPLUS_SF),
        ("dabplus_heaacv2.asc", DABPLUS_ASC),
        ("tone.mp2", TONE_MP2),
    ] {
        let original =
            std::fs::read(dir.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            embedded == original.as_slice(),
            "fixture drift: tests/fixtures/{name} differs from \
             neowon-codec/tests/fixtures/{name}"
        );
    }
}

/// The whole path headless — the composed `rf-dab` IQ through the
/// FIC and MSC, the DAB+ transport and the HE-AAC v2 adapter to PCM — with
/// the fixture's known source tone asserted in that PCM's spectrum. The
/// windowed `sdr_dab_audio` proves playback state over the socket; this
/// proves the audio actually decodes to the tone the fixture encodes, in
/// the default suite, with no window, socket or sink.
#[test]
fn the_rf_dab_scene_decodes_to_the_known_tone() {
    let samples = iq_samples();
    let mut rx = DabReceiver::new();
    for chunk in samples.chunks(4096) {
        rx.push_iq(chunk);
    }
    assert!(rx.status().locked, "the scene must lock first");

    let mut stream = Vec::new();
    for frame in rx.take_msc_frames() {
        if frame.sub_channel == DABPLUS_SUB_CHANNEL {
            stream.extend_from_slice(&frame.bytes);
        }
    }

    // The scene's DAB+ programme uses the fixture's own ASC (the
    // standard-derived one asks for the 960 transform the fixture does
    // not have — `tests/fixtures/README.md`).
    let config = AudioSpecificConfig::parse(DABPLUS_ASC).expect("the fixture ASC parses");
    let mut decoder = AacDecoder::new(config).expect("decoder");
    let mut sync = crate::sdr::dab_audio::DabPlusSync::new(10).expect("index 10");
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut stereo_aus = 0usize;
    for superframe in sync.push(&stream) {
        assert!(superframe.firecode_ok);
        for au in &superframe.aus {
            assert!(au.crc_ok, "the scene's AUs are CRC-clean");
            let decoded = decoder.decode(&au.data).expect("AU decodes");
            // The codec's priming blocks report the mono core before
            // Parametric Stereo engages; only the stereo blocks carry the
            // separated 440 Hz left / 880 Hz right fixture tone.
            if decoded.channels == 2 {
                stereo_aus += 1;
                for pair in decoded.pcm.chunks_exact(2) {
                    left.push(pair[0]);
                    right.push(pair[1]);
                }
            }
        }
    }
    assert!(stereo_aus >= 12, "only {stereo_aus} stereo AUs decoded");

    let (l_tone, l_leak) = (
        goertzel(&left, 48_000.0, 440.0),
        goertzel(&left, 48_000.0, 880.0),
    );
    assert!(
        l_tone > 8.0 * l_leak,
        "left: 440 Hz {l_tone:.5} should dominate 880 Hz {l_leak:.5}"
    );
    let (r_leak, r_tone) = (
        goertzel(&right, 48_000.0, 440.0),
        goertzel(&right, 48_000.0, 880.0),
    );
    assert!(
        r_tone > 8.0 * r_leak,
        "right: 880 Hz {r_tone:.5} should dominate 440 Hz {r_leak:.5}"
    );
}

/// The oracle claim, end to end: the composed IQ locks the receiver,
/// the table names the five services with both protections, the DLS
/// carrier's labels — the multi-frame one included — come back exactly
/// as composed, and the two audio programmes' streams reach the playback
/// transports as the committed fixtures (cyclically: the scene loops).
#[test]
fn the_ensemble_locks_and_its_programmes_round_trip() {
    let samples = iq_samples();
    let mut rx = DabReceiver::new();
    for chunk in samples.chunks(4096) {
        rx.push_iq(chunk);
    }
    let status = rx.status();
    assert!(status.locked, "the FIC should lock: {status:?}");
    assert_eq!(status.ensemble.eid, Some(EID));
    assert_eq!(status.ensemble.label.as_deref(), Some(ENSEMBLE_LABEL));
    assert_eq!(status.ensemble.services.len(), 5);
    assert!(
        status
            .ensemble
            .sub_channels
            .values()
            .any(|s| matches!(s.protection, Protection::Eep { .. }))
    );
    assert!(
        status
            .ensemble
            .sub_channels
            .values()
            .any(|s| matches!(s.protection, Protection::Uep { .. }))
    );

    let mut parsers: BTreeMap<u8, PadParser> = BTreeMap::new();
    let mut streams: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();
    for frame in rx.take_msc_frames() {
        streams
            .entry(frame.sub_channel)
            .or_default()
            .extend_from_slice(&frame.bytes);
        let parser = parsers.entry(frame.sub_channel).or_default();
        if let Some(dls) = parser.push_pad_region(&frame.bytes) {
            seen.push(dls);
        }
    }
    for text in DLS_TEXTS {
        assert!(
            seen.iter().any(|s| s == text),
            "{text:?} never decoded: {seen:?}"
        );
    }

    // DAB+ (sub-channel 3): the app's own trial-sync finds the
    // super-frame phase, and the AUs are the fixture's, cyclically.
    let dabplus = streams
        .get(&DABPLUS_SUB_CHANNEL)
        .expect("the DAB+ stream was emitted");
    let mut sync = crate::sdr::dab_audio::DabPlusSync::new(10).expect("index 10");
    let header = SuperframeHeader {
        rfa: false,
        dac_rate: true,
        sbr_flag: true,
        aac_channel_mode: false,
        ps_flag: true,
        mpeg_surround_config: 0,
    };
    let mut recovered: Vec<Vec<u8>> = Vec::new();
    for superframe in sync.push(dabplus) {
        assert!(superframe.firecode_ok);
        assert_eq!(superframe.rs_uncorrectable, 0);
        assert_eq!(superframe.header, Some(header));
        for au in &superframe.aus {
            assert!(au.crc_ok, "AU CRC");
            recovered.push(au.data.clone());
        }
    }
    assert!(
        recovered.len() >= 6,
        "only {} AUs recovered",
        recovered.len()
    );
    let fixture = fixture_access_units();
    let start = fixture
        .iter()
        .position(|au| *au == recovered[0])
        .expect("the first recovered AU is a fixture AU");
    for (i, au) in recovered.iter().enumerate() {
        assert_eq!(*au, fixture[(start + i) % fixture.len()], "AU {i}");
    }

    // MP2 (sub-channel 4): one 384-byte frame per logical frame, so the
    // emitted stream decodes frame by frame with no resync.
    let mp2 = streams
        .get(&MP2_SUB_CHANNEL)
        .expect("the MP2 stream was emitted");
    let mut decoder = Mp2Decoder::new();
    let frames = decoder.decode_all(mp2).expect("MP2 decodes");
    assert!(frames.len() >= 24, "only {} MP2 frames", frames.len());
    assert_eq!(decoder.skipped_frames(), 0, "MP2 needed no resync");
    assert!(
        frames
            .iter()
            .all(|f| f.channels == 2 && f.sample_rate == 48_000)
    );
}

/// The fixture's AUs as the transport frames them, for the cyclic match.
fn fixture_access_units() -> Vec<Vec<u8>> {
    let mut decoder = SuperframeDecoder::new(10).expect("index 10");
    decoder
        .push(DABPLUS_SF)
        .into_iter()
        .flat_map(|sf| sf.aus.into_iter().map(|au| au.data))
        .collect()
}
