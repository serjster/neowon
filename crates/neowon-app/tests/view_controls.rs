//! View-control verification through the control socket: one
//! app launch, drive zoom/pan/home over TCP, assert the config state and
//! the rendered pixels.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test view_controls -- --ignored

mod common;
use common::*;

use std::path::PathBuf;
use std::time::{Duration, Instant};

impl Conn {
    fn wait_config(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let cfg = self.request("get config");
            if cfg.contains(needle) {
                return cfg;
            }
            assert!(
                Instant::now() < deadline,
                "config never saw {needle}: {cfg}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn load_ppm(path: &PathBuf) -> (usize, usize, Vec<[u8; 3]>) {
    let data = std::fs::read(path).unwrap();
    let header_end = data
        .windows(4)
        .position(|w| w == b"255\n")
        .expect("ppm header")
        + 4;
    let header = std::str::from_utf8(&data[..header_end]).unwrap();
    let mut dims = header.lines().nth(1).unwrap().split_whitespace();
    let w: usize = dims.next().unwrap().parse().unwrap();
    let h: usize = dims.next().unwrap().parse().unwrap();
    let px = data[header_end..].as_chunks::<3>().0.to_vec();
    assert_eq!(px.len(), w * h);
    (w, h, px)
}

/// Mean row of the lit pixels (the trace).
fn mean_row(path: &PathBuf) -> f64 {
    let (w, _h, px) = load_ppm(path);
    let lit: Vec<usize> = px
        .iter()
        .enumerate()
        .filter(|(_, p)| p[0] as u16 + p[1] as u16 + p[2] as u16 > 60)
        .map(|(i, _)| i / w)
        .collect();
    assert!(lit.len() > 500, "only {} lit pixels", lit.len());
    lit.iter().sum::<usize>() as f64 / lit.len() as f64
}

#[test]
#[ignore = "opens a window"]
fn view_controls_move_the_window_and_the_pixels() {
    let dir = scratch("viewctrl");
    let (mut child, mut conn) = launch(&["--sim"], &[]);

    // Shot via socket, polled to disk, retried while the display is still
    // blank (startup races the first readback).
    let shot = |conn: &mut Conn, path: &PathBuf| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let _ = std::fs::remove_file(path);
            assert!(
                conn.request(&format!("shotplot {}", path.display()))
                    .contains(r#""ok":true"#)
            );
            let file_deadline = Instant::now() + Duration::from_secs(10);
            while !path.exists() {
                assert!(Instant::now() < file_deadline, "shot never written");
                std::thread::sleep(Duration::from_millis(100));
            }
            let (_w, _h, px) = load_ppm(path);
            let lit = px
                .iter()
                .filter(|p| p[0] as u16 + p[1] as u16 + p[2] as u16 > 60)
                .count();
            if lit > 500 || Instant::now() > deadline {
                assert!(lit > 500, "display stayed blank");
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let status = conn.request("get status");
            if status.contains(r#""frames_seen":0"#) {
                assert!(Instant::now() < deadline, "no frames: {status}");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            break;
        }
        // Deterministic flat trace: 1 V DC at 0.5 V/div -> row ~125.
        for cmd in [
            "stimulus dc-1v",
            "vdiv 0 0.5",
            "enable 1 0",
            "persist off",
            "mode vectors",
        ] {
            assert!(conn.request(cmd).contains(r#""ok":true"#), "{cmd}");
        }
        conn.wait_config(r#""volts_div":0.5"#);
        std::thread::sleep(Duration::from_millis(800));

        // The time base control still walks the rate ladder all the way down
        // into seconds per division — that is the acquisition control.
        assert!(conn.request("timebase 4").contains(r#""ok":true"#));
        conn.wait_config(r#""sample_rate":125,"#);

        // Zooming out, though, must NOT cost sample rate: past the record it
        // engages the timeline and spans history instead. This is the whole
        // point of the feature, so it is asserted rather than described.
        assert!(conn.request("timebase 0.002").contains(r#""ok":true"#));
        conn.wait_config(r#""sample_rate":250000"#);
        assert!(conn.request("hzoom out").contains(r#""ok":true"#));
        let cfg = conn.wait_config(r#""deep":{"on":true"#);
        assert!(
            cfg.contains(r#""sample_rate":250000"#),
            "zooming out must keep the sample rate: {cfg}"
        );
        // Zooming back in hands the display to the record again — stop as
        // soon as it does, since further zoom-ins then narrow the record's
        // own window, which is the correct next behaviour.
        let mut cfg = String::new();
        for _ in 0..14 {
            assert!(conn.request("hzoom in").contains(r#""ok":true"#));
            cfg = conn.request("get config");
            if cfg.contains(r#""deep":{"on":false"#) {
                break;
            }
        }
        assert!(
            cfg.contains(r#""deep":{"on":false"#),
            "deep never released: {cfg}"
        );
        assert!(cfg.contains(r#""sample_rate":250000"#), "{cfg}");
        // Horizontal position is the trigger delay while unzoomed.
        assert!(conn.request("trigpos 0.5").contains(r#""ok":true"#));
        conn.wait_config(r#""trigger_position":0.5"#);
        assert!(conn.request("pan right").contains(r#""ok":true"#));
        let cfg = conn.wait_config(r#""trigger_position":0.6"#);
        assert!(cfg.contains(r#""hview":[0.5,1]"#), "{cfg}");
        // Back to a sane rate, then switch the zoom window on: the same
        // gesture now scales the window and leaves the rate alone.
        assert!(conn.request("timebase 0.002").contains(r#""ok":true"#));
        conn.wait_config(r#""sample_rate":250000"#);
        assert!(conn.request("zoomwin on").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.5,0.5]"#);
        assert!(conn.request("hzoom in").contains(r#""ok":true"#));
        let cfg = conn.wait_config(r#""hview":[0.5,0.25]"#);
        assert!(cfg.contains(r#""sample_rate":250000"#), "{cfg}");
        assert!(conn.request("zoomwin off").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.5,1]"#);

        // hview lands in the config JSON (window [0, 0.5]).
        assert!(conn.request("hview 0.25 0.5").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.25,0.5]"#);
        // hzoom halves the span about the window's centre, which stays put
        // (the manuals' rescaling reference point).
        assert!(conn.request("hzoom in").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.25,0.25]"#);
        // pan left/right slides the window a tenth of its span (±0.025).
        assert!(conn.request("pan left").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.275,0.25]"#);
        assert!(conn.request("pan right").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.25,0.25]"#);

        // Pixel level: pan up shifts the DC trace, home restores it.
        let base = dir.join("base.ppm");
        let panned = dir.join("panned.ppm");
        let homed = dir.join("homed.ppm");
        shot(&mut conn, &base);

        assert!(conn.request("pan up").contains(r#""ok":true"#));
        std::thread::sleep(Duration::from_millis(600));
        shot(&mut conn, &panned);

        assert!(conn.request("home").contains(r#""ok":true"#));
        conn.wait_config(r#""hview":[0.5,1]"#);
        // Home restores the startup V/div (0.2); at that scale a 1 V DC
        // trace sits outside the +-4-div window. Re-apply the test scale
        // so the restored centring is visible.
        assert!(conn.request("vdiv 0 0.5").contains(r#""ok":true"#));
        conn.wait_config(r#""volts_div":0.5"#);
        std::thread::sleep(Duration::from_millis(600));
        shot(&mut conn, &homed);

        let (b, p, h) = (mean_row(&base), mean_row(&panned), mean_row(&homed));
        assert!((b - 125.0).abs() < 8.0, "base row {b:.1}");
        assert!(
            p < b - 40.0,
            "pan up did not move the trace: {p:.1} vs {b:.1}"
        );
        assert!(
            (h - b).abs() < 8.0,
            "home did not restore: {h:.1} vs {b:.1}"
        );
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
