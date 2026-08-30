//! Timeline (deep view) verification through the control socket: the point
//! of the feature is spanning more time than one record holds *without*
//! giving up sample rate, so that is what these assert.
//!
//! Opens a window, so `#[ignore]` by default:
//!   cargo test -p neowon-app --test deep_view -- --ignored

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Conn {
    out: TcpStream,
    lines: std::io::Lines<BufReader<TcpStream>>,
}

impl Conn {
    fn request(&mut self, line: &str) -> String {
        writeln!(self.out, "{line}").unwrap();
        self.lines.next().expect("connection closed").unwrap()
    }

    fn ok(&mut self, line: &str) {
        let r = self.request(line);
        assert!(r.contains(r#""ok":true"#), "{line} -> {r}");
    }

    /// Poll `get config` until `needle` appears.
    fn wait_config(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let cfg = self.request("get config");
            if cfg.contains(needle) {
                return cfg;
            }
            assert!(
                Instant::now() < deadline,
                "config never saw {needle}: {cfg}"
            );
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

/// Field of the `"deep"` object in a `get config` reply.
fn deep_field(cfg: &str, name: &str) -> String {
    let deep = cfg.split(r#""deep":{"#).nth(1).expect("no deep block");
    let deep = deep.split('}').next().unwrap();
    deep.split(&format!(r#""{name}":"#))
        .nth(1)
        .expect("no such field")
        .split(',')
        .next()
        .unwrap()
        .to_string()
}

fn load_ppm(path: &Path) -> (usize, usize, Vec<[u8; 3]>) {
    let data = std::fs::read(path).unwrap();
    let end = data.windows(4).position(|w| w == b"255\n").expect("ppm") + 4;
    let header = std::str::from_utf8(&data[..end]).unwrap();
    let mut dims = header.lines().nth(1).unwrap().split_whitespace();
    let w: usize = dims.next().unwrap().parse().unwrap();
    let h: usize = dims.next().unwrap().parse().unwrap();
    (w, h, data[end..].as_chunks::<3>().0.to_vec())
}

#[test]
#[ignore = "opens a window"]
fn timeline_spans_history_without_losing_sample_rate() {
    let port = free_port();
    let dir = std::env::temp_dir().join("neowon-deep");
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_CONTROL", port.to_string())
        .env("NEOWON_WINDOW", "1520x820")
        .env("NEOWON_UI_SCALE", "1.0")
        .env_remove("NEOWON_SCRIPT")
        .spawn()
        .expect("launch app");

    let deadline = Instant::now() + Duration::from_secs(25);
    let stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                let _ = child.kill();
                panic!("cannot connect: {e}");
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut conn = Conn {
        out: stream.try_clone().unwrap(),
        lines: BufReader::new(stream).lines(),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for cmd in [
            "stimulus sine-1k",
            "enable 0 1",
            "vdiv 0 0.5",
            "timebase 0.002",
            "persist off",
        ] {
            conn.ok(cmd);
        }
        conn.wait_config(r#""sample_rate":250000"#);
        std::thread::sleep(Duration::from_millis(1500));

        // The whole point: a window far longer than one 20 ms record, at the
        // same sample rate the instrument is running.
        conn.ok("deep on");
        conn.ok("deepspan 1");
        let cfg = conn.wait_config(r#""on":true"#);
        assert!(
            cfg.contains(r#""sample_rate":250000"#),
            "the sample rate must not change to span more time: {cfg}"
        );
        let records: usize = deep_field(&cfg, "records").parse().unwrap();
        assert!(
            records > 10,
            "a 1 s window should stitch many 20 ms records, got {records}"
        );

        // A paused recorder leaves real dead time; widen the window so it
        // falls inside, and the gap must be both counted and drawn.
        conn.ok("deepspan 5");
        conn.ok("record 0");
        std::thread::sleep(Duration::from_millis(1200));
        conn.ok("record 1");
        std::thread::sleep(Duration::from_millis(1500));

        let deadline = Instant::now() + Duration::from_secs(10);
        let gaps = loop {
            let cfg = conn.request("get config");
            let gaps: usize = deep_field(&cfg, "gaps").parse().unwrap();
            let coverage: f64 = deep_field(&cfg, "coverage").parse().unwrap();
            if gaps > 0 {
                assert!(coverage < 1.0, "gaps but full coverage: {cfg}");
                assert!(coverage > 0.0, "no coverage at all: {cfg}");
                break gaps;
            }
            assert!(Instant::now() < deadline, "no gap appeared: {cfg}");
            std::thread::sleep(Duration::from_millis(200));
        };

        // The markers are drawn into the display texture, not as an overlay,
        // precisely so a screenshot can prove they exist.
        let shot: PathBuf = dir.join("deep.ppm");
        let _ = std::fs::remove_file(&shot);
        conn.ok(&format!("shot {}", shot.display()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !shot.exists() {
            assert!(Instant::now() < deadline, "shot never written");
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(100));
        let (w, h, px) = load_ppm(&shot);
        let red_cols = (0..w)
            .filter(|&x| {
                (0..h).step_by(4).any(|y| {
                    let p = px[y * w + x];
                    p[0] > 90 && p[0] as u16 > 2 * p[1] as u16 && p[0] as u16 > 2 * p[2] as u16
                })
            })
            .count();
        assert!(
            red_cols >= 2,
            "expected discontinuity markers in the display, found {red_cols} red columns \
             (gaps reported: {gaps})"
        );

        // Turning it off returns to the single-record view.
        conn.ok("deep off");
        let cfg = conn.wait_config(r#""on":false"#);
        assert!(cfg.contains(r#""sample_rate":250000"#), "{cfg}");
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
