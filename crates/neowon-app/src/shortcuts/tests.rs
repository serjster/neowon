use super::*;
use neowon_backend::Backend;

/// A link on no instrument whose command channel the test reads.
fn link() -> (Link, crossbeam_channel::Receiver<Command>) {
    let mut sup = neowon_backend::spawn(|| -> Result<Box<dyn Backend>, String> {
        Err("no instrument in unit tests".into())
    });
    let (tx, rx) = crossbeam_channel::unbounded();
    sup.commands = tx;
    let mut config = crate::view::startup_config();
    config.position = 0.3; // off home, so `H` has something to do
    let link = Link {
        sup,
        caps: None,
        status: String::new(),
        latest: None,
        config,
        dirty: false,
        frames_seen: 0,
        multi: neowon_backend::MultiMode::TriggerOut,
        last_frame_at: 0.0,
        arrived: Vec::new(),
        stimulus: String::new(),
        selected: 0,
        last_shot: None,
    };
    (link, rx)
}

fn phosphor() -> Phosphor {
    Phosphor {
        hview: (0.3, 0.2),
        zoom_on: true,
        ..Default::default()
    }
}

/// What one press changed: the scope config, the display, the acquisition
/// choice, or a command down the link.
#[derive(Debug, PartialEq)]
struct Seen {
    config: neowon_backend::ScopeConfig,
    dirty: bool,
    mode: TraceMode,
    persistence: Persistence,
    hview: (f64, f64),
    user_acq: AcqMode,
    commands: usize,
}

fn press(key: KeyCode, workspace: Home) -> (Seen, Seen) {
    let (mut link, rx) = link();
    let mut phosphor = phosphor();
    let mut autopeak = AutoPeak::default();
    let seen = |l: &Link, p: &Phosphor, a: &AutoPeak, n: usize| Seen {
        config: l.config.clone(),
        dirty: l.dirty,
        mode: p.mode,
        persistence: p.persistence,
        hview: p.hview,
        user_acq: a.user_acq,
        commands: n,
    };
    let before = seen(&link, &phosphor, &autopeak, 0);
    let mut target = Target {
        link: &mut link,
        phosphor: &mut phosphor,
        autopeak: &mut autopeak,
    };
    fire(|k| k == key, workspace, &mut target);
    let after = seen(&link, &phosphor, &autopeak, rx.try_iter().count());
    (before, after)
}

/// The invariant, table-driven over every binding: a scope key acts in the
/// scope workspace and does nothing at all in the SDR one.
#[test]
fn a_scope_shortcut_acts_only_in_the_scope_workspace() {
    let scope: Vec<_> = SHORTCUTS.iter().filter(|s| s.home == Home::Scope).collect();
    assert!(!scope.is_empty());
    for s in scope {
        let (before, after) = press(s.key, Home::Scope);
        assert_ne!(
            before, after,
            "{:?} ({}) did nothing in scope",
            s.key, s.what
        );
        let (before, after) = press(s.key, Home::Sdr);
        assert_eq!(before, after, "{:?} ({}) acted in SDR", s.key, s.what);
    }
}

#[test]
fn autoset_is_not_sent_from_the_sdr_workspace() {
    let (_, after) = press(KeyCode::KeyA, Home::Sdr);
    assert_eq!(after.commands, 0);
    let (_, after) = press(KeyCode::KeyA, Home::Scope);
    assert_eq!(after.commands, 1);
}

#[test]
fn every_key_is_bound_once() {
    for (i, a) in SHORTCUTS.iter().enumerate() {
        for b in &SHORTCUTS[i + 1..] {
            assert_ne!(a.key, b.key, "{} and {} share a key", a.what, b.what);
        }
    }
}

#[test]
fn a_global_shortcut_acts_everywhere_and_a_homed_one_only_at_home() {
    for w in [Home::Scope, Home::Sdr] {
        assert!(Home::Global.acts_in(w));
        assert!(w.acts_in(w));
    }
    assert!(!Home::Scope.acts_in(Home::Sdr));
    assert!(!Home::Sdr.acts_in(Home::Scope));
    assert_eq!(Home::active(true), Home::Sdr);
    assert_eq!(Home::active(false), Home::Scope);
}
