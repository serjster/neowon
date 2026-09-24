//! Acknowledged writes survive a crash at each failpoint.
//!
//! `NEOWON_CATALOG_KILL=<point> cargo test -p neowon-catalog --test crash`
//! exercises one point; without the variable every point runs in turn.
//! Points: `wal-append` (dies half-way through a frame: a torn tail),
//! `before-rename` and `after-rename-before-fsync` (die mid-checkpoint,
//! around the manifest swap), and `segment-create` (dies half-way through
//! a new segment's header — once in a checkpoint, where the torn segment
//! is not the manifest's, and once while a fresh catalog creates its
//! first, where it is).
//!
//! The crash happens in a child process (this test binary re-run with
//! `NEOWON_CATALOG_CRASH_DIR` set). It prints `ACK <id>` after each
//! commit returns, arms the failpoint after a few commits, and aborts at
//! it. The parent then reopens the catalog in a second child, which checks
//! that every acknowledged id is present and the catalog is sound and
//! still writable.

mod common;
use std::io::Write;

use common::*;
use neowon_catalog::*;

/// `(point, commits before it is armed)`. Checkpoints run every 3 commits,
/// so the rename and segment points fire at commit 9; armed at 0, the
/// point is live before `open`, which on a fresh directory creates the
/// first segment.
const CASES: [(&str, usize); 5] = [
    ("wal-append", ARM_AFTER),
    ("before-rename", ARM_AFTER),
    ("after-rename-before-fsync", ARM_AFTER),
    ("segment-create", ARM_AFTER),
    ("segment-create", 0),
];
const ARM_AFTER: usize = 7;

#[test]
fn child() {
    let Ok(dir) = std::env::var("NEOWON_CATALOG_CRASH_DIR") else {
        return;
    };
    match std::env::var("NEOWON_CATALOG_CRASH_MODE").as_deref() {
        Ok("crash") => {
            let point = std::env::var("NEOWON_CATALOG_CRASH_POINT").unwrap();
            let arm: usize = std::env::var("NEOWON_CATALOG_CRASH_ARM")
                .unwrap()
                .parse()
                .unwrap();
            // SAFETY: this child is single-threaded (run with
            // --test-threads 1) and nothing else reads the environment
            // concurrently.
            let arm_now = || unsafe { std::env::set_var("NEOWON_CATALOG_KILL", &point) };
            if arm == 0 {
                arm_now();
            }
            let mut cat = Catalog::open(&dir).unwrap();
            cat.checkpoint_every = 3;
            let mut out = std::io::stdout().lock();
            for i in 0..20 {
                if i == arm && arm > 0 {
                    arm_now();
                }
                let id = add(&mut cat, |id| {
                    signal(id, &format!("S{i}"), 100e6 + i as f64, None)
                });
                writeln!(out, "ACK {}", id.0).unwrap();
                out.flush().unwrap();
            }
            writeln!(out, "SURVIVED").unwrap();
        }
        Ok("verify") => {
            let acked: Vec<u64> = std::env::var("NEOWON_CATALOG_ACKED")
                .unwrap()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect();
            let mut cat = Catalog::open(&dir).unwrap();
            for id in &acked {
                assert!(cat.state().get(Id(*id)).is_some(), "acked #{id} lost");
            }
            assert!(cat.state().integrity().is_empty());
            // Writable again, and the write survives a reopen.
            let id = add(&mut cat, |id| signal(id, "after", 1e6, None));
            drop(cat);
            let cat = Catalog::open(&dir).unwrap();
            assert!(cat.state().get(id).is_some());
            println!("VERIFIED {} acked", acked.len());
        }
        other => panic!("unknown mode {other:?}"),
    }
}

fn run_child(dir: &std::path::Path, mode: &str, extra: &[(&str, String)]) -> std::process::Output {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["child", "--exact", "--nocapture", "--test-threads", "1"])
        .env_remove("NEOWON_CATALOG_KILL")
        .env("NEOWON_CATALOG_CRASH_DIR", dir)
        .env("NEOWON_CATALOG_CRASH_MODE", mode);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

#[test]
fn acknowledged_writes_survive_a_crash_at_each_point() {
    if std::env::var("NEOWON_CATALOG_CRASH_DIR").is_ok() {
        return; // we are a child
    }
    let only = std::env::var("NEOWON_CATALOG_KILL").ok();
    let cases = CASES
        .iter()
        .filter(|(p, _)| only.as_deref().is_none_or(|o| o == *p));
    // Every case runs, and every unrecovered one is reported.
    let mut failed = Vec::new();
    for &(point, arm) in cases {
        let dir = scratch(&format!("crash-{point}-{arm}"));
        let out = run_child(
            &dir,
            "crash",
            &[
                ("NEOWON_CATALOG_CRASH_POINT", point.to_string()),
                ("NEOWON_CATALOG_CRASH_ARM", arm.to_string()),
            ],
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !out.status.success(),
            "{point}: the child did not crash\n{stdout}"
        );
        assert!(
            !stdout.contains("SURVIVED"),
            "{point}: failpoint never fired"
        );
        // libtest prints "test child ... " on the first ACK's line, so find
        // the marker anywhere in the line.
        let acked: Vec<String> = stdout
            .lines()
            .filter_map(|l| l.split("ACK ").nth(1))
            .map(|s| s.trim().to_string())
            .collect();
        assert!(acked.len() >= arm, "{point}: only {} acks", acked.len());
        let v = run_child(&dir, "verify", &[("NEOWON_CATALOG_ACKED", acked.join(","))]);
        let vout = String::from_utf8_lossy(&v.stdout);
        let recovered = v.status.success() && vout.contains("VERIFIED");
        println!(
            r#"{{"point":"{point}","armed_after":{arm},"acked":{},"recovered":{recovered}}}"#,
            acked.len()
        );
        if recovered {
            std::fs::remove_dir_all(&dir).unwrap();
        } else {
            failed.push(format!(
                "{point} (armed after {arm}): recovery failed\n{vout}\n{}",
                String::from_utf8_lossy(&v.stderr)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n---\n"));
}
