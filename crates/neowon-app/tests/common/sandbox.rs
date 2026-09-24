//! Test isolation: a test's outcome depends only on the tree and its seed —
//! never on another process, another test, or what the operator left on
//! disk.
//!
//! Every app a test spawns goes through [`Sandbox::command`], which
//!
//! - points `HOME` (and `USERPROFILE`) at a fresh directory of its own, so
//!   everything the app keeps under `~` — `~/.neowon/{catalog, state.nws,
//!   refdb, location.json, bandplans, control/<port>.token}` and
//!   `~/neowon-captures` — is the sandbox's, never the operator's. A path
//!   the app adds under `~` later is covered without touching this file;
//! - drops every `NEOWON_*` variable inherited from the shell that ran
//!   `cargo test`, so an operator's exported override (a catalog, a refdb,
//!   a port, a sim knob) cannot reach a test's app;
//! - turns the control socket off (a scripted run otherwise binds the
//!   default port 7777 and writes that port's token file) and saved-state
//!   persistence off. A test opts back in by setting the variable itself;
//! - sets `NEOWON_NO_INPUT`: the app ignores the host's keyboard,
//!   mouse and wheel and opens without taking focus, so the operator typing
//!   or scrolling elsewhere can neither steer a test nor lose focus to it.
//!   Tests act through scripts and the socket, which it does not touch.
//!
//! [`scratch`] is the same rule for a test's own files: a directory no other
//! test and no other process shares.
//!
//! This file has no dependency on the app, so `neowon-mcp`'s end-to-end test
//! includes it by path rather than restating the rule.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// `<label>-<pid>-<n>`: distinct from every name another live process can
/// make (the pid) and from every other call in this one (the counter).
pub fn unique(label: &str) -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{label}-{}-{n}", std::process::id())
}

/// A fresh, empty directory under the OS temp dir that no other test and no
/// other process uses. The caller removes it when it is done (or leaves it
/// for a post-mortem; the name says whose it was).
pub fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neowon-{}", unique(label)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("scratch {}: {e}", dir.display()));
    dir
}

/// A private home for one spawned app, removed when dropped.
pub struct Sandbox {
    home: PathBuf,
}

impl Sandbox {
    pub fn new(label: &str) -> Self {
        Self {
            home: scratch(&format!("home-{label}")),
        }
    }

    /// The directory the app sees as `~`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `program`, isolated from this process's environment (module note).
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        isolate(Command::new(program), &self.home, std::env::vars_os())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// The isolation rule as a function of the parent's environment, so it can
/// be tested with a synthetic one.
pub fn isolate(
    mut cmd: Command,
    home: &Path,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    for (key, _) in parent {
        if key.to_string_lossy().starts_with("NEOWON_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("NEOWON_CONTROL", "off")
        .env("NEOWON_NO_STATE", "1")
        // No control-socket client watches a scripted run: the guard exits
        // an app a killed harness would otherwise leave on screen.
        .env("NEOWON_ORPHAN_EXIT", "120")
        .env("NEOWON_NO_INPUT", "1");
    cmd
}
