//! Pixel-level rendering verification: spawn the real app with `--sim` and a
//! NEOWON_SCRIPT, read back the plot texture, assert on the pixels.
//!
//! These need a window (briefly) so they are `#[ignore]` by default:
//!   cargo test -p neowon-app --test ui_pixels -- --ignored
//!
//! The plot texture contains ONLY the waveform render (graticule and
//! cursors are gizmo overlays), so the assertions see pure signal.

use std::path::PathBuf;
use std::process::Command;

const PLOT_W: usize = 1000;
const PLOT_H: usize = 500;

fn run_script(name: &str, script: &str) -> Vec<PathBuf> {
    let dir = std::env::temp_dir().join(format!("neowon-uitest-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    // Rewrite `shot NAME ...` to absolute paths, collect them.
    let mut shots = Vec::new();
    let script_text: String = script
        .lines()
        .map(|l| {
            let l = l.trim();
            if let Some(rest) = l.strip_prefix("shot ") {
                let mut parts = rest.split_whitespace();
                let file = dir.join(parts.next().unwrap());
                shots.push(file.clone());
                let tail: Vec<&str> = parts.collect();
                format!("shot {} {}\n", file.display(), tail.join(" "))
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
    for s in &shots {
        assert!(s.exists(), "shot {} was not written", s.display());
    }
    shots
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
    let px = data[header_end..]
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect::<Vec<_>>();
    assert_eq!(px.len(), w * h);
    (w, h, px)
}

/// Pixels meaningfully brighter than the background.
fn lit(px: &[[u8; 3]], w: usize) -> Vec<(usize, usize)> {
    px.iter()
        .enumerate()
        .filter(|(_, p)| p[0] as u16 + p[1] as u16 + p[2] as u16 > 60)
        .map(|(i, _)| (i % w, i / w))
        .collect()
}

#[test]
#[ignore = "needs a window; run with -- --ignored"]
fn dc_level_renders_at_expected_row() {
    // 1 V DC on a 0.5 V/div range (5 V full scale): raw = 50 counts above
    // center. Display window is +-4 div = +-100 counts -> row
    // (0.5 - 50/200) * (H-1) ~= 125.
    let shots = run_script(
        "dc",
        r#"
        stimulus dc-1v
        vdiv 0 0.5
        enable 1 0
        persist off
        mode vectors
        wait 1.5
        shot dc.ppm
        quit
        "#,
    );
    let (w, _h, px) = load_ppm(&shots[0]);
    let lit = lit(&px, w);
    assert!(lit.len() > 500, "only {} lit pixels", lit.len());
    let mean_row = lit.iter().map(|&(_, y)| y as f64).sum::<f64>() / lit.len() as f64;
    let expect = (0.5 - 50.0 / 200.0) * (PLOT_H as f64 - 1.0);
    assert!(
        (mean_row - expect).abs() < 5.0,
        "trace at row {mean_row:.1}, expected {expect:.1}"
    );
    // A DC trace is flat: rows cluster tightly.
    let spread = lit
        .iter()
        .map(|&(_, y)| (y as f64 - mean_row).abs())
        .fold(0.0f64, f64::max);
    assert!(spread < 8.0, "trace spread {spread}");
    // ...and spans the full width.
    let cols: std::collections::HashSet<usize> = lit.iter().map(|&(x, _)| x).collect();
    assert!(cols.len() > 900, "only {} columns lit", cols.len());
}

#[test]
#[ignore = "needs a window; run with -- --ignored"]
fn square_renders_two_bands() {
    // probe-comp: 0..5 V square on CH1. 2 V/div (20 V FS): 5 V -> 62.5 raw.
    // Display window +-100 counts -> row (0.5 - 62.5/200)*(H-1) ~= 94;
    // 0 V -> row ~250.
    let shots = run_script(
        "square",
        r#"
        stimulus probe-comp
        vdiv 0 2.0
        enable 1 0
        persist off
        mode vectors
        wait 1.5
        shot square.ppm
        quit
        "#,
    );
    let (w, _h, px) = load_ppm(&shots[0]);
    let lit = lit(&px, w);
    assert!(lit.len() > 1000, "only {} lit pixels", lit.len());
    let top_expect = (0.5 - 62.5 / 200.0) * (PLOT_H as f64 - 1.0); // ~93.6
    let bot_expect = 0.5 * (PLOT_H as f64 - 1.0); // ~249.5
    let near = |y: usize, e: f64| (y as f64 - e).abs() < 6.0;
    let top = lit.iter().filter(|&&(_, y)| near(y, top_expect)).count();
    let bot = lit.iter().filter(|&&(_, y)| near(y, bot_expect)).count();
    // Both dwell bands carry a solid share of the trace energy.
    assert!(top > lit.len() / 5, "top band {top} of {}", lit.len());
    assert!(bot > lit.len() / 5, "bottom band {bot} of {}", lit.len());
}

#[test]
#[ignore = "needs a window; run with -- --ignored"]
fn xy_circle_renders_as_ellipse_ring() {
    // xy-circle at 1.5 V amplitude on 0.5 V/div (5 V FS): radius 75 raw.
    // Display window +-100 counts -> x semi-axis 75/200*(W-1) ~= 375 px,
    // y semi-axis ~187 px (the plot is 2:1, so a circle in volts is a 2:1
    // ellipse in pixels).
    let shots = run_script(
        "xy",
        r#"
        stimulus xy-circle
        vdiv 0 0.5
        vdiv 1 0.5
        enable 1 1
        mode xy
        persist off
        wait 1.5
        shot xy.ppm
        quit
        "#,
    );
    let (w, _h, px) = load_ppm(&shots[0]);
    let lit = lit(&px, w);
    assert!(lit.len() > 300, "only {} lit pixels", lit.len());
    let cx = (PLOT_W as f64 - 1.0) / 2.0;
    let cy = (PLOT_H as f64 - 1.0) / 2.0;
    let (rx, ry) = (
        75.0 / 200.0 * (PLOT_W as f64 - 1.0),
        75.0 / 200.0 * (PLOT_H as f64 - 1.0),
    );
    let mut on_ring = 0usize;
    for &(x, y) in &lit {
        let nx = (x as f64 - cx) / rx;
        let ny = (y as f64 - cy) / ry;
        let r = (nx * nx + ny * ny).sqrt();
        if (0.85..=1.15).contains(&r) {
            on_ring += 1;
        }
    }
    let frac = on_ring as f64 / lit.len() as f64;
    assert!(
        frac > 0.9,
        "only {:.0}% of lit pixels on the ring",
        frac * 100.0
    );
}
