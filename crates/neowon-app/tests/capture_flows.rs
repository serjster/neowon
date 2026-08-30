//! Phase 7 capture-workflow verification: spawn the real app with `--sim`
//! and a NEOWON_SCRIPT that records, saves/reloads captures, scrubs
//! history, and round-trips a session — then assert on the files.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test capture_flows -- --ignored

use std::path::Path;
use std::process::Command;

fn run(dir: &Path, script: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let script_path = dir.join("script.txt");
    std::fs::write(&script_path, script).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_SCRIPT", &script_path)
        .env_remove("NEOWON_SHOT")
        .status()
        .expect("launch app");
    assert!(status.success(), "app exited with {status}");
}

#[test]
#[ignore = "opens a window"]
fn capture_history_session_roundtrip() {
    let dir = std::env::temp_dir().join("neowon-capture-flows");
    let d = dir.display();
    run(
        &dir,
        &format!(
            "run 1\n\
             record 1\n\
             wait 1.0\n\
             record 0\n\
             capsave {d}/flow.nwc\n\
             refsave 0\n\
             ref on\n\
             history 0\n\
             history next\n\
             sessionsave {d}/flow.nws\n\
             vdiv 0 5.0\n\
             trigpos 0.25\n\
             sessionload {d}/flow.nws\n\
             wait 0.3\n\
             sessionsave {d}/flow2.nws\n\
             recordclear\n\
             capload {d}/flow.nwc\n\
             history 2\n\
             shot {d}/flow.png\n\
             wait 0.5\n\
             quit\n"
        ),
    );

    // The capture file reloads with identical frames.
    let frames = neowon_core::nwc::read(&dir.join("flow.nwc")).unwrap();
    assert!(frames.len() >= 5, "recorded only {} frames", frames.len());
    let f = &frames[0];
    assert_eq!(f.sample_rate, 250e3);
    assert!(f.channels.iter().any(|c| c.ch == 0));
    assert!(f.channels[0].raw.len() >= 1000);

    // Session round-trip: the mutations after the first save (vdiv 5,
    // trigpos 0.25) must be reverted by the load, so the second save is
    // identical to the first.
    let a = std::fs::read_to_string(dir.join("flow.nws")).unwrap();
    let b = std::fs::read_to_string(dir.join("flow2.nws")).unwrap();
    assert!(a.contains("vdiv 0 0.2"), "unexpected session: {a}");
    assert!(a.contains("trigpos 0.5"));
    assert_eq!(a, b, "session did not round-trip");

    // The PNG export is a real PNG of the plot.
    let png = std::fs::read(dir.join("flow.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(png.len() > 1000);
}

#[test]
#[ignore = "opens a window"]
fn vendor_cap_import_loads() {
    // Build a minimal vendor-format .cap (the byte-exact fixtures live in
    // neowon-core's owon_cap tests) and load it through the app.
    let dir = std::env::temp_dir().join("neowon-capture-flows-cap");
    std::fs::create_dir_all(&dir).unwrap();
    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(b"SPBVDS1022");
    for v in [100i32, 4, 0x03000000, 0, 10, 1] {
        f.extend_from_slice(&v.to_be_bytes());
    }
    let samples: Vec<i8> = (0..1000).map(|i| ((i % 250) - 125) as i8).collect();
    let mut frame: Vec<u8> = Vec::new();
    for v in [16i32, 0] {
        frame.extend_from_slice(&v.to_be_bytes()); // timebase 1 ms/div, trig pos
    }
    frame.push(0); // peak off
    frame.extend_from_slice(&0i32.to_be_bytes()); // DM len
    frame.push(0); // ch 0
    frame.extend_from_slice(&(40 + samples.len() as i32).to_be_bytes());
    for v in [
        0i32,
        0,
        samples.len() as i32,
        samples.len() as i32,
        0,
        0,
        4,
        1,
    ] {
        frame.extend_from_slice(&v.to_be_bytes());
    }
    frame.extend_from_slice(&1000f32.to_be_bytes());
    frame.extend_from_slice(&0.001f32.to_be_bytes());
    frame.extend(samples.iter().map(|&s| s as u8));
    f.extend_from_slice(&(frame.len() as i32).to_be_bytes());
    f.extend_from_slice(&frame);
    let cap = dir.join("vendor.cap");
    std::fs::write(&cap, &f).unwrap();

    let d = dir.display();
    run(
        &dir,
        &format!(
            "run 1\n\
             wait 0.3\n\
             capload {d}/vendor.cap\n\
             shot {d}/vendor.png\n\
             wait 0.5\n\
             quit\n"
        ),
    );
    let png = std::fs::read(dir.join("vendor.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}
