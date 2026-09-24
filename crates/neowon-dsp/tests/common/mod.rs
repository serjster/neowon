//! Shared plumbing for the DSP golden runs: filing a run's readout.
//!
//! Every golden in this directory measures numbers a later reader is meant
//! to re-judge — macro precision at an SNR, an EVM against its closed form,
//! a detected bandwidth against the exact one. Printed to stdout they live
//! only as long as the terminal scrollback (and `cargo test` hides them
//! without `--nocapture`), so what survives a run is the assertion's
//! verdict and not the numbers it passed on. A number nobody can read back
//! is a number nobody can re-judge.
//!
//! So each run **files** its readout as one JSON document at a path it
//! prints, and the filing is itself checked: [`file_readout`] re-reads what
//! it wrote and fails the test if the bytes did not survive the trip or are
//! not a balanced JSON object. A readout that did not reach the disk fails
//! the run that produced it.
//!
//! The directory is Cargo's own per-crate test scratch,
//! `CARGO_TARGET_TMPDIR` (`target/tmp/` in this workspace), with
//! a `readouts/` subdirectory: inside the project as the harness requires,
//! already git-ignored with the rest of `target/`, and removed by
//! `cargo clean` like any other build output.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn readouts_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("readouts")
}

/// File `body` (one complete JSON object) as `<name>.json`, print the path,
/// and return it. Panics if the document is not a balanced JSON object or
/// does not read back exactly as written.
pub fn file_readout(name: &str, body: &str) -> PathBuf {
    assert!(
        is_json_object(body),
        "readout {name} is not a balanced JSON object: {body}"
    );
    let dir = readouts_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    let path = dir.join(format!("{name}.json"));
    // Written under a name only this process uses, checked there, then
    // renamed into place: two concurrent runs of the same golden never read
    // each other's half-written file, and the filed path is always complete.
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body).unwrap_or_else(|e| panic!("write {}: {e}", tmp.display()));
    let back = std::fs::read_to_string(&tmp).unwrap_or_else(|e| panic!("reread: {e}"));
    assert_eq!(back, body, "readout {name} did not survive the round trip");
    std::fs::rename(&tmp, &path).unwrap_or_else(|e| panic!("file {}: {e}", path.display()));
    println!("readout: {}", path.display());
    path
}

/// A cheap structural check — balanced braces and brackets outside strings,
/// opening `{` and closing `}`, nothing after it. Enough to catch a readout
/// assembled from an unbalanced `format!`, which is the way these break;
/// the crate has no JSON dependency and is not getting one for a test.
fn is_json_object(s: &str) -> bool {
    let s = s.trim();
    if !s.starts_with('{') || !s.ends_with('}') {
        return false;
    }
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (i, c) in s.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                // The object must close exactly once, at the very end.
                if depth == 0 && i + c.len_utf8() != s.len() {
                    return false;
                }
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && !in_string
}

#[cfg(test)]
mod tests {
    use super::is_json_object;

    #[test]
    fn the_structural_check_rejects_what_it_should() {
        assert!(is_json_object(r#"{"a":[1,2],"b":{"c":"}"}}"#));
        assert!(!is_json_object(r#"{"a":[1,2}"#), "crossed brackets");
        assert!(!is_json_object(r#"{"a":1"#), "unclosed");
        assert!(!is_json_object(r#"{"a":1}{"b":2}"#), "two documents");
        assert!(!is_json_object(r#"[1,2]"#), "not an object");
        assert!(!is_json_object(r#"{"a":"unterminated}"#), "open string");
    }
}
