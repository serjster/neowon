//! Forward error correction for the DAB FIC: the energy-dispersal PRBS, the
//! punctured convolutional code, and the soft-decision Viterbi decoder.
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **10.1** the PRBS `P(X) = X^9 + X^5 + 1`, initialized with all ones, whose
//!   first 16 bits the standard publishes in table 12 (our test vector).
//! - **10.2** energy dispersal in the FIC: the three FIBs of one CIF form a
//!   768-bit vector scrambled with the PRBS from index 0 — so the sequence
//!   restarts for every 768-bit group, which is why [`energy_dispersal`] takes
//!   one group at a time.
//! - **11.1.1** the mother code: constraint length 7, rate 1/4, generators
//!   133, 171, 145, 133 (octal) with six zero tail bits. [`conv_encode`]
//!   implements the clause's four equations directly.
//! - **11.1.2** puncturing and the tail vector `V_T` (see [`tables`]).
//! - **11.2.1** what the FIC actually does with it: a 768-bit vector is coded
//!   to 3096 bits, split into 24 blocks of 128, punctured with PI = 16 for the
//!   first 21 blocks, PI = 15 for the remaining 3, and the last 24 bits with
//!   `V_T` — 2304 transmitted bits.
//!
//! **Soft-bit convention.** A soft bit is an `i8` where positive means "likely
//! 1" and negative means "likely 0"; the decoder maximizes correlation, so a
//! sign flip is a systematic failure rather than a graceful degradation. That
//! convention is asserted by `soft_bit_polarity_is_detectable`, the test the
//! spec asks for (D26/row 6), because a polarity error is exactly how a DAB
//! decoder produces clean-looking data and zero valid FIBs.

use super::tables::{P_CODES, PI_TAIL};

/// Mother code rate: one information bit produces four code bits.
pub const MOTHER_CODE_RATE: usize = 4;
/// Information bits per FIC codeword: 768 data + 6 tail (clause 11.2.1).
pub const FIC_INFO_BITS: usize = 774;
/// Mother codeword length for the FIC: `774 * 4` (clause 11.2.1).
pub const FIC_MOTHER_BITS: usize = FIC_INFO_BITS * MOTHER_CODE_RATE;
/// Transmitted bits per FIC codeword after puncturing (clause 11.2.1).
pub const FIC_TRANSMITTED_BITS: usize = 2304;
/// Data bits in one FIC codeword (three FIBs), excluding the tail.
pub const FIC_DATA_BITS: usize = 768;
/// Constraint length 7, so 6 bits of history: 64 states.
const STATES: usize = 64;

// ---------------------------------------------------------------------------
// Energy dispersal (clause 10.1, 10.2)
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
/// This is its own inverse. Call it once per 768-bit group (clause 10.2 — the
/// sequence restarts at the first bit of each group, not once per frame).
pub fn energy_dispersal(bits: &mut [u8]) {
    let mut reg: u16 = 0x01FF;
    for bit in bits.iter_mut() {
        let prbs = ((reg >> 8) ^ (reg >> 4)) & 1;
        *bit ^= prbs as u8;
        reg = ((reg << 1) | prbs) & 0x01FF;
    }
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
fn mother_outputs(bit: u8, history: [u8; 6]) -> [u8; 4] {
    let [b0, b1, b2, b3, b4, b5] = history;
    let x0 = bit ^ b1 ^ b2 ^ b4 ^ b5;
    let x1 = bit ^ b0 ^ b1 ^ b2 ^ b5;
    let x2 = bit ^ b0 ^ b3 ^ b5;
    [x0, x1, x2, x0]
}

/// Encode `info` with the mother code, appending the six zero tail bits
/// (clause 11.1.1: the codeword runs to `i = I + 5`).
///
/// Used by the golden-test encoder and by later tiers; the decoder shares
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
// Puncturing (clause 11.1.2, 11.2.1)
// ---------------------------------------------------------------------------

/// Apply one puncturing vector to a 128-bit block: four 32-bit sub-blocks, each
/// punctured by the same vector (clause 11.1.2).
fn puncture_block(block: &[u8], vector: &[u8; 32], out: &mut Vec<u8>) {
    for sub in 0..4 {
        let base = sub * 32;
        for (i, keep) in vector.iter().enumerate() {
            if *keep == 1 {
                out.push(block[base + i]);
            }
        }
    }
}

/// Puncture a FIC mother codeword: 3096 bits in, 2304 bits out (clause 11.2.1).
///
/// 21 blocks with PI = 16, 3 blocks with PI = 15, then the tail with `V_T`.
pub fn puncture_fic(mother: &[u8]) -> Vec<u8> {
    assert_eq!(mother.len(), FIC_MOTHER_BITS, "mother codeword length");
    let pi16 = &P_CODES[15];
    let pi15 = &P_CODES[14];
    let mut out = Vec::with_capacity(FIC_TRANSMITTED_BITS);
    for block in 0..21 {
        puncture_block(&mother[block * 128..block * 128 + 128], pi16, &mut out);
    }
    for block in 21..24 {
        puncture_block(&mother[block * 128..block * 128 + 128], pi15, &mut out);
    }
    let tail = &mother[24 * 128..];
    assert_eq!(tail.len(), 24);
    for (i, keep) in PI_TAIL.iter().enumerate() {
        if *keep == 1 {
            out.push(tail[i]);
        }
    }
    assert_eq!(out.len(), FIC_TRANSMITTED_BITS);
    out
}

/// Inverse of [`puncture_fic`] for soft bits: 2304 in, 3096 out, with
/// punctured positions marked as erasures (`0`, which the correlating decoder
/// treats as no evidence).
pub fn depuncture_fic(soft: &[i8]) -> Vec<i8> {
    assert_eq!(soft.len(), FIC_TRANSMITTED_BITS, "soft codeword length");
    let pi16 = &P_CODES[15];
    let pi15 = &P_CODES[14];
    let mut out = Vec::with_capacity(FIC_MOTHER_BITS);
    let mut read = 0;
    let block = |vector: &[u8; 32], out: &mut Vec<i8>, read: &mut usize| {
        for _ in 0..4 {
            for keep in vector.iter() {
                if *keep == 1 {
                    out.push(soft[*read]);
                    *read += 1;
                } else {
                    out.push(0);
                }
            }
        }
    };
    for _ in 0..21 {
        block(pi16, &mut out, &mut read);
    }
    for _ in 0..3 {
        block(pi15, &mut out, &mut read);
    }
    for keep in PI_TAIL.iter() {
        if *keep == 1 {
            out.push(soft[read]);
            read += 1;
        } else {
            out.push(0);
        }
    }
    assert_eq!(read, soft.len());
    assert_eq!(out.len(), FIC_MOTHER_BITS);
    out
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

/// Decode a punctured FIC codeword of soft bits, returning the information
/// bits and the winning path metric (higher is better).
///
/// The code is tail-terminated (six zero bits, clause 11.1.1), so the survivor
/// is read back from state 0. Punctured positions arrive as `0` from
/// [`depuncture_fic`] and contribute nothing, which is the whole point of
/// soft-decision decoding.
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

    /// Puncture then depuncture then Viterbi must return the input bits. This
    /// is the FEC chain end to end, on a deterministic pattern.
    #[test]
    fn fec_round_trip_recovers_the_information() {
        for seed in 0..4u32 {
            let info: Vec<u8> = (0..FIC_DATA_BITS)
                .map(|i| {
                    let mixed = (i as u32)
                        .wrapping_mul(2_654_435_761)
                        .wrapping_add(seed.wrapping_mul(40_503));
                    ((mixed >> 13) & 1) as u8
                })
                .collect();
            let coded = conv_encode(&info);
            let punctured = puncture_fic(&coded);
            assert_eq!(punctured.len(), FIC_TRANSMITTED_BITS);
            let soft: Vec<i8> = punctured
                .iter()
                .map(|b| if *b == 1 { 100 } else { -100 })
                .collect();
            let depunctured = depuncture_fic(&soft);
            let (decoded, metric) = viterbi_decode(&depunctured);
            assert_eq!(decoded, info, "seed {seed}");
            assert!(metric > 0, "metric {metric} should be positive");
        }
    }

    /// The soft-bit polarity trap, as an explicit test: inverting the sign
    /// convention (which is how a decoder "sees clean data" and produces
    /// nothing) must fail, loudly and on this test.
    #[test]
    fn soft_bit_polarity_is_detectable() {
        let info: Vec<u8> = (0..FIC_DATA_BITS).map(|i| (i % 7 == 0) as u8).collect();
        let punctured = puncture_fic(&conv_encode(&info));
        let inverted: Vec<i8> = punctured
            .iter()
            .map(|b| if *b == 1 { -100 } else { 100 })
            .collect();
        let (decoded, _) = viterbi_decode(&depuncture_fic(&inverted));
        assert_ne!(decoded, info, "an inverted soft-bit sign must not decode");
        let flipped: u32 = decoded
            .iter()
            .zip(&info)
            .map(|(a, b)| (a != b) as u32)
            .sum();
        assert!(flipped > 300, "expected garbage, got {flipped} bit errors");
    }

    /// Erasures carry no evidence: a punctured position must not bias the
    /// decision. Decoding an all-erasure codeword is possible *only* because
    /// every path is equally likely, so the survivor is the all-zero codeword's
    /// information vector (the tail-terminated zero path).
    #[test]
    fn erasures_carry_no_evidence() {
        let soft = vec![0i8; FIC_MOTHER_BITS];
        let (decoded, metric) = viterbi_decode(&soft);
        assert!(decoded.iter().all(|b| *b == 0));
        assert_eq!(metric, 0);
    }
}
