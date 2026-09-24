//! The app saves its state and comes back the way it
//! was left — UI scale, window size, dock sections, SDR tuning and
//! demodulator — while scripted runs never touch the file, and a saved
//! value the instrument cannot take is dropped with a status line.
//!
//!   cargo test -p neowon-app --test state_persist -- --ignored

mod common;
use common::*;

use std::time::{Duration, Instant};

fn tmp(name: &str) -> std::path::PathBuf {
    scratch(&format!("state-{name}"))
}

/// Ask the app to quit and wait for it to exit by itself (the exit save
/// runs on the way out).
fn quit(mut child: App, c: &mut Conn) {
    let _ = c.request("quit");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("app did not quit");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn layout(c: &mut Conn, dir: &std::path::Path) -> String {
    let path = dir.join("layout.json");
    let _ = std::fs::remove_file(&path);
    c.ok(&format!("layout {}", path.display()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "no layout dump");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::read_to_string(&path).unwrap()
}

#[test]
#[ignore = "opens a window"]
fn state_survives_a_restart() {
    let dir = tmp("restart");
    let state = dir.join("state.nws");
    let st = state.to_str().unwrap();
    let env = [("NEOWON_STATE", st)];

    // First run: change things, then quit.
    let (child, mut c) = launch(&["--sdr-sim"], &env);
    c.wait("get sdr", 15, |r| {
        r.contains(r#""active":true"#) && field(r, "frames_seen") > 0.0
    });
    c.ok("uiscale 1.25");
    c.ok("window 1500x900");
    c.ok("dock measure,trigger");
    c.ok("sdr tune 100.3M");
    c.ok("sdr demod wfm");
    c.ok("sdr span 500k");
    c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 100.3e6);
    quit(child, &mut c);
    let text = std::fs::read_to_string(&state).expect("state written on exit");
    for line in [
        "uiscale 1.25",
        "window 1500x900",
        "dock measure,trigger",
        "sdr tune 100300000",
        "sdr demod wfm",
        "sdr span 500000",
    ] {
        assert!(text.lines().any(|l| l == line), "{line:?} missing:\n{text}");
    }
    assert!(!text.contains("stimulus "), "{text}");

    // Second run: everything is back.
    let (child, mut c) = launch(&["--sdr-sim"], &env);
    with_app(child, || {
        let r = c.wait("get sdr", 15, |r| field(r, "tuned_hz") == 100.3e6);
        assert_eq!(raw(&r, "centre_hz"), "100000000", "{r}");
        c.wait("get audio", 5, |r| r.contains(r#""demod":"wfm""#));
        let l = layout(&mut c, &dir);
        assert_eq!(field(&l, "scale"), 1.25, "{l}");
        assert!(l.contains(r#""window": [1500.0, 900.0]"#), "{l}");
        assert!(l.contains(r#""menu": "measure,trigger""#), "{l}");
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// The workspace mode (SCOPE | SDR) is part
/// of the saved state — the launch flags still pick the family, the file
/// picks which of the family's two instruments comes up.
#[test]
#[ignore = "opens a window"]
fn the_workspace_mode_survives_a_restart() {
    let dir = tmp("workspace");
    let state = dir.join("state.nws");
    let st = state.to_str().unwrap();

    // A scope launch (`--sim`) left in SDR mode.
    let (child, mut c) = launch(&["--sim"], &[("NEOWON_STATE", st)]);
    c.wait("get status", 15, |r| field(r, "frames_seen") > 0.0);
    c.ok("instrument sdr");
    c.wait("get sdr", 15, |r| {
        r.contains(r#""active":true"#) && field(r, "frames_seen") > 0.0
    });
    c.ok("sdr tune 100.3M");
    c.wait("get sdr", 5, |r| field(r, "tuned_hz") == 100.3e6);
    quit(child, &mut c);
    let text = std::fs::read_to_string(&state).expect("state written on exit");
    assert!(text.lines().any(|l| l == "instrument sdr"), "{text}");

    // The same scope launch comes back in SDR mode, with the tuning.
    let (child, mut c) = launch(&["--sim"], &[("NEOWON_STATE", st)]);
    c.wait("get sdr", 15, |r| {
        r.contains(r#""active":true"#) && field(r, "frames_seen") > 0.0
    });
    let r = c.request("get sdr");
    assert_eq!(field(&r, "tuned_hz"), 100.3e6, "{r}");
    // Left in scope mode this time.
    c.ok("instrument scope");
    c.wait("get sdr", 10, |r| r.contains(r#""active":false"#));
    c.wait("get status", 10, |r| field(r, "frames_seen") > 0.0);
    quit(child, &mut c);
    let text = std::fs::read_to_string(&state).expect("state written on exit");
    assert!(text.lines().any(|l| l == "instrument scope"), "{text}");

    // A launch on the other family member (`--sdr-sim`) honours the saved
    // scope mode: the flag picks sim-vs-hardware, the file picks the mode.
    let (child, mut c) = launch(&["--sdr-sim"], &[("NEOWON_STATE", st)]);
    with_app(child, || {
        c.wait("get sdr", 15, |r| r.contains(r#""active":false"#));
        c.wait("get status", 10, |r| field(r, "frames_seen") > 0.0);
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "opens a window"]
fn env_wins_over_the_saved_scale() {
    let dir = tmp("env");
    let state = dir.join("state.nws");
    std::fs::write(&state, "uiscale 2\nwindow 1400x800\n").unwrap();
    let (child, mut c) = launch(
        &["--sim"],
        &[
            ("NEOWON_STATE", state.to_str().unwrap()),
            ("NEOWON_UI_SCALE", "1.0"),
        ],
    );
    with_app(child, || {
        c.wait("get status", 15, |r| field(r, "frames_seen") > 0.0);
        let l = layout(&mut c, &dir);
        assert_eq!(field(&l, "scale"), 1.0, "{l}");
        assert!(l.contains(r#""window": [1400.0, 800.0]"#), "{l}");
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "opens a window"]
fn an_impossible_saved_value_is_dropped_with_a_status_line() {
    let dir = tmp("caps");
    let state = dir.join("state.nws");
    std::fs::write(&state, "sdr tune 5G\nsdr span 200k\n").unwrap();
    let (child, mut c) = launch(&["--sdr-sim"], &[("NEOWON_STATE", state.to_str().unwrap())]);
    with_app(child, || {
        let s = c.wait("get status", 15, |r| r.contains("outside"));
        assert!(s.contains("5000000000"), "{s}");
        // The valid line still applied.
        c.wait("get sdr", 5, |r| field(r, "span_hz") == 200e3);
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "opens a window"]
fn a_scripted_run_writes_no_state() {
    let dir = tmp("script");
    let state = dir.join("state.nws");
    let script = dir.join("run.nws");
    std::fs::write(&script, "uiscale 1.5\nwait 3\nquit\n").unwrap();
    let sandbox = Sandbox::new("state-script");
    let status = sandbox
        .command(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_SCRIPT", &script)
        .env("NEOWON_STATE", &state)
        .env_remove("NEOWON_NO_STATE")
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!state.exists(), "a scripted run wrote {}", state.display());
    let _ = std::fs::remove_dir_all(&dir);
}
