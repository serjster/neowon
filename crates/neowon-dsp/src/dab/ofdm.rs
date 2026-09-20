//! The DAB OFDM front end for mode I: the phase reference symbol, the frequency
//! interleaver, and the differential demapper.
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **14.2/14.4** carrier mapping: 1536 active carriers, `k` in
//!   `[-768, -1] ∪ [1, 768]`, in the natural DFT ordering — carrier `k` sits in
//!   bin `k` for `k > 0` and bin `2048 + k` for `k < 0`, so the band is
//!   DC-centred and 1.536 MHz wide.
//! - **14.3.2** the phase reference symbol: `z_k = exp(j·phi_k)` with unit
//!   amplitude, `phi_k = (pi/2)·(h[i, k - k'] + n)` from tables 23 and 24. It is
//!   the differential reference for the first data symbol.
//! - **14.6.1** the frequency interleaver: `Pi(i) = 13·Pi(i-1) + 511 (mod 2048)`,
//!   `D` = the elements of that permutation in `[256, 1792]` excluding `1024`,
//!   and `k = F(n) = d_n - 1024`. Symbol index `n` of the QPSK vector maps to
//!   carrier `k`, one-to-one onto `[-768, 768] \ {0}`.
//! - **14.5** the QPSK mapper: the first `K` bits of a symbol's bit block are
//!   the in-phase components, the next `K` are the quadrature ones,
//!   `q = (1/sqrt2)·[(1 - 2p) + j·(1 - 2p')]`.
//! - **14.7** differential modulation, symbol to symbol on each carrier:
//!   `z_{l,k} = z_{l-1,k} · q_{l,n}` with `k = F(n)`.
//!
//! ## What the receiver can ignore, and why
//!
//! Because the demodulation is differential *along time on the same carrier*
//! (clause 14.7), two impairments cancel in the product `R_l[k]·conj(R_{l-1}[k])`:
//!
//! - a **carrier-independent phase**, so the channel's phase drops out;
//! - a **fixed timing offset within the guard interval**, since the same
//!   relative window is taken for every symbol, so its phase ramp is identical
//!   in both factors.
//!
//! What does *not* cancel is the **frequency offset**: it rotates the product by
//! `2·pi·df·Ts` per symbol, which is what breaks DQPSK on a drifting dongle
//! clock. That is why the front end estimates and removes it before demapping
//! (see [`super::receiver`]).

use std::f32::consts::FRAC_PI_2;
use std::sync::OnceLock;

use rustfft::num_complex::Complex32;

use super::tables::{H, PRS_RANGES};
use super::{CARRIERS, T_G, T_U};

/// The DFT bin holding carrier `k` (clause 14.2/14.4): `k` for positive
/// carriers, `T_U + k` for negative ones.
pub fn carrier_bin(k: i16) -> usize {
    if k > 0 {
        k as usize
    } else {
        T_U - k.unsigned_abs() as usize
    }
}

/// The phase `phi_k` of the phase reference symbol (clause 14.3.2, tables 23
/// and 24), in radians. Carriers outside the active set have no phase.
///
/// `phi_k = (pi/2)·(h[i, k - k'] + n)` for the table-23 range that contains `k`.
pub fn prs_phase(k: i16) -> f32 {
    for (kmin, kmax, i, n) in PRS_RANGES {
        if k >= kmin && k <= kmax {
            let j = (k - kmin) as usize;
            return FRAC_PI_2 * (H[i as usize][j] as f32 + n as f32);
        }
    }
    0.0
}

/// The phase reference symbol as a 2048-bin spectrum, unit amplitude on the
/// 1536 active carriers and zero everywhere else (clause 14.3.2).
pub fn prs_reference() -> &'static [Complex32] {
    static PRS: OnceLock<Vec<Complex32>> = OnceLock::new();
    PRS.get_or_init(|| {
        let mut bins = vec![Complex32::new(0.0, 0.0); T_U];
        for k in -768..=768i16 {
            if k == 0 {
                continue;
            }
            bins[carrier_bin(k)] = Complex32::from_polar(1.0, prs_phase(k));
        }
        bins
    })
}

/// The mode I frequency interleaver `F(n) = d_n - 1024` (clause 14.6.1):
/// symbol index `n` in `0..1536` maps to the carrier it is transmitted on.
pub fn interleaver() -> &'static [i16; CARRIERS] {
    static TABLE: OnceLock<[i16; CARRIERS]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut pi = [0u16; 2048];
        for i in 1..2048usize {
            pi[i] = ((13 * pi[i - 1] as u32 + 511) % 2048) as u16;
        }
        let mut out = [0i16; CARRIERS];
        let mut n = 0usize;
        for value in pi {
            if (256..=1792).contains(&value) && value != 1024 {
                out[n] = value as i16 - 1024;
                n += 1;
            }
        }
        assert_eq!(n, CARRIERS, "the interleaver must cover every carrier once");
        out
    })
}

/// A soft bit from a normalized decision: `|x| <= 1`, and positive means
/// "likely 1" (the decoder's convention, see `super::fec`).
///
/// Scaled so a clean QPSK carrier — whose real and imaginary parts are
/// `1/sqrt(2)` — lands on `+/-100`, with the rest of the range left for noise
/// and saturation at `+/-127`. The absolute scale does not matter to the
/// Viterbi decoder (metrics are relative), but an erasure must stay a true zero.
fn soft(x: f32) -> i8 {
    (x * 100.0 * std::f32::consts::SQRT_2)
        .round()
        .clamp(-127.0, 127.0) as i8
}

/// Demap one symbol against its reference into `2K` soft bits, in symbol order:
/// the first `K` are the in-phase bits, the next `K` the quadrature ones
/// (clauses 14.5, 14.6.1, 14.7).
///
/// `symbol` and `reference` are 2048-bin spectra; the reference is the previous
/// OFDM symbol's spectrum, or the phase reference symbol for the first data
/// symbol. A carrier with no energy produces an erasure (`0`), which the
/// Viterbi decoder treats as no evidence.
///
/// The bits are taken as `-Re` and `-Im` of the normalized product because the
/// mapper's convention is `q = (1/sqrt2)[(1 - 2p) + j(1 - 2p')]`: bit 0 sits at
/// `+1`, so a positive real part means bit 0.
pub fn demap_soft(symbol: &[Complex32], reference: &[Complex32], out: &mut [i8]) {
    assert_eq!(out.len(), 2 * CARRIERS, "soft bits per symbol");
    for (n, k) in interleaver().iter().enumerate() {
        let bin = carrier_bin(*k);
        let product = symbol[bin] * reference[bin].conj();
        let magnitude = product.norm();
        if magnitude > 1e-12 {
            out[n] = soft(-product.re / magnitude);
            out[n + CARRIERS] = soft(-product.im / magnitude);
        } else {
            out[n] = 0;
            out[n + CARRIERS] = 0;
        }
    }
}

/// The QPSK symbol for a bit pair, as the mapper of clause 14.5 defines it.
/// Used by the modulation side (`super::encoder`) and the tests here.
pub fn qpsk(bit_i: u8, bit_q: u8) -> Complex32 {
    let scale = std::f32::consts::FRAC_1_SQRT_2;
    Complex32::new(
        scale * (1.0 - 2.0 * bit_i as f32),
        scale * (1.0 - 2.0 * bit_q as f32),
    )
}

/// A shared 2048-point FFT pair: the forward transform for the receiver, the
/// inverse for the modulation side. Both are unit-scaled, so a spectrum with a
/// unit-amplitude carrier comes back with unit amplitude in time.
pub struct Fft2048 {
    forward: std::sync::Arc<dyn rustfft::Fft<f32>>,
    inverse: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<Complex32>,
}

impl Fft2048 {
    pub fn new() -> Self {
        let mut planner = rustfft::FftPlanner::new();
        Self {
            forward: planner.plan_fft_forward(T_U),
            inverse: planner.plan_fft_inverse(T_U),
            scratch: Vec::new(),
        }
    }

    /// In-place forward transform.
    pub fn forward(&mut self, buffer: &mut [Complex32]) {
        assert_eq!(buffer.len(), T_U);
        self.scratch.resize(
            self.forward.get_inplace_scratch_len(),
            Complex32::new(0.0, 0.0),
        );
        self.forward.process_with_scratch(buffer, &mut self.scratch);
    }

    /// A whole OFDM symbol in the time domain, guard interval included
    /// (clause 14.2: the guard is a copy of the symbol's last `T_G` samples).
    /// Returns `T_G + T_U` samples.
    pub fn symbol_from_spectrum(&mut self, spectrum: &[Complex32]) -> Vec<Complex32> {
        assert_eq!(spectrum.len(), T_U);
        let mut buffer = spectrum.to_vec();
        self.scratch.resize(
            self.inverse.get_inplace_scratch_len(),
            Complex32::new(0.0, 0.0),
        );
        self.inverse
            .process_with_scratch(&mut buffer, &mut self.scratch);
        let scale = 1.0 / T_U as f32;
        for value in buffer.iter_mut() {
            *value *= scale;
        }
        let mut out = Vec::with_capacity(T_G + T_U);
        out.extend_from_slice(&buffer[T_U - T_G..]);
        out.extend_from_slice(&buffer);
        out
    }
}

impl Default for Fft2048 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 25 is the standard's own worked example of the interleaver. Its
    /// first eleven entries pin both the permutation and the direction of the
    /// mapping (which way round `n` and `k` go).
    #[test]
    fn interleaver_matches_table_25() {
        let f = interleaver();
        let expected = [-513i16, -14, 329, 692, -733, 13, 680, 273, -36, 43, 85];
        assert_eq!(&f[..expected.len()], &expected);
    }

    /// `F` is one-to-one onto `[-768, 768] \ {0}` — no carrier twice, none
    /// missing, DC never used.
    #[test]
    fn interleaver_is_a_bijection_onto_the_carriers() {
        let mut seen = vec![false; 2048];
        for k in interleaver() {
            assert_ne!(*k, 0, "DC must never carry a symbol");
            assert!((-768..=768).contains(k));
            let bin = carrier_bin(*k);
            assert!(!seen[bin], "carrier {k} used twice");
            seen[bin] = true;
        }
        assert_eq!(seen.iter().filter(|s| **s).count(), CARRIERS);
        // The extreme carriers are reachable: 256 and 1792 are in the table.
        assert_eq!(carrier_bin(768), 768);
        assert_eq!(carrier_bin(-768), 1280);
    }

    /// The PRS has unit amplitude on every active carrier, nothing elsewhere,
    /// and — as a DAB phase reference — only quarter-turn phases.
    #[test]
    fn prs_is_unit_amplitude_and_qpsk_valued() {
        let prs = prs_reference();
        assert_eq!(prs.len(), T_U);
        let mut active = 0;
        for (bin, value) in prs.iter().enumerate() {
            let k = if bin <= 768 {
                bin as i16
            } else {
                bin as i16 - T_U as i16
            };
            let is_active = k != 0 && (-768..=768).contains(&k) && (bin <= 768 || bin >= 1280);
            if is_active {
                assert!((value.norm() - 1.0).abs() < 1e-6, "bin {bin} amplitude");
                // Multiples of pi/2 land exactly on the axes.
                let axis = value.re.abs() < 1e-6 || value.im.abs() < 1e-6;
                assert!(
                    axis,
                    "bin {bin} phase {} is not a quarter turn",
                    value.arg()
                );
                active += 1;
            } else {
                assert_eq!(value.norm(), 0.0, "bin {bin} should be empty");
            }
        }
        assert_eq!(active, CARRIERS);
    }

    /// A spot value from tables 23 and 24: carrier `k = 1` is the first entry
    /// of the range `(1, 32, i = 0, n = 3)` with `h[0][0] = 0`, so its phase is
    /// `(pi/2)·3` and the reference point is `-j`.
    #[test]
    fn prs_phase_follows_tables_23_and_24() {
        assert!((prs_phase(1) - 3.0 * FRAC_PI_2).abs() < 1e-6);
        let value = prs_reference()[carrier_bin(1)];
        assert!(value.re.abs() < 1e-6);
        assert!((value.im + 1.0).abs() < 1e-6, "expected -j, got {value:?}");
        // Every active carrier has a defined phase, i.e. some table-23 range,
        // and the formula only ever produces quarter turns.
        for k in -768..=768i16 {
            if k != 0 {
                let phi = prs_phase(k);
                assert!(
                    (0.0..=3.0 * std::f32::consts::PI).contains(&phi),
                    "carrier {k}"
                );
                let quarters = phi / FRAC_PI_2;
                assert!(
                    (quarters - quarters.round()).abs() < 1e-4,
                    "carrier {k} phase {phi} is not a quarter turn"
                );
            }
        }
    }

    /// Demapping inverts mapping: build a symbol from known bits and get them
    /// back, with the encoder's own QPSK mapper.
    #[test]
    fn demap_recovers_mapped_bits() {
        let mut reference = vec![Complex32::new(0.0, 0.0); T_U];
        for k in -768..=768i16 {
            if k != 0 {
                reference[carrier_bin(k)] = Complex32::from_polar(1.0, 0.3 * k as f32);
            }
        }
        let bits: Vec<u8> = (0..2 * CARRIERS)
            .map(|i| ((i * 7 + i / 13) % 3 == 0) as u8)
            .collect();
        let mut symbol = vec![Complex32::new(0.0, 0.0); T_U];
        for (n, k) in interleaver().iter().enumerate() {
            let q = qpsk(bits[n], bits[n + CARRIERS]);
            symbol[carrier_bin(*k)] = reference[carrier_bin(*k)] * q;
        }
        let mut soft = vec![0i8; 2 * CARRIERS];
        demap_soft(&symbol, &reference, &mut soft);
        for (i, bit) in bits.iter().enumerate() {
            let decoded = soft[i] > 0;
            assert_eq!(decoded, *bit == 1, "soft bit {i} = {}", soft[i]);
            assert_eq!(soft[i].abs(), 100, "a clean carrier should be full scale");
        }
    }

    /// A carrier with no energy becomes an erasure, not a decision.
    #[test]
    fn silent_carriers_are_erasures() {
        let symbol = vec![Complex32::new(0.0, 0.0); T_U];
        let reference = vec![Complex32::new(0.0, 0.0); T_U];
        let mut soft = vec![9i8; 2 * CARRIERS];
        demap_soft(&symbol, &reference, &mut soft);
        assert!(soft.iter().all(|s| *s == 0));
    }
}
