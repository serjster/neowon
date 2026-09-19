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
        // A throwaway reference store with one known station, and a
        // throwaway location file: the spawned app must never read the
        // operator's.
        let dir = std::env::temp_dir().join(format!("neowon-mcp-ref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_neowon-mcp"))
            .arg("--spawn-sim")
            .env("NEOWON_MCP_PORT", port.to_string())
            // The spawned app inherits this: a throwaway catalog, not the
            // user's.
            .env(
                "NEOWON_CATALOG",
                std::env::temp_dir().join(format!("neowon-mcp-cat-{}", std::process::id())),
            )
            .env("NEOWON_REFDB", &dir)
            .env("NEOWON_LOCATION", dir.join("location.json"))
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
