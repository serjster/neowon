//! The integrity query is empty after merges, including a merge chain.

mod common;
use common::*;
use neowon_catalog::*;

#[test]
fn integrity_is_empty_after_merges() {
    let dir = scratch("integrity");
    let mut cat = Catalog::open(&dir).unwrap();
    let so = add(&mut cat, source);
    let a = add(&mut cat, |id| signal(id, "A", 99.4e6, Some(so)));
    let b = add(&mut cat, |id| signal(id, "B", 99.4e6, Some(so)));
    let c = add(&mut cat, |id| signal(id, "C", 99.4e6, None));
    add(&mut cat, |id| transmission(id, a));
    for (s, t) in [(a, 1.0), (b, 2.0), (c, 3.0)] {
        add(&mut cat, |id| observation(id, s, t));
    }
    // C into A, then A into B: C's redirect is compressed to point at B.
    cat.commit(Op::Merge {
        from: c,
        to: a,
        at: AT.into(),
    })
    .unwrap();
    cat.commit(Op::Merge {
        from: a,
        to: b,
        at: AT.into(),
    })
    .unwrap();
    let st = cat.state();
    assert_eq!(st.integrity(), Vec::<String>::new());
    assert_eq!(st.resolve(c).unwrap(), b);
    assert_eq!(st.redirects[&c].to, b, "chain compressed to one hop");
    // Merging into something that redirects back to the source is refused.
    assert!(
        cat.commit(Op::Merge {
            from: b,
            to: c,
            at: AT.into()
        })
        .is_err()
    );
    // A deleted referrer is refused without cascade, and cascades
    // transitively with it (source -> signal -> observations).
    assert!(matches!(
        cat.commit(Op::Delete {
            id: so,
            cascade: false,
            at: AT.into()
        }),
        Err(Error::Referenced(..))
    ));
    cat.commit(Op::Delete {
        id: so,
        cascade: true,
        at: AT.into(),
    })
    .unwrap();
    assert!(
        cat.state().integrity().is_empty(),
        "{:?}",
        cat.state().integrity()
    );
    drop(cat);
    assert!(Catalog::open(&dir).unwrap().state().integrity().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}
