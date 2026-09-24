//! UEP (Unequal Error Protection) profiles for the MSC: clauses 11.3.1,
//! **table 8** (sub-channel size by index) and **table 15** (protection
//! profiles).
//!
//! **The tables are one table.** Table 8's 64 indices are ordered exactly like
//! table 15's 64 rows — bit rate, then protection level 5 down to 1 — with six
//! combinations marked "x" in table 8 omitted from both. So [`UEP_PROFILES`]
//! is indexed by the FIC's six-bit table index and carries, per entry, the
//! sub-channel size in 64-bit capacity units (table 8), the bit rate and level,
//! the four `(L1..L4, PI1..PI4)` regions (table 15), and the trailing padding
//! count table 15 specifies.
//!
//! The invariant that makes the merge safe is checked for every entry by
//! `every_profile_fills_its_size`: `L1..L4` cover exactly `4I` mother bits
//! where `I = 24·bitrate` (clause 11.3.1, table 14), and the punctured
//! codeword plus its padding fills the table-8 size exactly — `padding` is 0, 4
//! or 8 bits, never anything else.
//!
//! The values agree with the MIT reference `dabradio` 0.5.0's independently
//! transcribed table, entry for entry (`docs/protocol-dab.md` has the notice);
//! the tests pin the standard's printed rows as well, so the two agreeing is
//! evidence rather than the authority.
//!
//! Which profiles exist is the standard's choice, not an implementation limit:
//! 56/1, 112/1, 320/1, 320/3, 384/2 and 384/4 are not provided, and
//! [`uep_profile_for`] returns `None` for them rather than inventing a row.

use super::{depuncture_regions, punctured_bits};

/// One UEP protection profile, keyed by its table-8 index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UepProfile {
    /// The FIC short form's six-bit table index (clause 6.2.1).
    pub table_index: u8,
    /// Sub-channel bit rate in kbit/s (table 8 / table 15).
    pub bitrate_kbps: u16,
    /// Protection level 1..=5, strongest first (table 15's `P`).
    pub level: u8,
    /// Sub-channel size in capacity units (table 8).
    pub size_cu: u16,
    /// 128-bit block counts of the four regions (table 15).
    pub l: [usize; 4],
    /// Puncturing indices 1..=24; `0` where the region is empty (table 15's "-").
    pub pi: [u8; 4],
    /// Zero padding bits appended after the punctured codeword (table 15).
    pub padding: u8,
}

impl UepProfile {
    /// Information bits per logical frame: 24 ms at the bit rate (table 14).
    pub fn info_bits(&self) -> usize {
        24 * self.bitrate_kbps as usize
    }

    /// Mother-code bits including the six tail bits.
    pub fn mother_bits(&self) -> usize {
        self.l.iter().sum::<usize>() * 128 + 24
    }

    /// The puncture regions in transmission order, empty regions included.
    pub fn regions(&self) -> [(usize, u8); 4] {
        [
            (self.l[0], self.pi[0]),
            (self.l[1], self.pi[1]),
            (self.l[2], self.pi[2]),
            (self.l[3], self.pi[3]),
        ]
    }

    /// Transmitted bits: the punctured codeword, tail included, before padding.
    pub fn punctured_bits(&self) -> usize {
        punctured_bits(&self.regions())
    }

    /// The capacity bits of the sub-channel's logical frame (table 8 size).
    pub fn cu_bits(&self) -> usize {
        self.size_cu as usize * 64
    }

    /// Depuncture one logical frame's soft bits to the mother code.
    pub fn depuncture(&self, soft: &[i8]) -> Vec<i8> {
        depuncture_regions(soft, &self.regions())
    }
}

pub use super::uep_table::UEP_PROFILES;

/// The profile a FIC short-form `table 8` index names. The table has no gaps:
/// the six "x" combinations of table 8 are simply not rows, so an index from a
/// FIC that is in range always resolves.
pub fn uep_profile(table_index: u8) -> Option<UepProfile> {
    UEP_PROFILES.get(table_index as usize).copied()
}

/// The profile for a bit rate and level, found by value rather than by index —
/// for callers that only have the two numbers.
pub fn uep_profile_for(bitrate_kbps: u16, level: u8) -> Option<UepProfile> {
    UEP_PROFILES
        .iter()
        .copied()
        .find(|p| p.bitrate_kbps == bitrate_kbps && p.level == level)
}

/// Depuncture one UEP logical frame's soft bits to the mother code.
pub fn depuncture_uep(soft: &[i8], profile: &UepProfile) -> Vec<i8> {
    profile.depuncture(soft)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clause 11.3.1's table 8 as printed: every index's size and bit rate, including the
    /// missing combinations.
    #[test]
    fn table_8_bit_rates_and_sizes_match_the_clause() {
        // The standard's table 8, as printed (index, size CU, level, kbit/s).
        let printed: [(u8, u16, u8, u16); 64] = [
            (0, 16, 5, 32),
            (1, 21, 4, 32),
            (2, 24, 3, 32),
            (3, 29, 2, 32),
            (4, 35, 1, 32),
            (5, 24, 5, 48),
            (6, 29, 4, 48),
            (7, 35, 3, 48),
            (8, 42, 2, 48),
            (9, 52, 1, 48),
            (10, 29, 5, 56),
            (11, 35, 4, 56),
            (12, 42, 3, 56),
            (13, 52, 2, 56),
            (14, 32, 5, 64),
            (15, 42, 4, 64),
            (16, 48, 3, 64),
            (17, 58, 2, 64),
            (18, 70, 1, 64),
            (19, 40, 5, 80),
            (20, 52, 4, 80),
            (21, 58, 3, 80),
            (22, 70, 2, 80),
            (23, 84, 1, 80),
            (24, 48, 5, 96),
            (25, 58, 4, 96),
            (26, 70, 3, 96),
            (27, 84, 2, 96),
            (28, 104, 1, 96),
            (29, 58, 5, 112),
            (30, 70, 4, 112),
            (31, 84, 3, 112),
            (32, 104, 2, 112),
            (33, 64, 5, 128),
            (34, 84, 4, 128),
            (35, 96, 3, 128),
            (36, 116, 2, 128),
            (37, 140, 1, 128),
            (38, 80, 5, 160),
            (39, 104, 4, 160),
            (40, 116, 3, 160),
            (41, 140, 2, 160),
            (42, 168, 1, 160),
            (43, 96, 5, 192),
            (44, 116, 4, 192),
            (45, 140, 3, 192),
            (46, 168, 2, 192),
            (47, 208, 1, 192),
            (48, 116, 5, 224),
            (49, 140, 4, 224),
            (50, 168, 3, 224),
            (51, 208, 2, 224),
            (52, 232, 1, 224),
            (53, 128, 5, 256),
            (54, 168, 4, 256),
            (55, 192, 3, 256),
            (56, 232, 2, 256),
            (57, 280, 1, 256),
            (58, 160, 5, 320),
            (59, 208, 4, 320),
            (60, 280, 2, 320),
            (61, 192, 5, 384),
            (62, 280, 3, 384),
            (63, 416, 1, 384),
        ];
        assert_eq!(printed.len(), 64);
        for (i, (index, size, level, bitrate)) in printed.iter().enumerate() {
            let p = UEP_PROFILES[i];
            assert_eq!(p.table_index, *index, "row {i}");
            assert_eq!(p.size_cu, *size, "index {index} size");
            assert_eq!(p.level, *level, "index {index} level");
            assert_eq!(p.bitrate_kbps, *bitrate, "index {index} bit rate");
            assert_eq!(uep_profile(*index), Some(p));
            assert_eq!(uep_profile_for(*bitrate, *level), Some(p));
        }
    }

    /// Table 15's row for the profile the MSC fixture uses: 128 kbit/s,
    /// level 3 is table index 35, `L = [11, 22, 60, 3]`, `PI = [16, 9, 6, 10]`,
    /// 4 padding bits, 96 CUs.
    #[test]
    fn table_15_128_kbps_level_3_matches_the_printed_row() {
        let p = uep_profile(35).expect("index 35");
        assert_eq!(p.bitrate_kbps, 128);
        assert_eq!(p.level, 3);
        assert_eq!(p.size_cu, 96);
        assert_eq!(p.l, [11, 22, 60, 3]);
        assert_eq!(p.pi, [16, 9, 6, 10]);
        assert_eq!(p.padding, 4);
        assert_eq!(p.info_bits(), 3072);
        assert_eq!(p.mother_bits(), 12_312);
        assert_eq!(p.punctured_bits(), 6140);
        assert_eq!(p.punctured_bits() + p.padding as usize, 96 * 64);
    }

    /// The invariants that make one merged table safe: the regions cover
    /// `4I` mother bits (table 14), and the punctured codeword plus its
    /// padding fills the table-8 size exactly.
    #[test]
    fn every_profile_fills_its_size() {
        for p in UEP_PROFILES {
            assert_eq!(
                p.l.iter().sum::<usize>() * 128,
                4 * p.info_bits(),
                "index {} covers the wrong mother-code length",
                p.table_index
            );
            assert_eq!(
                p.punctured_bits() + p.padding as usize,
                p.cu_bits(),
                "index {} does not fill its {} CUs",
                p.table_index,
                p.size_cu
            );
            assert!(
                matches!(p.padding, 0 | 4 | 8),
                "index {} has padding {}",
                p.table_index,
                p.padding
            );
            for (i, &blocks) in p.l.iter().enumerate() {
                assert_eq!(
                    blocks == 0,
                    p.pi[i] == 0,
                    "index {} region {i}: blocks and PI disagree",
                    p.table_index
                );
                if blocks > 0 {
                    assert!((1..=24).contains(&p.pi[i]));
                }
            }
        }
    }

    /// The six combinations the standard marks "x" are absent, and looking
    /// them up by value finds nothing rather than a neighbour.
    #[test]
    fn the_six_missing_combinations_are_absent() {
        for (bitrate, level) in [(56, 1), (112, 1), (320, 1), (320, 3), (384, 2), (384, 4)] {
            assert_eq!(uep_profile_for(bitrate, level), None, "{bitrate}/{level}");
        }
        assert_eq!(uep_profile(64), None, "index 64 does not exist");
    }

    /// Depuncture consumes exactly the punctured bits (padding ignored) and
    /// produces the mother codeword length.
    #[test]
    fn depuncture_consumes_the_punctured_codeword() {
        let p = uep_profile(35).expect("index 35");
        let mut input = vec![42i8; p.cu_bits()];
        // The last four bits are padding and must not be read.
        for bit in &mut input[p.punctured_bits()..] {
            *bit = -7;
        }
        let out = p.depuncture(&input);
        assert_eq!(out.len(), p.mother_bits());
        assert_eq!(out.iter().filter(|b| **b == 42).count(), p.punctured_bits());
        assert!(out.iter().all(|b| *b != -7));
    }
}
