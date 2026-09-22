//! EEP (Equal Error Protection) parameters for the MSC: what clause 11.3.2
//! does with the convolutional code for a sub-channel's bit rate and level.
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **11.3.2** two protection sets are defined. Set A is for bit rates in
//!   multiples of 8 kbit/s, with code rates 1/4, 3/8, 1/2, 3/4 for levels 1–4;
//!   set B is for multiples of 32 kbit/s, with rates 4/9, 4/7, 2/3, 4/5.
//!   The first `L1` 128-bit blocks are punctured with `PI1`, the remaining
//!   `L2` with `PI2`, and the last 24 mother bits with `V_T`; there is **no
//!   padding**.
//! - **Table 18** (set A) and **table 20** (set B) give `(L1, L2, PI1, PI2)`
//!   as functions of the bit rate and level. [`eep_profile`] implements those
//!   formulas directly; [`EepProfile::for_size`] inverts them for a receiver
//!   that was told the sub-channel size instead.
//! - **Table 9/10** are the same relationship seen from the size side: the
//!   punctured codeword fills the signalled number of 64-bit CUs exactly, so a
//!   profile is valid only when [`EepProfile::punctured_bits`] equals the size.
//!
//! The reference implementation (`dabradio` 0.5.0, MIT) uses the same formulas;
//! our tests hold them to the standard's printed values, and the one place the
//! reference differs — the 8 kbit/s 2-A row, which its formula rejects — is
//! handled explicitly here and noted in `docs/protocol-dab.md`.

use super::depuncture_regions;

/// An EEP protection profile: the puncturing plan for one logical frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EepProfile {
    /// Sub-channel bit rate in kbit/s (a multiple of 8 for set A, 32 for set B).
    pub bitrate_kbps: u16,
    /// 0 = option A / set A, 1 = option B / set B.
    pub option: u8,
    /// Raw signalled level, 0..=3 (displayed as 1..=4).
    pub level: u8,
    /// 128-bit blocks punctured with `PI1`.
    pub l1: usize,
    /// 128-bit blocks punctured with `PI2`.
    pub l2: usize,
    /// Puncturing index 1..=24 (clause 11.1.2 / table 13).
    pub pi1: u8,
    pub pi2: u8,
}

impl EepProfile {
    /// Information bits per logical frame: 24 ms at the bit rate (clause 5.3).
    pub fn info_bits(&self) -> usize {
        24 * self.bitrate_kbps as usize
    }

    /// The two puncture regions in transmission order.
    pub fn regions(&self) -> [(usize, u8); 2] {
        [(self.l1, self.pi1), (self.l2, self.pi2)]
    }

    /// Mother-code bits including the six tail bits.
    pub fn mother_bits(&self) -> usize {
        (self.l1 + self.l2) * 128 + 24
    }

    /// Transmitted bits: the punctured codeword plus the 12 tail bits kept.
    pub fn punctured_bits(&self) -> usize {
        super::punctured_bits(&self.regions())
    }

    /// Capacity units the punctured codeword occupies (EEP has no padding, so
    /// this is exact).
    pub fn size_cu(&self) -> usize {
        self.punctured_bits() / 64
    }

    /// The profile for a sub-channel signalled by its size instead of its bit
    /// rate: clauses 11.3.2's tables 9/10 make the size a multiple of `n` that
    /// depends on the level, and the profile is accepted only when the
    /// puncturing fills that size exactly.
    pub fn for_size(size_cu: u16, level: u8, option: u8) -> Option<Self> {
        let size = size_cu as usize;
        let (bitrate, n_ok) = match (option, level) {
            // Set A: size 12n (1-A), 8n (2-A), 6n (3-A), 4n (4-A).
            (0, 0) => (size / 12 * 8, size.is_multiple_of(12)),
            (0, 1) => (size / 8 * 8, size.is_multiple_of(8)),
            (0, 2) => (size / 6 * 8, size.is_multiple_of(6)),
            (0, 3) => (size / 4 * 8, size.is_multiple_of(4)),
            // Set B: size 27n, 21n, 18n, 15n at 32 kbit/s per n.
            (1, 0) => (size / 27 * 32, size.is_multiple_of(27)),
            (1, 1) => (size / 21 * 32, size.is_multiple_of(21)),
            (1, 2) => (size / 18 * 32, size.is_multiple_of(18)),
            (1, 3) => (size / 15 * 32, size.is_multiple_of(15)),
            _ => return None,
        };
        if !n_ok || bitrate == 0 || bitrate > u16::MAX as usize {
            return None;
        }
        let profile = eep_profile(bitrate as u16, level, option)?;
        (profile.size_cu() == size).then_some(profile)
    }

    /// Depuncture one logical frame's soft bits to the mother code.
    pub fn depuncture(&self, soft: &[i8]) -> Vec<i8> {
        depuncture_regions(soft, &self.regions())
    }
}

/// The EEP profile for a bit rate, level (raw 0..=3) and option (0 = A,
/// 1 = B), from tables 18 and 20 (clause 11.3.2). `None` for a combination the
/// standard does not define.
pub fn eep_profile(bitrate_kbps: u16, level: u8, option: u8) -> Option<EepProfile> {
    if level > 3 {
        return None;
    }
    // Ported from dabradio 0.5.0 (MIT); notice in docs/protocol-dab.md
    let (l1, l2, pi1, pi2) = match option {
        0 => {
            // Set A, clauses 11.3.2 tables 9/18: n = bit rate / 8.
            if bitrate_kbps < 8 || !bitrate_kbps.is_multiple_of(8) {
                return None;
            }
            let n = (bitrate_kbps / 8) as i32;
            match level {
                0 => (6 * n - 3, 3, 24, 23),
                // Table 18 gives 8 kbit/s 2-A its own row (L1 = 5, L2 = 1).
                1 if n == 1 => (5, 1, 13, 12),
                1 => (2 * n - 3, 4 * n + 3, 14, 13),
                2 => (6 * n - 3, 3, 8, 7),
                _ => (4 * n - 3, 2 * n + 3, 3, 2),
            }
        }
        1 => {
            // Set B, clauses 11.3.2 tables 10/20: n = bit rate / 32.
            if bitrate_kbps < 32 || !bitrate_kbps.is_multiple_of(32) {
                return None;
            }
            let n = (bitrate_kbps / 32) as i32;
            let pi = match level {
                0 => (10, 9),
                1 => (6, 5),
                2 => (4, 3),
                _ => (2, 1),
            };
            (24 * n - 3, 3, pi.0, pi.1)
        }
        _ => return None,
    };
    if l1 < 0 || l2 < 0 {
        return None;
    }
    Some(EepProfile {
        bitrate_kbps,
        option,
        level,
        l1: l1 as usize,
        l2: l2 as usize,
        pi1,
        pi2,
    })
}

/// Depuncture one EEP logical frame's soft bits to the mother code.
pub fn depuncture_eep(soft: &[i8], profile: &EepProfile) -> Vec<i8> {
    profile.depuncture(soft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::tables::P_CODES;

    fn ones(pi: u8) -> usize {
        P_CODES[pi as usize - 1].iter().map(|b| *b as usize).sum()
    }

    /// Table 18, 88 kbit/s 3-A (the profile of the hardware ensemble's
    /// services): n = 11, L1 = 63, L2 = 3, PI 8/7, and the punctured codeword is
    /// exactly 66 CUs — the size the FIC signals.
    #[test]
    fn eep_3a_88_kbps_matches_the_schedule() {
        let p = eep_profile(88, 2, 0).expect("3-A 88 kbit/s");
        assert_eq!((p.l1, p.l2, p.pi1, p.pi2), (63, 3, 8, 7));
        assert_eq!(p.info_bits(), 2112);
        assert_eq!(p.punctured_bits(), 4224);
        assert_eq!(p.size_cu(), 66);
        assert_eq!(p.mother_bits(), 8472);
        // Independent arithmetic: 63·4·16 + 3·4·15 + 12.
        assert_eq!(63 * 4 * ones(8) + 3 * 4 * ones(7) + 12, 4224);
    }

    /// Table 18, 88 kbit/s 2-A: L1 = 2n-3 = 19, L2 = 4n+3 = 47, PI 14/13.
    #[test]
    fn eep_2a_88_kbps_matches_the_schedule() {
        let p = eep_profile(88, 1, 0).expect("2-A 88 kbit/s");
        assert_eq!((p.l1, p.l2, p.pi1, p.pi2), (19, 47, 14, 13));
        assert_eq!(p.punctured_bits(), 5632);
        assert_eq!(p.size_cu(), 88);
    }

    /// Table 20, 32 kbit/s 1-B: n = 1, L1 = 21, L2 = 3, PI 10/9, 27 CUs.
    #[test]
    fn eep_1b_32_kbps_matches_the_schedule() {
        let p = eep_profile(32, 0, 1).expect("1-B 32 kbit/s");
        assert_eq!((p.l1, p.l2, p.pi1, p.pi2), (21, 3, 10, 9));
        assert_eq!(p.punctured_bits(), 1728);
        assert_eq!(p.size_cu(), 27);
    }

    /// The 8 kbit/s 2-A row is the standard's own special case (table 18);
    /// the general `2n-3` formula would reject it, so it is pinned here.
    #[test]
    fn eep_2a_8_kbps_is_the_special_row() {
        let p = eep_profile(8, 1, 0).expect("2-A 8 kbit/s");
        assert_eq!((p.l1, p.l2, p.pi1, p.pi2), (5, 1, 13, 12));
        assert_eq!(p.size_cu(), 8);
    }

    /// Every profile the formulas produce must fill its signalled size exactly
    /// (this is tables 9/10) and carry `24·bitrate` information bits.
    #[test]
    fn every_defined_profile_fills_its_size() {
        for option in 0..=1u8 {
            for level in 0..=3u8 {
                let step = if option == 0 { 8 } else { 32 };
                for bitrate in (step..=384).step_by(step as usize) {
                    let Some(p) = eep_profile(bitrate as u16, level, option) else {
                        continue;
                    };
                    assert_eq!(p.info_bits(), 24 * bitrate as usize);
                    assert_eq!(
                        p.mother_bits(),
                        4 * (p.info_bits() + 6),
                        "{bitrate}/{level}/{option}"
                    );
                    assert_eq!(
                        p.punctured_bits(),
                        p.size_cu() * 64,
                        "{bitrate}/{level}/{option}"
                    );
                    // Inverting from the size returns the same profile.
                    assert_eq!(
                        EepProfile::for_size(p.size_cu() as u16, level, option),
                        Some(p)
                    );
                }
            }
        }
    }

    /// Undefined combinations are refused, not guessed (D27).
    #[test]
    fn undefined_combinations_are_refused() {
        assert_eq!(eep_profile(88, 1, 2), None, "no option 2");
        assert_eq!(eep_profile(88, 4, 0), None, "no level 5 in EEP");
        assert_eq!(eep_profile(90, 1, 0), None, "90 is not a multiple of 8");
        assert_eq!(eep_profile(48, 0, 1), None, "48 is not a multiple of 32");
        // A size that does not match the level's multiple is refused too.
        assert_eq!(EepProfile::for_size(65, 2, 0), None);
    }

    /// Depuncture consumes exactly the punctured bits and produces the mother
    /// codeword length, with every transmitted bit kept.
    #[test]
    fn depuncture_consumes_the_punctured_codeword() {
        let p = eep_profile(88, 2, 0).expect("3-A");
        let input = vec![42i8; p.punctured_bits()];
        let out = p.depuncture(&input);
        assert_eq!(out.len(), p.mother_bits());
        assert_eq!(out.iter().filter(|b| **b != 0).count(), input.len());
    }
}
