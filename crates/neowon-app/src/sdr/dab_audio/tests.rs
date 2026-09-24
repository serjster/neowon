//! Worker-level tests for the playback stream: what reaches `error`, and
//! what must not. They drive the real worker thread, so they see the
//! transport's own gate decisions.

use super::*;

/// A ragged start — several super frames of junk before the stream proper —
/// is not a verdict on the stream: a receiver still acquiring must not trip
/// the gate and leave playback unable to recover.
#[test]
fn worker_survives_a_ragged_start() {
    let fixture: &[u8] = include_bytes!("../../../tests/fixtures/dabplus_heaacv2.sf");
    let spec = StreamSpec {
        coding: Coding::DabPlus,
        sub_channel: 1,
        // The fixture's shape: 80 kbit/s useful, EEP 3-A.
        subchannel_index: 10,
        asc_override: None,
    };
    let audio = DabAudio::start(0x1003, spec, 48_000.0);
    // Six super frames of junk in logical-frame-sized chunks, then the real
    // stream: a ragged start of 720 ms, not a wrong sub-channel.
    for _ in 0..30 {
        audio.push(1, &[0xA5u8; 240]);
    }
    audio.push(1, fixture);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = audio.status();
        // `decoded` is the verdict here, not `state`: the fixture is three
        // super frames, less than the start-up cushion the feed holds back
        // (`decode::PRIME_SECONDS`), so a stream this short decodes without
        // ever reaching `playing`. What must not happen is the ragged start
        // being read as "not DAB+".
        if status.decoded > 0 || status.state == AudioState::Error {
            assert!(
                !status.reason.contains("no valid DAB+ super frame"),
                "a ragged start was read as not DAB+: {:?}",
                status.reason
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker decoded nothing from a ragged start: {status:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A sub-channel that is not the DAB+ shape it claims must reach `error`
/// with a reason rather than staying silent, and the status must be
/// readable while it does. This is the no-window half of the windowed
/// `sdr_dab_audio` case (the seeded fillers in the sim scene).
#[test]
fn worker_surfaces_a_stream_with_no_superframe_sync() {
    let spec = StreamSpec {
        coding: Coding::DabPlus,
        sub_channel: 1,
        // `NEOWON TWO`'s shape: 128 kbit/s useful, but the bytes are the
        // encoder's filler, not super frames.
        subchannel_index: 16,
        asc_override: None,
    };
    let audio = DabAudio::start(0x1002, spec, 48_000.0);
    for _ in 0..400 {
        audio.push(1, &[0xA5u8; 384]);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = audio.status();
        if status.state == AudioState::Error {
            assert!(
                status.reason.contains("no valid DAB+ super frame"),
                "{:?}",
                status.reason
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker never errored: {status:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(audio.status().decoded, 0, "nothing should have decoded");
}
