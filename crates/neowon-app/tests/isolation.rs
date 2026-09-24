//! The harness's own invariant: a test's outcome depends
//! only on the tree and its seed — never on another process, another test,
//! or what the operator left on disk.
//!
//! The first two tests are pure and run by default. The windowed ones
//! check the same rule against a running app, and that a test which panics
//! takes its app with it:
//!   cargo test -p neowon-app --test isolation -- --ignored

mod common;
use common::*;

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// What `cmd` will set (`Some`) or remove (`None`) for `key`; absent when
/// the child simply inherits it.
fn env_of<'a>(cmd: &'a std::process::Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn a_spawned_app_gets_a_private_home_and_none_of_the_callers_neowon_vars() {
    let home = Path::new("/sandbox/home");
    let parent: Vec<(OsString, OsString)> = [
        ("HOME", "/Users/operator"),
        ("PATH", "/usr/bin"),
        ("NEOWON_CATALOG", "/Users/operator/.neowon/catalog"),
        ("NEOWON_REFDB", "/Users/operator/.neowon/refdb"),
        ("NEOWON_SIM_FPS", "5"),
        ("NEOWON_CONTROL_TOKEN", "the-operators-token"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    let cmd = sandbox::isolate(std::process::Command::new("app"), home, parent);

    for key in ["HOME", "USERPROFILE"] {
        assert_eq!(
            env_of(&cmd, key),
            Some(Some(home.as_os_str())),
            "{key} is not the sandbox"
        );
    }
    for key in [
        "NEOWON_CATALOG",
        "NEOWON_REFDB",
        "NEOWON_SIM_FPS",
        "NEOWON_CONTROL_TOKEN",
    ] {
        assert_eq!(env_of(&cmd, key), Some(None), "{key} leaks into the app");
    }
    // Off unless the test turns them on: no socket on the shared default
    // port, no saved workspace.
    assert_eq!(
        env_of(&cmd, "NEOWON_CONTROL"),
        Some(Some(OsStr::new("off")))
    );
    assert_eq!(env_of(&cmd, "NEOWON_NO_STATE"), Some(Some(OsStr::new("1"))));
    // Host input ignored, no focus taken: the operator's keys and
    // wheel belong to the operator.
    assert_eq!(env_of(&cmd, "NEOWON_NO_INPUT"), Some(Some(OsStr::new("1"))));
    // Everything else is inherited untouched.
    assert_eq!(env_of(&cmd, "PATH"), None);
}

#[test]
fn scratch_directories_are_fresh_and_never_shared() {
    let a = scratch("iso");
    let b = scratch("iso");
    assert_ne!(a, b, "two calls in one process share a directory");
    let pid = std::process::id().to_string();
    for d in [&a, &b] {
        let name = d.file_name().unwrap().to_string_lossy();
        assert!(name.contains(&pid), "{name} is not namespaced by process");
        assert_eq!(
            std::fs::read_dir(d).unwrap().count(),
            0,
            "{name} is not empty"
        );
    }
    // A leftover from an earlier run under the same name is cleared.
    let sb = Sandbox::new("iso");
    std::fs::write(sb.home().join("stale"), b"x").unwrap();
    let home = sb.home().to_path_buf();
    drop(sb);
    assert!(!home.exists(), "a sandbox outlived its launch");
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// Another launch that lands on a port already taken must not end up
/// talking to the app holding it.
#[test]
#[ignore = "opens a window"]
fn a_launch_never_talks_to_an_app_it_did_not_start() {
    let (holder, mut first) = launch(&["--sim"], &[]);
    with_app(holder, || {
        first.wait("get status", 15, |r| field(r, "frames_seen") > 0.0);
        // A second launch forced onto the first one's port: its app cannot
        // bind, so the only listener there is the first app.
        match launch_on(first.port, &["--sim"], &[]) {
            Ok((mut stray, _)) => {
                let _ = stray.kill();
                let _ = stray.wait();
                panic!("a launch accepted port {} held by another app", first.port);
            }
            Err(e) => assert!(e.contains("did not start"), "{e}"),
        }
        // The holder was not disturbed.
        assert!(first.request("get status").contains(r#""ok":true"#));
    });
}

/// The app's per-user state resolves under the sandbox, not under the
/// `HOME` of the process that ran the tests.
#[test]
#[ignore = "opens a window"]
fn the_app_keeps_its_per_user_state_in_the_sandbox() {
    let (child, mut c) = launch(&["--sdr-sim"], &[]);
    with_app(child, || {
        let cat = c.request("get catalog");
        let path = raw(&cat, "path").trim_matches('"').to_string();
        assert!(
            Path::new(&path).starts_with(c.home()),
            "catalog {path} is outside the sandbox {}",
            c.home().display()
        );
        if let Some(operator) = std::env::var_os("HOME")
            && !c.home().starts_with(&operator)
        {
            assert!(
                !Path::new(&path).starts_with(&operator),
                "catalog {path} is under the caller's HOME"
            );
        }
    });
}

/// Whatever spawns the app ends it on every path, a panic
/// included: a test body that panics with its app running must not leave
/// that app to the 15 s orphan watchdog. The body runs on its own thread so
/// the panic is real (it unwinds through the test's locals) and this test
/// can still look at the pid afterwards.
#[test]
#[ignore = "opens a window"]
fn a_test_that_panics_takes_its_app_with_it() {
    let (tx, rx) = std::sync::mpsc::channel();
    let body = std::thread::spawn(move || {
        let (app, mut c) = launch(&["--sim"], &[]);
        c.wait("get status", 15, |r| field(r, "frames_seen") > 0.0);
        tx.send(app.id()).unwrap();
        panic!("a test body failing mid-way");
    });
    assert!(body.join().is_err(), "the body was meant to panic");
    let pid = rx.recv().expect("the app was launched");
    let alive = || {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while alive() {
        assert!(
            std::time::Instant::now() < deadline,
            "the app (pid {pid}) outlived its panicking test by 3 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
