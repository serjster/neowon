//! Cyclostationary estimates: spectral lines that a linearly
//! modulated signal's nonlinear transforms carry. `|x|²` of a pulse-shaped
//! signal has a line at the symbol rate (for roll-off > 0); `x^M` of an
//! M-fold symmetric constellation has one at M × the carrier offset.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

/// Frequency of the strongest spectral line of `z` (sampled at `rate`)
/// whose frequency passes `keep`, refined by a parabola through the
/// log-magnitude peak. The transform length is the next power of two,
/// zero-padded.
fn strongest_line(mut z: Vec<Complex64>, rate: f64, keep: impl Fn(f64) -> bool) -> Option<f64> {
    let n = z.len().next_power_of_two();
    z.resize(n, Complex64::new(0.0, 0.0));
    FftPlanner::new().plan_fft_forward(n).process(&mut z);
    let freq = |k: usize| {
        let k = if k >= n / 2 {
            k as f64 - n as f64
        } else {
            k as f64
        };
        k * rate / n as f64
    };
    let k = (0..n)
        .filter(|&k| keep(freq(k)))
        .max_by(|&a, &b| z[a].norm().total_cmp(&z[b].norm()))?;
    let mag = |k: usize| z[k % n].norm().max(1e-300).ln();
    let (a, b, c) = (mag(k + n - 1), mag(k), mag(k + 1));
    let d = a - 2.0 * b + c;
    let frac = if d.abs() > 1e-300 {
        0.5 * (a - c) / d
    } else {
        0.0
    };
    Some(freq(k) + frac * rate / n as f64)
}

fn complex(iq: &[f32]) -> impl Iterator<Item = Complex64> + '_ {
    iq.as_chunks::<2>()
        .0
        .iter()
        .map(|p| Complex64::new(p[0] as f64, p[1] as f64))
}

/// Symbol rate from the `|x|²` line between `min_hz` and `max_hz`.
pub fn symbol_rate(iq: &[f32], rate: f64, min_hz: f64, max_hz: f64) -> Option<f64> {
    let env: Vec<f64> = complex(iq).map(|x| x.norm_sqr()).collect();
    let mean = env.iter().sum::<f64>() / env.len().max(1) as f64;
    let z = env.iter().map(|&e| Complex64::new(e - mean, 0.0)).collect();
    strongest_line(z, rate, |f| (min_hz..=max_hz).contains(&f))
}

/// Carrier offset from the `x^M` line (unambiguous within ±rate / 2M).
pub fn carrier_offset(iq: &[f32], rate: f64, m: u32) -> Option<f64> {
    let z = complex(iq).map(|x| x.powu(m)).collect();
    Some(strongest_line(z, rate, |_| true)? / m as f64)
}
