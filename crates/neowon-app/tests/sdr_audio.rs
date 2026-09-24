//! The audio demodulator over the control socket on the
//! simulator. Audible sound cannot be asserted; the DSP oracle
//! (`neowon-dsp --test demod_golden`) covers demodulation, and this asserts
//! the state machine and that audio is actually produced on a real output
//! device (rms > 0).
//!
//!   cargo test -p neowon-app --test sdr_audio -- --ignored

mod common;
use common::*;

#[test]
#[ignore = "opens a window and plays audio"]
fn audio_states_and_demod_over_the_socket() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        // A 1 kHz tone on a 0.5 FS AM carrier at 100.1 MHz (100 kHz above the
        // default centre).
        c.ok("stimulus rf-am");
        c.ok("sdr tune 100.1M");
        c.ok("sdr width 10k");
        c.ok("sdr demod am");
        let a = c.wait("get audio", 20, |r| {
            r.contains(r#""state":"playing""#) && field(r, "rms") > 0.0
        });
        assert!(a.contains(r#""demod":"am""#), "{a}");

        // Mute is a named state, not silence in the dark.
        c.ok("sdr mute on");
        c.wait("get audio", 5, |r| r.contains(r#""state":"muted""#));
        c.ok("sdr mute off");
        c.wait("get audio", 5, |r| r.contains(r#""state":"playing""#));

        // A squelch above the channel power holds it.
        c.ok("sdr squelch 0");
        c.wait("get audio", 5, |r| r.contains(r#""state":"squelched""#));
        c.ok("sdr squelch off");
        c.wait("get audio", 5, |r| r.contains(r#""state":"playing""#));

        // Refusals: an out-of-range volume lands on the status line; an
        // unknown mode is refused at parse time.
        c.ok("sdr volume 2");
        c.wait("get status", 5, |r| r.contains("volume"));
        let bad = c.request("sdr demod ssb");
        assert!(bad.contains("unknown demod"), "{bad}");

        // Off is a state of its own.
        c.ok("sdr demod off");
        c.wait("get audio", 5, |r| r.contains(r#""state":"off""#));
    });
}
