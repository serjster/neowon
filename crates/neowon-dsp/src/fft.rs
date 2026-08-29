//! Spectrum analysis: windowed FFT with amplitude-correct scaling.
//! CPU implementation via rustfft — the correctness oracle for the GPU FFT
//! that arrives with the waterfall view.

use rustfft::num_complex::Complex64;
use rustfft::FftPlanner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Rectangle,
    Hamming,
    Hann,
    Blackman,
    Flattop,
    Triangular,
}

impl Window {
    pub const ALL: [Window; 6] = [
        Window::Rectangle,
        Window::Hamming,
        Window::Hann,
        Window::Blackman,
        Window::Flattop,
        Window::Triangular,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Window::Rectangle => "Rectangle",
            Window::Hamming => "Hamming",
            Window::Hann => "Hann",
            Window::Blackman => "Blackman",
            Window::Flattop => "Flattop",
            Window::Triangular => "Triangular",
        }
    }

    fn coeff(&self, i: usize, n: usize) -> f64 {
        let x = i as f64 / (n - 1) as f64;
        let tau = std::f64::consts::TAU;
        match self {
            Window::Rectangle => 1.0,
            Window::Hamming => 0.54 - 0.46 * (tau * x).cos(),
            Window::Hann => 0.5 - 0.5 * (tau * x).cos(),
            Window::Blackman => 0.42 - 0.5 * (tau * x).cos() + 0.08 * (2.0 * tau * x).cos(),
            Window::Flattop => {
                0.21557895 - 0.41663158 * (tau * x).cos() + 0.277263158 * (2.0 * tau * x).cos()
                    - 0.083578947 * (3.0 * tau * x).cos()
                    + 0.006947368 * (4.0 * tau * x).cos()
            }
            Window::Triangular => 1.0 - (2.0 * x - 1.0).abs(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Spectrum {
    /// Frequency of bin i is `i * bin_hz`.
    pub bin_hz: f64,
    /// Peak amplitude per bin, volts.
    pub amplitude: Vec<f64>,
}

impl Spectrum {
    /// dBV of a bin (20·log10 of the amplitude), floored at -120.
    pub fn dbv(&self, i: usize) -> f64 {
        (20.0 * self.amplitude[i].max(1e-12).log10()).max(-120.0)
    }

    /// (frequency, amplitude) of the strongest non-DC bin.
    pub fn peak(&self) -> Option<(f64, f64)> {
        let (i, &a) = self
            .amplitude
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))?;
        Some((i as f64 * self.bin_hz, a))
    }
}

/// Windowed amplitude spectrum of up to `size` samples (power of two).
/// Coherent-gain corrected so a sine of amplitude A reads A at its bin.
pub fn spectrum(
    raw: &[i8],
    volts_per_lsb: f64,
    sample_rate: f64,
    window: Window,
    size: usize,
) -> Option<Spectrum> {
    let n = size.min(raw.len()).next_power_of_two() >> 1;
    let n = n.min(raw.len()).max(64);
    if raw.len() < n || sample_rate <= 0.0 {
        return None;
    }
    // Coherent gain: sum(w)/n, corrects the window's amplitude loss.
    let mut cg = 0.0;
    let mut buf: Vec<Complex64> = (0..n)
        .map(|i| {
            let w = window.coeff(i, n);
            cg += w;
            Complex64::new(raw[i] as f64 * volts_per_lsb * w, 0.0)
        })
        .collect();
    cg /= n as f64;

    FftPlanner::new().plan_fft_forward(n).process(&mut buf);

    let scale = 2.0 / (n as f64 * cg);
    let amplitude = buf[..n / 2]
        .iter()
        .enumerate()
        .map(|(i, c)| c.norm() * if i == 0 { 0.5 * scale } else { scale })
        .collect();
    Some(Spectrum { bin_hz: sample_rate / n as f64, amplitude })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, cycles: f64, amp: f64) -> Vec<i8> {
        (0..n)
            .map(|i| {
                (amp * (i as f64 / n as f64 * cycles * std::f64::consts::TAU).sin()) as i8
            })
            .collect()
    }

    #[test]
    fn sine_peak_bin_and_amplitude() {
        // 4096 samples, exactly 64 cycles -> bin 64, amplitude 100 LSB.
        let raw = sine(4096, 64.0, 100.0);
        for window in Window::ALL {
            let s = spectrum(&raw, 0.01, 4096.0, window, 4096).unwrap();
            let (f, a) = s.peak().unwrap();
            assert!((f - 64.0).abs() < 1.5, "{window:?}: peak at {f}");
            // 100 LSB * 0.01 V = 1.0 V amplitude, +-10% (quantization).
            assert!((a - 1.0).abs() < 0.1, "{window:?}: amplitude {a}");
        }
    }

    #[test]
    fn dc_reads_correctly() {
        let raw = vec![50i8; 4096];
        let s = spectrum(&raw, 0.01, 4096.0, Window::Rectangle, 4096).unwrap();
        assert!((s.amplitude[0] - 0.5).abs() < 0.01, "dc {}", s.amplitude[0]);
    }
}
