//! D8: the counter-indexed SplitMix64 against the published reference
//! sequence (Vigna's `splitmix64.c`, seed 0), independent of any fixture.

use neowon_sim::iq::splitmix64;

#[test]
fn seed0_matches_the_published_sequence() {
    assert_eq!(splitmix64(0, 0), 0xE220_A839_7B1D_CDAF);
    assert_eq!(splitmix64(0, 1), 0x6E78_9E6A_A1B9_65F4);
    assert_eq!(splitmix64(0, 2), 0x06C4_5D18_8009_454F);
}

#[test]
fn indexed_draws_equal_the_sequential_generator() {
    let seed = 0x1234_5678_9ABC_DEF0u64;
    let mut state = seed;
    for i in 0..1000 {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        assert_eq!(splitmix64(seed, i), z ^ (z >> 31), "draw {i}");
    }
}
