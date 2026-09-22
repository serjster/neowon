//! Phase 10.15.3: DAB audio playback over the control socket, on the sim.
//!
//! The `rf-dab` scene carries two real audio programmes (see
//! `src/sdr/dab_scene.rs`): a DAB+ sub-channel fed by the committed
//! libfdk-aac HE-AAC v2 fixture through its own AudioSpecificConfig, and a
//! DAB classic sub-channel fed by the committed `tone.mp2`. This test plays
//! both, asserts the playback worker's own readout (`get dab.audio`:
//! `playing`, the compiled backend, 48 kHz stereo, a non-zero peak), then
//! stops and asserts the transport is `off` with no peak. Playing a
//! sub-channel that is not a codec stream must surface an `error` with its
//! typed reason, never silence.
//!
//! **What this test does *not* re-derive:** that the decoded PCM matches the
//! source tone. That is asserted at the codec level, where the reference PCM
//! is available: `neowon-codec --test dabplus` (transport) and
//! `aac_960_sbr` / `mp2_fixture` (HE-AAC v2 correlation ≥ 0.99, MP2
//! correlation ≥ 0.999 against the source tone). Here the sink is behind a
//! socket, so only the transport state and the presence of audio are asserted.
//!
//!   cargo test -p neowon-app --test sdr_dab_audio -- --ignored

mod common;
use common::*;

use neowon_codec::aac::AacDecoder;

/// From the scene (`dab_scene.rs`, the stable sim layout).
const DABPLUS_SID: u16 = 0x1004;
const MP2_SID: u16 = 0x1005;
/// `NEOWON ONE`'s sub-channel is 192 CU (DAB+ index 32, outside 1..=24) and
/// `NEOWON TWO`'s is the encoder's seeded filler, not a codec stream: those
/// are the "cannot decode this" cases the readout must surface.
const TOO_BIG_SID: u16 = 0x1001;
const FILLER_SID: u16 = 0x1002;

fn select(c: &mut Conn, sid: u16) {
    c.ok(&format!("sdr dab service {sid}"));
    c.wait("get dab", 5, |r| raw(r, "service") == sid.to_string());
}

#[test]
#[ignore = "opens a window and plays audio"]
fn dab_audio_plays_both_codings_and_surfaces_errors() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        c.ok("stimulus rf-dab");
        c.ok("sdr dab on");
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));

        // DAB+: the worker decodes the fixture's HE-AAC v2 super frames; the
        // backend is whichever this build links (DAB-G2's feature flip).
        select(&mut c, DABPLUS_SID);
        c.ok("sdr dab play");
        // The first decoded blocks are the codec's priming frames, which
        // report the core (mono) layout before Parametric Stereo engages:
        // wait for the stereo output the fixture encodes, not the first play.
        let dab = c.wait("get dab", 30, |r| {
            r.contains(r#""state":"playing""#)
                && field(r, "rate") == 48_000.0
                && field(r, "channels") == 2.0
                && field(r, "peak") > 0.0
        });
        assert_eq!(
            raw(&dab, "backend"),
            format!("{:?}", AacDecoder::BACKEND),
            "DAB+ backend: {dab}"
        );
        assert_eq!(field(&dab, "rate"), 48_000.0, "{dab}");
        assert_eq!(field(&dab, "channels"), 2.0, "{dab}");
        assert_eq!(raw(&dab, "state"), "\"playing\"", "{dab}");
        // The block counter only moves forward, and the peak tracks the
        // fixture's tone (the first AUs are the codec's silent warm-up).
        let blocks = field(&dab, "blocks");
        c.wait("get dab", 10, |r| field(r, "blocks") > blocks);

        // MP2: selecting a service while playing switches the stream. The
        // backend is the MP2 adapter, never the AAC one.
        select(&mut c, MP2_SID);
        let dab = c.wait("get dab", 30, |r| {
            r.contains(r#""backend":"oxideav-mp2""#)
                && r.contains(r#""state":"playing""#)
                && field(r, "peak") > 0.0
        });
        assert_eq!(field(&dab, "rate"), 48_000.0, "{dab}");
        assert_eq!(field(&dab, "channels"), 2.0, "{dab}");

        // Stop: the worker is gone with its decoders and the sink is
        // cleared, so the readout settles on `off` with no peak left to
        // advance.
        c.ok("sdr dab stop");
        let off = c.wait("get dab", 5, |r| r.contains(r#""audio":{"state":"off"}"#));
        assert!(!off.contains(r#""peak""#), "stopped readout: {off}");

        // A sub-channel that carries no codec stream is an error with a
        // typed reason, not a silent `playing`.
        select(&mut c, FILLER_SID);
        c.ok("sdr dab play");
        let dab = c.wait("get dab", 20, |r| r.contains(r#""state":"error""#));
        assert!(
            dab.contains("no valid DAB+ super frame"),
            "typed reason missing: {dab}"
        );
        c.ok("sdr dab stop");
        c.wait("get dab", 5, |r| r.contains(r#""audio":{"state":"off"}"#));

        // A DAB+ sub-channel too large for the transport (index 32) is
        // refused at the action with the same limit, not started and left
        // to fail silently.
        select(&mut c, TOO_BIG_SID);
        c.ok("sdr dab play");
        c.wait("get status", 5, |r| r.contains("outside DAB+"));
    });
}
