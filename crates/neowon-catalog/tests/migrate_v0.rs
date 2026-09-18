//! A v0 catalog loads and is re-saved at v1; a newer one is refused.
//!
//! The v0 catalog is built here from the documented difference (manifest
//! without `schema`; transmissions tagged `"entity": "emission"`), in both
//! the snapshot and a schema-0 WAL segment.

mod common;
use std::io::Write;

use common::*;
use neowon_catalog::*;

fn v0(json: String) -> String {
    json.replace(r#""entity":"transmission""#, r#""entity":"emission""#)
}

#[test]
fn v0_loads_and_resaves_at_v1() {
    let dir = scratch("v0");
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = State::default();
    let (sig, t1, t2) = (st.alloc_id(), st.alloc_id(), st.alloc_id());
    st.apply(&Op::Insert {
        entity: signal(sig, "A", 99.4e6, None),
    })
    .unwrap();
    st.apply(&Op::Insert {
        entity: transmission(t1, sig),
    })
    .unwrap();
    let snapshot = v0(serde_json::to_string(&st).unwrap());
    assert!(snapshot.contains("emission"));
    std::fs::write(dir.join("snapshot-2.json"), snapshot).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"format":"neowon-catalog","last_seq":2,"snapshot":"snapshot-2.json","wal":"wal-1.log"}"#,
    )
    .unwrap();
    // A schema-0 segment holding one more emission after the snapshot.
    let mut seg = std::fs::File::create(dir.join("wal-1.log")).unwrap();
    seg.write_all(&wal::frame(
        br#"{"format":"neowon-catalog-wal","schema":0}"#,
    ))
    .unwrap();
    let rec = wal::Record {
        seq: 3,
        op: Op::Insert {
            entity: transmission(t2, sig),
        },
    };
    seg.write_all(&wal::frame(
        v0(serde_json::to_string(&rec).unwrap()).as_bytes(),
    ))
    .unwrap();
    drop(seg);

    let cat = Catalog::open(&dir).unwrap();
    for t in [t1, t2] {
        assert_eq!(cat.state().get(t).map(Entity::kind), Some("transmission"));
    }
    assert!(cat.state().integrity().is_empty());
    drop(cat);
    // Re-saved at v1: the manifest carries the schema and nothing on disk
    // says "emission" any more.
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert!(manifest.contains(r#""schema": 1"#), "{manifest}");
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let bytes = std::fs::read(e.path()).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("emission"),
            "{:?}",
            e.path()
        );
    }
    let again = Catalog::open(&dir).unwrap();
    assert_eq!(again.state().entities.len(), 3);
    drop(again);

    // A catalog from a newer build is refused, not misread.
    let newer = manifest.replace(r#""schema": 1"#, r#""schema": 2"#);
    std::fs::write(dir.join("manifest.json"), newer).unwrap();
    assert!(matches!(Catalog::open(&dir), Err(Error::TooNew(2))));
    std::fs::remove_dir_all(&dir).unwrap();
}
