//! Phase 10.14 (band map) and the D12 workspace split, asserted from the
//! UI element tree (`get uitree`) rather than pixels: the SDR workspace
//! carries no scope chrome, the SCOPE | SDR switch is in the app bar, the
//! band strip names the band in view, the RF map window opens, and the
//! band verbs tune the radio.
//!
//!   cargo test -p neowon-app --test sdr_refmap -- --ignored

mod common;
use common::*;

/// The rect `[x0, y0, x1, y1]` of the first node with this label.
fn rect_of(tree: &str, label: &str) -> Option<[f32; 4]> {
    let at = tree.find(&format!(r#""label":"{label}""#))?;
    let rest = &tree[at..];
    let start = rest.find(r#""rect":["#)? + r#""rect":["#.len();
    let inner = &rest[start..rest[start..].find(']')? + start];
    let mut it = inner.split(',').map(|v| v.trim().parse::<f32>().ok());
    Some([it.next()??, it.next()??, it.next()??, it.next()??])
}

/// Labels of every node with `role`, anywhere in the tree JSON.
fn labels(tree: &str, role: &str) -> Vec<String> {
    let pat = format!(r#"{{"role":"{role}","label":""#);
    tree.match_indices(&pat)
        .map(|(i, _)| {
            let rest = &tree[i + pat.len()..];
            rest[..rest.find('"').unwrap()].to_string()
        })
        .collect()
}

/// Directory contents as `(name, bytes)` pairs, sorted — how a test checks
/// that one store did not touch another.
fn listing(dir: &std::path::Path) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        e.metadata().map_or(0, |m| m.len() as usize),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
#[ignore = "opens a window"]
fn stations_location_and_catalog_bridge() {
    let dir = std::env::temp_dir().join(format!("neowon-refmap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A fixture store: FM broadcast 20 km north of Lisbon, aviation at
    // Lisbon. Nothing here came from the network.
    std::fs::write(
        dir.join("wikidata.json"),
        r#"[
          {"source":"wikidata","id":"Q1001","name":"Antena 1","freq_hz":100300000.0,
           "bandwidth_hz":180000.0,"modulation":"wfm","service":"broadcast",
           "lat":38.9023,"lon":-9.1393,"country":"PT","callsign":"CSB1","power_kw":12.5},
          {"source":"wikidata","id":"Q2002","name":"Cascais Tower","freq_hz":118100000.0,
           "modulation":"am","service":"aviation","lat":38.7223,"lon":-9.1393,"country":"PT"}
        ]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("meta.json"),
        r#"[{"source":"wikidata","count":2,"fetched_at":"2026-09-19T10:00:00Z",
             "origin":"fixture","licence":"CC0"}]"#,
    )
    .unwrap();
    let loc = dir.join("location.json");
    let cat = dir.join("catalog");
    let eibi = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../neowon-refdb/tests/fixtures/eibi.csv");
    let env = [
        ("NEOWON_REFDB", dir.to_str().unwrap()),
        ("NEOWON_LOCATION", loc.to_str().unwrap()),
        ("NEOWON_CATALOG", cat.to_str().unwrap()),
    ];

    let (child, mut c) = launch(&["--sdr-sim"], &env);
    with_app(child, || {
        c.wait("get sdr", 15, |r| field(r, "frames_seen") > 0.0);

        // The store loaded at startup, with no fetch.
        let db = c.request("get refdb");
        assert_eq!(field(&db, "stations"), 2.0, "{db}");
        assert!(db.contains(r#""origin":"fixture""#), "{db}");

        // Location: manual fix, locator reported back.
        c.ok("location 38.7223 -9.1393");
        let l = c.wait("get location", 5, |r| r.contains(r#""set":true"#));
        assert_eq!(field(&l, "lat"), 38.7223, "{l}");
        assert_eq!(field(&l, "lon"), -9.1393, "{l}");
        assert_eq!(raw(&l, "source"), r#""manual""#, "{l}");
        assert_eq!(raw(&l, "locator"), r#""IM58kr""#, "{l}");

        // The window joins the UI tree and its filters reach the readout.
        c.ok("stations window on");
        c.wait("get uitree", 10, |r| r.contains(r#""label":"Stations""#));
        c.ok("stations find antena");
        let s = c.wait("get stations all", 5, |r| field(r, "total") == 1.0);
        assert!(s.contains("wikidata:Q1001"), "{s}");
        c.ok("stations find -");

        // The window's "in view" rows: the FM station with its distance.
        c.ok("sdr tune 100.3M");
        let s = c.wait("get stations view", 5, |r| field(r, "total") >= 1.0);
        let item = items(&s, "key")
            .into_iter()
            .find(|i| i.contains("wikidata:Q1001"))
            .unwrap_or_else(|| panic!("no FM station: {s}"));
        let km = field(item, "km");
        assert!((km - 20.0).abs() <= 0.5, "km {km}: {s}");
        assert!(item.contains(r#""mod":"WFM""#), "{s}");

        // Click-to-tune: station → tuned + the fitting demodulator (D21).
        c.ok("stations tune wikidata:Q1001");
        c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 100.3e6);
        c.wait("get audio", 5, |r| raw(r, "demod") == r#""wfm""#);
        // A far target recentres the hardware and swaps the demod.
        c.ok("stations tune wikidata:Q2002");
        let sdr = c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 118.1e6);
        assert_eq!(field(&sdr, "centre_hz"), 118.1e6, "{sdr}");
        c.wait("get audio", 5, |r| raw(r, "demod") == r#""am""#);

        // Import (offline): the EiBi fixture's three rows join the store.
        let cat_before = listing(&cat);
        c.ok(&format!("refdb import eibi {}", eibi.display()));
        let db = c.wait("get refdb", 10, |r| field(r, "stations") == 5.0);
        assert!(db.contains(r#""count":3"#), "{db}");
        assert!(db.contains(r#""source":"eibi""#), "{db}");
        assert_eq!(
            listing(&cat),
            cat_before,
            "refdb import wrote into the catalog"
        );

        // Add to catalog (D20): a copy with refdb provenance.
        let refdb_after_import = listing(&dir);
        c.ok("stations catalog wikidata:Q1001");
        let ca = c.wait("get catalog", 5, |r| field(r, "entities") == 1.0);
        assert!(ca.contains(r#""input_ref":"refdb:wikidata:Q1001""#), "{ca}");
        assert!(ca.contains(r#""kind":"db""#), "{ca}");
        assert!(ca.contains(r#""tool":"neowon-refdb""#), "{ca}");
        // The refdb store is untouched by the catalog copy.
        assert_eq!(
            listing(&dir),
            refdb_after_import,
            "catalog wrote into refdb"
        );
    });
}

#[test]
#[ignore = "opens a window"]
fn band_map_and_sdr_workspace() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        c.wait("get sdr", 15, |r| field(r, "frames_seen") > 0.0);
        c.ok("sdr tune 99.4M");

        // The band plan: FM broadcast at the tuned frequency, in view.
        let b = c.wait("get bands", 5, |r| field(r, "tuned_hz") == 99.4e6);
        assert_eq!(raw(&b, "plan"), r#""general""#, "{b}");
        assert!(b.contains(r#""at_tuned":[{"name":"FM Broadcast""#), "{b}");

        // The workspace, from the tree: the SDR's keys, none of the scope's.
        let t = c.wait("get uitree", 10, |r| r.contains("band strip"));
        let buttons = labels(&t, "Button");
        for want in ["SCOPE", "SDR", "WFM", "Follow", "RF map"] {
            assert!(
                buttons.iter().any(|b| b == want),
                "{want} missing: {buttons:?}"
            );
        }
        for scope_only in ["CH1", "CH2", "AutoSetup", "Lvl 50%", "Pos 50%"] {
            assert!(
                !buttons.iter().any(|b| b == scope_only),
                "{scope_only} in SDR: {buttons:?}"
            );
        }
        assert!(!buttons.iter().any(|b| b == "Instrument"), "{buttons:?}");
        assert!(
            buttons
                .iter()
                .any(|b| b.starts_with("FM Broadcast · 87.5–108 MHz")),
            "no FM band in the strip: {buttons:?}"
        );
        for group in [
            "SDR canvas",
            "SDR dock",
            "RF minimap",
            "dock section Tuning",
        ] {
            assert!(
                t.contains(&format!(r#""label":"{group}""#)),
                "{group} missing"
            );
        }
        // Geometry, from the same tree: the strip and minimap live inside
        // the canvas, and neither ever reaches the dock rail (10.14.4).
        let canvas = rect_of(&t, "SDR canvas").expect("canvas rect");
        let dock = rect_of(&t, "SDR dock").expect("dock rect");
        for part in ["band strip", "RF minimap"] {
            let r = rect_of(&t, part).unwrap_or_else(|| panic!("{part} rect missing: {t}"));
            assert!(
                r[0] >= canvas[0] - 0.5
                    && r[2] <= canvas[2] + 0.5
                    && r[1] >= canvas[1] - 0.5
                    && r[3] <= canvas[3] + 0.5,
                "{part} {r:?} escapes the canvas {canvas:?}"
            );
            assert!(
                r[3] <= dock[1] + 0.5 && (r[2] <= dock[0] + 0.5 || r[0] >= dock[2] - 0.5),
                "{part} {r:?} overlaps the dock {dock:?}"
            );
        }

        // Band verbs: goto tunes to the band's centre and moves the window.
        c.ok("bandmap goto 2m Ham Band");
        let s = c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 146e6);
        assert_eq!(field(&s, "centre_hz"), 146e6, "{s}"); // general: 144–148 MHz
        let b = c.request("get bands");
        assert!(b.contains(r#""at_tuned":[{"name":"2m Ham Band""#), "{b}");
        c.ok("bandplan usa");
        c.wait("get bands", 5, |r| raw(r, "plan") == r#""usa""#);
        let bad = c.request("bandmap goto Nowhere Band");
        println!("{bad}");
        c.wait("get status", 5, |r| r.contains("no band"));

        // The RF map window joins the tree when opened.
        c.ok("bandmap window on");
        c.wait("get uitree", 10, |r| r.contains(r#""label":"RF map""#));
        c.ok("bandmap strip off");
        c.wait("get uitree", 10, |r| !r.contains("band strip"));

        // Back to the scope: its keys return, the radio's go.
        c.ok("instrument scope");
        let t = c.wait("get uitree", 15, |r| r.contains(r#""label":"CH1""#));
        let buttons = labels(&t, "Button");
        assert!(!buttons.iter().any(|b| b == "WFM"), "{buttons:?}");
        assert!(!t.contains("SDR canvas"));
    });
}
