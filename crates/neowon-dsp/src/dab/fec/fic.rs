//! The FIC's use of the convolutional code: 768 information bits → 3096
//! mother bits → 2304 transmitted bits, four codewords per frame
//! (clause 11.2.1, mode I).
//!
//! The block arithmetic is the standard's: 24 blocks of 128 mother bits, the
//! first 21 punctured with `PI = 16`, the remaining three with `PI = 15`, then
//! the last 24 bits with `V_T` — 21·96 + 3·92 + 12 = 2304.

use super::MOTHER_CODE_RATE;
use crate::dab::tables::{P_CODES, PI_TAIL};

/// Information bits per FIC codeword: 768 data + 6 tail (clause 11.2.1).
pub const FIC_INFO_BITS: usize = 774;
/// Mother codeword length for the FIC: `774 * 4` (clause 11.2.1).
pub const FIC_MOTHER_BITS: usize = FIC_INFO_BITS * MOTHER_CODE_RATE;
/// Transmitted bits per FIC codeword after puncturing (clause 11.2.1).
pub const FIC_TRANSMITTED_BITS: usize = 2304;
/// Data bits in one FIC codeword (three FIBs), excluding the tail.
pub const FIC_DATA_BITS: usize = 768;

/// Puncture a FIC mother codeword: 3096 bits in, 2304 bits out (clause 11.2.1).
///
/// 21 blocks with PI = 16, 3 blocks with PI = 15, then the tail with `V_T`.
pub fn puncture_fic(mother: &[u8]) -> Vec<u8> {
    assert_eq!(mother.len(), FIC_MOTHER_BITS, "mother codeword length");
    let pi16 = &P_CODES[15];
    let pi15 = &P_CODES[14];
    let mut out = Vec::with_capacity(FIC_TRANSMITTED_BITS);
    for block in 0..21 {
        super::puncture_block(&mother[block * 128..block * 128 + 128], pi16, &mut out);
    }
    for block in 21..24 {
        super::puncture_block(&mother[block * 128..block * 128 + 128], pi15, &mut out);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::fec::{conv_encode, viterbi_decode};

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

    /// The clause-11.2.1 block arithmetic: 21·96 + 3·92 + 12 = 2304, and the
    /// punctured count of each block matches the vector it uses.
    #[test]
    fn fic_block_arithmetic_matches_the_clause() {
        let ones = |pi: u8| {
            P_CODES[pi as usize - 1]
                .iter()
                .map(|b| *b as usize)
                .sum::<usize>()
        };
        assert_eq!(ones(16), 24);
        assert_eq!(ones(15), 23);
        assert_eq!(
            21 * 4 * ones(16) + 3 * 4 * ones(15) + 12,
            FIC_TRANSMITTED_BITS
        );
    }
}
