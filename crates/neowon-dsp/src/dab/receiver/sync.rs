use std::f32::consts::TAU;

use rustfft::num_complex::Complex32;

use super::DabReceiver;
use crate::dab::ofdm::prs_reference;
use crate::dab::{FRAME_SAMPLES, SAMPLE_RATE, T_G, T_NULL, T_U};

/// Stride of the null-symbol search, in samples. The guard interval is 504
/// samples, so an 8-sample grid leaves ample margin.
const NULL_SEARCH_STRIDE: usize = 8;

impl DabReceiver {
    /// Refine a coarse frame start (a null-symbol power dip) to the PRS
    /// correlation peak. Under multipath/SFN the null is shallow and its dip
    /// position jitters by hundreds of samples, while the PRS correlation
    /// peak is stable — a frame grid built on the dip loses lock almost every
    /// frame on air, one built on the peak does not. Coarse then fine, and the
    /// last extraction leaves `prs_spectrum` aligned with the chosen start
    /// (it is the first FIC symbol's differential reference).
    pub(super) fn refine_frame_start(&mut self, coarse: usize) -> (usize, f32) {
        const COARSE_STEP: usize = 32;
        const FINE_STEP: usize = 8;
        const COARSE_SPAN: usize = 1024;
        let offset = self.freq_offset_hz;
        let mut best = (coarse, f32::MIN);
        // The refinement may only move the start later or earlier by
        // `COARSE_SPAN`; a candidate without a whole frame behind it cannot be
        // demapped, and returning one would overrun the sample buffer in the
        // caller's symbol loops.
        let last = self.pending.len().saturating_sub(FRAME_SAMPLES);
        let mut at = coarse.saturating_sub(COARSE_SPAN);
        let hi = (coarse + COARSE_SPAN).min(last);
        while at <= hi {
            let metric = self.extract_prs_spectrum(at, offset);
            if metric > best.1 {
                best = (at, metric);
            }
            at += COARSE_STEP;
        }
        let mut at = best.0.saturating_sub(COARSE_STEP);
        let hi = (best.0 + COARSE_STEP).min(last);
        while at <= hi {
            let metric = self.extract_prs_spectrum(at, offset);
            if metric > best.1 {
                best = (at, metric);
            }
            at += FINE_STEP;
        }
        let metric = self.extract_prs_spectrum(best.0, offset);
        (best.0, metric)
    }

    /// Position of the deepest sustained-power dip in the first frame period:
    /// the null symbol, assuming a frame starts somewhere in the buffer.
    pub(super) fn find_null_symbol(&mut self) -> usize {
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
    /// sequence, storing it for the first FIC symbol's demap. Returns the
    /// normalized correlation.
    ///
    /// A frame start without a full `T_NULL + T_G + T_U` behind it scores zero:
    /// the refine scan can propose starts near the end of the buffered samples,
    /// and "not enough samples to look" is not a signal quality.
    pub(super) fn extract_prs_spectrum(&mut self, frame_start: usize, offset_hz: f64) -> f32 {
        let start = frame_start + T_NULL + T_G;
        if start + T_U > self.pending.len() {
            return 0.0;
        }
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
        let metric = if denominator > 0.0 {
            correlation.norm() / denominator
        } else {
            0.0
        };
        self.prs_spectrum.copy_from_slice(&window);
        self.spectrum = window;
        metric
    }

    /// Fill `window` with `T_U` samples at `start`, de-rotated by the estimated
    /// carrier offset on the frame's time base.
    ///
    /// `base` is the window's offset from the frame start, in samples: the
    /// phasor must run *continuously* across symbols, because the thing being
    /// undone is a rotation accumulating in time.
    pub(super) fn corrected_window(
        &self,
        start: usize,
        base: f64,
        offset_hz: f64,
        window: &mut [Complex32],
    ) {
        let step = -f64::from(TAU) * offset_hz / SAMPLE_RATE;
        for (i, slot) in window.iter_mut().enumerate() {
            let phase = (step * (base + i as f64)) as f32;
            *slot = self.pending[start + i] * Complex32::from_polar(1.0, phase);
        }
    }

    /// Carrier offset in Hz from the PRS symbol's cyclic prefix, or `None`
    /// when the guard correlation is too weak to trust (a faded frame, or no
    /// signal). It runs on **every** attempt, not only accepted frames: the
    /// dongle's LO drifts as it warms, and an offset that is only refreshed by
    /// accepted frames strands the receiver the moment acceptance stops.
    ///
    /// The guard interval repeats the symbol's last `T_G` samples one useful
    /// period earlier, so the correlation of the two copies carries
    /// `arg = -2·pi·df·Tu/fs` — the sign is checked by
    /// `frequency_offset_sign_is_pinned`, because getting it backwards doubles
    /// the error instead of removing it.
    pub(super) fn estimate_frequency_offset(&self, frame_start: usize) -> Option<f64> {
        let start = frame_start + T_NULL;
        if start + T_U + T_G > self.pending.len() {
            return None;
        }
        let mut accumulator = Complex32::new(0.0, 0.0);
        let mut energy = 0.0f32;
        for m in 0..T_G {
            let guard = self.pending[start + m];
            let copy = self.pending[start + T_U + m];
            accumulator += guard * copy.conj();
            energy += guard.norm_sqr();
        }
        if energy <= f32::MIN_POSITIVE || accumulator.norm() < 0.3 * energy {
            return None;
        }
        Some(-f64::from(accumulator.arg()) * SAMPLE_RATE / (f64::from(TAU) * T_U as f64))
    }
}
