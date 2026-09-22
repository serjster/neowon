//! Whole-window capture: `shot` must produce the UI the operator sees, not a
//! black frame. The earlier `shot window` attempt wrote all-zero PNGs on
//! macOS/Metal (see phase10-sdr-spec §10.9); this test is the guard that the
//! capture can never silently regress to black.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test ui_capture -- --ignored

mod common;
use common::*;

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// `[x, y, w, h]` of the first node labelled `label` in a uitree JSON.
/// Rects are logical window pixels, like the `layout` dump.
fn find_rect(tree: &str, label: &str) -> [f64; 4] {
    let needle = format!("\"label\":\"{label}\"");
    let at = tree
        .find(&needle)
        .unwrap_or_else(|| panic!("no {label} node in tree"))
        + needle.len();
    let rest = &tree[at..];
    let start = rest.find("\"rect\":[").expect("rect after label") + 8;
    let end = rest[start..].find(']').expect("rect end");
    let nums: Vec<f64> = rest[start..start + end]
        .split(',')
        .map(|s| s.trim().parse().expect("rect number"))
        .collect();
    assert_eq!(nums.len(), 4, "rect needs four numbers: {nums:?}");
    [nums[0], nums[1], nums[2], nums[3]]
}

/// The logical window size from a uitree JSON.
fn window_size(tree: &str) -> (f64, f64) {
    let at = tree.find("\"window\":[").expect("window") + 10;
    let end = tree[at..].find(']').expect("window end");
    let nums: Vec<f64> = tree[at..at + end]
        .split(',')
        .map(|s| s.trim().parse().expect("window number"))
        .collect();
    (nums[0], nums[1])
}

struct Shot {
    w: usize,
    h: usize,
    px: Vec<[u8; 3]>,
}

impl Shot {
    fn load(path: &PathBuf) -> Self {
        let file = std::fs::File::open(path).unwrap();
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(
            info.color_type,
            png::ColorType::Rgb,
            "the shot writer must emit RGB PNGs"
        );
        let px = buf[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        Self {
            w: info.width as usize,
            h: info.height as usize,
            px,
        }
    }

    fn lit(&self, p: &[u8; 3], floor: u8) -> bool {
        p[0].max(p[1]).max(p[2]) > floor
    }
}

#[test]
#[ignore = "opens a window"]
fn a_window_shot_contains_the_sdr_ui() {
    let dir = std::env::temp_dir().join(format!("neowon-ui-capture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("window.png");

    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        // A non-scope view: the spectrum and waterfall fill the grid where
        // the scope plot would be; the dock is on the right.
        c.ok("stimulus rf-digital");
        c.ok("sdr tune 100.3M");
        // The waterfall pushes one row per IQ frame (WF_H = 320 rows); ~120
        // frames are enough to fill a third of it and still run in seconds.
        c.wait("get sdr", 20, |r| field(r, "frames_seen") > 120.0);

        let tree = c.wait("get uitree", 15, |r| {
            r.contains(r#""label":"waterfall""#) && r.contains(r#""label":"SDR dock""#)
        });
        let (win_w, win_h) = window_size(&tree);
        let wf = find_rect(&tree, "waterfall");
        let dock = find_rect(&tree, "SDR dock");

        let _ = std::fs::remove_file(&png);
        c.ok(&format!("shot {}", png.display()));
        let deadline = Instant::now() + Duration::from_secs(15);
        while !png.exists() {
            assert!(Instant::now() < deadline, "shot never written");
            std::thread::sleep(Duration::from_millis(100));
        }
        // The status line names the file that was written.
        let status = c.request("get status");
        assert!(
            status.contains("window.png") && status.contains(r#""shot":"#),
            "shot not reported in status: {status}"
        );

        let shot = Shot::load(&png);
        assert!(shot.w > 100 && shot.h > 100, "{}x{}", shot.w, shot.h);

        // 1. Not black: the window is mostly UI (menu, spectrum, waterfall,
        // front panel, dock), not the unwritten capture texture.
        let lit = shot.px.iter().filter(|p| shot.lit(p, 8)).count();
        let frac = lit as f64 / shot.px.len() as f64;
        assert!(
            frac > 0.5,
            "only {:.1}% of the capture is non-black",
            frac * 100.0
        );

        // The capture is physical pixels; the tree is logical pixels.
        let k = shot.w as f64 / win_w;
        let kh = shot.h as f64 / win_h;
        assert!(
            (k - kh).abs() < 1e-3,
            "capture aspect must match the window ({}x{} vs {win_w}x{win_h})",
            shot.w,
            shot.h
        );

        // 2. The dock's panel colour is present: its fill is rgb(22, 25, 31)
        // (ui/sdr_view.rs), painted behind every dock section.
        let (mut fill, mut total) = (0usize, 0usize);
        for row in (dock[1] * k) as usize..((dock[1] + dock[3]) * k) as usize {
            for col in (dock[0] * k) as usize..((dock[0] + dock[2]) * k) as usize {
                total += 1;
                let p = shot.px[row.min(shot.h - 1) * shot.w + col.min(shot.w - 1)];
                if p[0].abs_diff(22) <= 8 && p[1].abs_diff(25) <= 8 && p[2].abs_diff(31) <= 8 {
                    fill += 1;
                }
            }
        }
        assert!(
            fill * 10 > total,
            "dock fill rgb(22,25,31) missing: {fill}/{total} pixels"
        );

        // 3. The waterfall has content (the RF scene paints its rows).
        let (mut wf_lit, mut wf_total) = (0usize, 0usize);
        for row in (wf[1] * k) as usize..((wf[1] + wf[3]) * k) as usize {
            for col in (wf[0] * k) as usize..((wf[0] + wf[2]) * k) as usize {
                wf_total += 1;
                let p = shot.px[row.min(shot.h - 1) * shot.w + col.min(shot.w - 1)];
                if shot.lit(&p, 16) {
                    wf_lit += 1;
                }
            }
        }
        let wf_frac = wf_lit as f64 / wf_total as f64;
        assert!(
            wf_frac > 0.15,
            "waterfall area is black: {:.1}% lit of {wf_total} pixels",
            wf_frac * 100.0
        );
    });
}

/// The luminance row `i` of the 320-row waterfall texture, sampled at the
/// centre of the texture row, across the rect's full width.
fn waterfall_row(shot: &Shot, wf: [f64; 4], k: f64, i: usize) -> Vec<f64> {
    let (x0, x1) = ((wf[0] * k) as usize, ((wf[0] + wf[2]) * k) as usize);
    let h = wf[3] * k / 320.0;
    let y = ((wf[1] * k) + (i as f64 + 0.5) * h) as usize;
    (x0..x1)
        .map(|x| {
            let p = shot.px[y.min(shot.h - 1) * shot.w + x.min(shot.w - 1)];
            0.25 * p[0] as f64 + 0.35 * p[1] as f64 + 0.40 * p[2] as f64
        })
        .collect()
}

/// A stationary carrier must stay in its waterfall column. The operator's
/// live bands move because the *channel* moves (an SFN fading pattern
/// translating through the band), not because the display's frequency axis
/// does; this test pins the display half of that distinction, and asserts
/// the carrier also lands where the tuned-frequency mapping says it should.
#[test]
#[ignore = "opens a window"]
fn a_stationary_carrier_stays_in_its_waterfall_column() {
    let dir = std::env::temp_dir().join(format!("neowon-wf-stability-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("tone.png");

    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        // One tone at +100 kHz (rf-reference), on a 1 MHz span so its column
        // is not a max-pool boundary; then let the whole waterfall (320 rows)
        // refill at that span.
        c.ok("stimulus rf-reference");
        c.ok("sdr span 1M");
        let before = field(&c.request("get sdr"), "frames_seen");
        let st = c.wait("get sdr", 40, |r| field(r, "frames_seen") > before + 340.0);
        let (centre, span) = (field(&st, "centre_hz"), field(&st, "span_hz"));
        let pan = field(&st, "pan_hz");
        assert_eq!(span, 1e6, "span did not take: {st}");

        // 100.1 MHz is the rf-reference tone; the window is centred on the
        // hardware centre (pan 0), so its display column is analytic.
        let want_col = 512.0 + (100.1e6 - (centre + pan)) / (span / 1024.0);

        let tree = c.wait("get uitree", 15, |r| r.contains(r#""label":"waterfall""#));
        let (win_w, _) = window_size(&tree);
        let wf = find_rect(&tree, "waterfall");
        assert!(wf[2] > 100.0 && wf[3] > 100.0, "waterfall rect {wf:?}");

        let _ = std::fs::remove_file(&png);
        c.ok(&format!("shot {}", png.display()));
        let deadline = Instant::now() + Duration::from_secs(15);
        while !png.exists() {
            assert!(Instant::now() < deadline, "shot never written");
            std::thread::sleep(Duration::from_millis(100));
        }
        // The shot is asynchronous; the status line names it only once the
        // file is complete, so a read cannot race the writer.
        c.wait("get status", 15, |r| r.contains("tone.png"));
        let shot = Shot::load(&png);
        let k = shot.w as f64 / win_w;
        let rows: Vec<Vec<f64>> = (0..320).map(|i| waterfall_row(&shot, wf, k, i)).collect();
        let n = rows[0].len();

        // The tone is the brightest thing in the mean row.
        let mean: Vec<f64> = (0..n)
            .map(|x| rows.iter().map(|r| r[x]).sum::<f64>() / rows.len() as f64)
            .collect();
        let peak = (0..n).max_by(|a, b| mean[*a].total_cmp(&mean[*b])).unwrap();
        let col_of = |px: f64| px / n as f64 * 1024.0 - 0.5;
        assert!(
            (col_of(peak as f64) - want_col).abs() < 2.0,
            "tone column {} vs expected {want_col}",
            col_of(peak as f64)
        );

        // Sub-column position per row: the centroid of the tone's lobe.
        let (lo, hi) = (peak.saturating_sub(6), (peak + 6).min(n - 1));
        let mut pos = Vec::with_capacity(320);
        for r in &rows {
            let seg = &r[lo..=hi];
            let (min, max) = seg
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                    (a.min(v), b.max(v))
                });
            let floor = min + 0.2 * (max - min);
            let w: Vec<f64> = seg.iter().map(|v| (v - floor).max(0.0)).collect();
            let sum: f64 = w.iter().sum();
            let at = if sum == 0.0 {
                (lo + hi) as f64 / 2.0
            } else {
                w.iter()
                    .enumerate()
                    .map(|(i, ww)| (lo + i) as f64 * ww)
                    .sum::<f64>()
                    / sum
            };
            pos.push(col_of(at));
        }

        // Fit pos = a*row + b: the sim tone must not walk columns.
        let nrows = pos.len() as f64;
        let mean_row = (nrows - 1.0) / 2.0;
        let mean_pos = pos.iter().sum::<f64>() / nrows;
        let (mut num, mut den) = (0.0, 0.0);
        for (i, p) in pos.iter().enumerate() {
            num += (i as f64 - mean_row) * (p - mean_pos);
            den += (i as f64 - mean_row).powi(2);
        }
        let slope = num / den;
        assert!(
            slope.abs() < 0.01,
            "stationary carrier drifts {slope:+.4} columns/row ({} Hz/row)",
            slope * 2000.0
        );
        assert!(
            (mean_pos - want_col).abs() < 2.0,
            "carrier sits at {mean_pos:.2} vs expected {want_col:.2}"
        );
    });
}
