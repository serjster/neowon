//! Phase 10.9: one scripted run through the whole SDR chain on the
//! simulator, starting from the scope and switching instrument at run
//! time: tune → detect → analyse → classify → catalog → export, then back
//! to the scope and into the SDR again with its settings kept.
//!
//! Decode is not in the chain yet: it is 10.6, which waits on independent
//! reference vectors (spec deviations).
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
        assert!(text.contains("100.3"), "{text}");

        // Back to the scope, then the SDR again: tuning was kept.
        c.ok("instrument scope");
        let s = c.wait("get status", 10, |r| raw_str(r, "backend") == "Simulated");
        println!("scope again: {s}");
        c.wait("get sdr", 5, |r| r.contains(r#""active":false"#));
        c.ok("instrument sdr");
        let r = c.wait("get sdr", 10, |r| {
            r.contains(r#""active":true"#) && field(r, "frames_seen") > 0.0
        });
        assert_eq!(field(&r, "centre_hz"), 100.3e6, "{r}");

        // A refused instrument name reaches the status line, not a crash.
        let bad = c.request("instrument radar");
        assert!(bad.contains("instrument"), "{bad}");
    });
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&export);
}
