//! UI geometry verification: the app dumps the rects it
//! actually painted, and this asserts the invariant that made the bug
//! visible — **no chrome may overlap the waveform grid**, whatever is
//! expanded, at any window size or UI scale.
//!
//! Geometry-as-JSON rather than pixel diffing: the assertion is exact, says
//! which region broke the rule and by how many pixels, and survives every
//! restyle. Pixel tests (ui_pixels, effects_pixels) still cover what the
//! trace itself looks like.
//!
//! Opens a window, so `#[ignore]` by default:
//!   cargo test -p neowon-app --test ui_geometry -- --ignored

mod common;
use common::*;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// Overlap area with `other`, in square pixels (0 = disjoint).
    fn overlap(&self, other: &Rect) -> f32 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        w * h
    }
}

/// Minimal reader for the `layout` dump — the app emits JSON by hand (it
/// stays serde-free), so the test parses by hand too.
fn rect_in(json: &str, section: &str, name: &str) -> Option<Rect> {
    let sec = json.split(&format!("\"{section}\": {{")).nth(1)?;
    let sec = sec.split("\n  }").next()?;
    let v = sec.split(&format!("\"{name}\": [")).nth(1)?;
    let nums: Vec<f32> = v
        .split(']')
        .next()?
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    Some(Rect {
        x: nums[0],
        y: nums[1],
        w: nums[2],
        h: nums[3],
    })
}

fn dump(conn: &mut Conn, path: &Path) -> String {
    let _ = std::fs::remove_file(path);
    conn.ok(&format!("layout {}", path.display()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "layout dump never written");
        std::thread::sleep(Duration::from_millis(100));
    }
    // Written atomically (`neowon_core::atomic_file`): present is whole.
    std::fs::read_to_string(path).unwrap()
}

const SECTIONS: [&str; 11] = [
    "trigger",
    "horizontal",
    "acquire",
    "channel 0",
    "channel 1",
    "math",
    "measure",
    "cursor",
    "display",
    "utility",
    "record",
];

#[test]
#[ignore = "opens a window"]
fn no_panel_ever_covers_the_plot() {
    let dir = scratch("uigeom");
    let (mut child, mut conn) = launch(
        &["--sim"],
        &[("NEOWON_WINDOW", "1520x820"), ("NEOWON_UI_SCALE", "1.0")],
    );
    conn.set_timeout(15);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let path: PathBuf = dir.join("layout.json");

        // Window size x UI scale matrix: 1080p and a 4K-class window at the
        // hi-DPI scale the app picks for such a panel.
        for (win, scale) in [
            ("1520x820", "1.0"),
            ("1920x1080", "1.0"),
            ("1920x1080", "1.5"),
            ("2688x1512", "2.0"),
        ] {
            conn.ok(&format!("window {win}"));
            conn.ok(&format!("uiscale {scale}"));
            std::thread::sleep(Duration::from_millis(500));

            for section in SECTIONS {
                conn.ok(&format!("menu {section}"));
                std::thread::sleep(Duration::from_millis(250));
                let json = dump(&mut conn, &path);
                let plot = rect_in(&json, "rois", "plot")
                    .unwrap_or_else(|| panic!("no plot roi in {json}"));

                // The promise: the ROI map says the dock sits beside the
                // plot. The check: what the dock *painted* honours it.
                for region in ["dialog", "menu_bar", "front_panel"] {
                    let Some(r) = rect_in(&json, "painted", region) else {
                        panic!("{region} missing from painted dump: {json}");
                    };
                    let over = r.overlap(&plot);
                    assert!(
                        over < 1.0,
                        "{win}@{scale} with '{section}' open: {region} painted {r:?} \
                         overlapping plot {plot:?} by {over:.0} px^2",
                    );
                }

                // The dock also has to stay inside its own allotted rail —
                // an Area that spills is one restyle away from covering the
                // trigger marker again.
                let rail = rect_in(&json, "rois", "dialog").unwrap();
                let painted = rect_in(&json, "painted", "dialog").unwrap();
                assert!(
                    painted.x >= rail.x - 1.0
                        && painted.right() <= rail.right() + 1.0
                        && painted.y >= rail.y - 1.0
                        && painted.bottom() <= rail.bottom() + 1.0,
                    "{win}@{scale} with '{section}' open: dock painted {painted:?} \
                     outside its rail {rail:?}",
                );
            }
        }
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
