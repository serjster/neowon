//! Verification row 15: the committed MPEG-1 Layer II fixture decodes
//! through the adapter within the stated metric, including the
//! `ancillary_data()` tail that carries DAB PAD.
//!
//! # The metric
//!
//! The two decoders share the ISO floating-point synthesis filterbank
//! contract, so the alignment is expected at (or very near) lag 0. The
//! test asserts: normalised correlation ≥ 0.999 (measured 0.999999990 L /
//! 0.999999989 R), relative RMS error ≤ 0.01 (measured ≤ 0.00015), the
//! expected tone dominating by ≥ 8×, and an amplitude ratio within 0.95–1.05.

mod common;

use neowon_codec::mp2::Mp2Decoder;

use oxideav_mp2::encoder_frame::{EncodeFrameState, encode_frame_auto_with_ancillary};
use oxideav_mp2::frame::{FrameDecodeState, decode_frame_with};

#[test]
fn mp2_fixture_decodes_within_the_stated_metric() {
    let bytes = common::fixture("tone.mp2");
    let mut decoder = Mp2Decoder::new();
    let frames = decoder.decode_all(&bytes).expect("decode");
    assert_eq!(frames.len(), 17, "0.4 s at 1152 samples per frame");
    assert_eq!(decoder.skipped_frames(), 0);

    let mut pcm = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.channels, 2, "frame {index}");
        assert_eq!(frame.sample_rate, 48_000, "frame {index}");
        assert_eq!(frame.pcm.len(), 2 * 1152, "frame {index}");
        // ffmpeg's encoder emits a zeroed ancillary tail here.
        assert!(
            frame.ancillary.iter().all(|&b| b == 0),
            "frame {index} unexpected ancillary payload"
        );
        pcm.extend_from_slice(&frame.pcm);
    }

    let reference = common::s16_to_f32(&common::fixture("tone_ref.s16"));
    let ours_channels = common::deinterleave(&pcm, 2);
    let reference_channels = common::deinterleave(&reference, 2);
    for (index, (ours, reference)) in ours_channels.iter().zip(&reference_channels).enumerate() {
        let expected = if index == 0 { 440.0 } else { 880.0 };
        let other = if index == 0 { 880.0 } else { 440.0 };

        let agreement = common::align_and_compare(ours, reference, 64);
        common::report_agreement("tone.mp2", index, &agreement);
        assert!(
            agreement.correlation >= 0.999,
            "channel {index}: correlation {:.6} (lag {})",
            agreement.correlation,
            agreement.lag
        );
        assert!(
            agreement.relative_rms_error <= 0.01,
            "channel {index}: relative RMS error {:.6}",
            agreement.relative_rms_error
        );
        assert!(
            agreement.lag <= 2,
            "channel {index}: MP2 alignment should be near zero, got {}",
            agreement.lag
        );

        let tone = common::goertzel(ours, 48_000.0, expected);
        let leak = common::goertzel(ours, 48_000.0, other);
        assert!(tone > 8.0 * leak, "channel {index}: tone dominance");
        let reference_tone = common::goertzel(reference, 48_000.0, expected);
        let amplitude_ratio = tone / reference_tone;
        assert!(
            (0.95..=1.05).contains(&amplitude_ratio),
            "channel {index}: tone amplitude ratio {amplitude_ratio:.4}"
        );
    }
}

#[test]
fn chunking_does_not_change_the_pcm() {
    let bytes = common::fixture("tone.mp2");
    let whole = Mp2Decoder::new().decode_all(&bytes).expect("whole");

    let mut chunked = Mp2Decoder::new();
    let mut frames = Vec::new();
    for chunk in bytes.chunks(7) {
        for frame in chunked.push(chunk) {
            frames.push(frame.expect("chunked frame"));
        }
    }
    assert_eq!(whole.len(), frames.len());
    for (a, b) in whole.iter().zip(&frames) {
        assert_eq!(a.pcm, b.pcm);
        assert_eq!(a.ancillary, b.ancillary);
    }
}

#[test]
fn decoder_resynchronises_after_leading_garbage() {
    let mut bytes = vec![0x00u8, 0x01, 0x02, 0x03, 0x04];
    bytes.extend_from_slice(&common::fixture("tone.mp2"));
    let frames = Mp2Decoder::new().decode_all(&bytes).expect("decode");
    assert_eq!(frames.len(), 17);
}

#[test]
fn ancillary_tail_round_trips_as_pad_carriage() {
    // Encode one legitimate Layer II frame carrying an ancillary payload
    // (this is where DAB classic places the PAD field, EN 300 401
    // clause 7.4.0) and read it back through the adapter.
    let fixture = common::fixture("tone.mp2");
    let mut state = FrameDecodeState::new();
    let decoded = decode_frame_with(&fixture, &mut state).expect("fixture frame");
    let ancillary = [0x12u8, 0x00, 0x02, 0x44, 0x4C, 0x53];

    let mut encoder_state = EncodeFrameState::new();
    let encoded = encode_frame_auto_with_ancillary(
        &decoded.header,
        &decoded.pcm,
        (ancillary.len() * 8) as u32,
        &ancillary,
        &mut encoder_state,
    )
    .expect("encode with ancillary");

    let frames = Mp2Decoder::new().decode_all(&encoded).expect("decode");
    assert_eq!(frames.len(), 1);
    // §2.4.1.8 surfaces the whole tail (payload plus zero fill to the
    // frame end), which is what the DAB PAD parser needs: the F-PAD is
    // the last two bytes of the frame, not of the payload.
    let tail = &frames[0].ancillary;
    assert!(tail.starts_with(&ancillary), "payload lost: {tail:?}");
    assert!(
        tail[ancillary.len()..].iter().all(|&b| b == 0),
        "unexpected non-zero fill: {tail:?}"
    );
}
