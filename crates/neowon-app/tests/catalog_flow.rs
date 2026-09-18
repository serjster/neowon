//! The catalog end to end on the simulated SDR: detect, file, manage,
//! and survive a restart.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test catalog_flow -- --ignored

mod common;
use common::*;

#[test]
#[ignore = "opens a window"]
fn catalog_files_manages_and_persists() {
    let dir = std::env::temp_dir().join(format!("neowon-catalog-flow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_s = dir.to_string_lossy().to_string();
    let export = dir.with_extension("json");
    let env = [("NEOWON_CATALOG", dir_s.as_str())];

    let (child, mut c) = launch(&["--sdr-sim"], &env);
    let mut kept = 0u64;
    with_app(child, || {
        c.ok("stimulus rf-fm-band");
        c.ok("sdr tune 99M");
        c.wait("get detections", 10, |r| items(r, "id").len() == 2);

        // The strongest detection (99.4 MHz), then one by hand.
        c.ok("catalog add");
        c.ok("catalog add 98.3M Station B");
        let cat = c.wait("get catalog", 5, |r| items(r, "id").len() == 2);
        let sigs = items(&cat, "id");
        let id_of = |hz: f64| -> u64 {
            let s = sigs
                .iter()
                .find(|s| (field(s, "centre_hz") - hz).abs() < 1000.0)
                .unwrap_or_else(|| panic!("{hz}: {cat}"));
            s.split(',').next().unwrap().parse().unwrap()
        };
        let (a, b) = (id_of(99.4e6), id_of(98.3e6));
        assert!(field(&cat, "integrity") == 0.0, "{cat}");

        // Observe files a live track against each (B was added by hand
        // with no bandwidth; its 1 Hz floor still overlaps the track).
        c.ok("catalog observe");
        let cat = c.wait("get catalog", 5, |r| {
            items(r, "id")
                .iter()
                .all(|s| field(s, "observations") >= 1.0)
        });
        assert_eq!(items(&cat, "id").len(), 2);

        // Manage: rename (old name becomes an alias), alias, tag + undo,
        // pin, then merge B into A.
        c.ok(&format!("catalog rename {a} Radio Two"));
        c.ok(&format!("catalog alias {a} R2"));
        c.ok(&format!("catalog tag {a} music"));
        c.ok(&format!("catalog tag {a} oops"));
        c.ok("catalog undo");
        c.ok(&format!("catalog pin {a}"));
        let cat = c.wait("get catalog", 5, |r| {
            r.contains("Radio Two") && r.contains(r#""pinned":true"#)
        });
        assert!(
            cat.contains(r#""tags":["broadcast","music"]"#) || cat.contains(r#""tags":["music"]"#),
            "{cat}"
        );
        assert!(!cat.contains("oops"), "undo: {cat}");
        assert!(cat.contains(r#""R2""#) && cat.contains("99.4"), "{cat}");
        c.ok(&format!("catalog merge {b} {a}"));
        let cat = c.wait("get catalog", 5, |r| items(r, "id").len() == 1);
        assert!(field(items(&cat, "id")[0], "observations") >= 2.0, "{cat}");
        let h = c.request(&format!("get history {b}"));
        assert_eq!(field(&h, "canonical") as u64, a, "{h}");

        // Export, then purge everything unpinned: the pinned one stays.
        c.ok(&format!("catalog export {}", export.display()));
        c.ok("catalog add 101.7M temporary");
        let cat = c.wait("get catalog", 5, |r| items(r, "id").len() == 2);
        let all: Vec<String> = items(&cat, "id")
            .iter()
            .map(|s| s.split(',').next().unwrap().to_string())
            .collect();
        c.ok(&format!("catalog purge {} cascade", all.join(",")));
        let cat = c.wait("get catalog", 5, |r| items(r, "id").len() == 1);
        assert!(cat.contains("Radio Two"), "{cat}");
        // Refusals reach the status line.
        c.ok(&format!("catalog delete {a}"));
        c.wait("get status", 5, |r| r.contains("pinned"));
        kept = a;
    });
    assert!(export.exists(), "export written");

    // A restart finds everything that was acknowledged.
    let (child, mut c) = launch(&["--sdr-sim"], &env);
    with_app(child, || {
        let cat = c.request("get catalog");
        let sigs = items(&cat, "id");
        assert_eq!(sigs.len(), 1, "{cat}");
        assert!(
            cat.contains("Radio Two") && cat.contains(r#""pinned":true"#),
            "{cat}"
        );
        assert!(field(&cat, "integrity") == 0.0);
        let h = c.request(&format!("get history {kept}"));
        assert!(items(&h, "id").len() >= 2, "{h}");
    });
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&export);
}
