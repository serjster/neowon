//! History asked for under a merged-away id returns the canonical
//! signal's rows, in (time, id) order.

mod common;
use common::*;
use neowon_catalog::*;

#[test]
fn history_follows_redirects() {
    let dir = scratch("history");
    let mut cat = Catalog::open(&dir).unwrap();
    let a = add(&mut cat, |id| signal(id, "A", 99.4e6, None));
    let b = add(&mut cat, |id| signal(id, "B", 99.4e6, None));
    let c = add(&mut cat, |id| signal(id, "C", 99.4e6, None));
    let mut rows = Vec::new();
    for (s, t) in [(a, 3.0), (b, 2.0), (a, 1.0), (c, 2.0), (b, 5.0)] {
        rows.push((t, add(&mut cat, |id| observation(id, s, t))));
    }
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
    rows.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    let want: Vec<Id> = rows.iter().map(|r| r.1).collect();
    for asked in [a, b, c] {
        let h = cat.state().history(asked).unwrap();
        assert_eq!(
            h.iter().map(|o| o.id).collect::<Vec<_>>(),
            want,
            "asked {asked}"
        );
        assert!(
            h.iter().all(|o| o.signal == b),
            "all rows on the canonical signal"
        );
    }
    // The same after a restart.
    drop(cat);
    let cat = Catalog::open(&dir).unwrap();
    assert_eq!(cat.state().history(a).unwrap().len(), 5);
    std::fs::remove_dir_all(&dir).unwrap();
}
