//! The `rf-dab` scene's loop-seam regression: replaying the buffer must keep
//! every recovered AU and MP2 frame clean.

use super::*;
use neowon_codec::mp2::Mp2Decoder;
use neowon_dsp::dab::receiver::DabReceiver;
use std::collections::BTreeMap;

/// The scene loops, so the chosen streams are replayed by the receiver:
/// every recovered DAB+ AU must be CRC-clean and every MP2 frame must
/// decode, across several loops. This guards the clause-12 warm-up
/// transient (`WARMUP_FRAMES`): replaying zero-history frames corrupts one
/// super frame per loop.
#[test]
fn the_looped_scene_recovers_only_clean_aus() {
    let samples = iq_samples();
    let mut rx = DabReceiver::new();
    for _ in 0..3 {
        for chunk in samples.chunks(4096) {
            rx.push_iq(chunk);
        }
    }
    let mut streams: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    for frame in rx.take_msc_frames() {
        streams
            .entry(frame.sub_channel)
            .or_default()
            .extend_from_slice(&frame.bytes);
    }
    let mut sync = crate::sdr::dab_audio::DabPlusSync::new(10).expect("index 10");
    let mut aus = 0usize;
    for superframe in sync.push(streams.get(&DABPLUS_SUB_CHANNEL).expect("sub-channel 3")) {
        assert!(superframe.firecode_ok);
        for au in &superframe.aus {
            aus += 1;
            assert!(au.crc_ok, "AU {aus} of the looped stream failed its CRC");
        }
    }
    assert!(aus >= 60, "only {aus} AUs over three loops");

    let mut decoder = Mp2Decoder::new();
    let frames = decoder
        .decode_all(streams.get(&MP2_SUB_CHANNEL).expect("sub-channel 4"))
        .expect("MP2 decodes");
    assert_eq!(decoder.skipped_frames(), 0, "the looped MP2 needed resync");
    assert!(frames.len() >= 100, "only {} MP2 frames", frames.len());
}
