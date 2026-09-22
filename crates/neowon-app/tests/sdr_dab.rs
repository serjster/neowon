//! Phase 10.15.2/10.15.4: the `rf-dab` sim scene end to end, over the
//! control socket. The app composes a real Mode I ensemble (three services,
//! EEP and UEP sub-channels, one carrying a DLS PAD stream), installs it as
//! the `rf-dab` scene, and the SDR path decodes it: FIC → MSC → PAD parser →
//! `get dab`. Nothing here is recorded: the expected EId, labels and DLS
//! strings come from the scene definition in `src/sdr/dab_scene.rs`, and the
//! assertions are bounded polls, never wall-clock sleeps.
//!
//! Needs a window, so `#[ignore]` by default:
//!   cargo test -p neowon-app --test sdr_dab -- --ignored

mod common;
use common::*;

/// The multi-frame label: its first segment's data group spans two logical
/// frames (clause 7.4.5.2), so seeing it proves the reassembly path.
const MULTI_FRAME_DLS: &str = "Now playing: Café - Perfect Day";

/// `frames` of one sub-channel's MSC counters, or 0 when absent.
fn msc_frames(dab: &str, sub: u8) -> u64 {
    for item in items(dab, "sub_channel") {
        // `items` splits on `{"sub_channel":`, so the item opens with the id.
        let id = item[..item.find(',').unwrap_or(item.len())].trim();
        if id.parse::<u8>() == Ok(sub) {
            return raw(item, "frames").parse().unwrap_or(0);
        }
    }
    0
}

#[test]
#[ignore = "opens a window"]
fn dab_sim_scene_locks_selects_and_carries_dls() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        c.ok("stimulus rf-dab");
        c.ok("sdr dab on");

        // Lock, identity and the full service table.
        let dab = c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
        assert!(dab.contains(r#""eid_hex":"1046""#), "{dab}");
        assert!(dab.contains(r#""label":"NEOWON SIM""#), "{dab}");
        let services = items(&dab, "sid");
        assert_eq!(services.len(), 5, "{dab}");
        for label in [
            "NEOWON ONE",
            "NEOWON TWO",
            "NEOWON THREE",
            "NEOWON DAB+",
            "NEOWON MP2",
        ] {
            assert!(dab.contains(label), "{label} missing: {dab}");
        }
        // Both protections are in the table: EEP 3-A and UEP index 35.
        assert!(dab.contains("EEP 3-A"), "{dab}");
        assert!(dab.contains("UEP index 35"), "{dab}");

        // The MSC decodes: the DLS carrier's logical frames arrive.
        let dab = c.wait("get dab", 15, |r| msc_frames(r, 0) > 0);
        assert!(msc_frames(&dab, 0) > 0, "{dab}");
        assert!(msc_frames(&dab, 1) > 0, "the UEP sub-channel: {dab}");

        // Transport before a selection is refused, and says why.
        c.ok("sdr dab play");
        c.wait("get status", 5, |r| r.contains("no service selected"));
        c.wait("get dab", 5, |r| r.contains(r#""audio":{"state":"off"}"#));

        // Select by 1-based position, then by SId (decimal and hex), then
        // back to the DLS carrier.
        c.ok("sdr dab service #2");
        c.wait("get dab", 5, |r| raw(r, "service") == "4098");
        c.ok("sdr dab service 0x1003");
        c.wait("get dab", 5, |r| raw(r, "service") == "4099");
        c.ok("sdr dab service #9");
        c.wait("get status", 5, |r| r.contains("outside 1..=5"));
        c.ok("sdr dab service 0x1001");
        c.wait("get dab", 5, |r| raw(r, "service") == "4097");

        // The DLS text arrives complete, multi-frame label included.
        let dab = c.wait("get dab", 30, |r| r.contains(MULTI_FRAME_DLS));
        assert!(dab.contains(r#""dls":"#), "{dab}");

        // Transport: the DAB+ programme's audio actually decodes (10.15.3).
        // The DLS carrier above is not an audio stream — `sdr_dab_audio`
        // covers both codings and the typed error a non-stream produces.
        c.ok("sdr dab service 0x1004");
        c.wait("get dab", 5, |r| raw(r, "service") == "4100");
        c.ok("sdr dab play");
        c.wait("get dab", 30, |r| r.contains(r#""state":"playing""#));
        c.ok("sdr dab stop");
        c.wait("get dab", 5, |r| r.contains(r#""audio":{"state":"off"}"#));

        // The dock draws the section and the DLS line (uitree is the DOM).
        // The exact text is asserted through `get dab` above; here the node's
        // presence proves the UI is wired to the same state (the label cycles
        // every few frames, so pinning its text would race the stream).
        let tree = c.wait("get uitree", 10, |r| r.contains("dock section DAB"));
        assert!(tree.contains("dab dls"), "{tree}");
        // The wheel honours a pin while the receiver runs (sdr::zoom), and
        // the dock states it instead of letting the gesture look broken.
        assert!(tree.contains("rate pinned to 2.048 MS/s"), "{tree}");

        // Reset forgets the label and the parsers with the table (D27).
        c.ok("sdr dab reset");
        c.wait("get dab", 5, |r| {
            r.contains(r#""dls":null"#) && r.contains(r#""locked":false"#)
        });

        // And the receiver re-locks from the same buffer.
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
    });
}

/// The operator's question — "do I need to know the DAB centre frequency by
/// heart?" — answered over the control socket: `sdr dab channel 11C` lands
/// the hardware exactly on the block, `next`/`prev` step the raster, and
/// `get dab` reports the block with the plan's allocation status even while
/// decoding is off. The dock draws the same fact (the UI tree is the DOM).
#[test]
#[ignore = "opens a window"]
fn dab_channel_selects_a_band_iii_block_over_the_control_socket() {
    // A fresh HOME: the shipped `general` plan is active and no user plan
    // can shadow it, so "no DAB allocation here" is deterministic.
    let home = std::env::temp_dir().join("neowon-sdr-dab-channel-home");
    std::fs::create_dir_all(&home).unwrap();
    let home = home.to_str().unwrap().to_string();
    let (child, mut c) = launch(&["--sdr-sim"], &[("HOME", &home)]);
    with_app(child, || {
        // The block is a fact about the hardware window, not the receiver:
        // it is reported before Decode is ever pressed. Off the raster at
        // the default 100 MHz centre, so `channel` is null.
        let dab = c.wait("get dab", 5, |r| r.contains(r#""on":false"#));
        assert!(dab.contains(r#""channel":null"#), "{dab}");

        c.ok("sdr dab channel 11C");
        c.wait("get status", 5, |r| r.contains("DAB 11C 220.352 MHz"));
        let sdr = c.wait("get sdr", 5, |r| field(r, "centre_hz") == 220_352_000.0);
        assert_eq!(field(&sdr, "tuned_hz"), 220_352_000.0);
        let dab = c.wait("get dab", 5, |r| r.contains(r#""label":"11C""#));
        assert!(dab.contains(r#""centre_hz":220352000"#), "{dab}");
        // The active plan (`general`) declares no DAB allocation: the
        // block is still named, and the plan's silence is reported.
        assert!(dab.contains(r#""allocated":false"#), "{dab}");
        assert!(dab.contains(r#""plan":"general""#), "{dab}");

        // Raster stepping: 11C next is 11D, prev comes back.
        c.ok("sdr dab channel next");
        c.wait("get sdr", 5, |r| field(r, "centre_hz") == 222_064_000.0);
        c.ok("sdr dab channel prev");
        c.wait("get sdr", 5, |r| field(r, "centre_hz") == 220_352_000.0);

        // An unknown block is refused with the range named.
        c.ok("sdr dab channel 99Z");
        c.wait("get status", 5, |r| r.contains("unknown Band III block"));

        // The dock's channel row draws the same state. Turning the receiver
        // on holds the section open (an off receiver's section is collapsed).
        c.ok("sdr dab on");
        let tree = c.wait("get uitree", 10, |r| r.contains("dab channel 11C"));
        assert!(tree.contains("not allocated by general"), "{tree}");
    });
}

/// D27's stale-table case, end to end: leaving the ensemble's signal clears
/// the published table. `rf-noise` keeps the sample stream flowing, so this
/// is the receiver's own expiry (no accepted FIC for `TABLE_EXPIRY_FRAMES`)
/// doing the work — the selection, the DLS text and playback must go with
/// the table — and returning to `rf-dab` must rebuild all of it.
#[test]
#[ignore = "opens a window"]
fn dab_table_clears_when_the_scene_leaves_the_ensemble() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        c.ok("stimulus rf-dab");
        c.ok("sdr dab on");
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
        // A service that actually plays (the DAB+ programme, as in
        // `sdr_dab_audio`), so the clear has a running transport to stop.
        c.ok("sdr dab service 0x1004");
        c.wait("get dab", 5, |r| raw(r, "service") == "4100");
        c.ok("sdr dab play");
        c.wait("get dab", 30, |r| r.contains(r#""state":"playing""#));

        // The signal is gone. The table is published only from a clean FIC,
        // so after the expiry run the receiver must report nothing — not the
        // old names, not the old selection, not the old label.
        c.ok("stimulus rf-noise");
        let dab = c.wait("get dab", 30, |r| r.contains(r#""locked":false"#));
        assert!(dab.contains(r#""services":[]"#), "{dab}");
        assert!(dab.contains(r#""service":null"#), "{dab}");
        assert!(dab.contains(r#""dls":null"#), "{dab}");
        assert!(dab.contains(r#""audio":{"state":"off"}"#), "{dab}");
        assert!(!dab.contains("NEOWON DAB+"), "a stale service name: {dab}");

        // The dock cannot draw what the readout no longer holds: while
        // unlocked it shows the sync line and nothing below it.
        let tree = c.wait("get uitree", 10, |r| r.contains("dock section DAB"));
        assert!(tree.contains("not locked"), "{tree}");
        assert!(!tree.contains("NEOWON DAB+"), "stale dock service: {tree}");

        // The ensemble is not gone forever: its scene rebuilds the table.
        c.ok("stimulus rf-dab");
        let dab = c.wait("get dab", 30, |r| {
            r.contains(r#""locked":true"#) && r.contains("NEOWON DAB+")
        });
        assert_eq!(
            raw(&dab, "service"),
            "null",
            "selection stays dropped: {dab}"
        );
    });
}

/// The operator's original symptom, pinned: retuning away or changing the
/// sample rate is a *different signal*, so the old ensemble's names must not
/// survive it — and unlike the scene switch above it clears at once, without
/// waiting for the receiver's expiry run. Both actions also stop playback and
/// drop the selection.
#[test]
#[ignore = "opens a window"]
fn dab_retune_or_rate_change_clears_the_table_at_once() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        c.ok("stimulus rf-dab");
        c.ok("sdr dab on");
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
        c.ok("sdr dab service 0x1004");
        c.wait("get dab", 5, |r| raw(r, "service") == "4100");
        c.ok("sdr dab play");
        c.wait("get dab", 30, |r| r.contains(r#""state":"playing""#));

        // Retune: the hardware window moves, so the table goes with it.
        c.ok("sdr centre 220352000");
        let dab = c.request("get dab");
        assert!(
            dab.contains(r#""locked":false"#),
            "a retune kept the lock: {dab}"
        );
        assert!(dab.contains(r#""services":[]"#), "{dab}");
        assert!(dab.contains(r#""service":null"#), "{dab}");
        assert!(dab.contains(r#""audio":{"state":"off"}"#), "{dab}");

        // Back on the ensemble it rebuilds, so the clear was not a wreck.
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));

        // Rate: Mode I is defined at 2.048 MS/s; another rate cannot decode.
        c.ok("sdr rate 1024000");
        let dab = c.request("get dab");
        assert!(
            dab.contains(r#""locked":false"#),
            "a rate change kept the lock: {dab}"
        );
        assert!(dab.contains(r#""services":[]"#), "{dab}");
        assert!(dab.contains(r#""service":null"#), "{dab}");

        // And back to Mode I.
        c.ok("sdr rate 2048000");
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
    });
}
