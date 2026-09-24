//! `catalog add` files the tuned track's modulation only from a
//! verdict on that same track, never from the classifier's verdict on
//! another signal.

use neowon_catalog::{Catalog, Entity};

use crate::sdr::join_tests::two_signals;

fn filed_modulation(sdr: &crate::sdr::SdrState, name: &str) -> Option<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("catalog-join-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cat = Catalog::open(&dir).unwrap();
    let id = super::from_tuned(&mut cat, sdr, "2026-09-23T00:00:00Z").unwrap();
    let m = match cat.state().entities.get(&id) {
        Some(Entity::Signal(s)) => s.modulation.clone(),
        e => panic!("not a signal: {e:?}"),
    };
    drop(cat);
    let _ = std::fs::remove_dir_all(&dir);
    m
}

#[test]
fn a_verdict_on_another_track_is_not_filed() {
    let (mut sdr, frame, _, qpsk) = two_signals();
    // The classifier last judged QPSK, confidently; the cursor is on 16QAM.
    sdr.classification = crate::sdr::analysis::classify(&frame, 100e6, &qpsk);
    let verdict = sdr.classification.clone().unwrap();
    assert!(!verdict.verdict.unknown, "{verdict:?}");
    assert_eq!(filed_modulation(&sdr, "other"), None);
    // With the cursor on QPSK the same verdict is its own, and is filed.
    sdr.tuned_hz = 100.3e6;
    assert_eq!(
        filed_modulation(&sdr, "own").as_deref(),
        Some(verdict.verdict.class.label())
    );
}
