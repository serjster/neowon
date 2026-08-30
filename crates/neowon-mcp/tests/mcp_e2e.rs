//! MCP end-to-end: spawn `neowon-mcp --spawn-sim` (which itself spawns
//! `neowon-app --sim`) and speak raw JSON-RPC over its stdio.
//!
//! Needs the app binary built first and briefly opens a window, so
//! `#[ignore]` by default:
//!   cargo build -p neowon-app && \
//!   cargo test -p neowon-mcp --test mcp_e2e -- --ignored

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Mcp {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_neowon-mcp"))
            .arg("--spawn-sim")
            .env("NEOWON_MCP_PORT", port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch neowon-mcp");
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, msg: &str) {
        writeln!(self.stdin, "{msg}").unwrap();
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    ] {
        assert!(tools.contains(name), "missing tool {name}: {tools}");
    }

    // Configure a channel, then read the change back.
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

    // Screenshot returns PNG image content.
    mcp.send(&format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"screenshot","arguments":{{}}}}}}"#
    ));
    let shot = mcp.recv_id(id);
    assert!(shot.contains(r#""type":"image""#), "shot: {shot}");
    assert!(shot.contains("image/png"), "shot: {shot}");
    // Base64 PNG magic: iVBORw0KGgo.
    assert!(shot.contains("iVBORw0KGgo"), "not a PNG payload");
}
