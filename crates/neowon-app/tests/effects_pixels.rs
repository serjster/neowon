//! User-effect verification: apply the shipped `invert` effect and assert
//! the readback is the color inverse of the unaffected display.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test effects_pixels -- --ignored

use std::path::PathBuf;
use std::process::Command;

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
    (w, h, px)
}

#[test]
#[ignore = "opens a window"]
fn invert_effect_inverts_the_display() {
    let dir = std::env::temp_dir().join("neowon-effects-test");
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display();
    let script = format!(
        "stimulus sine-1k\n\
         persist inf\n\
         crt 0\n\
         run 1\n\
         wait 1.0\n\
         run 0\n\
         wait 0.3\n\
         shot {d}/plain.ppm\n\
         effect invert\n\
         wait 2.0\n\
         shot {d}/inverted.ppm\n\
         wait 0.5\n\
         quit\n"
    );
    let script_path = dir.join("script.txt");
    std::fs::write(&script_path, script).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_SCRIPT", &script_path)
        // The shipped examples live in the repo's assets dir.
        .env(
            "NEOWON_SHADER_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/user"),
        )
        .env_remove("NEOWON_SHOT")
        .status()
        .expect("launch app");
    assert!(status.success(), "app exited with {status}");

    let (w, h, plain) = load_ppm(&dir.join("plain.ppm"));
    let (w2, h2, inv) = load_ppm(&dir.join("inverted.ppm"));
    assert_eq!((w, h), (w2, h2));
    // Acquisition is stopped, so both shots show the identical frame; the
    // effect must be an exact per-channel inversion.
    let mut worst = 0i32;
    for (a, b) in plain.iter().zip(&inv) {
        for c in 0..3 {
            worst = worst.max(((255 - a[c] as i32) - b[c] as i32).abs());
        }
    }
    assert!(worst <= 2, "not an inversion (worst channel delta {worst})");
    // Sanity: the plain shot actually contains a trace (non-black pixels).
    assert!(plain.iter().any(|p| p[0] > 32 || p[1] > 32));
}
