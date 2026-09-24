//! Control-socket harness shared by the socket tests: launch the app,
//! talk to it, read flat JSON fields.
//!
//! The socket's write verbs (`shot`, `export`, `quit`, `catalog …`, …) need
//! the connection's token. A harness that spawns the app picks the
//! token itself with `NEOWON_CONTROL_TOKEN` and sends `auth` as its first
//! line, so tests drive the whole grammar.
//!
//! Two isolation rules hold for every launch:
//!
//! - **Its own state.** The app runs in a [`Sandbox`]: a private `HOME` and
//!   none of the caller's `NEOWON_*` variables (see `sandbox.rs`).
//! - **Its own app.** The port is picked free-then-bound, so another
//!   process can take it in between; the app then cannot bind and whoever
//!   holds the port answers instead. The token is the nonce that tells them
//!   apart: it is unique per launch (pid and counter — never derived from
//!   the port, which two launches can share), and a launch proves the
//!   listener is its own by authenticating with it before it hands back a
//!   connection. A stranger — another test's app, the operator's app, any
//!   other listener — cannot accept it, so the launch retries on a new port
//!   instead of talking to an app it did not start.

#![allow(dead_code)]

pub mod sandbox;
pub mod tree;

#[allow(unused_imports)]
pub use sandbox::{Sandbox, scratch, unique};

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A spawned app, ended on every path: dropping the handle kills and reaps
/// it, so a test that panics mid-body — or returns early, or forgets —
/// takes its app with it instead of leaving it to the orphan watchdog.
/// Derefs to the `Child`, so `id`, `try_wait`, `kill` and
/// `wait` work as before; ending it explicitly first is harmless (std does
/// not signal a child it has already reaped).
pub struct App(std::process::Child);

impl App {
    pub fn new(child: std::process::Child) -> Self {
        Self(child)
    }
}

impl std::ops::Deref for App {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
    /// The token this launch gave the app, for `auth`.
    pub token: String,
    /// The port the app serves, proven to be this launch's app.
    pub port: u16,
    /// The app's private home; removed once the last connection is gone.
    sandbox: Arc<Sandbox>,
}

impl Conn {
    fn open(port: u16, token: &str, sandbox: Arc<Sandbox>) -> std::io::Result<Conn> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok(Conn {
            out: stream.try_clone()?,
            lines: BufReader::new(stream).lines(),
            token: token.to_string(),
            port,
            sandbox,
        })
    }

    pub fn request(&mut self, line: &str) -> String {
        writeln!(self.out, "{line}").unwrap();
        self.lines.next().expect("connection closed").unwrap()
    }

    /// The next line the app sent unprompted, or `None` once it has closed
    /// the connection.
    pub fn next_line(&mut self) -> Option<String> {
        self.lines.next().and_then(Result::ok)
    }

    /// Send a command that must be refused, and return the reply.
    pub fn refused(&mut self, line: &str) -> String {
        let r = self.request(line);
        assert!(r.contains(r#""ok":false"#), "{line} was not refused: {r}");
        r
    }

    /// Open a second connection to the same app, unauthenticated.
    pub fn second(&self) -> Conn {
        Conn::open(self.port, &self.token, Arc::clone(&self.sandbox)).unwrap()
    }

    /// How long a reply may take (10 s unless a suite asks for more).
    pub fn set_timeout(&self, secs: u64) {
        self.out
            .set_read_timeout(Some(Duration::from_secs(secs)))
            .unwrap();
    }

    /// The directory the app sees as `~`.
    pub fn home(&self) -> &Path {
        self.sandbox.home()
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

/// Launch the app with `args` and `env`, and connect to its socket,
/// authenticated (see the module note).
pub fn launch(args: &[&str], env: &[(&str, &str)]) -> (App, Conn) {
    let (child, mut conn) = launch_raw(args, env);
    let line = format!("auth {}", conn.token);
    conn.ok(&line);
    (child, conn)
}

/// Launch and connect without the `auth` handshake on the returned
/// connection — for the tests that assert what an unauthenticated client
/// may and may not do. The launch itself still proves the app is its own.
pub fn launch_raw(args: &[&str], env: &[(&str, &str)]) -> (App, Conn) {
    const ATTEMPTS: usize = 3;
    for attempt in 1..=ATTEMPTS {
        match launch_on(free_port(), args, env) {
            Ok(launched) => return launched,
            Err(stranger) => {
                eprintln!("launch {attempt}/{ATTEMPTS}: {stranger}; retrying on a new port")
            }
        }
    }
    panic!("launch: every port tried was answered by another process");
}

/// Launch on `port` exactly. `Err` when the listener there is not this
/// launch's app — the app is killed and nothing talked to the stranger
/// beyond the one `auth` line that unmasked it.
pub fn launch_on(port: u16, args: &[&str], env: &[(&str, &str)]) -> Result<(App, Conn), String> {
    let sandbox = Arc::new(Sandbox::new("app"));
    // The nonce: unique among live processes (pid) and launches (counter).
    let token = unique("test-token");
    let mut cmd = sandbox.command(env!("CARGO_BIN_EXE_neowon-app"));
    cmd.args(args)
        .env("NEOWON_CONTROL", port.to_string())
        .env("NEOWON_CONTROL_TOKEN", &token)
        // If this test process is killed, the app must not linger: with no
        // live client for this many seconds it exits by itself
        // (`neowon-app/src/control/orphan.rs`). The connection is held for
        // the test's lifetime, so a killed harness is what closes it.
        .env("NEOWON_ORPHAN_EXIT", "15");
    for (k, v) in env {
        // Saved state is off by default; `NEOWON_STATE` opts in.
        if *k == "NEOWON_STATE" {
            cmd.env_remove("NEOWON_NO_STATE");
        }
        cmd.env(k, v);
    }
    // Owned from the first instant: every exit below, a panic included,
    // ends it.
    let mut child = App::new(cmd.spawn().expect("launch app"));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut probe = loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the app exited before serving its socket: {status}");
        }
        match Conn::open(port, &token, Arc::clone(&sandbox)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cannot connect: {e}");
            }
        }
    };
    // Identity: only the app this launch started knows the token. A
    // stranger answers "bad token", something else, or nothing at all.
    let answer = match writeln!(probe.out, "auth {token}") {
        Ok(()) => probe.next_line().unwrap_or_default(),
        Err(e) => e.to_string(),
    };
    drop(probe);
    if !answer.contains(r#""authed":true"#) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "port {port} is held by a process this launch did not start (it answered {answer:?})"
        ));
    }
    let conn = Conn::open(port, &token, sandbox).map_err(|e| e.to_string())?;
    Ok((child, conn))
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

/// Run `f`, then end the app whatever happened (the `App` would on drop
/// anyway; this ends it before the caller's cleanup runs).
pub fn with_app(app: App, f: impl FnOnce()) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    drop(app);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
