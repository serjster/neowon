//! Channel selection: move one signal to baseband and low-pass it, so the
//! lab's estimators see it alone (another emitter's x^M line would
//! otherwise win the carrier estimate).

use rustfft::num_complex::Complex64;

/// Shift `iq` by `-offset_hz` and low-pass it to `cutoff_hz` (one-sided)
/// with a Hamming-windowed sinc of `taps` taps (odd). Output has the input
/// length; the filter's group delay is compensated.
pub fn select(iq: &[f32], rate: f64, offset_hz: f64, cutoff_hz: f64, taps: usize) -> Vec<f32> {
    let taps = taps | 1;
    let half = (taps / 2) as i64;
    let fc = (cutoff_hz / rate).clamp(1e-6, 0.5);
    let h: Vec<f64> = (-half..=half)
        .map(|k| {
            let x = k as f64;
            let sinc = if k == 0 {
                2.0 * fc
            } else {
                (std::f64::consts::TAU * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.54 + 0.46 * (std::f64::consts::PI * x / half as f64).cos();
            sinc * w
        })
        .collect();
    let gain: f64 = h.iter().sum();
    let x: Vec<Complex64> = iq
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(k, p)| {
            Complex64::new(p[0] as f64, p[1] as f64)
                * Complex64::from_polar(1.0, -std::f64::consts::TAU * offset_hz * k as f64 / rate)
        })
        .collect();
    let n = x.len() as i64;
    let mut out = Vec::with_capacity(iq.len());
    for k in 0..n {
        let mut acc = Complex64::new(0.0, 0.0);
        for (j, &c) in h.iter().enumerate() {
            let i = k + j as i64 - half;
            if (0..n).contains(&i) {
                acc += x[i as usize] * c;
            }
        }
        acc /= gain;
        out.push(acc.re as f32);
        out.push(acc.im as f32);
    }
    out
}
