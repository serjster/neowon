//! The DAB receiver: interleaved IQ at 2.048 MS/s in, an ensemble table out.
//!
//! This is what makes the FIC decoder reachable from a real signal. It is a
//! stateful object fed whatever block sizes the caller has, holding a sample
//! buffer across calls, so a caller can stream a capture in chunks and read
//! [`DabReceiver::status`] whenever it likes.
//!
//! ## How it locks
//!
//! 1. **Find the null symbol.** The transmitter is silent for `T_NULL` samples
//!    at the start of every 96 ms frame (clause 14.3.1), so the frame start is
//!    the deepest sustained power dip — found on a coarse stride with an
//!    integral of the sample power, not by scanning every offset.
//! 2. **Confirm it is DAB, not a dip in the noise.** The phase reference symbol
//!    that must follow the null is a known 1536-carrier sequence, so the
//!    normalized correlation between the received spectrum and
//!    [`prs_reference`] is ~1 for DAB and ~`1/sqrt(1536)` for noise. A frame
//!    below [`PRS_METRIC_MIN`] is rejected and *nothing* is decoded from it —
//!    this is what keeps the receiver from naming an ensemble that is not there
//!    (D27, and the spec's false-lock criterion).
//! 3. **Remove the frequency offset.** The dongle's clock error rotates the
//!    differential product by `2·pi·df·Ts` per symbol, which breaks DQPSK, so
//!    the cyclic prefix of the PRS measures `df` and the frame's symbols are
//!    de-rotated on a *single* continuous time base (restarting the phasor per
//!    symbol would re-introduce exactly the rotation it is meant to remove).
//! 4. **Demap three symbols** against the PRS and each other
//!    ([`demap_soft`]), and hand 9216 soft bits to the FIC decoder.
//!
//! Timing needs no fine search: the PRS correlation is insensitive to where
//! inside the guard the window starts, and the resulting phase ramp is the same
//! for every symbol, so it cancels in the differential product — both facts are
//! properties of the modulation (clause 14.7), not of this implementation.

use std::f32::consts::TAU;

use rustfft::num_complex::Complex32;

use super::fic::FicState;
use super::ofdm::{Fft2048, demap_soft, prs_reference};
use super::{
    DabStatus, Ensemble, FIC_BITS_PER_SYMBOL, FIC_SOFT_BITS, FRAME_SAMPLES, SAMPLE_RATE, T_G,
    T_NULL, T_S, T_U,
};

/// Lowest normalized PRS correlation accepted as a frame. DAB scores ~1, noise
/// scores ~`1/sqrt(K)`; anything in between is a judgement call, and rejecting
/// a borderline frame costs a frame, while accepting one corrupts the table.
pub const PRS_METRIC_MIN: f32 = 0.5;

/// Stride of the null-symbol search, in samples. The guard interval is 504
/// samples, so an 8-sample grid leaves ample margin.
const NULL_SEARCH_STRIDE: usize = 8;

/// A streaming DAB Mode I receiver.
pub struct DabReceiver {
    fft: Fft2048,
    /// Samples not yet consumed, as complex baseband.
    pending: Vec<Complex32>,
    /// Scratch: power prefix sums for the null search.
    prefix: Vec<f32>,
    /// Scratch: one symbol's spectrum, and the PRS spectrum to demap against.
    spectrum: Vec<Complex32>,
    prs_spectrum: Vec<Complex32>,
    /// Scratch: the three FIC symbols' spectra.
    fic_spectra: Vec<Complex32>,
    /// Scratch: the frame's soft bits.
    soft: Vec<i8>,
    fic: FicState,
    /// Carrier frequency offset estimated on the last frame, Hz.
    pub freq_offset_hz: f64,
    /// Normalized PRS correlation of the last frame.
    pub prs_metric: f32,
    /// Frames decoded, and frames rejected for a failed PRS check.
    pub frames_decoded: u64,
    pub frames_rejected: u64,
}

impl DabReceiver {
    pub fn new() -> Self {
        Self {
            fft: Fft2048::new(),
            pending: Vec::new(),
            prefix: Vec::new(),
            spectrum: vec![Complex32::new(0.0, 0.0); T_U],
            prs_spectrum: vec![Complex32::new(0.0, 0.0); T_U],
            fic_spectra: vec![Complex32::new(0.0, 0.0); 3 * T_U],
            soft: vec![0i8; FIC_SOFT_BITS],
            fic: FicState::new(),
            freq_offset_hz: 0.0,
            prs_metric: 0.0,
            frames_decoded: 0,
            frames_rejected: 0,
        }
    }

    /// Feed interleaved I, Q samples in full-scale units (as
    /// `SampleLayout::Complex` frames carry them). Returns how many transmission
    /// frames were decoded by this call.
    pub fn push_iq(&mut self, interleaved: &[f32]) -> usize {
        self.pending.extend(
            interleaved
                .chunks_exact(2)
                .map(|pair| Complex32::new(pair[0], pair[1])),
        );

        let mut decoded = 0;
        while self.pending.len() >= FRAME_SAMPLES + T_NULL {
            let frame_start = self.find_null_symbol();
            if frame_start + FRAME_SAMPLES > self.pending.len() {
                break;
            }
            if !self.extract_prs_spectrum(frame_start, 0.0) {
                self.frames_rejected += 1;
                let consume = (frame_start + FRAME_SAMPLES / 2).min(self.pending.len());
                self.pending.drain(..consume);
                continue;
            }

            let offset_hz = self.estimate_frequency_offset(frame_start);
            self.freq_offset_hz = offset_hz;
            // Re-extract the PRS on the corrected time base: it is the
            // reference for the first FIC symbol, so it must be corrected too.
            self.extract_prs_spectrum(frame_start, offset_hz);

            for symbol in 0..3usize {
                let start = frame_start + T_NULL + (symbol + 1) * T_S + T_G;
                let base = (start - frame_start) as f64;
                let mut window = std::mem::take(&mut self.spectrum);
                self.corrected_window(start, base, offset_hz, &mut window);
                self.fft.forward(&mut window);
                let at = symbol * T_U;
                self.fic_spectra[at..at + T_U].copy_from_slice(&window);
                let mut soft = std::mem::take(&mut self.soft);
                {
                    let (bits, _) = soft.split_at_mut((symbol + 1) * FIC_BITS_PER_SYMBOL);
                    let slice = &mut bits[symbol * FIC_BITS_PER_SYMBOL..];
                    let reference = if symbol == 0 {
                        &self.prs_spectrum
                    } else {
                        let at = (symbol - 1) * T_U;
                        &self.fic_spectra[at..at + T_U]
                    };
                    demap_soft(&window, reference, slice);
                }
                self.soft = soft;
                self.spectrum = window;
            }

            self.fic.process_frame(&self.soft);
            self.frames_decoded += 1;
            decoded += 1;

            let consume = (frame_start + FRAME_SAMPLES).min(self.pending.len());
            self.pending.drain(..consume);
        }
        decoded
    }

    /// The ensemble table as the FIC decoder sees it (empty until locked).
    pub fn status(&self) -> DabStatus {
        self.fic.status()
    }

    /// The ensemble, locked or not — for callers that show raw progress.
    pub fn ensemble(&self) -> &Ensemble {
        self.fic.ensemble()
    }

    /// Forget the lock, the table and the buffered samples.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.fic.reset();
        self.freq_offset_hz = 0.0;
        self.prs_metric = 0.0;
        self.frames_decoded = 0;
        self.frames_rejected = 0;
    }

    /// Position of the deepest sustained-power dip in the first frame period:
    /// the null symbol, assuming a frame starts somewhere in the buffer.
    fn find_null_symbol(&mut self) -> usize {
        let available = self.pending.len().min(FRAME_SAMPLES + T_NULL);
        self.prefix.clear();
        self.prefix.push(0.0);
        let mut running = 0.0f32;
        for sample in &self.pending[..available] {
            running += sample.norm_sqr();
            self.prefix.push(running);
        }
        let mut best = (f32::MAX, 0usize);
        let mut at = 0usize;
        while at + T_NULL <= available && at < FRAME_SAMPLES {
            let power = self.prefix[at + T_NULL] - self.prefix[at];
            if power < best.0 {
                best = (power, at);
            }
            at += NULL_SEARCH_STRIDE;
        }
        best.1
    }

    /// Transform the PRS symbol's useful part and score it against the known
    /// sequence, storing it for the first FIC symbol's demap. Returns whether
    /// the score clears [`PRS_METRIC_MIN`].
    fn extract_prs_spectrum(&mut self, frame_start: usize, offset_hz: f64) -> bool {
        let start = frame_start + T_NULL + T_G;
        let base = (start - frame_start) as f64;
        let mut window = std::mem::take(&mut self.spectrum);
        self.corrected_window(start, base, offset_hz, &mut window);
        self.fft.forward(&mut window);

        let reference = prs_reference();
        let mut correlation = Complex32::new(0.0, 0.0);
        let mut energy = 0.0f32;
        let mut reference_energy = 0.0f32;
        for bin in 0..T_U {
            correlation += window[bin] * reference[bin].conj();
            energy += window[bin].norm_sqr();
            reference_energy += reference[bin].norm_sqr();
        }
        let denominator = (energy * reference_energy).sqrt();
        self.prs_metric = if denominator > 0.0 {
            correlation.norm() / denominator
        } else {
            0.0
        };
        self.prs_spectrum.copy_from_slice(&window);
        self.spectrum = window;
        self.prs_metric >= PRS_METRIC_MIN
    }

    /// Fill `window` with `T_U` samples at `start`, de-rotated by the estimated
    /// carrier offset on the frame's time base.
    ///
    /// `base` is the window's offset from the frame start, in samples: the
    /// phasor must run *continuously* across symbols, because the thing being
    /// undone is a rotation accumulating in time.
    fn corrected_window(&self, start: usize, base: f64, offset_hz: f64, window: &mut [Complex32]) {
        let step = -f64::from(TAU) * offset_hz / SAMPLE_RATE;
        for (i, slot) in window.iter_mut().enumerate() {
            let phase = (step * (base + i as f64)) as f32;
            *slot = self.pending[start + i] * Complex32::from_polar(1.0, phase);
        }
    }

    /// Carrier frequency offset in Hz, from the cyclic prefix of the phase
    /// reference symbol.
    ///
    /// The guard interval repeats the symbol's last `T_G` samples one useful
    /// period earlier, so the correlation of the two copies carries
    /// `arg = -2·pi·df·Tu/fs` — the sign is checked by
    /// `frequency_offset_sign_is_pinned`, because getting it backwards doubles
    /// the error instead of removing it.
    fn estimate_frequency_offset(&self, frame_start: usize) -> f64 {
        let start = frame_start + T_NULL;
        let mut accumulator = Complex32::new(0.0, 0.0);
        for m in 0..T_G {
            let guard = self.pending[start + m];
            let copy = self.pending[start + T_U + m];
            accumulator += guard * copy.conj();
        }
        if accumulator.norm_sqr() <= f32::MIN_POSITIVE {
            return 0.0;
        }
        -f64::from(accumulator.arg()) * SAMPLE_RATE / (f64::from(TAU) * T_U as f64)
    }
}

impl Default for DabReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::ofdm::{Fft2048, prs_reference};

    /// Build the PRS symbol's samples (guard + useful part), optionally with a
    /// carrier offset applied.
    fn prs_samples(offset_hz: f64) -> Vec<Complex32> {
        let mut fft = Fft2048::new();
        let mut symbol = fft.symbol_from_spectrum(prs_reference());
        for (i, value) in symbol.iter_mut().enumerate() {
            let phase = (TAU as f64 * offset_hz * i as f64 / SAMPLE_RATE) as f32;
            *value *= Complex32::from_polar(1.0, phase);
        }
        symbol
    }

    /// A frame-shaped buffer: the null symbol, then the samples given.
    fn frame_with(samples: Vec<Complex32>) -> Vec<Complex32> {
        let mut out = vec![Complex32::new(0.0, 0.0); T_NULL];
        out.extend_from_slice(&samples);
        out
    }

    /// The estimated offset has the right sign and size — the single most
    /// dangerous sign in the front end, since a flip doubles the error.
    #[test]
    fn frequency_offset_sign_is_pinned() {
        for injected in [-120.0f64, -40.0, 40.0, 120.0] {
            let mut receiver = DabReceiver::new();
            receiver.pending = frame_with(prs_samples(injected));
            let estimated = receiver.estimate_frequency_offset(0);
            assert!(
                (estimated - injected).abs() < 5.0,
                "injected {injected} Hz, estimated {estimated} Hz"
            );
        }
    }

    /// The PRS score separates DAB from noise by more than an order of
    /// magnitude: that gap is what the false-lock criterion rests on.
    #[test]
    fn prs_metric_separates_signal_from_noise() {
        let mut receiver = DabReceiver::new();
        receiver.pending = frame_with(prs_samples(0.0));
        assert!(receiver.extract_prs_spectrum(0, 0.0));
        let signal_metric = receiver.prs_metric;
        assert!(signal_metric > 0.95, "PRS metric {signal_metric}");

        let mut noise = DabReceiver::new();
        let mut rng: u32 = 0xDEAD_BEEF;
        for _ in 0..(T_NULL + T_G + T_U) {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let i = ((rng >> 16) as i16 as f32) / 32768.0;
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let q = ((rng >> 16) as i16 as f32) / 32768.0;
            noise.pending.push(Complex32::new(i, q));
        }
        assert!(!noise.extract_prs_spectrum(0, 0.0));
        assert!(
            noise.prs_metric < 0.1,
            "noise metric {} should be far below the DAB score",
            noise.prs_metric
        );
    }

    /// An empty buffer decodes nothing and does not panic.
    #[test]
    fn empty_input_is_harmless() {
        let mut receiver = DabReceiver::new();
        assert_eq!(receiver.push_iq(&[]), 0);
        assert_eq!(receiver.push_iq(&[0.0, 0.0]), 0);
        let status = receiver.status();
        assert!(!status.locked);
        assert_eq!(status.frames, 0);
    }
}
