//! All entities round-trip field-exact, through WAL replay and through a
//! snapshot.

mod common;
use common::*;
use neowon_catalog::*;

fn populate(cat: &mut Catalog) {
    let (so, si, tr, ob, em, bp, su) = (
        cat.next_id(),
        cat.next_id(),
        cat.next_id(),
        cat.next_id(),
        cat.next_id(),
        cat.next_id(),
        cat.next_id(),
    );
    insert(cat, source(so));
    insert(cat, signal(si, "BBC R2", 99.405_4e6, Some(so)));
    insert(cat, transmission(tr, si));
    let mut o = observation(ob, si, 12.5);
    if let Entity::Observation(x) = &mut o {
        x.transmission = Some(tr);
        x.confidence = Some(0.25);
    }
    insert(cat, o);
    insert(cat, emitter(em, so));
    insert(cat, band_plan(bp));
    insert(cat, survey(su));
    cat.commit(Op::Rename {
        id: si,
        name: "Radio 2".into(),
        at: AT.into(),
    })
    .unwrap();
    cat.commit(Op::Tag {
        id: si,
        tag: "music".into(),
        on: true,
    })
    .unwrap();
}

#[test]
fn every_entity_survives_wal_replay_and_snapshot() {
    let dir = scratch("roundtrip");
    let before = {
        let mut cat = Catalog::open(&dir).unwrap();
        populate(&mut cat);
        cat.state().clone()
        // Dropped without close: the next open replays the WAL.
    };
    assert_eq!(before.entities.len(), 7);
    let after_wal = Catalog::open(&dir).unwrap();
    assert_eq!(after_wal.state(), &before, "WAL replay");
    after_wal.close().unwrap();
    let after_snapshot = Catalog::open(&dir).unwrap();
    assert_eq!(after_snapshot.state(), &before, "snapshot");
    // Field-exact means bit-exact for floats, not approximately equal.
    let Some(Entity::Signal(s)) = after_snapshot
        .state()
        .entities
        .values()
        .find(|e| e.kind() == "signal")
    else {
        panic!("no signal");
    };
    assert_eq!(s.bandwidth_hz.to_bits(), 115_123.456_789_012_3f64.to_bits());
    assert_eq!(s.aliases[0].name, "BBC R2");
    drop(after_snapshot);

    let doc = {
        let cat = Catalog::open(&dir).unwrap();
        exchange::export(&cat)
    };
    let dir2 = scratch("roundtrip-import");
    let mut other = Catalog::open(&dir2).unwrap();
    let ids = exchange::import(&mut other, &doc, AT).unwrap();
    assert_eq!(ids.len(), 7);
    assert!(
        other.state().integrity().is_empty(),
        "{:?}",
        other.state().integrity()
    );
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&dir2).unwrap();
}

#[test]
fn undo_takes_back_edits_exactly() {
    let dir = scratch("undo");
    let mut cat = Catalog::open(&dir).unwrap();
    let si = cat.next_id();
    insert(&mut cat, signal(si, "A", 100e6, None));
    let orig = cat.state().get(si).unwrap().clone();
    cat.commit(Op::Rename {
        id: si,
        name: "B".into(),
        at: AT.into(),
    })
    .unwrap();
    cat.commit(Op::Tag {
        id: si,
        tag: "broadcast".into(), // already present: undo must keep it
        on: true,
    })
    .unwrap();
    cat.commit(Op::Edit {
        id: si,
        field: "notes".into(),
        value: serde_json::json!("edited"),
    })
    .unwrap();
    for _ in 0..3 {
        cat.undo().unwrap().unwrap();
    }
    assert_eq!(cat.state().get(si).unwrap(), &orig);
    cat.commit(Op::Delete {
        id: si,
        cascade: false,
        at: AT.into(),
    })
    .unwrap();
    cat.undo().unwrap().unwrap();
    assert_eq!(cat.state().get(si).unwrap(), &orig);
    std::fs::remove_dir_all(&dir).unwrap();
}
