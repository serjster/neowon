//! An op is undoable only if its inverse really restores the prior state.
//! For every op kind: commit it on a catalog whose undo stack holds
//! one earlier, unrelated edit, then undo once. Either the undo takes the
//! op back exactly — entities and redirects equal to what they were — or
//! there is nothing to take back. An undo that errors, or lands on some
//! other state (the earlier edit rewound, a redirect lost), is the defect.
//!
//! Tombstones are bookkeeping, not state a reader sees: undoing an insert
//! leaves one for the id it handed out, which is never reused anyway.

mod common;
use std::collections::BTreeMap;

use common::*;
use neowon_catalog::*;

struct Fixture {
    cat: Catalog,
    /// A signal with a source, an observation and a transmission.
    a: Id,
    /// A signal nothing references.
    b: Id,
    /// A signal another signal was merged into (a redirect points at it).
    d: Id,
    /// The id merged into `d`.
    c: Id,
    /// A deleted signal, as it was.
    f_entity: Entity,
}

fn fixture(name: &str) -> Fixture {
    let mut cat = Catalog::open(scratch(name)).unwrap();
    let so = add(&mut cat, source);
    let a = add(&mut cat, |id| signal(id, "A", 100e6, Some(so)));
    add(&mut cat, |id| observation(id, a, 1.0));
    add(&mut cat, |id| transmission(id, a));
    let b = add(&mut cat, |id| signal(id, "B", 101e6, None));
    let c = add(&mut cat, |id| signal(id, "C", 102e6, None));
    let d = add(&mut cat, |id| signal(id, "D", 102e6, None));
    let f = add(&mut cat, |id| signal(id, "F", 103e6, None));
    let f_entity = cat.state().get(f).unwrap().clone();
    cat.commit(Op::Delete {
        id: f,
        cascade: false,
        at: AT.into(),
    })
    .unwrap();
    // The merge clears the undo stack; the edit after it is then the one
    // entry beneath the op under test, which a declined undo must leave.
    cat.commit(Op::Merge {
        from: c,
        to: d,
        at: AT.into(),
    })
    .unwrap();
    let e = add(&mut cat, |id| signal(id, "E", 104e6, None));
    cat.commit(Op::Edit {
        id: e,
        field: "notes".into(),
        value: serde_json::json!("earlier"),
    })
    .unwrap();
    Fixture {
        cat,
        a,
        b,
        d,
        c,
        f_entity,
    }
}

type Seen = (BTreeMap<Id, Entity>, BTreeMap<Id, state::Redirect>);

fn seen(cat: &Catalog) -> Seen {
    (cat.state().entities.clone(), cat.state().redirects.clone())
}

#[test]
fn every_undo_restores_exactly_or_declines() {
    type Make = fn(&mut Fixture) -> Op;
    let cases: Vec<(&str, Make)> = vec![
        ("insert", |x| Op::Insert {
            entity: signal(x.cat.next_id(), "new", 1e6, None),
        }),
        ("insert pinned", |x| {
            let mut e = signal(x.cat.next_id(), "pinned", 1e6, None);
            if let Entity::Signal(s) = &mut e {
                s.pinned = true;
            }
            Op::Insert { entity: e }
        }),
        ("restore", |x| Op::Restore {
            entity: x.f_entity.clone(),
        }),
        ("replace", |x| {
            let mut e = x.cat.state().get(x.a).unwrap().clone();
            if let Entity::Signal(s) = &mut e {
                s.notes = "replaced".into();
            }
            Op::Replace { entity: e }
        }),
        ("rename", |x| Op::Rename {
            id: x.a,
            name: "A2".into(),
            at: AT.into(),
        }),
        ("alias", |x| Op::Alias {
            id: x.a,
            name: "a".into(),
            at: AT.into(),
        }),
        ("unalias", |x| Op::Unalias {
            id: x.d,
            name: "C".into(),
        }),
        ("tag", |x| Op::Tag {
            id: x.a,
            tag: "t".into(),
            on: true,
        }),
        ("pin", |x| Op::Pin { id: x.b, on: true }),
        ("edit", |x| Op::Edit {
            id: x.a,
            field: "notes".into(),
            value: serde_json::json!("edited"),
        }),
        ("delete", |x| Op::Delete {
            id: x.b,
            cascade: false,
            at: AT.into(),
        }),
        // `d` has nothing referring to it, but `c` redirects into it; the
        // delete drops that redirect and a restore would not bring it back.
        ("delete a merge target", |x| Op::Delete {
            id: x.d,
            cascade: false,
            at: AT.into(),
        }),
        ("delete cascade", |x| Op::Delete {
            id: x.a,
            cascade: true,
            at: AT.into(),
        }),
        ("merge", |x| Op::Merge {
            from: x.b,
            to: x.a,
            at: AT.into(),
        }),
        ("purge", |x| Op::Purge {
            ids: vec![x.b],
            cascade: false,
            at: AT.into(),
        }),
    ];
    let mut wrong = Vec::new();
    for (what, make) in cases {
        let mut x = fixture(&format!("undo-{}", what.replace(' ', "-")));
        let op = make(&mut x);
        let before = seen(&x.cat);
        x.cat.commit(op).unwrap_or_else(|e| panic!("{what}: {e}"));
        let undone = x.cat.undo();
        let outcome = match &undone {
            Err(e) => Some(format!("undo failed: {e}")),
            Ok(Some(_)) if seen(&x.cat) != before => Some(format!(
                "undo landed elsewhere (c resolves to {:?})",
                x.cat.state().resolve(x.c)
            )),
            _ => None,
        };
        println!(
            "{what}: undo {}",
            match (&undone, &outcome) {
                (_, Some(o)) => o.clone(),
                (Ok(Some(_)), None) => "restored exactly".into(),
                _ => "declined (nothing to take back)".into(),
            }
        );
        if let Some(o) = outcome {
            wrong.push(format!("{what}: {o}"));
        }
        std::fs::remove_dir_all(x.cat.dir()).unwrap();
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn undo_after_a_cascade_delete_takes_back_nothing_earlier() {
    let mut x = fixture("undo-probe-b");
    x.cat
        .commit(Op::Rename {
            id: x.a,
            name: "renamed".into(),
            at: AT.into(),
        })
        .unwrap();
    x.cat
        .commit(Op::Delete {
            id: x.a,
            cascade: true,
            at: AT.into(),
        })
        .unwrap();
    assert_eq!(
        x.cat.undo().unwrap(),
        None,
        "the cascade cannot be taken back"
    );
    assert!(x.cat.state().get(x.a).is_none());
    let _ = std::fs::remove_dir_all(scratch("undo-probe-b"));
}

#[test]
fn undo_after_a_cascade_delete_leaves_an_unrelated_edit() {
    let mut x = fixture("undo-probe-b3");
    x.cat
        .commit(Op::Edit {
            id: x.b,
            field: "notes".into(),
            value: serde_json::json!("keep me"),
        })
        .unwrap();
    x.cat
        .commit(Op::Delete {
            id: x.a,
            cascade: true,
            at: AT.into(),
        })
        .unwrap();
    let undone = x.cat.undo().unwrap();
    let Some(Entity::Signal(b)) = x.cat.state().get(x.b) else {
        panic!("b gone");
    };
    assert_eq!(b.notes, "keep me", "undo rewound an unrelated edit");
    assert_eq!(undone, None);
    let _ = std::fs::remove_dir_all(scratch("undo-probe-b3"));
}
