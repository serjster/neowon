//! Forward error correction for DAB: the energy-dispersal PRBS, the punctured
//! convolutional code, and the soft-decision Viterbi decoder — shared by the
//! FIC ([`fic`]) and the MSC ([`eep`], [`uep`]).
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **10.1** the PRBS `P(X) = X^9 + X^5 + 1`, initialized with all ones, whose
//!   first 16 bits the standard publishes in table 12 (our test vector).
//! - **10.2** energy dispersal in the FIC: the three FIBs of one CIF form a
//!   768-bit vector scrambled with the PRBS from index 0 — so the sequence
//!   restarts for every 768-bit group, which is why [`energy_dispersal`] takes
//!   one group at a time.
//! - **10.3** energy dispersal in the MSC: the first bit of each *logical
//!   frame* of a sub-channel is added to PRBS bit 0, so the same function is
//!   called once per logical frame with the frame's bits — not once per CIF
//!   group as in the FIC.
//! - **11.1.1** the mother code: constraint length 7, rate 1/4, generators
//!   133, 171, 145, 133 (octal) with six zero tail bits. [`conv_encode`]
//!   implements the clause's four equations directly.
//! - **11.1.2** puncturing: a 128-bit block is four 32-bit sub-blocks, each
//!   punctured with the same 32-entry vector (see [`super::tables`]); the last
//!   24 bits of the serial mother codeword use `V_T` `1100…`.
//! - **11.2.1** what the FIC does with it ([`fic`]).
//! - **11.3.1/11.3.2** what the MSC does with it ([`eep`], [`uep`]).
//! - **annex E** the CRC word used by FIBs, DLS data groups and the MSC's
//!   oracle payload check ([`crc16`]).
//!
//! **Soft-bit convention.** A soft bit is an `i8` where positive means "likely
//! 1" and negative means "likely 0"; the decoder maximizes correlation, so a
//! sign flip is a systematic failure rather than a graceful degradation. That
//! convention is asserted by `soft_bit_polarity_is_detectable` (FIC) and
//! `msc_polarity_is_detectable` (MSC), because a polarity error is exactly how
//! a DAB decoder produces clean-looking data and zero valid FIBs.
//!
//! Portions of this module follow the MIT-licensed reference `dabradio` 0.5.0
//! (`xoolive/desperado`) for algorithm structure; the notice is recorded in
//! `docs/protocol-dab.md`.

pub mod eep;
pub mod fic;
pub mod uep;
mod uep_table;

pub use eep::{EepProfile, depuncture_eep, eep_profile};
pub use fic::{
    FIC_DATA_BITS, FIC_INFO_BITS, FIC_MOTHER_BITS, FIC_TRANSMITTED_BITS, depuncture_fic,
    puncture_fic,
};
pub use uep::{UEP_PROFILES, UepProfile, depuncture_uep, uep_profile, uep_profile_for};

use super::tables::{P_CODES, PI_TAIL};

/// Mother code rate: one information bit produces four code bits.
pub const MOTHER_CODE_RATE: usize = 4;
/// Constraint length 7, so 6 bits of history: 64 states.
const STATES: usize = 64;

// ---------------------------------------------------------------------------
// Energy dispersal (clause 10.1, 10.2, 10.3)
// ---------------------------------------------------------------------------

/// The energy-dispersal PRBS, `P(X) = X^9 + X^5 + 1`, initialized to all ones
/// (clause 10.1, figure 54).
///
/// The first 16 bits are `[0, 0, 0, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0]`
/// per table 12, which `tables::PRBS_FIRST_16` pins.
pub fn prbs_bits(n: usize) -> Vec<u8> {
    let mut reg: u16 = 0x01FF;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        // Output taps 9 and 5 (bit 8 and bit 4 of the 9-stage register).
        let bit = ((reg >> 8) ^ (reg >> 4)) & 1;
        out.push(bit as u8);
        reg = ((reg << 1) | bit) & 0x01FF;
    }
    out
}

/// Scramble `bits` in place with the energy-dispersal PRBS starting at index 0.
///
/// This is its own inverse. Call it once per *vector*: in the FIC that is a
/// 768-bit group (clause 10.2), in the MSC one sub-channel's logical frame
/// (clause 10.3).
pub fn energy_dispersal(bits: &mut [u8]) {
    let mut reg: u16 = 0x01FF;
    for bit in bits.iter_mut() {
        let prbs = ((reg >> 8) ^ (reg >> 4)) & 1;
        *bit ^= prbs as u8;
        reg = ((reg << 1) | prbs) & 0x01FF;
    }
}

// ---------------------------------------------------------------------------
// CRC (annex E)
// ---------------------------------------------------------------------------

/// The CRC-16 of **annex E**: `G(X) = X^16 + X^12 + X^5 + 1`, shift register
/// initialized to all ones, word complemented before transmission.
///
/// This is the same check the FIB carries (clause 5.2.1), and the same one the
/// DLS data group carries (clause 7.4.5.0). It is *not* an on-air MSC CRC —
/// EN 300 401 defines none at this layer (DAB+ has its own, TS 102 563)
/// — so the MSC decoder applies it only when the sim's oracle transport says
/// the payload carries one.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc ^ 0xFFFF
}

// ---------------------------------------------------------------------------
// Mother code (clause 11.1.1)
// ---------------------------------------------------------------------------

/// The four parity outputs for one input bit and the six previous bits.
///
/// `history[0]` is `a_{i-1}` … `history[5]` is `a_{i-6}`, and the four outputs
/// are `(x0, x1, x2, x3)` in the clause's order:
///
/// ```text
/// x0 = a_i ^ a_{i-2} ^ a_{i-3} ^ a_{i-5} ^ a_{i-6}
/// x1 = a_i ^ a_{i-1} ^ a_{i-2} ^ a_{i-3} ^ a_{i-6}
/// x2 = a_i ^ a_{i-1} ^ a_{i-4} ^ a_{i-6}
/// x3 = x0
/// ```
pub(crate) fn mother_outputs(bit: u8, history: [u8; 6]) -> [u8; 4] {
    let [b0, b1, b2, b3, b4, b5] = history;
    let x0 = bit ^ b1 ^ b2 ^ b4 ^ b5;
    let x1 = bit ^ b0 ^ b1 ^ b2 ^ b5;
    let x2 = bit ^ b0 ^ b3 ^ b5;
    [x0, x1, x2, x0]
}

/// Encode `info` with the mother code, appending the six zero tail bits
/// (clause 11.1.1: the codeword runs to `i = I + 5`).
///
/// Used by the golden-test encoder; the decoder shares
/// [`mother_outputs`] with it, so the two cannot disagree about the code.
pub fn conv_encode(info: &[u8]) -> Vec<u8> {
    let mut history = [0u8; 6];
    let mut out = Vec::with_capacity((info.len() + 6) * MOTHER_CODE_RATE);
    for bit in info.iter().copied().chain(std::iter::repeat_n(0u8, 6)) {
        out.extend_from_slice(&mother_outputs(bit, history));
        history.rotate_right(1);
        history[0] = bit;
    }
    out
}

// ---------------------------------------------------------------------------
// Puncturing (clause 11.1.2)
// ---------------------------------------------------------------------------

/// Apply one puncturing vector to a 128-bit block: four 32-bit sub-blocks, each
/// punctured by the same vector (clause 11.1.2).
pub(crate) fn puncture_block(block: &[u8], vector: &[u8; 32], out: &mut Vec<u8>) {
    for sub in 0..4 {
        let base = sub * 32;
        for (i, keep) in vector.iter().enumerate() {
            if *keep == 1 {
                out.push(block[base + i]);
            }
        }
    }
}

/// The number of bits a puncturing vector keeps in one 32-bit sub-block
/// (`PI n` keeps `8 + n`; clause 11.1.2, table 13).
pub(crate) fn pi_ones(pi: u8) -> usize {
    assert!((1..=24).contains(&pi), "puncturing index {pi} out of range");
    8 + pi as usize
}

/// Depuncture soft bits into the mother code, following the clause-11.1.2
/// block structure: `regions` is a list of `(blocks, PI)` in transmission
/// order (the MSC's EEP has two regions, UEP four), and the 24-bit tail is
/// appended with `V_T`. Punctured positions become erasures (`0`).
///
/// `soft` is the punctured codeword (possibly followed by zero padding, which
/// this function never reads).
pub fn depuncture_regions(soft: &[i8], regions: &[(usize, u8)]) -> Vec<i8> {
    let mut out = Vec::with_capacity(
        regions
            .iter()
            .map(|(blocks, _)| blocks * 128)
            .sum::<usize>()
            + 24,
    );
    let mut read = 0usize;
    for &(blocks, pi) in regions {
        if blocks == 0 {
            continue;
        }
        let vector = &P_CODES[pi as usize - 1];
        for _ in 0..blocks {
            // One 128-bit block is four 32-bit sub-blocks, one vector each
            // (clause 11.1.2).
            for _ in 0..4 {
                for keep in vector.iter() {
                    if *keep == 1 {
                        out.push(soft.get(read).copied().unwrap_or(0));
                        read += 1;
                    } else {
                        out.push(0);
                    }
                }
            }
        }
    }
    for keep in PI_TAIL.iter() {
        if *keep == 1 {
            out.push(soft.get(read).copied().unwrap_or(0));
            read += 1;
        } else {
            out.push(0);
        }
    }
    out
}

/// The transmitted (punctured) bit count of a set of regions, tail included.
/// Empty regions (`blocks == 0`, as UEP's fourth region often is) contribute
/// nothing.
pub fn punctured_bits(regions: &[(usize, u8)]) -> usize {
    regions
        .iter()
        .filter(|(blocks, _)| *blocks > 0)
        .map(|(blocks, pi)| blocks * 4 * pi_ones(*pi))
        .sum::<usize>()
        + 12
}

/// Puncture a mother codeword with a region list — the transmitter side of
/// [`depuncture_regions`], used by the oracle encoder.
pub fn puncture_regions(mother: &[u8], regions: &[(usize, u8)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(punctured_bits(regions));
    let mut read = 0usize;
    for &(blocks, pi) in regions {
        if blocks == 0 {
            continue;
        }
        let vector = &P_CODES[pi as usize - 1];
        for _ in 0..blocks {
            puncture_block(&mother[read..read + 128], vector, &mut out);
            read += 128;
        }
    }
    let tail = &mother[read..read + 24];
    for (i, keep) in PI_TAIL.iter().enumerate() {
        if *keep == 1 {
            out.push(tail[i]);
        }
    }
    out
}

/// The mother-code bit count of a set of regions, tail included.
pub fn mother_bits(regions: &[(usize, u8)]) -> usize {
    regions
        .iter()
        .map(|(blocks, _)| blocks * 128)
        .sum::<usize>()
        + 24
}

// ---------------------------------------------------------------------------
// Viterbi (clause 11.1.1)
// ---------------------------------------------------------------------------

/// Branch outputs for every (input bit, state) pair, built once from
/// [`mother_outputs`] so the decoder cannot drift from the clause.
fn output_table() -> &'static [[[u8; 4]; STATES]; 2] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[[[u8; 4]; STATES]; 2]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [[[0u8; 4]; STATES]; 2];
        for (bit, entry) in table.iter_mut().enumerate() {
            for (state, slot) in entry.iter_mut().enumerate() {
                let history = [
                    (state & 1) as u8,
                    ((state >> 1) & 1) as u8,
                    ((state >> 2) & 1) as u8,
                    ((state >> 3) & 1) as u8,
                    ((state >> 4) & 1) as u8,
                    ((state >> 5) & 1) as u8,
                ];
                *slot = mother_outputs(bit as u8, history);
            }
        }
        table
    })
}

/// Decode a punctured codeword of soft bits, returning the information bits and
/// the winning path metric (higher is better).
///
/// The code is tail-terminated (six zero bits, clause 11.1.1) for the FIC and
/// for both MSC protections (clause 11.3: each vector is processed "as defined
/// in clause 11.1.1"), so the survivor is read back from state 0. Punctured
/// positions arrive as `0` from the depuncturers and contribute nothing, which
/// is the whole point of soft-decision decoding.
pub fn viterbi_decode(soft: &[i8]) -> (Vec<u8>, i64) {
    assert_eq!(soft.len() % MOTHER_CODE_RATE, 0, "soft bits per step");
    let steps = soft.len() / MOTHER_CODE_RATE;
    let outputs = output_table();
    let mut metrics = vec![i64::MIN; STATES];
    metrics[0] = 0;
    let mut next = vec![i64::MIN; STATES];
    // `trace[step][state]` records the **predecessor state**, not the input
    // bit: the transition `next = (state << 1) | bit` drops the oldest bit, so
    // a state has two possible predecessors that no input bit can tell apart.
    let mut trace = vec![0u8; steps * STATES];

    for step in 0..steps {
        let s = &soft[step * 4..step * 4 + 4];
        next.iter_mut().for_each(|m| *m = i64::MIN);
        for state in 0..STATES {
            let base = metrics[state];
            if base == i64::MIN {
                continue;
            }
            for bit in 0..2u8 {
                let out = &outputs[bit as usize][state];
                let mut score = base;
                for i in 0..4 {
                    let soft = s[i] as i64;
                    score += if out[i] == 1 { soft } else { -soft };
                }
                let next_state = ((state << 1) | bit as usize) & (STATES - 1);
                if score > next[next_state] {
                    next[next_state] = score;
                    trace[step * STATES + next_state] = state as u8;
                }
            }
        }
        std::mem::swap(&mut metrics, &mut next);
    }

    // The tail bits drive the encoder back to state 0, so trace back from it.
    // A state's own low bit is the input bit that produced it.
    let mut bits = vec![0u8; steps];
    let mut state = 0usize;
    for step in (0..steps).rev() {
        bits[step] = (state & 1) as u8;
        state = trace[step * STATES + state] as usize;
    }
    let metric = metrics[0];
    // Drop the six tail bits: the information is the first `steps - 6` bits.
    bits.truncate(steps - 6);
    (bits, metric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::tables::PRBS_FIRST_16;

    /// The PRBS matches the standard's own published first 16 bits (table 12).
    #[test]
    fn prbs_matches_the_standards_table_12() {
        assert_eq!(prbs_bits(16), PRBS_FIRST_16);
    }

    #[test]
    fn energy_dispersal_is_its_own_inverse() {
        let original: Vec<u8> = (0..768).map(|i| (i % 3 == 0) as u8).collect();
        let mut bits = original.clone();
        energy_dispersal(&mut bits);
        assert_ne!(bits, original, "scrambling must change the data");
        energy_dispersal(&mut bits);
        assert_eq!(bits, original);
    }

    /// The clause-11.1.1 equations, checked against a hand-computed codeword.
    #[test]
    fn mother_code_matches_the_clause() {
        // info = 1,0,0,0,0,0,0 then the six tail zeros.
        let info = [1u8, 0, 0, 0, 0, 0, 0];
        let coded = conv_encode(&info);
        assert_eq!(coded.len(), (info.len() + 6) * 4);
        // i = 0: a_0 = 1, all history 0 -> x0 = x1 = x2 = x3 = 1.
        assert_eq!(&coded[0..4], &[1, 1, 1, 1]);
        // i = 1: a_1 = 0, history a_0 = 1.
        // x0 = 0 ^ a_{-1}(0) ^ a_{-2}(0) ^ a_{-4}(0) ^ a_{-5}(0) = 0
        // x1 = 0 ^ 1 = 1 ; x2 = 0 ^ 1 = 1 ; x3 = x0 = 0
        assert_eq!(&coded[4..8], &[0, 1, 1, 0]);
        // i = 2: a_2 = 0, history a_1 = 0, a_0 = 1.
        // x0 = 0 ^ a_0 = 1 ; x1 = 0 ^ 0 ^ 1 = 1 ; x2 = 0 ^ 0 = 0
        assert_eq!(&coded[8..12], &[1, 1, 0, 1]);
    }

    /// Erasures carry no evidence: a punctured position must not bias the
    /// decision. Decoding an all-erasure codeword is possible *only* because
    /// every path is equally likely, so the survivor is the all-zero codeword's
    /// information vector (the tail-terminated zero path).
    #[test]
    fn erasures_carry_no_evidence() {
        let soft = vec![0i8; 3096];
        let (decoded, metric) = viterbi_decode(&soft);
        assert!(decoded.iter().all(|b| *b == 0));
        assert_eq!(metric, 0);
    }

    /// Table 13's 24 puncturing vectors each keep exactly `8 + PI` of their 32
    /// bits (clause 11.1.2), which is what makes the MSC sizes add up.
    #[test]
    fn puncture_vectors_keep_eight_plus_pi_bits() {
        for pi in 1..=24u8 {
            assert_eq!(pi_ones(pi), 8 + pi as usize, "PI {pi}");
            let kept = P_CODES[pi as usize - 1].iter().filter(|b| **b == 1).count();
            assert_eq!(kept, 8 + pi as usize, "PI {pi} vector");
        }
    }

    /// Clause 10.3: the MSC PRBS restarts at index 0 for each logical frame,
    /// so two identical frames scramble identically — a continuous PRBS (the
    /// FIC's per-768-bit grouping applied across frame boundaries) would not.
    #[test]
    fn msc_energy_dispersal_restarts_per_logical_frame() {
        let mut first = vec![1u8; 24];
        let mut second = first.clone();
        energy_dispersal(&mut first);
        energy_dispersal(&mut second);
        assert_eq!(first, second);

        let prbs = prbs_bits(48);
        let continuous: Vec<u8> = (0..48).map(|i| 1u8 ^ prbs[i]).collect();
        let mut grouped = vec![1u8; 48];
        energy_dispersal(&mut grouped[..24]);
        energy_dispersal(&mut grouped[24..]);
        assert_ne!(continuous, grouped);
    }

    /// The annex-E CRC parameters: all-ones init, complemented output, MSb-first
    /// (the FIB path already proves the parameters on air). The known answer for
    /// `123456789` is the complement of CRC-16/CCITT-FALSE, and a single-bit
    /// flip must fail.
    #[test]
    fn crc16_matches_the_published_check_value() {
        assert_eq!(crc16(b"123456789"), 0xD64E);
        for bit in 0..8 {
            assert_ne!(crc16(&[b'A' ^ (1 << bit)]), crc16(b"A"));
        }
    }
}
