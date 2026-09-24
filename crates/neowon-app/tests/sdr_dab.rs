//! The `rf-dab` sim scene end to end, over the control socket. The app composes a real Mode I ensemble (three services,
//! EEP and UEP sub-channels, one carrying a DLS PAD stream), installs it as
//! the `rf-dab` scene, and the SDR path decodes it: FIC → MSC → PAD parser →
//! `get dab`. Nothing here is recorded: the expected EId, labels and DLS
//! strings come from the scene definition in `src/sdr/dab_scene.rs`, and the
//! assertions are bounded polls, never wall-clock sleeps.
//!
//! Needs a window, so `#[ignore]` by default:
//!   cargo test -p neowon-app --test sdr_dab -- --ignored

mod common;
use common::tree::{Node, exact, find, nodes};
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
        // The MSC queue's own drop counter is reported, not just kept: a
        // decoded frame nobody drained is coverage lost. Zero here — the app
        // drains every frame — but it has to be a number the run states.
        assert_eq!(field(&dab, "msc_dropped"), 0.0, "{dab}");

        // Transport: the DAB+ programme's audio actually decodes.
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

        // Reset forgets the label and the parsers with the table.
        c.ok("sdr dab reset");
        c.wait("get dab", 5, |r| {
            r.contains(r#""dls":null"#) && r.contains(r#""locked":false"#)
        });

        // And the receiver re-locks from the same buffer.
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
    });
}

/// Over the control socket, `sdr dab channel 11C` lands the hardware
/// exactly on the block, `next`/`prev` step the raster, and `get dab` reports the block with the plan's allocation status even while
/// decoding is off. The dock draws the same fact (the UI tree is the DOM).
#[test]
#[ignore = "opens a window"]
fn dab_channel_selects_a_band_iii_block_over_the_control_socket() {
    // `launch` gives the app a fresh HOME: the shipped `general` plan is
    // active and no user plan can shadow it, so "no DAB allocation here" is
    // deterministic.
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
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

/// The stale-table case, end to end: leaving the ensemble's signal clears
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
        // unlocked it says the lock went and draws nothing below it.
        let tree = c.wait("get uitree", 10, |r| r.contains("dock section DAB"));
        assert!(tree.contains("dab lock lock lost"), "{tree}");
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

/// Retuning away or changing the
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

/// Poll `query` until `ok` accepts the reply or `secs` pass, and return the
/// last reply either way: for a state the test then *checks* (as a
/// collected problem) rather than waits on, so a broken fix is reported
/// with every other check instead of as a bare timeout.
fn settle(c: &mut Conn, query: &str, secs: u64, ok: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let r = c.request(query);
        if ok(&r) || std::time::Instant::now() >= deadline {
            return r;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The rail invariant at one state. The SDR rail scrolls vertically only, so a
/// node that ends past its right edge is cut off with no way to reach it;
/// every node that starts in the rail must end inside it. The `visible`
/// readouts must also be on screen at the default window — inside the
/// rail's rect, not below the fold. Problems are collected, not asserted,
/// so one run reports every state that breaks.
fn rail_problems(state: &str, tree: &str, visible: &[&str]) -> Vec<String> {
    let ns = nodes(tree);
    let Some(dock) = exact(&ns, "Group", "SDR dock") else {
        return vec![format!("{state}: no SDR dock in the tree")];
    };
    let in_rail = |n: &&Node| n.rect[0] >= dock.rect[0] - 0.5 && n.rect[1] >= dock.rect[1] - 0.5;
    let mut out: Vec<String> = ns
        .iter()
        .filter(in_rail)
        .filter(|n| n.right() > dock.right() + 0.5)
        .map(|n| {
            format!(
                "{state}: {} {:?} ends at x {:.1}, past the rail's {:.1}",
                n.role,
                n.text(),
                n.right(),
                dock.right()
            )
        })
        .collect();
    for want in visible {
        match find(&ns, want) {
            Some(n) if n.inside(dock) => {}
            Some(n) => out.push(format!(
                "{state}: {want:?} at {:?} is not in the rail's view {:?}",
                n.rect, dock.rect
            )),
            None => out.push(format!("{state}: no {want:?} node")),
        }
    }
    if let Some(s) = exact(&ns, "Group", "dock section DAB") {
        eprintln!(
            "{state}: dock section DAB {:?} in rail {:?}",
            s.rect, dock.rect
        );
    }
    out
}

/// The interface a user meets, at the default window:
/// the DAB readout fits the rail in every state it has (off, locked, a
/// refused play, playing, expired, no input, wrong rate); turning the
/// receiver on brings its section into view with the transport and DLS on
/// screen; a service that can never play is labelled as such in the dock
/// and in `get dab`, with Play disabled; a lost lock says what was lost
/// and when; the lifetime counters say they are cumulative.
#[test]
#[ignore = "opens a window"]
fn dab_dock_fits_the_rail_and_tells_the_truth_in_every_state() {
    // `launch` gives the app a fresh HOME, so the shipped `general` plan is
    // active and the block row carries its widest text ("not allocated by
    // general", and the plans that do declare DAB) deterministically.
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        let mut problems = Vec::new();
        c.ok("stimulus rf-dab");
        c.ok("sdr dab channel 11C");
        // Off, the section is collapsed: only its header is in the rail.
        let t = c.wait("get uitree", 10, |r| r.contains("Tuned 220.3520 MHz"));
        problems.extend(rail_problems("off", &t, &["DAB"]));

        // On: the section comes into view at the top of the rail, and once
        // locked the transport and the DLS line are on screen.
        c.ok("sdr dab on");
        c.wait("get dab", 30, |r| r.contains(r#""locked":true"#));
        let t = c.wait("get uitree", 10, |r| r.contains("dab service 1005"));
        problems.extend(rail_problems(
            "locked",
            &t,
            &[
                "DAB",
                "Play",
                "dab dls",
                "dab cumulative since on/reset: FIB CRC",
            ],
        ));
        let ns = nodes(&t);
        let dock = exact(&ns, "Group", "SDR dock").expect("dock");
        // Revealed: the whole section in view, or — when it is taller than
        // the rail — its header at the top.
        let header = exact(&ns, "Button", "DAB").expect("DAB header");
        let section = exact(&ns, "Group", "dock section DAB").expect("DAB section");
        if !section.inside(dock) && header.rect[1] > dock.rect[1] + 8.0 {
            problems.push(format!(
                "locked: DAB turned on but its section {:?} is not in the rail's view {:?}",
                section.rect, dock.rect
            ));
        }

        // The service whose sub-channel DAB+ cannot carry is labelled in
        // the dock and in `get dab`, from the same answer `dab play` gives.
        let dab = c.request("get dab");
        let one = items(&dab, "sid")
            .into_iter()
            .find(|i| i.starts_with("4097"))
            .expect("service 1001");
        assert_eq!(raw(one, "playable"), "false", "{dab}");
        assert!(one.contains("outside DAB+"), "{dab}");
        let four = items(&dab, "sid")
            .into_iter()
            .find(|i| i.starts_with("4100"))
            .expect("service 1004");
        assert_eq!(raw(four, "playable"), "true", "{dab}");
        match find(&ns, "dab service 1001") {
            Some(n) if n.text().contains("cannot play") && n.text().contains("outside DAB+") => {}
            other => problems.push(format!(
                "service 1001 is not labelled unplayable: {other:?}"
            )),
        }
        if !find(&ns, "dab service 1004").is_some_and(|n| n.text().ends_with("playable")) {
            problems.push("service 1004 is not labelled playable".into());
        }

        // Selecting it disables Play (with the reason) rather than offering
        // it; the script's play is still refused, and the refusal fits.
        c.ok("sdr dab service 0x1001");
        c.ok("sdr dab play");
        c.wait("get status", 5, |r| r.contains("outside DAB+"));
        let t = c.wait("get uitree", 10, |r| r.contains("outside DAB+"));
        problems.extend(rail_problems("refused", &t, &["Play"]));
        if !exact(&nodes(&t), "Button", "Play").is_some_and(|n| n.disabled) {
            problems.push("refused: Play is enabled for a service that cannot play".into());
        }

        // Playing: the meter row fits.
        c.ok("sdr dab service 0x1004");
        c.ok("sdr dab play");
        c.wait("get dab", 30, |r| r.contains(r#""state":"playing""#));
        let t = c.wait("get uitree", 10, |r| r.contains("dab audio playing"));
        problems.extend(rail_problems("playing", &t, &["dab audio", "dab dls"]));
        c.ok("sdr dab stop");

        // Expired: the signal leaves; the dock says the lock was lost,
        // which ensemble and how long ago — not a bare "not locked".
        c.ok("stimulus rf-reference");
        c.wait("get dab", 30, |r| r.contains(r#""locked":false"#));
        let dab = settle(&mut c, "get dab", 5, |r| r.contains(r#""cause":"expired""#));
        if !(dab.contains(r#""cause":"expired""#) && dab.contains(r#""label":"NEOWON SIM""#)) {
            problems.push(format!(
                "expired: get dab does not say what was lost: {dab}"
            ));
        }
        let t = settle(&mut c, "get uitree", 5, |r| {
            r.contains("dab lock lock lost")
        });
        problems.extend(rail_problems(
            "expired",
            &t,
            &[
                "dab lock lock lost",
                "last attempt PRS",
                "dab cumulative since on/reset: FIB CRC",
            ],
        ));
        if !find(&nodes(&t), "dab lock").is_some_and(|n| n.text().contains("NEOWON SIM")) {
            problems.push("expired: the lock line does not name the ensemble".into());
        }

        // No input: stopped is said as stopped.
        c.ok("sdr run off");
        let t = settle(&mut c, "get uitree", 5, |r| r.contains("dab lock no input"));
        problems.extend(rail_problems("no input", &t, &["dab lock no input"]));

        // The wrong rate: the warning and its one-click fix fit.
        c.ok("sdr run on");
        c.ok("sdr rate 1024000");
        let t = c.wait("get uitree", 10, |r| r.contains("DAB needs 2.048 MS/s"));
        problems.extend(rail_problems("wrong rate", &t, &["DAB needs 2.048 MS/s"]));

        // The rest of the rail, swept for the same mechanism: the Analysis
        // section open (the lab's five lines and the constellation).
        c.ok("sdr analyse on");
        let t = c.wait("get uitree", 10, |r| r.contains("dock section Analysis"));
        problems.extend(rail_problems("analysis", &t, &[]));

        assert!(problems.is_empty(), "{}", problems.join("\n"));
    });
}

/// The gesture map has a one-line hint on the canvas (the waterfall's free
/// corner), the DC notch under the hardware centre is tagged, and a refused
/// action shows in the app bar while a device is named there — in the SDR
/// and in the scope workspace.
#[test]
#[ignore = "opens a window"]
fn sdr_canvas_hints_and_the_bar_shows_refusals() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        let mut problems = Vec::new();
        c.ok("sdr rate 1234");
        c.wait("get status", 5, |r| r.contains("error: rate 1234"));
        let t = settle(&mut c, "get uitree", 5, |r| r.contains("error: rate 1234"));
        let ns = nodes(&t);
        let spectrum = exact(&ns, "Image", "spectrum").expect("spectrum");
        let waterfall = exact(&ns, "Image", "waterfall").expect("waterfall");
        let bar = exact(&ns, "Region", "menu_bar").expect("menu bar");
        // The hint lives in the waterfall: on the spectrum it would sit on
        // the span / peak readout.
        for (what, prefix, on) in [
            ("gesture hint", "gesture hint ", waterfall),
            ("DC notch tag", "dc notch ", spectrum),
        ] {
            match find(&ns, prefix) {
                Some(n) if n.inside(on) => {}
                other => problems.push(format!("{what} not on the {}: {other:?}", on.label)),
            }
        }
        if !find(&ns, "gesture hint").is_some_and(|n| n.text().contains("scroll")) {
            problems.push("the hint does not name the wheel".into());
        }
        match find(&ns, "error: rate 1234") {
            Some(n) if n.inside(bar) => {}
            other => problems.push(format!("the refusal is not in the app bar: {other:?}")),
        }

        // With an instrument connected the scope's bar names the device; a
        // refusal must still show there.
        c.ok("instrument scope");
        c.wait("get uitree", 15, |r| r.contains(r#""label":"CH1""#));
        c.ok("bandplan nosuchplan");
        c.wait("get status", 5, |r| r.contains("no band plan"));
        let t = settle(&mut c, "get uitree", 5, |r| {
            r.contains("error: no band plan")
        });
        let ns = nodes(&t);
        let bar = exact(&ns, "Region", "menu_bar").expect("menu bar");
        match find(&ns, "error: no band plan") {
            Some(n) if n.inside(bar) => {}
            other => problems.push(format!("scope: the refusal is not in the bar: {other:?}")),
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    });
}
