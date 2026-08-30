//! Control-socket integration: spawn the app with `NEOWON_CONTROL`,
//! drive it over TCP, and read structured state back.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test control_socket -- --ignored

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
}

#[test]
#[ignore = "opens a window"]
fn socket_drives_and_queries_the_app() {
    let port = free_port();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sim")
        .env("NEOWON_CONTROL", port.to_string())
        .env_remove("NEOWON_SCRIPT")
        .spawn()
        .expect("launch app");

    // Connect (the app takes a moment to bind).
    let deadline = Instant::now() + Duration::from_secs(20);
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut conn = Conn {
        out: stream.try_clone().unwrap(),
        lines: BufReader::new(stream).lines(),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Status query answers.
        let status = conn.request("get status");
        assert!(status.contains(r#""ok":true"#), "status: {status}");
        assert!(status.contains(r#""running""#), "status: {status}");

        // A command round-trips into the config.
        assert!(conn.request("vdiv 0 0.05").contains(r#""ok":true"#));
        assert!(conn.request("trigpos 0.3").contains(r#""ok":true"#));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let cfg = conn.request("get config");
            if cfg.contains(r#""volts_div":0.05"#) && cfg.contains(r#""trigger_position":0.3"#) {
                assert!(cfg.contains(r#""kind":"edge""#), "config: {cfg}");
                break;
            }
            assert!(Instant::now() < deadline, "config never updated: {cfg}");
            std::thread::sleep(Duration::from_millis(100));
        }

        // Measurements appear once frames flow (probe-comp 1 kHz sim).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let m = conn.request("get measure");
            if m.contains(r#""name":"Freq","value":9"#)
                || m.contains(r#""name":"Freq","value":1000"#)
            {
                break;
            }
            assert!(Instant::now() < deadline, "no measurements: {m}");
            std::thread::sleep(Duration::from_millis(200));
        }

        // Bad input gets a structured error, not a hang.
        let bad = conn.request("florp 1 2 3");
        assert!(bad.contains(r#""ok":false"#), "bad: {bad}");
        let badq = conn.request("get nonsense");
        assert!(badq.contains(r#""ok":false"#), "badq: {badq}");
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
