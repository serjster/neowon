//! A deterministic DAB Mode I **encoder**, used as the golden-test oracle.
//!
//! This is the transmitter side of tier 1: it builds FCCs (FIGs) from a chosen
//! ensemble, assembles them into FIBs, applies energy dispersal and the
//! punctured convolutional code, and can emit either the transmitted bits or
//! ideal soft bits for the FIC.
//!
//! **Why it lives here and not in `neowon-sim`.** `neowon-sim` does not depend
//! on `neowon-dsp` (the dependency runs the other way, in tests), and adding
//! that edge to get an IQ source would be the wrong direction for the
//! workspace. So the encoder sits next to the decoder it tests, and the
//! `rf-dab` sim preset lands with the app wiring in 10.15.4 (deviation recorded
//! in `docs/tasks/phase10-dab-spec.md`).
//!
//! **Keep in mind what this proves and what it does not.** The encoder shares
//! the normative tables and the clause-11.1.1 code with the decoder, so a shared
//! misreading of the standard passes every test here. That is exactly why the
//! spec's row 7 (a real capture on real air) exists, and why the table tests
//! are written against the standard's own published figures.

use rustfft::num_complex::Complex32;

use super::fec::{conv_encode, energy_dispersal, puncture_fic};
use super::fib::fib_crc;
use super::ofdm::{Fft2048, carrier_bin, interleaver, prs_reference, qpsk};
use super::{
    CARRIERS, FIB_BYTES, FIB_DATA_BYTES, FIBS_PER_FRAME, FIC_BITS_PER_SYMBOL, FIC_DATA_BITS,
    FIC_SOFT_BITS, FIC_SUBBLOCK_BITS, FRAME_SAMPLES, SYMBOLS_PER_FRAME, T_G, T_NULL, T_S, T_U,
};

/// One service to encode: identifier, label, and its sub-channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSpec<'a> {
    pub sid: u16,
    pub label: &'a str,
    pub sub_channel: u8,
    /// `ASCTy`: 0 = MPEG-1 Layer II (DAB), 63 = DAB+.
    pub ascty: u8,
}

/// The ensemble to encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsembleSpec<'a> {
    pub eid: u16,
    pub label: &'a str,
    pub services: Vec<ServiceSpec<'a>>,
}

/// A capacity unit is 64 bits; the encoder gives every sub-channel 96 CUs
/// (256 kbit/s) with EEP 3-A, which is a realistic DAB+ profile.
const SUB_CHANNEL_CU: u16 = 96;

/// A FIG as it appears on the wire: the `(type | length)` header byte followed
/// by the data field.
fn fig(fig_type: u8, data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= 31, "FIG data field is at most 31 bytes");
    let mut out = Vec::with_capacity(data.len() + 1);
    out.push(((fig_type & 0x07) << 5) | data.len() as u8);
    out.extend_from_slice(data);
    out
}

fn label_field(text: &str, width: usize) -> Vec<u8> {
    let mut out = vec![0u8; width];
    for (i, byte) in text.bytes().take(width).enumerate() {
        out[i] = byte;
    }
    out
}

impl<'a> EnsembleSpec<'a> {
    /// The FIGs of this ensemble, in the order a broadcaster would repeat them:
    /// identity, sub-channel organization, service organization, labels.
    fn figs(&self) -> Vec<Vec<u8>> {
        let mut figs = Vec::new();

        // FIG 0/0 — ensemble information (clause 6.4.1).
        let mut data = vec![0x00]; // C/N 0, OE 0, P/D 0, extension 0
        data.extend_from_slice(&self.eid.to_be_bytes());
        figs.push(fig(0, &data));

        // FIG 0/1 — sub-channel organization, long form / EEP (clause 6.2.1).
        let mut data = vec![0x01]; // extension 1
        for (index, service) in self.services.iter().enumerate() {
            let start_cu = index as u16 * SUB_CHANNEL_CU;
            data.push((service.sub_channel << 2) | ((start_cu >> 8) as u8 & 0x03));
            data.push(start_cu as u8);
            // Long form, option 0 (EEP-A), signalled level 2 (= level 3),
            // size in the low 10 bits.
            // Long form bit 7, option 0 (= EEP-A), level 2 (= level 3).
            data.push(0x80 | (2 << 2) | ((SUB_CHANNEL_CU >> 8) as u8 & 0x03));
            data.push(SUB_CHANNEL_CU as u8);
        }
        figs.push(fig(0, &data));

        // FIG 0/2 — service organization: one FIG per service (clause 6.3.1).
        for service in &self.services {
            let mut data = vec![0x02]; // extension 2, P/D 0 (16-bit SId)
            data.extend_from_slice(&service.sid.to_be_bytes());
            data.push(0x01); // Rfa 0, CAId 0, one component
            data.push(service.ascty & 0x3F); // TMId 0 = stream audio
            data.push((service.sub_channel << 2) | (1 << 1)); // primary
            figs.push(fig(0, &data));
        }

        // FIG 1/0 — ensemble label, referenced by the EId (clause 8.1.13).
        let mut data = vec![0x00]; // charset 0, Rfu 0, extension 0
        data.extend_from_slice(&self.eid.to_be_bytes());
        data.extend_from_slice(&label_field(self.label, 16));
        figs.push(fig(1, &data));

        // FIG 1/1 — programme service labels (clause 8.1.14.1).
        for service in &self.services {
            let mut data = vec![0x01]; // charset 0, extension 1
            data.extend_from_slice(&service.sid.to_be_bytes());
            data.extend_from_slice(&label_field(service.label, 16));
            figs.push(fig(1, &data));
        }
        figs
    }

    /// Pack the FIGs into the frame's twelve FIBs, cycling the FIG list the way
    /// a real multiplexer repeats its FIC. Each FIB's data field is filled with
    /// whole FIGs only, padded with `0x00`.
    pub fn fibs(&self) -> Vec<[u8; FIB_BYTES]> {
        let figs = self.figs();
        assert!(!figs.is_empty());
        let mut fibs = Vec::with_capacity(FIBS_PER_FRAME);
        let mut next_fig = 0usize;
        while fibs.len() < FIBS_PER_FRAME {
            let mut data = Vec::with_capacity(FIB_DATA_BYTES);
            // Whole FIGs only, and at least one per FIB: every FIG this encoder
            // builds is far smaller than a FIB's 30-byte data field.
            loop {
                let candidate = &figs[next_fig % figs.len()];
                if data.len() + candidate.len() > FIB_DATA_BYTES {
                    assert!(!data.is_empty(), "a FIG larger than a FIB");
                    break;
                }
                data.extend_from_slice(candidate);
                next_fig += 1;
            }
            let mut fib = [0u8; FIB_BYTES];
            fib[..data.len()].copy_from_slice(&data);
            let crc = fib_crc(&fib);
            fib[30] = (crc >> 8) as u8;
            fib[31] = crc as u8;
            fibs.push(fib);
        }
        fibs
    }
}

/// One transmission frame's FIC, as transmitted bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FicFrame {
    /// 4 codewords of 2304 bits, in CIF order — 9216 bits in total.
    bits: Vec<u8>,
}

impl FicFrame {
    /// Encode an ensemble into one frame's FIC.
    pub fn new(spec: &EnsembleSpec<'_>) -> Self {
        let fibs = spec.fibs();
        let mut bits = Vec::with_capacity(FIC_SOFT_BITS);
        for group in fibs.chunks(3) {
            // Three FIBs = 768 bits, MSb first within each byte.
            let mut info = Vec::with_capacity(FIC_DATA_BITS);
            for fib in group {
                for byte in fib.iter() {
                    for bit in (0..8).rev() {
                        info.push((byte >> bit) & 1);
                    }
                }
            }
            assert_eq!(info.len(), FIC_DATA_BITS);
            // The scrambler sits between the FIB assembler and the coder
            // (clause 10.2), so dispersal comes first.
            energy_dispersal(&mut info);
            let coded = conv_encode(&info);
            let punctured = puncture_fic(&coded);
            assert_eq!(punctured.len(), FIC_SUBBLOCK_BITS);
            bits.extend_from_slice(&punctured);
        }
        assert_eq!(bits.len(), FIC_SOFT_BITS);
        Self { bits }
    }

    /// The transmitted FIC bits of the frame.
    pub fn transmitted_bits(&self) -> &[u8] {
        &self.bits
    }

    /// Ideal soft bits for a noiseless channel, at the given confidence.
    pub fn soft_bits_with(&self, amplitude: i8) -> Vec<i8> {
        self.bits
            .iter()
            .map(|bit| if *bit == 1 { amplitude } else { -amplitude })
            .collect()
    }

    /// Ideal soft bits at full confidence — what the FIC decoder sees when the
    /// front end is perfect, which isolates the FEC and FIG layers from the
    /// OFDM layer's problems.
    pub fn ideal_soft_bits(&self) -> Vec<i8> {
        self.soft_bits_with(100)
    }

    /// Modulate this frame as a whole Mode I transmission frame at 2.048 MS/s:
    /// the null symbol, the phase reference symbol, the three FIC symbols, and
    /// the 72 MSC symbols (clauses 14.2, 14.3.1, 14.3.2, 14.4.1.1, 14.7).
    ///
    /// The MSC symbols carry a deterministic pseudo-random DQPSK pattern: tier
    /// 1 does not decode them, and they must *not* be silent, or the null symbol
    /// would stop being the only power dip in the frame and the receiver's sync
    /// would be tested against something no broadcaster transmits.
    ///
    /// The result is `FRAME_SAMPLES` complex samples, scaled to the given RMS
    /// (a typical SDR capture sits near 0.2 of full scale).
    pub fn iq_frame(&self, rms: f32) -> Vec<Complex32> {
        let mut fft = Fft2048::new();
        let mut frame = vec![Complex32::new(0.0, 0.0); FRAME_SAMPLES];

        // Symbol 0 is the null symbol: the transmitter is off (clause 14.3.1),
        // which is exactly what the receiver's sync looks for.
        let mut at = T_NULL;
        let mut previous = prs_reference().to_vec();
        write_symbol(&mut frame, at, &fft.symbol_from_spectrum(&previous));
        at += T_S;

        // Symbols 2, 3, 4 carry the FIC, differentially modulated against the
        // PRS and then against each other (clauses 14.7, 14.4.1.1).
        for symbol in 0..3usize {
            let bits = &self.bits[symbol * FIC_BITS_PER_SYMBOL..(symbol + 1) * FIC_BITS_PER_SYMBOL];
            let mut spectrum = vec![Complex32::new(0.0, 0.0); T_U];
            for (n, k) in interleaver().iter().enumerate() {
                let bin = carrier_bin(*k);
                spectrum[bin] = previous[bin] * qpsk(bits[n], bits[n + CARRIERS]);
            }
            write_symbol(&mut frame, at, &fft.symbol_from_spectrum(&spectrum));
            at += T_S;
            previous = spectrum;
        }

        // The rest of the frame is the MSC, which tier 1 does not decode.
        let mut rng: u32 = 0x5EED_1234;
        for _ in (3 + 1)..SYMBOLS_PER_FRAME {
            let mut spectrum = vec![Complex32::new(0.0, 0.0); T_U];
            for k in -768..=768i16 {
                if k == 0 {
                    continue;
                }
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let bit_i = ((rng >> 17) & 1) as u8;
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let bit_q = ((rng >> 17) & 1) as u8;
                let bin = carrier_bin(k);
                spectrum[bin] = previous[bin] * qpsk(bit_i, bit_q);
            }
            write_symbol(&mut frame, at, &fft.symbol_from_spectrum(&spectrum));
            at += T_S;
            previous = spectrum;
        }
        assert_eq!(at, FRAME_SAMPLES);

        let power: f32 = frame.iter().map(|s| s.norm_sqr()).sum::<f32>() / frame.len() as f32;
        let current = power.sqrt();
        if current > 0.0 {
            let gain = rms / current;
            for sample in frame.iter_mut() {
                *sample *= gain;
            }
        }
        frame
    }
}

/// Copy one OFDM symbol (`T_G + T_U` samples) into the frame at `at`.
fn write_symbol(frame: &mut [Complex32], at: usize, symbol: &[Complex32]) {
    assert_eq!(symbol.len(), T_G + T_U);
    frame[at..at + symbol.len()].copy_from_slice(symbol);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> EnsembleSpec<'static> {
        EnsembleSpec {
            eid: 0xF044,
            label: "METROPOLITAIN 2",
            services: vec![
                ServiceSpec {
                    sid: 0x1001,
                    label: "FRANCE INTER",
                    sub_channel: 0,
                    ascty: 63,
                },
                ServiceSpec {
                    sid: 0x1002,
                    label: "FRANCE MUSIQUE",
                    sub_channel: 1,
                    ascty: 63,
                },
            ],
        }
    }

    /// The frame is exactly the size the standard says: twelve FIBs, four
    /// codewords, 9216 transmitted bits.
    #[test]
    fn frame_sizes_match_the_standard() {
        let frame = FicFrame::new(&spec());
        assert_eq!(frame.transmitted_bits().len(), FIC_SOFT_BITS);
        assert_eq!(frame.ideal_soft_bits().len(), FIC_SOFT_BITS);
        assert_eq!(spec().fibs().len(), FIBS_PER_FRAME);
    }

    /// Every FIB the encoder produces carries a valid CRC and repeats the
    /// ensemble's identity.
    #[test]
    fn encoded_fibs_are_self_consistent() {
        for fib in spec().fibs() {
            assert!(crate::dab::fib_crc_ok(&fib));
        }
        // The first FIB leads with FIG 0/0, so its first bytes are the EId.
        let fibs = spec().fibs();
        assert_eq!((fibs[0][2], fibs[0][3]), (0xF0, 0x44));
    }

    /// Encoding is deterministic: same spec, same bits.
    #[test]
    fn encoding_is_deterministic() {
        assert_eq!(
            FicFrame::new(&spec()).transmitted_bits(),
            FicFrame::new(&spec()).transmitted_bits()
        );
    }

    /// Every sub-channel in the FIG 0/1 blob is described by four bytes, so a
    /// FIG with two services still parses as two entries.
    #[test]
    fn sub_channel_fig_has_one_entry_per_service() {
        let figs = spec().figs();
        let sub_channel_fig = figs
            .iter()
            .find(|f| (f[0] >> 5) == 0 && (f[1] & 0x1F) == 1)
            .expect("FIG 0/1");
        // header + flags + 2 services x 4 bytes
        assert_eq!(sub_channel_fig.len(), 1 + 1 + 8);
    }
}
