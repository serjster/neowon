//! One scripted run through the whole SDR chain on the
//! simulator, starting from the scope and switching instrument at run
//! time: tune → detect → analyse → classify → catalog → export, then back
//! to the scope and into the SDR again with its settings kept.
//!
//! Decode is not in the chain: it waits on independent reference vectors.
//!
//! The action-level half of the parity rule (every UI control and catalog
//! op prints as a script line that parses back to itself) is the
//! `every_action_round_trips` unit tests in `sdr::actions` and
//! `catalog::grammar`.
//!
//!   cargo test -p neowon-app --test sdr_integration -- --ignored

mod common;
use common::*;

fn raw_str<'a>(json: &'a str, key: &str) -> &'a str {
    raw(json, key).trim_matches('"')
}

#[test]
#[ignore = "opens a window"]
fn scope_to_sdr_chain_to_export_and_back() {
    let dir = std::env::temp_dir().join(format!("neowon-sdr-int-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_s = dir.to_string_lossy().to_string();
    let export = dir.with_extension("json");
    let (child, mut c) = launch(&["--sim"], &[("NEOWON_CATALOG", &dir_s)]);
    with_app(child, || {
        // Scope first.
        let s = c.wait("get status", 10, |r| field(r, "frames_seen") > 0.0);
        assert_eq!(raw_str(&s, "backend"), "Simulated", "{s}");

        // Switch instrument; the simulated SDR connects.
        c.ok("instrument sdr");
        c.wait("get sdr", 10, |r| {
            r.contains(r#""active":true"#) && raw_str(r, "backend") == "Simulated SDR"
        });

        // Tune and detect.
        c.ok("stimulus rf-digital");
        c.ok("sdr tune 100.3M");
        let d = c.wait("get detections", 15, |r| {
            items(r, "id")
                .iter()
                .any(|t| (field(t, "centre_hz") - 100.3e6).abs() < 5e3)
        });
        println!("detections: {d}");

        // Analyse and classify the signal at the tuned frequency.
        c.ok("sdr analyse on");
        let m = c.wait("get modmeas", 15, |r| {
            r.contains(r#""lab":{"#) && raw_str(r, "modulation") == "QPSK"
        });
        assert!(
            (field(&m, "symbol_rate_hz") / 102.4e3 - 1.0).abs() < 0.01,
            "{m}"
        );
        let k = c.wait("get classify", 10, |r| raw_str(r, "label") == "qpsk");
        assert!(k.contains(r#""unknown":false"#), "{k}");

        // File it and export the catalog.
        c.ok("catalog add");
        let cat = c.wait("get catalog", 5, |r| items(r, "id").len() == 1);
        let sig = items(&cat, "id")[0];
        assert!((field(sig, "centre_hz") - 100.3e6).abs() < 5e3, "{cat}");
        c.ok(&format!("catalog export {}", export.display()));
        c.wait("get status", 5, |_| export.exists());
        let text = std::fs::read_to_string(&export).unwrap();
        assert!((field(&text, "centre_hz") - 100.3e6).abs() < 5e3, "{text}");

        // Back to the scope, then the SDR again: tuning was kept.
        c.ok("instrument scope");
        let s = c.wait("get status", 10, |r| raw_str(r, "backend") == "Simulated");
        println!("scope again: {s}");
        c.wait("get sdr", 5, |r| r.contains(r#""active":false"#));
        c.ok("instrument sdr");
        let r = c.wait("get sdr", 10, |r| {
            r.contains(r#""active":true"#) && field(r, "frames_seen") > 0.0
        });
        assert_eq!(field(&r, "centre_hz"), 100e6, "{r}");
        assert_eq!(field(&r, "tuned_hz"), 100.3e6, "{r}");

        // The tuned cursor is independent of the hardware window.
        // Follow pins the window to the tuned frequency; centre moves it
        // by itself; width is auto or manual.
        c.ok("sdr follow on");
        c.wait("get sdr", 5, |r| {
            field(r, "centre_hz") == 100.3e6 && r.contains(r#""follow":true"#)
        });
        c.ok("sdr follow off");
        c.ok("sdr centre 100M");
        c.wait("get sdr", 5, |r| field(r, "centre_hz") == 100e6);
        assert_eq!(field(&c.request("get sdr"), "tuned_hz"), 100.3e6);
        c.ok("sdr width 15k");
        c.wait("get sdr", 5, |r| {
            field(r, "width_hz") == 15e3 && r.contains(r#""width_auto":false"#)
        });
        c.ok("sdr width auto");
        c.wait("get sdr", 5, |r| r.contains(r#""width_auto":true"#));

        // The view pans inside the IQ band (2.048 MS/s): a 200 kHz span
        // can sit at most 924 kHz off the tuned centre; full span recentres.
        c.ok("sdr span 200k");
        c.ok("sdr pan 300k");
        c.wait("get sdr", 5, |r| field(r, "pan_hz") == 300e3);
        c.ok("sdr pan 5M");
        c.wait("get sdr", 5, |r| field(r, "pan_hz") == 924e3);
        c.ok("sdr span 0");
        c.wait("get sdr", 5, |r| field(r, "pan_hz") == 0.0);

        // Tuning outside the IQ band recentres the hardware on the target;
        // inside it the window stays put.
        c.ok("sdr tune 145.5M");
        c.wait("get sdr", 5, |r| {
            field(r, "centre_hz") == 145.5e6 && field(r, "tuned_hz") == 145.5e6
        });
        c.ok("sdr tune 145.9M");
        c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 145.9e6);
        assert_eq!(field(&c.request("get sdr"), "centre_hz"), 145.5e6);
        c.ok("sdr list 300");
        c.wait("get sdr", 5, |r| field(r, "list_px") == 300.0);
        c.ok("sdr list 5");
        c.wait("get status", 5, |r| r.contains("list height"));

        // Continuity is reported, not assumed. The simulated SDR
        // never loses a sample, so the honest live answer is "none" — but it
        // has to be *said*, in `get sdr` and in the dock, or a real drop
        // would be invisible. (That the count also splices the DAB receiver
        // is the unit test `sdr::dab::tests::a_reported_drop`, which can
        // inject a gap the simulator cannot.)
        let r = c.wait("get sdr", 5, |r| field(r, "frames_seen") > 0.0);
        assert_eq!(field(&r, "dropped_pairs"), 0.0, "{r}");
        assert_eq!(field(&r, "drop_events"), 0.0, "{r}");
        // The waterfall's row cadence (`protocol-rtlsdr.md`, ~15–20 rows/s)
        // is reported as a number too.
        assert!(field(&r, "wf_rows") > 0.0, "{r}");
        let t = c.wait("get uitree", 10, |r| r.contains(r#""value":"Drops""#));
        assert!(
            t.contains("none in "),
            "the dock must state the drop count, not leave it implied: {t}"
        );

        // A refused instrument name reaches the status line, not a crash.
        let bad = c.request("instrument radar");
        assert!(bad.contains("instrument"), "{bad}");
    });
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&export);
}
