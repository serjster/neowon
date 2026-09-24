//! A batch of ops costs one fsync, not one per op — and nothing about
//! durability, ordering or the failure shape changes with it.
//!
//! The check is on `wal_syncs`, the fsync count the writer keeps, because an
//! fsync is what a commit costs. Without that number the batching claim would
//! be unfalsifiable: the ops land either way.

mod common;
use common::*;
use neowon_catalog::*;

fn tag(id: Id, tag: &str) -> Op {
    Op::Tag {
        id,
        tag: tag.into(),
        on: true,
    }
}

#[test]
fn a_batch_pays_one_fsync_and_a_loop_pays_one_each() {
    let dir = scratch("batch-sync");
    let mut cat = Catalog::open(&dir).unwrap();
    // Take checkpointing out of the picture: it starts a new segment and
    // resets the count.
    cat.checkpoint_every = usize::MAX;
    let a = add(&mut cat, |id| signal(id, "A", 100e6, None));
    let b = add(&mut cat, |id| signal(id, "B", 101e6, None));

    let before = cat.wal_syncs();
    cat.commit_many(vec![tag(a, "one"), tag(a, "two"), tag(b, "three")])
        .unwrap();
    assert_eq!(
        cat.wal_syncs() - before,
        1,
        "three ops in one batch must cost one fsync"
    );

    let before = cat.wal_syncs();
    for t in ["four", "five", "six"] {
        cat.commit(tag(a, t)).unwrap();
    }
    assert_eq!(
        cat.wal_syncs() - before,
        3,
        "the per-op path is what the batch saves against"
    );

    // Every op really landed, in order, and survives a reopen.
    let seq = cat.seq();
    drop(cat);
    let cat = Catalog::open(&dir).unwrap();
    assert_eq!(cat.seq(), seq, "the batch is durable, not just in memory");
    let tags = |id: Id| match cat.state().entities.get(&id) {
        Some(Entity::Signal(s)) => s.tags.clone(),
        _ => panic!("signal {id} missing after replay"),
    };
    assert_eq!(
        tags(a),
        tags_of(&["broadcast", "one", "two", "four", "five", "six"])
    );
    assert_eq!(tags(b), tags_of(&["broadcast", "three"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_op_keeps_the_batch_before_it_and_reports_the_error() {
    let dir = scratch("batch-refused");
    let mut cat = Catalog::open(&dir).unwrap();
    cat.checkpoint_every = usize::MAX;
    let a = add(&mut cat, |id| signal(id, "A", 100e6, None));
    let missing = Id(9_999);

    let before = cat.wal_syncs();
    let err = cat
        .commit_many(vec![
            tag(a, "kept"),
            tag(missing, "refused"),
            tag(a, "never"),
        ])
        .unwrap_err();
    assert!(
        format!("{err}").contains("9999") || matches!(err, Error::NotFound(_)),
        "the refusal must name what was refused: {err}"
    );
    assert_eq!(
        cat.wal_syncs() - before,
        1,
        "the ops that did apply are still made durable, in one sync"
    );

    // Same shape as a `commit` loop: everything before the refusal stands,
    // nothing after it does.
    drop(cat);
    let cat = Catalog::open(&dir).unwrap();
    let Some(Entity::Signal(s)) = cat.state().entities.get(&a) else {
        panic!("signal missing after replay");
    };
    assert_eq!(s.tags, tags_of(&["broadcast", "kept"]));
    let _ = std::fs::remove_dir_all(&dir);
}

fn tags_of(v: &[&str]) -> std::collections::BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}
