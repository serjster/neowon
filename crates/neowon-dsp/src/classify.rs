//! The DSP classifier (Phase 10.5, C1/C3): what kind of signal an
//! isolated channel holds — noise, a CW carrier, AM, FM, or a linearly
//! modulated digital signal and its constellation — with a confidence, the
//! runner-up and margin, an explicit `unknown`, and a trust state.
//!
//! Features: in-band SNR, the fraction of power in the strongest spectral
//! line (a carrier), envelope variation, instantaneous-frequency spread,
//! and the strength of the `|x|²` symbol-rate line (a digital signal's
//! mark). Soft thresholds on them score the families; a digital score is
//! weighted per constellation by the absolute fit of noise-corrected
//! cumulants (C42, |C40|). A constant "none of these" score competes, so a
//! signal no family fits is `unknown` rather than the least-bad guess (a
//! real broadcast FM station, cut by a too-narrow channel, once read as
//! 64QAM at 0.95 without it).
//!
//! Trust is `Unproven` for every class: the classifier is tested on the
//! simulator only, and D9 asks for an over-the-air, held-out-frequency and
//! cross-day evaluation before anything is `Validated`.

use neowon_core::Modulation;

use crate::fft::Window;
use crate::iq::iq_spectrum;
use crate::modlab::{cumulants, recover, select, symbol_rate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Noise,
    Cw,
    Am,
    Fm,
    Digital(Modulation),
}

impl Class {
    /// Preset labels (C7): the stable names results and scripts use.
    pub fn label(self) -> &'static str {
        match self {
            Class::Noise => "noise",
            Class::Cw => "cw",
            Class::Am => "am",
            Class::Fm => "fm",
            Class::Digital(Modulation::Bpsk) => "bpsk",
            Class::Digital(Modulation::Qpsk) => "qpsk",
            Class::Digital(Modulation::Psk8) => "8psk",
            Class::Digital(Modulation::Qam16) => "16qam",
            Class::Digital(Modulation::Qam64) => "64qam",
        }
    }

    pub const ALL: [Class; 9] = [
        Class::Noise,
        Class::Cw,
        Class::Am,
        Class::Fm,
        Class::Digital(Modulation::Bpsk),
        Class::Digital(Modulation::Qpsk),
        Class::Digital(Modulation::Psk8),
        Class::Digital(Modulation::Qam16),
        Class::Digital(Modulation::Qam64),
    ];
}

/// How far a result can be relied on (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Met the D9 floor on an over-the-air held-out evaluation.
    Validated,
    /// Not yet evaluated that way.
    Unproven,
    /// No evaluation could establish it (e.g. `unknown` itself).
    Unprovable,
}

impl Trust {
    pub fn label(self) -> &'static str {
        match self {
            Trust::Validated => "validated",
            Trust::Unproven => "unproven",
            Trust::Unprovable => "unprovable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    pub snr_db: f64,
    /// Power in the strongest line (±1 bin) over total in-band power.
    pub carrier_fraction: f64,
    /// Standard deviation of |x| over its mean.
    pub envelope_cv: f64,
    /// Standard deviation of the instantaneous frequency over the
    /// occupied bandwidth.
    pub freq_spread: f64,
    /// The `|x|²` symbol-rate line over the median of its search band, dB.
    pub cyclic_line_db: f64,
    pub symbol_rate_hz: Option<f64>,
    /// Noise-corrected cumulants of the recovered symbols (digital only).
    pub c42: f64,
    pub c40_abs: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub class: Class,
    /// Normalised score of the winner, 0–1.
    pub confidence: f64,
    pub runner_up: Class,
    pub margin: f64,
    /// Too weak or too close a call to name.
    pub unknown: bool,
    pub trust: Trust,
    pub features: Features,
}

/// Below this winning score, or this margin, the answer is `unknown`.
pub const MIN_CONFIDENCE: f64 = 0.5;
pub const MIN_MARGIN: f64 = 0.1;
/// Width of a constellation's fit in (C42, |C40|).
pub const FIT_SIGMA: f64 = 0.15;
/// The "none of these" score (families score up to 1).
pub const OTHER_SCORE: f64 = 0.15;

fn hi(x: f64, t: f64, w: f64) -> f64 {
    1.0 / (1.0 + (-(x - t) / w).exp())
}

/// Features of the signal at `centre_hz` (offset from the tuned centre)
/// occupying about `obw_hz` in `iq`. SNR and the carrier line come from
/// the whole capture's spectrum (the floor is read outside the band);
/// everything else from the selected channel, so out-of-band noise does
/// not swamp the envelope and frequency statistics.
pub fn features(iq: &[f32], rate: f64, centre_hz: f64, obw_hz: f64) -> Option<Features> {
    let n = iq.len() / 2;
    if n < 4096 {
        return None;
    }
    let nfft = 1024;
    let spec = iq_spectrum(iq, rate, Window::Hann, nfft)?;
    let lin: Vec<f64> = spec.power_db.iter().map(|d| 10f64.powf(d / 10.0)).collect();
    // In-band: the occupied band (at least a few bins); floor: the median
    // outside it.
    let half = (obw_hz / 2.0).max(3.0 * spec.bin_hz);
    let inside = |k: usize| (spec.offset_hz(k) - centre_hz).abs() <= half;
    let mut out: Vec<f64> = (0..nfft).filter(|&k| !inside(k)).map(|k| lin[k]).collect();
    out.sort_by(f64::total_cmp);
    let floor = out.get(out.len() / 2).copied().unwrap_or(1e-30).max(1e-30);
    let band: Vec<usize> = (0..nfft).filter(|&k| inside(k)).collect();
    let total: f64 = band.iter().map(|&k| lin[k]).sum();
    let noise = floor * band.len() as f64;
    let snr_db = 10.0 * ((total - noise).max(1e-30) / noise).log10();
    let peak = band
        .iter()
        .copied()
        .max_by(|&a, &b| lin[a].total_cmp(&lin[b]))?;
    let line: f64 = (peak.saturating_sub(1)..=(peak + 1).min(nfft - 1))
        .map(|k| lin[k])
        .sum();
    let carrier_fraction = ((line - 3.0 * floor) / (total - noise).max(1e-30)).clamp(0.0, 1.0);

    // The channel: at least 5 kHz, so a carrier's jitter and a narrow
    // modulation stay inside it.
    let cutoff = (0.8 * obw_hz).max(5e3);
    let taps = ((8.0 * rate / cutoff) as usize).clamp(65, 401);
    let chan = select(iq, rate, centre_hz, cutoff, taps);
    let iq = &chan[..];
    let env: Vec<f64> = iq
        .chunks_exact(2)
        .map(|p| (p[0] as f64).hypot(p[1] as f64))
        .collect();
    let mean = env.iter().sum::<f64>() / n as f64;
    let var = env.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n as f64;
    let envelope_cv = var.sqrt() / mean.max(1e-30);

    let freq: Vec<f64> = iq
        .chunks_exact(2)
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| {
            let (a, b) = (
                (w[0][0] as f64, w[0][1] as f64),
                (w[1][0] as f64, w[1][1] as f64),
            );
            (b.1 * a.0 - b.0 * a.1).atan2(b.0 * a.0 + b.1 * a.1) * rate / std::f64::consts::TAU
        })
        .collect();
    let fm = freq.iter().sum::<f64>() / freq.len() as f64;
    let fstd = (freq.iter().map(|f| (f - fm).powi(2)).sum::<f64>() / freq.len() as f64).sqrt();
    let freq_spread = fstd / obw_hz.max(spec.bin_hz);

    // The |x|² line near the rate an RRC signal of this width would have.
    let (lo, hi_hz) = (0.4 * obw_hz, 1.2 * obw_hz);
    let rs = symbol_rate(iq, rate, lo, hi_hz);
    let cyclic_line_db = rs.map_or(0.0, |rs| line_strength(iq, rate, rs, lo, hi_hz));

    let (mut c42, mut c40_abs) = (0.0, 0.0);
    if let Some(rs) = rs
        && let Some(r) = recover(iq, rate, rs, Modulation::Qpsk, 0.35)
    {
        let syms: Vec<f32> = r
            .symbols
            .iter()
            .flat_map(|z| [z.re as f32, z.im as f32])
            .collect();
        if let Some(c) = cumulants(&syms) {
            // Noise shrinks normalised fourth-order cumulants by
            // (S / (S + N))²; undo it with the measured SNR.
            let s = 10f64.powf(snr_db / 10.0);
            let k = ((1.0 + s) / s.max(1e-9)).powi(2).min(4.0);
            c42 = c.c42 * k;
            c40_abs = c.c40.norm() * k;
        }
    }
    Some(Features {
        snr_db,
        carrier_fraction,
        envelope_cv,
        freq_spread,
        cyclic_line_db,
        symbol_rate_hz: rs,
        c42,
        c40_abs,
    })
}

/// The `|x|²` spectrum at `rs` over its median in `lo..hi`, dB.
fn line_strength(iq: &[f32], rate: f64, rs: f64, lo: f64, hi: f64) -> f64 {
    use rustfft::FftPlanner;
    use rustfft::num_complex::Complex64;
    let env: Vec<f64> = iq
        .chunks_exact(2)
        .map(|p| (p[0] as f64).powi(2) + (p[1] as f64).powi(2))
        .collect();
    let m = env.iter().sum::<f64>() / env.len() as f64;
    let n = env.len().next_power_of_two();
    let mut z: Vec<Complex64> = env.iter().map(|e| Complex64::new(e - m, 0.0)).collect();
    z.resize(n, Complex64::new(0.0, 0.0));
    FftPlanner::new().plan_fft_forward(n).process(&mut z);
    let bin = |f: f64| ((f * n as f64 / rate).round() as usize).min(n / 2);
    let mut band: Vec<f64> = (bin(lo)..=bin(hi)).map(|k| z[k].norm_sqr()).collect();
    let at = (bin(rs).saturating_sub(2)..=bin(rs) + 2)
        .map(|k| z[k.min(n - 1)].norm_sqr())
        .fold(0.0, f64::max);
    band.sort_by(f64::total_cmp);
    let med = band
        .get(band.len() / 2)
        .copied()
        .unwrap_or(1e-30)
        .max(1e-30);
    10.0 * (at / med).log10()
}

/// Ideal (C42, |C40|) per constellation (Swami & Sadler).
fn ideal(m: Modulation) -> (f64, f64) {
    match m {
        Modulation::Bpsk => (-2.0, 2.0),
        Modulation::Qpsk => (-1.0, 1.0),
        Modulation::Psk8 => (-1.0, 0.0),
        Modulation::Qam16 => (-0.68, 0.68),
        Modulation::Qam64 => (-0.619, 0.619),
    }
}

pub fn classify(f: &Features) -> Classification {
    let sig = hi(f.snr_db, 6.0, 1.5);
    let carrier = hi(f.carrier_fraction, 0.45, 0.08);
    let flat = 1.0 - hi(f.envelope_cv, 0.12, 0.03);
    let spread = hi(f.freq_spread, 0.08, 0.03);
    let line = hi(f.cyclic_line_db, 12.0, 2.0);
    let mut scores: Vec<(Class, f64)> = vec![
        (Class::Noise, 1.0 - sig),
        (Class::Cw, sig * carrier * flat * (1.0 - spread)),
        (Class::Am, sig * carrier * (1.0 - flat)),
        (
            Class::Fm,
            sig * (1.0 - carrier) * flat * spread * (1.0 - line),
        ),
    ];
    // A pulse-shaped digital signal's envelope is never flat (an RRC
    // PSK's varies by ~25%); FM's is, even when the channel filter's clipped
    // sidebands give it tone-harmonic lines in the symbol-rate band.
    let digital = sig * (1.0 - carrier) * line * (1.0 - flat);
    // Whether the signal is plausibly digital at all is the *absolute* fit
    // of its best constellation (σ = FIT_SIGMA in (C42, |C40|)); a signal
    // far from every constellation earns no digital score. Which
    // constellation is then a relative split (σ = 0.08), which separates
    // 16QAM from 64QAM, whose ideals lie only 0.086 apart.
    let d2: Vec<(Modulation, f64)> = Modulation::ALL
        .into_iter()
        .map(|m| {
            let (c42, c40) = ideal(m);
            (m, (f.c42 - c42).powi(2) + (f.c40_abs - c40).powi(2))
        })
        .collect();
    let best = d2.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
    let fit = (-best / (2.0 * FIT_SIGMA.powi(2))).exp();
    let rel: Vec<f64> = d2
        .iter()
        .map(|x| (-x.1 / (2.0 * 0.08f64.powi(2))).exp())
        .collect();
    let rsum = rel.iter().sum::<f64>().max(1e-300);
    scores.extend(
        d2.iter()
            .zip(&rel)
            .map(|(&(m, _), r)| (Class::Digital(m), digital * fit * r / rsum)),
    );
    // "None of these": a constant competing score, so when no family fits
    // the normalised winner is this, and the answer is unknown.
    let other = OTHER_SCORE;
    let total: f64 = scores.iter().map(|s| s.1).sum::<f64>() + other;
    scores.iter_mut().for_each(|s| s.1 /= total);
    let other = other / total;
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (class, confidence) = scores[0];
    let (runner_up, second) = scores[1];
    let margin = confidence - second.max(other);
    let unknown = confidence < MIN_CONFIDENCE || margin < MIN_MARGIN || other >= confidence;
    Classification {
        class,
        confidence,
        runner_up,
        margin,
        unknown,
        trust: if unknown {
            Trust::Unprovable
        } else {
            Trust::Unproven
        },
        features: *f,
    }
}
