//! Verification row 13: the DAB+ superframe, RS(120,110) and CRC layer
//! against ETSI TS 102 563 V2.1.1.
//!
//! The standard publishes **no numeric check values** for the header
//! Fire code or the AU CRC, so those tests assert the defining algebraic
//! properties (the polynomial expansion, linearity, burst detection) and
//! the published CRC-16 check value; the RS tests assert the clause 6.1
//! generator-root property and the correction capability. This is
//! stated in the report rather than dressed up as a published vector.

use neowon_codec::Error;
use neowon_codec::dabplus::{
    FIRE_CODE_POLY, RS_K, RS_N, RS_PARITY, Rs120_110, RsError, SuperframeDecoder,
    SuperframeEncoder, SuperframeHeader, crc16, extract_pad, fire_code,
};

// ---------------------------------------------------------------------
// Check words (EN 300 401 annex E; TS 102 563 clause 5.2)
// ---------------------------------------------------------------------

#[test]
fn crc16_matches_the_published_check_value() {
    // CRC-16, G(x) = x^16 + x^12 + x^5 + 1, init all ones, complement
    // before transmission: the standard "123456789" check value.
    assert_eq!(crc16(b"123456789"), 0xD64E);
    // No data: register 0xFFFF complemented.
    assert_eq!(crc16(&[]), 0x0000);
}

#[test]
fn fire_code_polynomial_is_the_clause_5_2_expansion() {
    fn carryless(a: u32, b: u32) -> u32 {
        let mut product = 0u32;
        for i in 0..32 {
            if (a >> i) & 1 == 1 {
                product ^= b << i;
            }
        }
        product
    }
    // (x^11 + 1) * (x^5 + x^3 + x^2 + x + 1), truncated to degree < 16.
    let expanded = carryless((1 << 11) | 1, 0b10_1111);
    assert_eq!(u32::from(FIRE_CODE_POLY), expanded & 0xFFFF);
    assert_eq!(expanded, 0x1_782F);
}

#[test]
fn fire_code_starts_at_zero_and_is_linear() {
    assert_eq!(fire_code(&[0u8; 9]), 0);
    let a = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let b = [0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA, 0x99, 0x88, 0x77];
    let xored: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x ^ y).collect();
    assert_eq!(fire_code(&xored), fire_code(&a) ^ fire_code(&b));
}

#[test]
fn fire_code_detects_every_single_bit_flip() {
    let original = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x0F];
    let baseline = fire_code(&original);
    for byte in 0..9 {
        for bit in 0..8 {
            let mut damaged = original;
            damaged[byte] ^= 1 << bit;
            assert_ne!(
                fire_code(&damaged),
                baseline,
                "bit {bit} of byte {byte} went undetected"
            );
        }
    }
}

// ---------------------------------------------------------------------
// RS(120,110) — TS 102 563 clause 6.1
// ---------------------------------------------------------------------

/// Independent GF(2^8) multiply (shift-and-reduce by 0x11D), used to
/// re-evaluate codewords at the clause 6.1 roots without the codec's
/// private tables.
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    product
}

/// `Σ word[i]·α^(j·(119−i))`, the syndrome S_j by definition.
fn evaluate(word: &[u8; RS_N], j: usize) -> u8 {
    let mut alpha_j = 1u8;
    for _ in 0..j {
        alpha_j = gf_mul(alpha_j, 2);
    }
    let mut acc = 0u8;
    for &byte in word {
        acc = gf_mul(acc, alpha_j) ^ byte;
    }
    acc
}

fn patterned_data(seed: u8) -> [u8; RS_K] {
    let mut data = [0u8; RS_K];
    let mut state = u16::from(seed) | 1;
    for byte in &mut data {
        state = state.wrapping_mul(0x9E37) ^ (state >> 3);
        *byte = (state >> 8) as u8;
    }
    data
}

fn codeword_for(data: &[u8; RS_K]) -> [u8; RS_N] {
    let codec = Rs120_110::new();
    let parity = codec.encode(data);
    let mut word = [0u8; RS_N];
    word[..RS_K].copy_from_slice(data);
    word[RS_K..].copy_from_slice(&parity);
    word
}

#[test]
fn encoded_codeword_vanishes_at_the_clause_6_1_generator_roots() {
    // G(x) = ∏_{i=0}^{9}(x + α^i): a codeword evaluates to zero at
    // every root. This is the clause's defining property, checked with
    // an independent field implementation.
    for seed in [1u8, 7, 0x42, 0xAB] {
        let word = codeword_for(&patterned_data(seed));
        for j in 0..RS_PARITY {
            assert_eq!(evaluate(&word, j), 0, "seed {seed}, syndrome {j}");
        }
    }
}

#[test]
fn decoder_accepts_clean_and_corrects_five_errors() {
    let codec = Rs120_110::new();
    let data = patterned_data(3);
    let clean = codeword_for(&data);
    let mut word = clean;
    assert_eq!(codec.decode(&mut word, &[]), Ok(0));
    assert_eq!(word, clean);

    for &position in &[0usize, 7, 42, 100, 119] {
        word[position] ^= 0x5A;
    }
    assert_eq!(codec.decode(&mut word, &[]), Ok(5));
    assert_eq!(word, clean);
}

#[test]
fn decoder_corrects_ten_erasures() {
    let codec = Rs120_110::new();
    let clean = codeword_for(&patterned_data(9));
    let erasures = [3usize, 17, 33, 60, 61, 82, 90, 105, 112, 118];
    let mut word = clean;
    for &position in &erasures {
        word[position] ^= 0xE7;
    }
    assert_eq!(codec.decode(&mut word, &erasures), Ok(erasures.len()));
    assert_eq!(word, clean);
}

#[test]
fn decoder_rejects_what_it_cannot_correct() {
    let codec = Rs120_110::new();
    let clean = codeword_for(&patterned_data(11));
    let mut six_errors = clean;
    for &position in &[1usize, 20, 40, 60, 80, 100] {
        six_errors[position] ^= 0xA5;
    }
    assert_eq!(
        codec.decode(&mut six_errors, &[]),
        Err(RsError::Uncorrectable)
    );

    let mut word = clean;
    let erasures: Vec<usize> = (0..11).collect();
    assert_eq!(
        codec.decode(&mut word, &erasures),
        Err(RsError::TooManyErasures(11))
    );
    assert_eq!(
        codec.decode(&mut word, &[5, 5]),
        Err(RsError::BadErasure(5))
    );
    assert_eq!(
        codec.decode(&mut word, &[120]),
        Err(RsError::BadErasure(120))
    );
}

// ---------------------------------------------------------------------
// Superframe header (clause 5.2, Tables 2/3/4/8)
// ---------------------------------------------------------------------

fn header(dac_rate: bool, sbr_flag: bool, stereo: bool, ps_flag: bool) -> SuperframeHeader {
    SuperframeHeader {
        rfa: false,
        dac_rate,
        sbr_flag,
        aac_channel_mode: stereo,
        ps_flag,
        mpeg_surround_config: 0,
    }
}

#[test]
fn table_2_maps_the_four_dab_plus_configurations() {
    // The contract's "5 AUs per superframe" is the standard's *five
    // logical DAB frames* carrying one superframe; clause 5.2 Table 2
    // fixes the AU count from (dac_rate, sbr_flag): 2/3/4/6.
    let cases = [
        (false, true, 2usize, 16_000u32, 5usize),
        (true, true, 3, 24_000, 6),
        (false, false, 4, 32_000, 8),
        (true, false, 6, 48_000, 11),
    ];
    for (dac_rate, sbr_flag, num_aus, core_rate, first_au) in cases {
        let header = header(dac_rate, sbr_flag, true, false);
        assert_eq!(header.num_aus(), num_aus);
        assert_eq!(header.core_rate_hz(), core_rate);
        assert_eq!(header.dac_rate_hz(), if dac_rate { 48_000 } else { 32_000 });
        assert_eq!(header.first_au_start(), first_au);
        assert_eq!(
            SuperframeHeader::from_params(header.to_params_byte()),
            header
        );
    }
}

// ---------------------------------------------------------------------
// Superframe encode/decode round trips
// ---------------------------------------------------------------------

/// An AU whose first syntactic element is a data_stream_element carrying
/// `pad` (ISO/IEC 14496-3 clause 4.4.2.5, the PAD carriage of TS 102
/// 563 clause 5.4.1).
fn au_with_pad(payload: &[u8], pad: &[u8]) -> Vec<u8> {
    let mut au = Vec::new();
    // id_syn_ele = 0b100 (3), element_instance_tag = 0 (4),
    // data_byte_align_flag = 0 (1), count = pad.len() (8).
    let mut writer = BitWriter::default();
    writer.write(0b100, 3);
    writer.write(0, 4);
    writer.write_bit(false);
    writer.write(pad.len() as u32, 8);
    au.extend_from_slice(&writer.finish());
    au.extend_from_slice(pad);
    au.extend_from_slice(payload);
    au
}

#[derive(Debug, Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    fn write(&mut self, value: u32, count: usize) {
        for j in 0..count {
            self.write_bit((value >> (count - 1 - j)) & 1 == 1);
        }
    }

    fn write_bit(&mut self, value: bool) {
        if self.bit.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if value {
            let index = self.bytes.len() - 1;
            self.bytes[index] |= 1 << (7 - (self.bit % 8));
        }
        self.bit += 1;
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[test]
fn superframe_round_trips_for_every_configuration() {
    for (dac_rate, sbr_flag, num_aus) in [
        (false, true, 2usize),
        (true, true, 3),
        (false, false, 4),
        (true, false, 6),
    ] {
        let header = header(dac_rate, sbr_flag, true, false);
        let pad: Vec<u8> = (0..6).map(|i| 0xA0 + i).collect();
        let mut aus: Vec<Vec<u8>> = (0..num_aus).map(|i| vec![(i + 1) as u8; 4]).collect();
        aus[0] = au_with_pad(&aus[0], &pad);
        let refs: Vec<&[u8]> = aus.iter().map(Vec::as_slice).collect();

        // 4 kbit/s-ish framing: capacity must hold header + Σ(AU+CRC).
        let encoder = SuperframeEncoder::new(1).expect("encoder");
        let framed = encoder.encode(&header, &refs).expect("encode");
        assert_eq!(framed.len(), 120);

        // Feed in deliberately awkward chunks; one whole superframe
        // comes out and nothing more.
        let mut decoder = SuperframeDecoder::new(1).expect("decoder");
        let mut decoded = Vec::new();
        for chunk in framed.chunks(7) {
            decoded.extend(decoder.push(chunk));
        }
        assert_eq!(decoded.len(), 1);
        let superframe = &decoded[0];
        assert!(superframe.firecode_ok);
        assert_eq!(superframe.header, Some(header));
        assert_eq!(superframe.rs_corrected, 0);
        assert_eq!(superframe.rs_uncorrectable, 0);
        assert_eq!(superframe.aus.len(), num_aus);
        for (index, unit) in superframe.aus.iter().enumerate() {
            assert!(unit.crc_ok, "AU {index} CRC");
            assert!(
                unit.data.starts_with(&aus[index]),
                "AU {index} bytes diverge"
            );
            // Only the last AU may carry zero stuffing.
            if index + 1 != num_aus {
                assert_eq!(unit.data, aus[index]);
            } else {
                assert!(unit.data[aus[index].len()..].iter().all(|&b| b == 0));
            }
        }
        assert_eq!(superframe.aus[0].pad, pad);
    }
}

#[test]
fn damaged_superframe_is_corrected_through_rs() {
    let header = header(true, true, true, false);
    let aus: Vec<Vec<u8>> = (0..3).map(|i| vec![0x40 + i as u8; 40]).collect();
    let refs: Vec<&[u8]> = aus.iter().map(Vec::as_slice).collect();
    let encoder = SuperframeEncoder::new(3).expect("encoder");
    let mut framed = encoder.encode(&header, &refs).expect("encode");

    // Five protected bytes damaged, spread through the superframe.
    for index in [13usize, 55, 120, 200, 333] {
        framed[index] ^= 0x3C;
    }
    let mut decoder = SuperframeDecoder::new(3).expect("decoder");
    let decoded = decoder.push(&framed);
    assert_eq!(decoded.len(), 1);
    let superframe = &decoded[0];
    assert_eq!(superframe.rs_corrected, 5);
    assert_eq!(superframe.rs_uncorrectable, 0);
    assert_eq!(superframe.header, Some(header));
    assert_eq!(superframe.aus.len(), 3);
    assert!(superframe.aus.iter().all(|au| au.crc_ok));
    assert_eq!(superframe.aus[1].data, aus[1]);
}

#[test]
fn a_ten_erasure_superframe_codeword_recovers() {
    // Row 13's erasure case at the RS layer: take one codeword out of an
    // encoded superframe, damage ten bytes and hand their positions in.
    let header = header(true, false, true, false);
    let aus: Vec<Vec<u8>> = (0..6).map(|i| vec![i as u8; 12]).collect();
    let refs: Vec<&[u8]> = aus.iter().map(Vec::as_slice).collect();
    let framed = SuperframeEncoder::new(1)
        .expect("encoder")
        .encode(&header, &refs)
        .expect("encode");

    let mut word = [0u8; RS_N];
    word[..RS_K].copy_from_slice(&framed[..RS_K]);
    word[RS_K..].copy_from_slice(&framed[RS_K..]);
    let clean = word;
    let erasures = [2usize, 15, 30, 44, 59, 73, 88, 99, 111, 118];
    for &position in &erasures {
        word[position] ^= 0xFE;
    }
    assert_eq!(Rs120_110::new().decode(&mut word, &erasures), Ok(10));
    assert_eq!(word, clean);
}

#[test]
fn a_bad_header_firecode_is_visible_and_rejects_the_header() {
    // Recompute a valid RS codeword over data whose Fire code field is
    // deliberately wrong: RS is clean, so only the Fire code check can
    // catch it.
    let header = header(true, true, true, false);
    let aus: Vec<Vec<u8>> = (0..3).map(|i| vec![0x11 + i as u8; 30]).collect();
    let refs: Vec<&[u8]> = aus.iter().map(Vec::as_slice).collect();
    let framed = SuperframeEncoder::new(1)
        .expect("encoder")
        .encode(&header, &refs)
        .expect("encode");

    let mut data = [0u8; RS_K];
    data.copy_from_slice(&framed[..RS_K]);
    data[0] ^= 0xFF; // break the stored Fire code
    let codec = Rs120_110::new();
    let parity = codec.encode(&data);
    let mut damaged = Vec::with_capacity(RS_N);
    damaged.extend_from_slice(&data);
    damaged.extend_from_slice(&parity);

    let mut decoder = SuperframeDecoder::new(1).expect("decoder");
    let decoded = decoder.push(&damaged);
    assert_eq!(decoded.len(), 1);
    assert!(!decoded[0].firecode_ok);
    assert_eq!(decoded[0].header, None);
    assert!(decoded[0].aus.is_empty());
    assert_eq!(decoded[0].rs_corrected, 0);
}

// ---------------------------------------------------------------------
// PAD extraction (clause 5.4.3) and footer API
// ---------------------------------------------------------------------

#[test]
fn pad_extraction_reads_the_leading_data_stream_element() {
    let pad = [0x12u8, 0x00, 0x02, 0x44, 0x4C, 0x53];
    let au = au_with_pad(&[0xFF, 0x01, 0x02], &pad);
    assert_eq!(extract_pad(&au), Some(pad.to_vec()));

    // No leading DSE.
    assert_eq!(extract_pad(&[0x00, 0x00, 0x00, 0x00]), None);
    // A DSE whose count is below the two-byte F-PAD minimum is invalid.
    let short = au_with_pad(&[], &[0x00]);
    assert_eq!(extract_pad(&short), None);
    // Truncated payload.
    assert_eq!(extract_pad(&[0x80, 0x06, 0x01, 0x02]), None);
    assert_eq!(extract_pad(&[]), None);
}

#[test]
fn encoder_and_decoder_reject_invalid_configurations() {
    assert!(matches!(
        SuperframeDecoder::new(0),
        Err(Error::InvalidSubchannelIndex(0))
    ));
    assert!(matches!(
        SuperframeEncoder::new(25),
        Err(Error::InvalidSubchannelIndex(25))
    ));
    let header = header(true, true, true, false);
    let encoder = SuperframeEncoder::new(1).expect("encoder");
    let one: &[u8] = &[0u8; 10];
    assert!(matches!(
        encoder.encode(&header, &[one]),
        Err(Error::WrongAuCount {
            expected: 3,
            got: 1
        })
    ));
    let big: Vec<u8> = vec![0u8; 200];
    let refs: Vec<&[u8]> = (0..3).map(|_| big.as_slice()).collect();
    assert!(matches!(
        encoder.encode(&header, &refs),
        Err(Error::SuperframeOverflow { .. })
    ));
}

#[test]
fn decoder_buffer_holds_partial_superframes() {
    let mut decoder = SuperframeDecoder::new(2).expect("decoder");
    assert_eq!(decoder.superframe_bytes(), 240);
    assert_eq!(decoder.data_bytes(), 220);
    assert_eq!(decoder.logical_frame_bytes(), 48);
    assert!(decoder.push(&[0u8; 239]).is_empty());
    assert_eq!(decoder.push(&[0u8; 1]).len(), 1);
}
