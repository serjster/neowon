//! Every file the app writes appears whole: a reader — an
//! operator's script, a test polling `exists()`, the next launch — sees the
//! target absent or complete, never partly written. The mechanism is one
//! helper, `neowon_core::atomic_file` (temp beside the target, sync, rename);
//! its own tests step a reader through a write. This test keeps every
//! writer on it: a new in-place `fs::write` / `File::create` in the app, or
//! in the crates whose writers the app exposes (`.nwc`, WAV, catalog
//! export), fails here until it is routed through the helper or listed
//! below with its reason.

use std::path::{Path, PathBuf};

/// In-place writers that are right as they are.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "neowon-core/src/atomic_file.rs",
        "*",
        "the helper itself; writes in place only a target that is not a regular file",
    ),
    (
        "neowon-catalog/src/wal.rs",
        "OpenOptions",
        "the WAL is an append-only log; a torn tail is detected by its CRC framing",
    ),
    (
        "neowon-catalog/src/store.rs",
        "OpenOptions",
        "the catalog's lock file: a claim, not content anyone reads",
    ),
];

const WRITERS: &[&str] = &["fs::write(", "File::create(", "OpenOptions"];

/// `w` appears in `line` as its own path segment (so `AtomicFile::create(`
/// is not `File::create(`).
fn uses(line: &str, w: &str) -> bool {
    line.match_indices(w).any(|(i, _)| {
        !line[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn every_app_writer_goes_through_the_atomic_helper() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for c in ["neowon-app", "neowon-core", "neowon-catalog"] {
        sources(&crates.join(c).join("src"), &mut files);
    }
    assert!(files.len() > 50, "scanned only {} files", files.len());
    let mut bad = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(f).unwrap();
        // Unit tests may write their own fixtures however they like.
        let code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        for (n, line) in code.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            for w in WRITERS {
                let allowed = ALLOWED
                    .iter()
                    .any(|(p, a, _)| *p == rel && (a == w || *a == "*"));
                if uses(line, w) && !allowed {
                    bad.push(format!("{rel}:{}: {line}", n + 1));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "in-place writers (route through neowon_core::atomic_file):\n{}",
        bad.join("\n")
    );
}
