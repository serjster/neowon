//! Synchroniser and slicer (M1, M2, M6): take a linearly modulated signal
//! from raw IQ to decided symbols, bits, EVM and MER.
//!
//! Chain: remove the carrier offset (the `x^M` line), matched-filter with
//! the root-raised-cosine pulse, pick the symbol timing that maximises
//! output energy, remove the phase blindly from ⟨y^M⟩, normalise gain,
//! then refine phase and gain decision-directed and slice to the nearest
//! Gray-labelled point. Blind phase leaves the constellation's rotational
//! ambiguity (π/2 for QPSK and QAM), which EVM does not see but bit
//! mapping does.

use neowon_core::Modulation;
use rustfft::num_complex::Complex64;

use super::cyclo::carrier_offset;

/// Symbols the matched filter spans either side of its centre.
pub const SPAN: i64 = 12;
/// Symbols per timing estimate.
pub const TIMING_BLOCK: usize = 256;

#[derive(Debug, Clone)]
pub struct Recovered {
    /// Decision-point samples, normalised to the constellation's energy.
    pub symbols: Vec<Complex64>,
    /// The Gray labels they sliced to.
    pub labels: Vec<u32>,
    pub carrier_offset_hz: f64,
    /// Timing phase of the first block, symbol periods.
    pub timing: f64,
    pub phase_rad: f64,
    pub evm_rms_pct: f64,
    pub mer_db: f64,
}

/// Root-raised-cosine pulse, unit energy (T = 1).
pub fn rrc(tau: f64, beta: f64) -> f64 {
    use std::f64::consts::PI;
    if tau.abs() < 1e-12 {
        return 1.0 - beta + 4.0 * beta / PI;
    }
    if beta > 0.0 && (tau.abs() - 1.0 / (4.0 * beta)).abs() < 1e-12 {
        let x = PI / (4.0 * beta);
        return beta / 2f64.sqrt() * ((1.0 + 2.0 / PI) * x.sin() + (1.0 - 2.0 / PI) * x.cos());
    }
    let num = (PI * tau * (1.0 - beta)).sin() + 4.0 * beta * tau * (PI * tau * (1.0 + beta)).cos();
    num / (PI * tau * (1.0 - (4.0 * beta * tau).powi(2)))
}

/// Order of the power that strips an M-fold constellation's modulation,
/// and the phase ⟨a^M⟩ has for its ideal points.
fn symmetry(m: Modulation) -> (u32, f64) {
    match m {
        Modulation::Bpsk => (2, 0.0),
        Modulation::Qpsk | Modulation::Qam16 | Modulation::Qam64 => (4, std::f64::consts::PI),
        Modulation::Psk8 => (8, 0.0),
    }
}

/// Least-squares line `y = a + b·x` through `pts`; flat for one point.
fn line_fit(pts: &[(f64, f64)]) -> (f64, f64) {
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
    let (mx, my) = (sx / n, sy / n);
    let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |(a, b), p| {
        (a + (p.0 - mx).powi(2), b + (p.0 - mx) * (p.1 - my))
    });
    if sxx <= 0.0 {
        return (my, 0.0);
    }
    let slope = sxy / sxx;
    (my - slope * mx, slope)
}

/// Linear interpolation of `y` at fractional index `t` (clamped).
fn at(y: &[Complex64], t: f64) -> Complex64 {
    let t = t.clamp(0.0, (y.len() - 1) as f64);
    let i = t.floor() as usize;
    let f = t - i as f64;
    y[i] * (1.0 - f) + y[(i + 1).min(y.len() - 1)] * f
}

/// Recover symbols from `iq` sampled at `rate`, given the symbol rate,
/// modulation and roll-off. `None` if there are too few symbols.
pub fn recover(
    iq: &[f32],
    rate: f64,
    symbol_rate: f64,
    modulation: Modulation,
    rolloff: f64,
) -> Option<Recovered> {
    let sps = rate / symbol_rate;
    let n = iq.len() / 2;
    if sps < 2.0 || (n as f64) < sps * (4 * SPAN) as f64 {
        return None;
    }
    let (m, ideal_phase) = symmetry(modulation);
    let offset = carrier_offset(iq, rate, m)?;

    // Derotate, then matched-filter (taps at the sample rate).
    let x: Vec<Complex64> = iq
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(k, p)| {
            Complex64::new(p[0] as f64, p[1] as f64)
                * Complex64::from_polar(1.0, -std::f64::consts::TAU * offset * k as f64 / rate)
        })
        .collect();
    let half = (SPAN as f64 * sps).ceil() as i64;
    let taps: Vec<f64> = (-half..=half)
        .map(|k| rrc(k as f64 / sps, rolloff) / sps.sqrt())
        .collect();
    let y: Vec<Complex64> = (0..n as i64)
        .map(|k| {
            let mut acc = Complex64::new(0.0, 0.0);
            for (j, &h) in taps.iter().enumerate() {
                let idx = k + j as i64 - half;
                if (0..n as i64).contains(&idx) {
                    acc += x[idx as usize] * h;
                }
            }
            acc
        })
        .collect();

    // Symbols clear of the filter's start-up and tail.
    let first = SPAN as f64 * sps;
    let count = ((n as f64 - 2.0 * first) / sps).floor() as usize - 1;
    let energy_at = |i: usize, phase: f64| at(&y, first + (i as f64 + phase) * sps).norm_sqr();
    // Timing: per block of TIMING_BLOCK symbols, the energy-maximising
    // phase (32 steps, refined by a parabola through the peak), unwrapped
    // across the symbol boundary; then one straight line through all
    // blocks. The line follows the drift an inexact symbol-rate estimate
    // causes, while every block's noise is averaged away by the fit.
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for b in (0..count).step_by(TIMING_BLOCK) {
        let end = (b + TIMING_BLOCK).min(count);
        let e: Vec<f64> = (0..32)
            .map(|k| (b..end).map(|i| energy_at(i, k as f64 / 32.0)).sum())
            .collect();
        let k = (0..32).max_by(|&a, &c| e[a].total_cmp(&e[c]))?;
        let (l, c, r) = (e[(k + 31) % 32], e[k], e[(k + 1) % 32]);
        let d = l - 2.0 * c + r;
        let frac = if d.abs() > 1e-300 {
            0.5 * (l - r) / d
        } else {
            0.0
        };
        let mut phase = (k as f64 + frac) / 32.0;
        if let Some(&(_, prev)) = pts.last() {
            phase += (prev - phase).round(); // unwrap
        }
        pts.push(((b + end) as f64 / 2.0, phase));
    }
    let (a0, drift) = line_fit(&pts);
    let timing = a0.rem_euclid(1.0);
    let mut s: Vec<Complex64> = (0..count)
        .map(|i| at(&y, first + (i as f64 + a0 + drift * i as f64) * sps))
        .collect();

    // Blind phase and gain.
    let mom = s.iter().map(|z| z.powu(m)).sum::<Complex64>();
    let phase = (mom.arg() - ideal_phase) / m as f64;
    let energy = s.iter().map(|z| z.norm_sqr()).sum::<f64>() / s.len() as f64;
    let rot = Complex64::from_polar(1.0 / energy.sqrt(), -phase);
    s.iter_mut().for_each(|z| *z *= rot);

    // Decision-directed refinement: fit a line to the phase error against
    // the decided points and remove it (a residual carrier offset of even a
    // fraction of a hertz leaves a phase ramp no single rotation removes),
    // then a least-squares gain; three rounds.
    let mut total_phase = phase;
    for _ in 0..3 {
        let decided: Vec<Complex64> = s
            .iter()
            .map(|z| {
                let (a, b) = modulation.point(modulation.slice((z.re, z.im)));
                Complex64::new(a, b)
            })
            .collect();
        let errs: Vec<(f64, f64)> = s
            .iter()
            .zip(&decided)
            .enumerate()
            .map(|(i, (z, d))| (i as f64, (z * d.conj()).arg()))
            .collect();
        let (p0, slope) = line_fit(&errs);
        total_phase += p0;
        for (i, z) in s.iter_mut().enumerate() {
            *z *= Complex64::from_polar(1.0, -(p0 + slope * i as f64));
        }
        let (mut num, mut den) = (0.0, 0.0);
        for (z, d) in s.iter().zip(&decided) {
            num += (z * d.conj()).re;
            den += d.norm_sqr();
        }
        let g = num / den;
        s.iter_mut().for_each(|z| *z /= g);
    }

    let labels: Vec<u32> = s.iter().map(|z| modulation.slice((z.re, z.im))).collect();
    let (mut err, mut sig) = (0.0, 0.0);
    for (z, &l) in s.iter().zip(&labels) {
        let (a, b) = modulation.point(l);
        err += (z - Complex64::new(a, b)).norm_sqr();
        sig += a * a + b * b;
    }
    Some(Recovered {
        symbols: s,
        labels,
        carrier_offset_hz: offset,
        timing,
        phase_rad: total_phase,
        evm_rms_pct: 100.0 * (err / sig).sqrt(),
        mer_db: 10.0 * (sig / err.max(1e-300)).log10(),
    })
}
