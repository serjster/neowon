//! Surveys in SDR mode on the simulator, end to end: a survey of
//! 97–101 MHz finds exactly the FM scene's emitters there; after switching
//! to the reference scene a second survey's diff says which went and which
//! arrived; the survey files into the catalog.
//!
//!   cargo test -p neowon-app --test sdr_survey -- --ignored

mod common;
use common::*;

#[test]
#[ignore = "opens a window"]
fn survey_finds_diffs_and_files() {
    let dir = std::env::temp_dir().join(format!("neowon-survey-cat-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().to_string();
    let (child, mut c) = launch(&["--sdr-sim"], &[("NEOWON_CATALOG", &dir_s)]);
    with_app(child, || {
        c.ok("stimulus rf-fm-band");
        // The stimulus reaches the backend asynchronously: survey only once
        // the new scene is on air, or a step can still see rf-reference's
        // 100.1 MHz tone.
        c.wait("get detections", 15, |r| {
            let ids = items(r, "id");
            ids.iter()
                .any(|t| (field(t, "centre_hz") - 99.4e6).abs() < 5e3)
                && !ids
                    .iter()
                    .any(|t| (field(t, "centre_hz") - 100.1e6).abs() < 5e3)
        });
        c.ok("sdr survey 97M 101M");
        let s = c.wait("get survey", 30, |r| {
            r.contains(r#""running":false"#) && r.contains(r#""completed":1"#)
        });
        let peaks: Vec<f64> = items(&s, "centre_hz")
            .iter()
            .map(|p| p.split(',').next().unwrap().parse().unwrap())
            .collect();
        println!("first survey peaks: {peaks:?}");
        // rf-fm-band's emitters in 97–101 MHz.
        for want in [98.3e6, 99.4e6] {
            assert!(
                peaks.iter().any(|p| (p - want).abs() < 2e3),
                "{want} missing: {s}"
            );
        }
        assert_eq!(peaks.len(), 2, "{s}");

        // rf-reference: one tone at 100.1 MHz.
        c.ok("stimulus rf-reference");
        c.ok("sdr survey 97M 101M");
        c.wait("get survey", 30, |r| {
            r.contains(r#""running":false"#) && r.contains(r#""completed":2"#)
        });
        let d = c.request("get surveydiff");
        println!("diff: {d}");
        let rows = items(&d, "centre_hz");
        let change = |hz: f64| {
            rows.iter()
                .find(|r| (r.split(',').next().unwrap().parse::<f64>().unwrap() - hz).abs() < 2e3)
                .map(|r| raw(r, "change").trim_matches('"').to_string())
        };
        assert_eq!(change(98.3e6).as_deref(), Some("gone"), "{d}");
        assert_eq!(change(99.4e6).as_deref(), Some("gone"), "{d}");
        assert_eq!(change(100.1e6).as_deref(), Some("new"), "{d}");

        c.ok("catalog survey FM check");
        let cat = c.wait("get catalog", 5, |r| field(r, "entities") >= 1.0);
        assert!(field(&cat, "integrity") == 0.0, "{cat}");
    });
    let _ = std::fs::remove_dir_all(&dir);
}
