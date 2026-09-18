//! D8: the reference IQ scene, seed 1, 1024 pairs, is bit-identical to the
//! checked-in fixture on every platform. The fixture was written by
//! `cargo run -p neowon-cli -- sim iq --seed 1 --n 1024 --out <fixture>`;
//! a legitimate change to `IqScene::reference` regenerates it the same way.

use neowon_sim::IqScene;
use neowon_sim::iq::to_le_bytes;

const FIXTURE: &[u8] = include_bytes!("fixtures/iq_seed1_n1024.f32");

#[test]
fn seed1_n1024_matches_fixture_bit_exact() {
    let bytes = to_le_bytes(&IqScene::reference().samples(1, 0, 1024));
    assert_eq!(bytes.len(), 1024 * 2 * 4);
    assert_eq!(FIXTURE.len(), bytes.len(), "fixture length");
    let first = bytes.iter().zip(FIXTURE).position(|(a, b)| a != b);
    assert_eq!(first, None, "first differing byte");
}

#[test]
fn another_seed_differs() {
    let bytes = to_le_bytes(&IqScene::reference().samples(2, 0, 1024));
    assert_ne!(bytes.as_slice(), FIXTURE);
}
