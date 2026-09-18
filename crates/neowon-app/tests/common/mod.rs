//! Control-socket harness shared by the SDR-mode tests: launch the app,
//! talk to it, read flat JSON fields.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Conn {
    out: TcpStream,
    lines: std::io::Lines<BufReader<TcpStream>>,
}

impl Conn {
    pub fn request(&mut self, line: &str) -> String {
        writeln!(self.out, "{line}").unwrap();
        self.lines.next().expect("connection closed").unwrap()
    }

    /// Send a command that must be accepted.
    pub fn ok(&mut self, line: &str) {
        let r = self.request(line);
        assert!(r.contains(r#""ok":true"#), "{line}: {r}");
    }

    /// Poll `query` until `ok` accepts the reply, or fail after `secs`.
    pub fn wait(&mut self, query: &str, secs: u64, ok: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let r = self.request(query);
            if ok(&r) {
                return r;
            }
            assert!(Instant::now() < deadline, "{query} never settled: {r}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Launch the app with `args` and `env`, and connect to its socket.
pub fn launch(args: &[&str], env: &[(&str, &str)]) -> (std::process::Child, Conn) {
    let port = free_port();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_neowon-app"));
    cmd.args(args)
        .env("NEOWON_CONTROL", port.to_string())
        .env_remove("NEOWON_SCRIPT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("launch app");
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
    let conn = Conn {
        out: stream.try_clone().unwrap(),
        lines: BufReader::new(stream).lines(),
    };
    (child, conn)
}

/// The raw text of a field of a flat JSON object.
pub fn raw<'a>(json: &'a str, key: &str) -> &'a str {
    let pat = format!("\"{key}\":");
    let start = json
        .find(&pat)
        .unwrap_or_else(|| panic!("{key} missing: {json}"))
        + pat.len();
    let rest = &json[start..];
    rest[..rest.find([',', '}']).unwrap()].trim()
}

/// A numeric field (`null` → NaN).
pub fn field(json: &str, key: &str) -> f64 {
    raw(json, key).parse().unwrap_or(f64::NAN)
}

/// Objects in a JSON array, split on a key every object starts with.
pub fn items<'a>(json: &'a str, first_key: &str) -> Vec<&'a str> {
    json.split(&format!("{{\"{first_key}\":")).skip(1).collect()
}

/// Run `f`, then kill the app whatever happened.
pub fn with_app(mut child: std::process::Child, f: impl FnOnce()) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
