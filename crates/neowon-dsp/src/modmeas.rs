//! Signal measurements over a band of a spectrum, or over IQ samples
//! (Phase 10.1 subset): occupied bandwidth, channel power, SNR, spectral
//! flatness, and instantaneous amplitude / phase / frequency.

use crate::fft::Window;
use crate::iq::IqSpectrum;

/// Equivalent noise bandwidth of `window` over `n` points, in bins: what a
/// sum of bin powers over-counts a signal's power by (Hann: 1.5).
pub fn enbw_bins(window: Window, n: usize) -> f64 {
    let (mut s1, mut s2) = (0.0, 0.0);
    for i in 0..n {
        let w = window.coeff(i, n);
        s1 += w;
        s2 += w * w;
    }
    n as f64 * s2 / (s1 * s1)
}

fn lin(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}

/// Fractional bin edges `(lo, hi)` containing `fraction` of the power in
/// `power` (linear), the rest split equally outside each edge — the ITU
/// occupied-bandwidth definition. Edges are in bin units from the slice's
/// start: bin k spans `[k, k + 1)`.
pub fn occupied_band(power: &[f64], fraction: f64) -> (f64, f64) {
    let total: f64 = power.iter().sum();
    if total <= 0.0 {
        return (0.0, power.len() as f64);
    }
    let tail = total * (1.0 - fraction) / 2.0;
    let edge = |it: &mut dyn Iterator<Item = (usize, f64)>| -> f64 {
        let mut acc = 0.0;
        for (k, p) in it {
            if acc + p > tail {
                return k as f64 + (tail - acc) / p;
            }
            acc += p;
        }
        0.0
    };
    let lo = edge(&mut power.iter().copied().enumerate());
    let hi = power.len() as f64 - edge(&mut power.iter().rev().copied().enumerate());
    (lo, hi)
}

/// A band of a spectrum by bin range `[a, b]` inclusive.
#[derive(Debug, Clone, Copy)]
pub struct Band {
    pub a: usize,
    pub b: usize,
}

impl Band {
    /// Bins covering `lo_hz..=hi_hz` (offsets from the tuned centre).
    pub fn of(s: &IqSpectrum, lo_hz: f64, hi_hz: f64) -> Self {
        Self {
            a: s.bin_of(lo_hz),
            b: s.bin_of(hi_hz),
        }
    }

    fn lin<'a>(&self, s: &'a IqSpectrum) -> impl Iterator<Item = f64> + 'a {
        s.power_db[self.a..=self.b].iter().map(|&d| lin(d))
    }
}

/// Integrated power in the band, dBFS (window noise bandwidth removed, so
/// a complex tone of amplitude A reads 20·log10(A)).
pub fn channel_power_dbfs(s: &IqSpectrum, band: Band, window: Window) -> f64 {
    let p: f64 = band.lin(s).sum();
    10.0 * (p / enbw_bins(window, s.len())).max(1e-30).log10()
}

/// Band power over the noise integrated across the same bins, dB, given a
/// per-bin floor (dBFS) such as `detect::floor`.
pub fn snr_db(s: &IqSpectrum, band: Band, floor_db: &[f64]) -> f64 {
    let sig: f64 = band.lin(s).sum();
    let noise: f64 = floor_db[band.a..=band.b].iter().map(|&d| lin(d)).sum();
    10.0 * (sig.max(1e-30) / noise.max(1e-30)).log10()
}

/// Spectral flatness over the band: geometric over arithmetic mean of
/// the bin powers, 0 (a tone) to 1 (flat).
pub fn flatness(s: &IqSpectrum, band: Band) -> f64 {
    let v: Vec<f64> = band.lin(s).collect();
    let n = v.len() as f64;
    let arith = v.iter().sum::<f64>() / n;
    let geo = (v.iter().map(|p| p.max(1e-30).ln()).sum::<f64>() / n).exp();
    if arith > 0.0 { geo / arith } else { 0.0 }
}

/// Instantaneous amplitude, phase (radians) and frequency (Hz) of
/// interleaved IQ. Frequency is the phase step between consecutive
/// samples, so it has one fewer point.
pub fn instantaneous(iq: &[f32], rate: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let z: Vec<(f64, f64)> = iq
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| (p[0] as f64, p[1] as f64))
        .collect();
    let amp = z.iter().map(|(i, q)| (i * i + q * q).sqrt()).collect();
    let phase = z.iter().map(|(i, q)| q.atan2(*i)).collect();
    let freq = z
        .windows(2)
        .map(|w| {
            let ((a, b), (c, d)) = (w[0], w[1]);
            // arg(z[n] · conj(z[n-1])).
            (d * a - c * b).atan2(c * a + d * b) * rate / std::f64::consts::TAU
        })
        .collect();
    (amp, phase, freq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iq::iq_spectrum;
    use neowon_sim::{IqComponent, IqScene};

    fn tone(amplitude: f64, noise: f64) -> IqScene {
        IqScene {
            sample_rate: 8192.0,
            components: vec![IqComponent::Tone {
                offset_hz: 1000.0,
                amplitude,
                phase: 0.0,
            }],
            noise_rms: noise,
        }
    }

    #[test]
    fn hann_noise_bandwidth_is_one_and_a_half_bins() {
        assert!((enbw_bins(Window::Hann, 4096) - 1.5).abs() < 1e-3);
        assert!((enbw_bins(Window::Rectangle, 64) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn occupied_band_of_a_flat_run_trims_half_a_percent_each_side() {
        let (lo, hi) = occupied_band(&[1.0; 100], 0.99);
        assert!(
            (lo - 0.5).abs() < 1e-9 && (hi - 99.5).abs() < 1e-9,
            "{lo} {hi}"
        );
    }

    #[test]
    fn channel_power_reads_the_tone_amplitude() {
        let s = tone(0.5, 0.0);
        let spec = iq_spectrum(&s.samples(1, 0, 8192), 8192.0, Window::Hann, 1024).unwrap();
        let band = Band::of(&spec, 900.0, 1100.0);
        let p = channel_power_dbfs(&spec, band, Window::Hann);
        assert!((p - 20.0 * 0.5f64.log10()).abs() < 0.05, "{p}");
        assert!(flatness(&spec, band) < 0.1);
    }

    #[test]
    fn noise_is_flat() {
        let s = tone(0.0, 0.1);
        let spec = iq_spectrum(&s.samples(3, 0, 65536), 8192.0, Window::Hann, 256).unwrap();
        // 256 blocks averaged: bins vary by ~6%, so flatness is near 1.
        let f = flatness(&spec, Band { a: 10, b: 245 });
        assert!(f > 0.99, "{f}");
    }

    #[test]
    fn instantaneous_frequency_of_a_tone() {
        let (amp, _, freq) = instantaneous(&tone(0.5, 0.0).samples(1, 0, 256), 8192.0);
        assert!(amp.iter().all(|a| (a - 0.5).abs() < 1e-6));
        assert!(freq.iter().all(|f| (f - 1000.0).abs() < 0.01));
    }
}
