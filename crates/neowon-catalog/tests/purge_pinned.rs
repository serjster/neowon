//! Purge never removes a pinned entry.

mod common;
use common::*;
use neowon_catalog::*;

#[test]
fn purge_skips_pinned() {
    let dir = scratch("purge");
    let mut cat = Catalog::open(&dir).unwrap();
    let ids: Vec<Id> = (0..4)
        .map(|i| {
            let id = cat.next_id();
            insert(
                &mut cat,
                signal(id, &format!("S{i}"), 100e6 + i as f64, None),
            )
        })
        .collect();
    cat.commit(Op::Pin {
        id: ids[1],
        on: true,
    })
    .unwrap();
    let pinned = |c: &Catalog| c.state().entities.values().filter(|e| e.pinned()).count();
    assert_eq!(pinned(&cat), 1);
    add(&mut cat, |id| observation(id, ids[2], 1.0));
    // An observation blocks the purge without cascade...
    assert!(
        cat.commit(Op::Purge {
            ids: ids.clone(),
            cascade: false,
            at: AT.into()
        })
        .is_err()
    );
    // ...and with it, everything but the pinned signal goes.
    cat.commit(Op::Purge {
        ids: ids.clone(),
        cascade: true,
        at: AT.into(),
    })
    .unwrap();
    assert_eq!(pinned(&cat), 1, "0 pinned removed");
    assert!(cat.state().get(ids[1]).is_some());
    assert_eq!(cat.state().entities.len(), 1);
    assert!(cat.state().tombstones.contains_key(&ids[2]));
    // Deleting a pinned entry is refused outright.
    assert!(matches!(
        cat.commit(Op::Delete {
            id: ids[1],
            cascade: true,
            at: AT.into()
        }),
        Err(Error::Pinned(_))
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}
