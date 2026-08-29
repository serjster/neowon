//! UI-level layout verification (Phase 6.5 pillar 3): drive the real app with
//! a `NEOWON_SCRIPT`, open every dialog, dump the named-ROI map via
//! `layout`, and assert the published geometry.
//!
//! These need a window (briefly) so they are `#[ignore]` by default:
//!   cargo test -p neowon-app --test ui_layout -- --ignored
//!
//! The geometry under test is computed at runtime from the window size
//! (`src/ui/layout.rs`); this test asserts the relational invariants of the
//! published map and proves every dialog is script-openable.
//!
//! For manual full-window visual checks you can additionally grab the
//! window with `screencapture -R x,y,w,h` (macOS) — not asserted here.

use std::path::PathBuf;
use std::process::Command;

// Fixed chrome sizes — must match src/ui/layout.rs; everything else is
// asserted relationally against the dumped window size.
const WINDOW_W: f64 = 1520.0;
const WINDOW_H: f64 = 820.0;
const MENU_H: f64 = 36.0;
const FRONT_PANEL_H: f64 = 96.0;
const DIALOG_W: f64 = 320.0;
const DESC_H: f64 = 54.0;
const DESC_GAP: f64 = 4.0;
const MARGIN: f64 = 8.0;

const MENUS: [&str; 9] = [
    "horizontal",
    "trigger",
    "acquire",
    "display",
    "measure",
    "math",
    "cursor",
    "utility",
    "channel 0",
];

fn run_layout_script(name: &str, script: &str) -> Vec<PathBuf> {
    let dir = std::env::temp_dir().join(format!("neowon-uilayout-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut outs = Vec::new();
    let script_text: String = script
        .lines()
        .map(|l| {
            let l = l.trim();
            if let Some(rest) = l.strip_prefix("layout ") {
                let mut parts = rest.split_whitespace();
                let file = dir.join(parts.next().unwrap());
                outs.push(file.clone());
                format!("layout {}\n", file.display())
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let script_path = dir.join("script.txt");
    std::fs::write(&script_path, script_text).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_SCRIPT", &script_path)
        .env_remove("NEOWON_SHOT")
        .status()
        .expect("launch app");
    assert!(status.success(), "app exited with {status}");
    for o in &outs {
        assert!(o.exists(), "layout {} was not written", o.display());
    }
    outs
}

/// Extract `"name": [x, y, w, h]` from the dump.
fn roi(json: &str, name: &str) -> [f64; 4] {
    let key = format!("\"{name}\": [");
    let start = json
        .find(&key)
        .unwrap_or_else(|| panic!("no {name} in {json}"))
        + key.len();
    let end = json[start..].find(']').unwrap() + start;
    let nums: Vec<f64> = json[start..end]
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    [nums[0], nums[1], nums[2], nums[3]]
}

fn open_menu(json: &str) -> Option<String> {
    let key = "\"menu\": ";
    let start = json.find(key).unwrap() + key.len();
    let end = json[start..].find(',').unwrap() + start;
    let v = json[start..end].trim();
    if v == "null" {
        None
    } else {
        Some(v.trim_matches('"').to_string())
    }
}

#[test]
#[ignore = "needs a window; run with -- --ignored"]
fn every_dialog_opens_and_geometry_holds() {
    let mut script = String::from("stimulus probe-comp\nwait 0.7\n");
    for (i, m) in MENUS.iter().enumerate() {
        script.push_str(&format!("menu {m}\nwait 0.15\nlayout step{i}.json\n"));
    }
    script.push_str("menu none\nwait 0.1\nlayout final.json\nquit\n");

    let outs = run_layout_script("all-menus", &script);
    assert_eq!(outs.len(), MENUS.len() + 1);

    // Each step records the dialog we asked for.
    let expected = [
        "horizontal",
        "trigger",
        "acquire",
        "display",
        "measure",
        "math",
        "cursor",
        "utility",
        "channel0",
    ];
    for (i, out) in outs[..MENUS.len()].iter().enumerate() {
        let json = std::fs::read_to_string(out).unwrap();
        assert_eq!(
            open_menu(&json).as_deref(),
            Some(expected[i]),
            "step {i} menu mismatch"
        );
    }
    // After `menu none` the dialog is collapsed.
    let final_json = std::fs::read_to_string(&outs[MENUS.len()]).unwrap();
    assert_eq!(open_menu(&final_json), None);

    // Published geometry, checked against the last dump. The plot fills
    // the middle area (runtime layout); the dialog overlays its right side.
    let plot = roi(&final_json, "plot");
    assert!((plot[0] - MARGIN).abs() < 0.5);
    assert!((plot[2] - (WINDOW_W - 2.0 * MARGIN)).abs() < 0.5);
    let expect_h = WINDOW_H - MENU_H - FRONT_PANEL_H - DESC_H - DESC_GAP - 2.0 * MARGIN;
    assert!((plot[3] - expect_h).abs() < 0.5, "plot h {}", plot[3]);

    let menu_bar = roi(&final_json, "menu_bar");
    assert!((menu_bar[2] - WINDOW_W).abs() < 0.5);
    assert!((menu_bar[3] - MENU_H).abs() < 0.5);

    let fp = roi(&final_json, "front_panel");
    assert!((fp[2] - WINDOW_W).abs() < 0.5);
    assert!((fp[3] - FRONT_PANEL_H).abs() < 0.5);
    assert!((fp[1] - (WINDOW_H - FRONT_PANEL_H)).abs() < 0.5);

    let dialog = roi(&final_json, "dialog");
    assert!((dialog[2] - DIALOG_W).abs() < 0.5);
    assert!((dialog[0] - (WINDOW_W - DIALOG_W)).abs() < 0.5);

    // Descriptors hug the plot bottom.
    let desc = roi(&final_json, "descriptors");
    assert!((desc[3] - DESC_H).abs() < 0.5);
    assert!((desc[1] - (plot[1] + plot[3] + DESC_GAP)).abs() < 0.5);
    assert!((desc[0] - plot[0]).abs() < 0.5);

    // Window sanity.
    let win = roi(&final_json, "menu_bar");
    assert!(win[0] >= 0.0 && win[1] >= 0.0);
}

#[test]
#[ignore = "needs a window; run with -- --ignored"]
fn plot_center_in_window() {
    let outs = run_layout_script("center", "menu none\nwait 0.5\nlayout c.json\nquit\n");
    let json = std::fs::read_to_string(&outs[0]).unwrap();
    let plot = roi(&json, "plot");
    let cx = plot[0] + plot[2] / 2.0;
    let cy = plot[1] + plot[3] / 2.0;
    assert!(cx > 0.0 && cx < WINDOW_W);
    assert!(cy > 0.0 && cy < WINDOW_H);
    // The plot must not overlap the front panel or menu bar.
    assert!(plot[1] >= MENU_H);
    assert!(plot[1] + plot[3] <= WINDOW_H - FRONT_PANEL_H);
}
