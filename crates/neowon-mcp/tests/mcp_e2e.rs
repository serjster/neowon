//! MCP end-to-end: spawn `neowon-mcp --spawn-sim` (which itself spawns
//! `neowon-app --sim`) and speak raw JSON-RPC over its stdio.
//!
//! Needs the app binary built first and briefly opens a window, so
//! `#[ignore]` by default:
//!   cargo build -p neowon-app && \
//!   cargo test -p neowon-mcp --test mcp_e2e -- --ignored

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// The app suites' isolation rule, included rather than restated: a private
// HOME and none of the caller's NEOWON_*.
#[path = "../../neowon-app/tests/common/sandbox.rs"]
mod sandbox;
use sandbox::{Sandbox, scratch, unique};

struct Mcp {
    child: Child,
    /// The spawned app's home (it inherits the MCP server's environment).
    _sandbox: Sandbox,
    /// `None` once closed: closing it is how an MCP client ends a stdio
    /// server.
    stdin: Option<std::process::ChildStdin>,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
    /// The server's stderr lines, drained on a thread so it never blocks.
    stderr: mpsc::Receiver<String>,
}

impl Mcp {
    fn spawn() -> Self {
        let app =
            std::path::Path::new(env!("CARGO_BIN_EXE_neowon-mcp")).with_file_name("neowon-app");
        assert!(
            app.exists(),
            "build the app first: cargo build -p neowon-app"
        );
        // Hermetic: a private control port so the test never attaches to
        // (or is joined by) a developer's running app on the default one.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        // A throwaway reference store with one known station, and a
        // throwaway location file: the spawned app must never read the
        // operator's.
        let dir = scratch("mcp-ref");
        std::fs::write(
            dir.join("wikidata.json"),
            r#"[{"source":"wikidata","id":"Q1001","name":"Antena 1","freq_hz":100300000.0,
                 "modulation":"wfm","service":"broadcast","lat":38.7223,"lon":-9.1393,
                 "country":"PT"}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("meta.json"),
            r#"[{"source":"wikidata","count":1,"fetched_at":"2026-09-19T10:00:00Z",
                 "origin":"fixture","licence":"CC0"}]"#,
        )
        .unwrap();
        // The spawned app inherits the server's environment: a private
        // HOME, a throwaway catalog, and a token both processes share. The
        // token is this launch's nonce: an app on the port that this test
        // did not start refuses it, so the gated tools fail loudly instead
        // of driving a stranger.
        let sandbox = Sandbox::new("mcp");
        let mut child = sandbox
            .command(env!("CARGO_BIN_EXE_neowon-mcp"))
            .arg("--spawn-sim")
            .env("NEOWON_MCP_PORT", port.to_string())
            .env("NEOWON_CONTROL_TOKEN", unique("mcp-token"))
            .env("NEOWON_CATALOG", scratch("mcp-cat"))
            .env("NEOWON_REFDB", &dir)
            .env("NEOWON_LOCATION", dir.join("location.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch neowon-mcp");
        let stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (tx, stderr) = mpsc::channel();
        let err = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Self {
            child,
            _sandbox: sandbox,
            stdin,
            lines,
            stderr,
        }
    }

    fn send(&mut self, msg: &str) {
        writeln!(self.stdin.as_mut().expect("stdin open"), "{msg}").unwrap();
    }

    /// The pid of the app the server spawned, from its stderr report.
    fn app_pid(&self) -> u32 {
        const TAG: &str = "neowon-mcp: spawned neowon-app pid ";
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.stderr.recv_timeout(left) {
                Ok(line) => {
                    if let Some(pid) = line.strip_prefix(TAG) {
                        return pid.trim().parse().expect("pid");
                    }
                }
                Err(e) => panic!("the server never reported its app's pid: {e}"),
            }
        }
    }

    /// End the server the way an MCP client does: close its stdin and let
    /// it exit, which ends the app it spawned. Killing the server instead
    /// would orphan that app until its `NEOWON_ORPHAN_EXIT` watchdog fired.
    /// The kill is only the fallback for a server that does not
    /// exit; `None` says it had to be used.
    fn shutdown(&mut self) -> Option<ExitStatus> {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Read messages until one carries the given id; requests from the
    /// server and notifications are skipped.
    fn recv_id(&mut self, id: u64) -> String {
        let want = format!("\"id\":{id}");
        for line in self.lines.by_ref() {
            let line = line.expect("server closed stdout");
            if line.contains(&want) && line.contains("\"result\"") {
                return line;
            }
        }
        panic!("no response with id {id}");
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Whether `pid` names a live process (`kill -0`: a signal check that
/// sends nothing).
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whatever spawns the app ends it: once the test is done with the MCP
/// server, the app that `--spawn-sim` started is gone too — checked by its
/// pid, within a bound far below the 30 s orphan watchdog.
#[test]
#[ignore = "opens a window (spawns the sim app)"]
fn the_spawned_app_ends_with_the_server() {
    let mut mcp = Mcp::spawn();
    mcp.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
    );
    assert!(mcp.recv_id(1).contains("serverInfo"));
    let app = mcp.app_pid();
    assert!(alive(app), "the spawned app (pid {app}) is not running");

    // The test is done with the server: this is what its end does.
    let status = mcp.shutdown();
    drop(mcp);
    let ended = Instant::now();
    while alive(app) {
        assert!(
            ended.elapsed() < Duration::from_secs(3),
            "the spawned app (pid {app}) outlived its server by {:.1}s (server: {status:?})",
            ended.elapsed().as_secs_f64()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        status.is_some_and(|s| s.success()),
        "the server did not exit by itself when its stdin closed: {status:?}"
    );
}

#[test]
#[ignore = "opens a window (spawns the sim app)"]
fn mcp_tools_drive_the_sim() {
    let mut mcp = Mcp::spawn();

    mcp.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
    );
    let init = mcp.recv_id(1);
    assert!(init.contains("serverInfo"), "init: {init}");
    mcp.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);

    // Tool discovery: the curated surface plus the escape hatch.
    mcp.send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = mcp.recv_id(2);
    for name in [
        "scope_status",
        "scope_config",
        "measurements",
        "configure_channel",
        "configure_trigger",
        "exec_script",
        "screenshot",
        "sdr_status",
        "sdr_tune",
        "sdr_detections",
        "sdr_modmeas",
        "catalog_list",
        "catalog_history",
        "catalog",
        "sdr_survey",
        "sdr_survey_result",
        "sdr_classify",
        "ui_tree",
        "rf_bands",
        "stations",
        "station_tune",
        "refdb",
        "location",
    ] {
        assert!(tools.contains(name), "missing tool {name}: {tools}");
    }

    mcp.send(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"configure_channel","arguments":{"ch":0,"volts_div":0.1}}}"#,
    );
    assert!(mcp.recv_id(3).contains("applied"));
    mcp.send(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"scope_config","arguments":{}}}"#,
    );
    let cfg = mcp.recv_id(4);
    assert!(cfg.contains("volts_div"), "config: {cfg}");
    assert!(cfg.contains("0.1"), "config: {cfg}");

    // Measurements flow from the sim's probe-comp default.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut id = 5u64;
    loop {
        mcp.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"measurements","arguments":{{}}}}}}"#
        ));
        let m = mcp.recv_id(id);
        id += 1;
        if m.contains("Freq") && m.contains("Vpp") && !m.contains(r#""slots\":[null"#) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no measurements: {m}");
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    mcp.send(&format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"screenshot","arguments":{{}}}}}}"#
    ));
    let shot = mcp.recv_id(id);
    assert!(shot.contains(r#""type":"image""#), "shot: {shot}");
    assert!(shot.contains("image/png"), "shot: {shot}");
    // Base64 PNG magic: iVBORw0KGgo.
    assert!(shot.contains("iVBORw0KGgo"), "not a PNG payload");

    // The catalog is instrument-agnostic: file a signal and list it back.
    let call = |id: u64, name: &str, args: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
        )
    };
    mcp.send(&call(
        900,
        "catalog",
        r#"{"command":"add 99.4M Radio Two"}"#,
    ));
    assert!(mcp.recv_id(900).contains("ok"));
    mcp.send(&call(901, "catalog_list", r#"{"filter":"radio"}"#));
    let list = mcp.recv_id(901);
    assert!(
        list.contains("Radio Two") && list.contains("99400000"),
        "list: {list}"
    );

    // The reference store: the fixture station through the script grammar.
    mcp.send(&call(910, "stations", r#"{"source":"wikidata"}"#));
    let st = mcp.recv_id(910);
    assert!(
        st.contains("Antena 1") && st.contains("wikidata:Q1001"),
        "stations: {st}"
    );
    mcp.send(&call(911, "refdb", r#"{"action":"status"}"#));
    let db = mcp.recv_id(911);
    assert!(
        db.contains("wikidata") && db.contains(r#"count\":1"#),
        "refdb: {db}"
    );
    mcp.send(&call(912, "location", r#"{"lat":38.7223,"lon":-9.1393}"#));
    let loc = mcp.recv_id(912);
    assert!(loc.contains("IM58kr"), "location: {loc}");
}
