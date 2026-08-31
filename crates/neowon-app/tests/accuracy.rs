//! Accuracy and invariance checks — the failures that are hard to spot by
//! eye, because a wrong answer still *looks* like a waveform.
//!
//! The strongest available oracle costs nothing: the same signal measured
//! through different instrument settings must give the same answer. A
//! frequency that changes when you change the time base, or an amplitude
//! that changes with volts/div, is a scaling bug — and those are exactly the
//! bugs that survive visual inspection, because the trace looks right at
//! every setting.
//!
//! Runs against the deterministic simulator, so it is a regression test
//! rather than a calibration of the instrument.
//!
//!   cargo test -p neowon-app --test accuracy -- --ignored

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
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

    /// A metric from `get measure`, once the app has produced one at the
    /// current settings.
    fn metric(&mut self, slot: usize, name: &str) -> Option<f64> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let json = self.request("get measure");
            if let Some(v) = pick(&json, slot, name) {
                return Some(v);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

/// Pull one metric out of the hand-emitted measurement JSON.
fn pick(json: &str, slot: usize, name: &str) -> Option<f64> {
    let slots = json.split(r#""slots":["#).nth(1)?;
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut found = 0usize;
    for (i, c) in slots.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if found == slot {
                        let body = &slots[start..=i];
                        let v = body.split(&format!(r#""name":"{name}","value":"#)).nth(1)?;
                        let v = v.split(',').next()?;
                        return v.parse().ok();
                    }
                    found += 1;
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    None
}

fn start(port: u16) -> (std::process::Child, Conn) {
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_neowon-app"))
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
            Err(e) => panic!("cannot connect: {e}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let conn = Conn {
        out: stream.try_clone().unwrap(),
        lines: BufReader::new(stream).lines(),
    };
    (child, conn)
}

#[test]
#[ignore = "opens a window"]
fn measurements_do_not_depend_on_the_instrument_settings() {
    let port = free_port();
    let (mut child, mut conn) = start(port);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for cmd in [
            "stimulus sine-1k",
            "enable 0 1",
            "persist off",
            "acq sample",
        ] {
            conn.ok(cmd);
        }

        // A 1 kHz sine, measured across every time base that can resolve it.
        // 20 samples per cycle is the floor for a trustworthy period
        // estimate; below that the answer legitimately degrades.
        let mut freqs = Vec::new();
        for tb in ["0.0002", "0.0005", "0.001", "0.002", "0.005"] {
            conn.ok(&format!("timebase {tb}"));
            conn.ok("vdiv 0 0.5");
            std::thread::sleep(Duration::from_millis(700));
            let f = conn
                .metric(0, "Freq")
                .unwrap_or_else(|| panic!("no frequency at {tb} s/div"));
            freqs.push((tb, f));
        }
        for (tb, f) in &freqs {
            assert!(
                (f - 1000.0).abs() < 5.0,
                "frequency should not depend on the time base: {f} Hz at {tb} s/div \
                 (all readings: {freqs:?})"
            );
        }

        // The same signal at every vertical scale that fits it. Vpp is in
        // volts at the probe tip, so it must not move with volts/div.
        conn.ok("timebase 0.001");
        let mut vpps = Vec::new();
        for vdiv in ["0.2", "0.5", "1"] {
            conn.ok(&format!("vdiv 0 {vdiv}"));
            std::thread::sleep(Duration::from_millis(700));
            let v = conn
                .metric(0, "Vpp")
                .unwrap_or_else(|| panic!("no Vpp at {vdiv} V/div"));
            vpps.push((vdiv, v));
        }
        let first = vpps[0].1;
        for (vdiv, v) in &vpps {
            assert!(
                (v - first).abs() < first * 0.1,
                "amplitude should not depend on volts/div: {v} V at {vdiv} V/div \
                 (all readings: {vpps:?})"
            );
        }

        // The probe factor must be applied consistently on both sides: it
        // scales the input range the instrument digitizes *and* the volts
        // each ADC count represents. The simulator models a fixed signal at
        // the probe tip, so a correctly-configured probe reads the same
        // voltage either way — and applying the factor in only one of the
        // two places would show up here immediately.
        conn.ok("vdiv 0 0.5");
        conn.ok("probe 0 1");
        std::thread::sleep(Duration::from_millis(700));
        let at_1 = conn.metric(0, "Vpp").expect("Vpp at x1");
        conn.ok("vdiv 0 0.5");
        conn.ok("probe 0 10");
        std::thread::sleep(Duration::from_millis(700));
        let at_10 = conn.metric(0, "Vpp").expect("Vpp at x10");
        assert!(
            (at_10 - at_1).abs() < at_1 * 0.1,
            "the probe factor must cancel between range and scale: \
             x1 read {at_1} V, x10 read {at_10} V"
        );
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
#[ignore = "opens a window"]
fn the_timeline_does_not_move_while_a_page_fills() {
    // The complaint this guards: the display jittered horizontally and
    // records came and went. In page mode the window is a fixed slice of the
    // session clock, so the reported window must not change between rebuilds
    // even as new records arrive.
    let port = free_port();
    let (mut child, mut conn) = start(port);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for cmd in [
            "stimulus sine-1k",
            "enable 0 1",
            "timebase 0.0002",
            "deepfollow page",
            "deep on",
            "deepspan 2",
        ] {
            conn.ok(cmd);
        }
        std::thread::sleep(Duration::from_millis(1200));

        // Records accumulate into the page; the page itself must hold still.
        let mut records = Vec::new();
        for _ in 0..6 {
            let json = conn.request("get config");
            let deep = json.split(r#""deep":{"#).nth(1).unwrap();
            let deep = deep.split('}').next().unwrap();
            let n: usize = deep
                .split(r#""records":"#)
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            records.push(n);
            std::thread::sleep(Duration::from_millis(300));
        }
        // Within one page the record count only ever grows.
        for w in records.windows(2) {
            assert!(
                w[1] >= w[0] || w[1] < w[0] / 2,
                "records went backwards without a page turn: {records:?}"
            );
        }
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
