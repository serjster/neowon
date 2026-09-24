//! `neowon sdr smoke` as the operator runs it, on the simulator only: the
//! binary, its exit status, the atomic JSON file and the `--doc` append.
//!
//! Every run goes through `smoke`, which refuses arguments without
//! `--sim`, so no test here can open the RTL dongle. `--doc` appends to a
//! copy of `docs/protocol-rtlsdr.md` under the target's temp directory;
//! the real file is read, never written, and checked unchanged.
//!
//!   cargo test -p neowon-cli --test sdr_smoke

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run `neowon sdr smoke <args>`. Hardware safety: `--sim` is mandatory.
fn smoke(args: &[&str]) -> Output {
    assert!(
        args.contains(&"--sim"),
        "tests run the smoke on the simulator only: {args:?}"
    );
    Command::new(env!("CARGO_BIN_EXE_neowon"))
        .args(["sdr", "smoke"])
        .args(args)
        .env("RUST_LOG", "warn")
        .output()
        .expect("run neowon")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sdr-smoke-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn real_doc() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol-rtlsdr.md")
}

#[test]
fn a_passing_sim_run_writes_the_json_and_appends_the_doc_copy() {
    let dir = scratch("pass");
    // The contract's `audit/` does not exist yet: the run creates it.
    let json_path = dir.join("audit/rtlsdr-smoke.json");
    let doc = dir.join("protocol-rtlsdr.md");
    let original = std::fs::read_to_string(real_doc()).unwrap();
    std::fs::write(&doc, &original).unwrap();

    let out = smoke(&[
        "--sim",
        "rf-am",
        "--freq",
        "100.1e6",
        "--json-out",
        json_path.to_str().unwrap(),
        "--doc",
        doc.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("SDR SMOKE OK (sim)"), "{stdout}");

    let json = std::fs::read_to_string(&json_path).unwrap();
    assert!(json.contains(r#""source":"sim""#), "{json}");
    assert!(json.contains(r#""dongle_serial":"sim-sdr-0""#), "{json}");
    assert!(json.contains(r#""class":"am""#), "{json}");
    assert!(json.contains(r#""pass":true"#), "{json}");
    // Written whole: no temp sibling is left behind.
    let names: Vec<String> = std::fs::read_dir(json_path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");

    // The copy is the original plus one dated section holding that JSON.
    let appended = std::fs::read_to_string(&doc).unwrap();
    let added = appended
        .strip_prefix(original.as_str())
        .expect("the doc was appended to, not rewritten");
    assert!(
        added.contains("## SDR smoke readout (20"),
        "dated heading: {added}"
    );
    assert!(added.contains("source `sim`, PASS"), "{added}");
    assert!(added.contains("not a hardware readout"), "{added}");
    assert!(
        added.contains(&format!("```json\n{}\n```", json.trim())),
        "{added}"
    );
    // The real protocol doc was never touched.
    assert_eq!(std::fs::read_to_string(real_doc()).unwrap(), original);
}

#[test]
fn a_failing_run_exits_non_zero_naming_the_rule_and_still_files_the_readout() {
    let dir = scratch("fail");
    let json_path = dir.join("rtlsdr-smoke.json");
    let out = smoke(&[
        "--sim",
        "rf-reference",
        "--freq",
        "100.1e6",
        "--json-out",
        json_path.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a bare carrier must FAIL");
    assert!(
        stderr.contains("SDR SMOKE FAILED (sim): empty-decode"),
        "{stderr}"
    );
    let json = std::fs::read_to_string(&json_path).unwrap();
    assert!(json.contains(r#""pass":false"#), "{json}");
}

#[test]
fn an_unknown_scene_fails_before_any_capture() {
    let out = smoke(&["--sim", "rf-nope", "--freq", "100.1e6"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown sim scene"));
}
