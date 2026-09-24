//! Control-socket integration: spawn the app with `NEOWON_CONTROL`,
//! drive it over TCP, and read structured state back.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test control_socket -- --ignored

mod common;
use common::*;

use std::time::{Duration, Instant};

#[test]
#[ignore = "opens a window"]
fn socket_drives_and_queries_the_app() {
    let (child, mut conn) = launch(&["--sim"], &[]);
    with_app(child, || {
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
    });
}

/// The socket is on by default and any local process can reach it,
/// so the verbs that leave the process are behind the token — on the wire,
/// not only in the classification unit tests.
#[test]
#[ignore = "opens a window"]
fn write_verbs_need_the_token_on_the_wire() {
    let dir = scratch("gated");
    let png = dir.join("gated.png");
    // The verb that proves the *allowed* half. `shot` reads back the
    // composited window, which macOS blanks while the screen is locked
    // (harness.md), so it cannot decide anything here; `uitree <path>` is
    // an equally gated write verb that only needs the UI tree.
    let tree = dir.join("gated.json");
    for p in [&png, &tree] {
        let _ = std::fs::remove_file(p);
    }

    let shot_line = format!("shot {}", png.display());
    let tree_line = format!("uitree {}", tree.display());
    let (child, authed) = launch(&["--sim"], &[]);
    with_app(child, || {
        let mut anon = authed.second();

        // 1. Ungated: queries and instrument control still work with no
        // handshake at all — the operator's `nc` loop is untouched.
        assert!(anon.request("get status").contains(r#""ok":true"#));
        anon.ok("vdiv 0 0.05");
        anon.ok("run 1");

        // 2. Gated: every write verb is refused, and the refusal says where
        // the token lives without disclosing it.
        let token = anon.token.clone();
        for line in [
            shot_line.as_str(),
            tree_line.as_str(),
            "shotplot /dev/null",
            "sdr iqdump /dev/null 1",
            "export csv /dev/null",
            "sessionload /etc/passwd",
            "quit",
        ] {
            let r = anon.refused(line);
            assert!(r.contains(r#""auth":"token""#), "{line}: {r}");
            assert!(!r.contains(&token), "{line} leaked the token: {r}");
        }
        // The refused verbs wrote nothing, and the app is still alive (the
        // refused `quit` did not end it).
        std::thread::sleep(Duration::from_millis(500));
        for p in [&png, &tree] {
            assert!(!p.exists(), "a refused verb still wrote {}", p.display());
        }
        assert!(anon.request("get status").contains(r#""ok":true"#));

        // 3. A guessed token is refused, and the third guess closes the
        // connection rather than allowing a fourth.
        let mut guesser = authed.second();
        for _ in 0..3 {
            assert!(guesser.request("auth not-the-token").contains("bad token"));
        }
        assert!(
            guesser
                .next_line()
                .is_none_or(|l| l.contains("too many auth failures")),
            "the connection must be closed after three guesses"
        );

        // 4. The same verb runs once the connection is authenticated.
        let mut good = authed.second();
        let token = good.token.clone();
        good.ok(&format!("auth {token}"));
        good.ok(&tree_line);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !tree.exists() {
            assert!(
                Instant::now() < deadline,
                "authenticated uitree never written"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&tree);
    });
}
