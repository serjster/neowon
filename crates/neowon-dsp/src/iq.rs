//! Spectrum of complex (IQ) samples: the oracle for the SDR views.
//!
//! Input is interleaved I, Q in full-scale units (|I|, |Q| <= 1 is the ADC
//! range). Output is DC-centred power in dBFS per bin, scaled so a complex
//! tone of amplitude A reads 20·log10(A) at its bin (coherent-gain
//! corrected, like `fft::spectrum`). Blocks are averaged in power (Welch,
//! no overlap) so a longer capture lowers the variance, not the resolution.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

use crate::fft::Window;

/// Floor for empty or silent bins, dBFS.
pub const FLOOR_DB: f64 = -200.0;

#[derive(Debug, Clone)]
pub struct IqSpectrum {
    /// Bin width, Hz.
    pub bin_hz: f64,
    /// Power per bin, dBFS. Bin `k` sits at `(k - n/2) * bin_hz` from the
    /// tuned centre; DC is bin `n/2`.
    pub power_db: Vec<f64>,
    /// Blocks averaged.
    pub blocks: usize,
}

impl IqSpectrum {
    pub fn len(&self) -> usize {
        self.power_db.len()
    }

    pub fn is_empty(&self) -> bool {
        self.power_db.is_empty()
    }

    /// Offset of bin `k` from the tuned centre, Hz.
    pub fn offset_hz(&self, k: usize) -> f64 {
        (k as f64 - (self.len() / 2) as f64) * self.bin_hz
    }

    /// Bin nearest to `offset_hz`, clamped to the spectrum.
    pub fn bin_of(&self, offset_hz: f64) -> usize {
        let k = (offset_hz / self.bin_hz).round() + (self.len() / 2) as f64;
        (k.max(0.0) as usize).min(self.len().saturating_sub(1))
    }

    /// Median bin power, dBFS: a robust noise-floor estimate while signals
    /// occupy less than half the band.
    pub fn median_db(&self) -> f64 {
        let mut v = self.power_db.clone();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(FLOOR_DB)
    }

    /// Strongest bin, skipping `guard` bins either side of DC (the RTL2832's
    /// DC spike): `(offset_hz, dBFS)`.
    pub fn peak(&self, guard: usize) -> Option<(f64, f64)> {
        let dc = self.len() / 2;
        self.power_db
            .iter()
            .enumerate()
            .filter(|(k, _)| k.abs_diff(dc) > guard)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, &p)| (self.offset_hz(k), p))
    }
}

/// Averaged power spectrum over every whole `n`-pair block of `iq`
/// (`n` a power of two). `None` if there is not one block.
pub fn iq_spectrum(iq: &[f32], sample_rate: f64, window: Window, n: usize) -> Option<IqSpectrum> {
    welch(iq, sample_rate, window, n, n)
}

/// Welch average over blocks of `n` pairs starting every `hop` pairs
/// (`hop = n / 2` is the usual 50% overlap: with a tapered window, no part
/// of the signal then sits only at block edges).
pub fn welch(
    iq: &[f32],
    sample_rate: f64,
    window: Window,
    n: usize,
    hop: usize,
) -> Option<IqSpectrum> {
    average(&stft(iq, window, n, hop)?, sample_rate)
}

/// Linear power spectra of the blocks of `n` pairs starting every `hop`
/// pairs, DC-centred and scaled like `iq_spectrum` (a tone of amplitude A
/// has power A² at its bin). `None` if there is not one block.
pub fn stft(iq: &[f32], window: Window, n: usize, hop: usize) -> Option<Vec<Vec<f64>>> {
    if !n.is_power_of_two() || n < 2 || hop == 0 || iq.len() < 2 * n {
        return None;
    }
    let win: Vec<f64> = (0..n).map(|i| window.coeff(i, n)).collect();
    let cg = win.iter().sum::<f64>() / n as f64;
    let scale = 1.0 / (n as f64 * cg).powi(2);
    let fft = FftPlanner::new().plan_fft_forward(n);
    let mut buf = vec![Complex64::default(); n];
    let starts = (0..)
        .map(|b| 2 * b * hop)
        .take_while(|&s| s + 2 * n <= iq.len());
    Some(
        starts
            .map(|s| {
                let block = &iq[s..s + 2 * n];
                for (i, (b, w)) in buf.iter_mut().zip(&win).enumerate() {
                    *b = Complex64::new(block[2 * i] as f64 * w, block[2 * i + 1] as f64 * w);
                }
                fft.process(&mut buf);
                let mut p = vec![0.0; n];
                for (k, c) in buf.iter().enumerate() {
                    // Rotate so DC lands at n/2.
                    p[(k + n / 2) % n] = c.norm_sqr() * scale;
                }
                p
            })
            .collect(),
    )
}

/// Mean of linear block spectra (from `stft`) as a dBFS spectrum.
pub fn average(blocks: &[Vec<f64>], sample_rate: f64) -> Option<IqSpectrum> {
    let n = blocks.first()?.len();
    if sample_rate <= 0.0 {
        return None;
    }
    let power_db = (0..n)
        .map(|k| {
            let p = blocks.iter().map(|b| b[k]).sum::<f64>() / blocks.len() as f64;
            if p > 0.0 {
                (10.0 * p.log10()).max(FLOOR_DB)
            } else {
                FLOOR_DB
            }
        })
        .collect();
    Some(IqSpectrum {
        bin_hz: sample_rate / n as f64,
        power_db,
        blocks: blocks.len(),
    })
}

#[cfg(test)]
mod tests {
    use neowon_sim::{IqComponent, IqScene};

    use super::*;

    fn tone(offset_hz: f64, amplitude: f64, noise_rms: f64) -> IqScene {
        IqScene {
            sample_rate: 2.048e6,
            components: vec![IqComponent::Tone {
                offset_hz,
                amplitude,
                phase: 0.0,
            }],
            noise_rms,
        }
    }

    #[test]
    fn tone_reads_its_amplitude_at_its_bin() {
        // 100 kHz is exactly bin 200 of 4096 at 2.048 MS/s.
        let scene = tone(100e3, 0.5, 0.0);
        let s = iq_spectrum(
            &scene.samples(1, 0, 8192),
            scene.sample_rate,
            Window::Hann,
            4096,
        )
        .unwrap();
        let (hz, db) = s.peak(4).unwrap();
        assert_eq!(hz, 100e3);
        assert!((db - 20.0 * 0.5f64.log10()).abs() < 0.01, "{db}");
        assert_eq!(s.blocks, 2);
    }

    #[test]
    fn negative_offsets_land_below_dc() {
        let scene = tone(-250e3, 0.25, 0.0);
        let s = iq_spectrum(
            &scene.samples(1, 0, 4096),
            scene.sample_rate,
            Window::Hann,
            4096,
        )
        .unwrap();
        let (hz, _) = s.peak(4).unwrap();
        assert!((hz + 250e3).abs() <= s.bin_hz / 2.0, "{hz}");
        assert_eq!(s.bin_of(-250e3), s.len() / 2 - 500);
    }

    #[test]
    fn noise_floor_follows_noise_power_per_bin() {
        // Complex white noise of total power P spreads P / n per bin; the
        // Hann window's noise bandwidth (1.5 bins) raises each bin by
        // 1.76 dB. Averaged over 64 blocks, a bin's power is close to
        // Gaussian, so its median is its mean (a single block would read
        // 1.59 dB low: the median of an exponential).
        let scene = tone(0.0, 0.0, 0.1);
        let n = 1024;
        let s = iq_spectrum(
            &scene.samples(3, 0, 64 * n),
            scene.sample_rate,
            Window::Hann,
            n,
        )
        .unwrap();
        let expect = 10.0 * (0.01f64 / n as f64).log10() + 1.76;
        assert!(
            (s.median_db() - expect).abs() < 0.3,
            "{} vs {expect}",
            s.median_db()
        );
    }

    #[test]
    fn short_input_is_refused() {
        assert!(iq_spectrum(&[0.0; 10], 1.0, Window::Hann, 8).is_none());
        assert!(iq_spectrum(&[0.0; 100], 1.0, Window::Hann, 12).is_none());
    }
}
