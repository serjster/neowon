//! Shared metric helpers for the codec acceptance fixtures.
//!
//! The metric is deliberately **alignment + correlation + relative RMS
//! error + tone spectral check**, not raw sample subtraction: an HE-AAC
//! encoder/decoder pair has its own delay (Apple's MP4 even carries the
//! priming explicitly), and two conforming decoders are allowed to
//! differ in the SBR/PS synthesis. Aligning first and stating the
//! correlation and error is the honest form of the row 14/15 criterion.
#![allow(dead_code)]

pub fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

/// Interleaved s16 little-endian bytes to `[-1, 1)` f32.
pub fn s16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0)
        .collect()
}

pub fn deinterleave(interleaved: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|channel| {
            interleaved
                .iter()
                .skip(channel)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect()
}

/// What [`align_and_compare`] measured.
#[derive(Debug, Clone, Copy)]
pub struct Agreement {
    /// Delay of `ours` relative to `reference`, in samples.
    pub lag: usize,
    /// Normalised cross-correlation at `lag`.
    pub correlation: f64,
    /// `‖ours[lag..] − reference‖ / ‖reference‖`.
    pub relative_rms_error: f64,
}

/// Print one measured agreement as a JSON line, so the correlation / lag /
/// relative-RMS numbers in `docs/protocol-dab.md`'s acceptance table are
/// regenerable from the command that measures them:
/// `cargo test -p neowon-codec ... -- --nocapture`.
pub fn report_agreement(fixture: &str, channel: usize, agreement: &Agreement) {
    println!(
        r#"{{"fixture":"{fixture}","channel":{channel},"lag":{},"correlation":{:.9},"relative_rms_error":{:.6}}}"#,
        agreement.lag, agreement.correlation, agreement.relative_rms_error
    );
}

/// Coarse lag step for [`align_and_compare`]. The fixture signals are
/// pure tones: an 880 Hz sine at 48 kHz has a half-period of ~27
/// samples, so a 32-sample grid steps over the correlation peak and can
/// lock onto a neighbouring period instead (observed with libfdk-aac,
/// which has a ~7000-sample delay). 8 samples keeps the peak on the
/// grid for every tone these fixtures use.
const COARSE_STEP: usize = 8;

/// Align `ours` against `reference` by normalised cross-correlation
/// (coarse 8-sample sweep, then unit refinement) and measure agreement.
///
/// `ours` may start earlier than `reference` (decoder priming), so only
/// positive lags are searched: `ours[lag..]` against `reference[..]`.
pub fn align_and_compare(ours: &[f32], reference: &[f32], max_lag: usize) -> Agreement {
    let mut best_lag = 0usize;
    let mut best_correlation = f64::NEG_INFINITY;
    let mut lag = 0usize;
    while lag <= max_lag {
        let correlation = correlation_at(ours, reference, lag).unwrap_or(f64::NEG_INFINITY);
        if correlation > best_correlation {
            best_correlation = correlation;
            best_lag = lag;
        }
        lag += COARSE_STEP;
    }
    let low = best_lag.saturating_sub(COARSE_STEP);
    let high = (best_lag + COARSE_STEP).min(max_lag);
    for lag in low..=high {
        let correlation = correlation_at(ours, reference, lag).unwrap_or(f64::NEG_INFINITY);
        if correlation > best_correlation {
            best_correlation = correlation;
            best_lag = lag;
        }
    }
    let relative_rms_error =
        relative_error_at(ours, reference, best_lag).expect("lag has at least 1000 samples");
    Agreement {
        lag: best_lag,
        correlation: best_correlation,
        relative_rms_error,
    }
}

fn overlap_len(ours: &[f32], reference: &[f32], lag: usize) -> Option<usize> {
    let len = reference.len().min(ours.len().checked_sub(lag)?);
    (len >= 1000).then_some(len)
}

fn correlation_at(ours: &[f32], reference: &[f32], lag: usize) -> Option<f64> {
    let len = overlap_len(ours, reference, lag)?;
    let mut dot = 0.0f64;
    let mut energy_ours = 0.0f64;
    let mut energy_reference = 0.0f64;
    for i in 0..len {
        let x = f64::from(ours[lag + i]);
        let y = f64::from(reference[i]);
        dot += x * y;
        energy_ours += x * x;
        energy_reference += y * y;
    }
    Some(dot / (energy_ours.sqrt() * energy_reference.sqrt() + 1e-30))
}

fn relative_error_at(ours: &[f32], reference: &[f32], lag: usize) -> Option<f64> {
    let len = overlap_len(ours, reference, lag)?;
    let mut error = 0.0f64;
    let mut energy = 0.0f64;
    for i in 0..len {
        let difference = f64::from(ours[lag + i]) - f64::from(reference[i]);
        error += difference * difference;
        energy += f64::from(reference[i]) * f64::from(reference[i]);
    }
    Some(error.sqrt() / (energy.sqrt() + 1e-30))
}

/// Single-bin Goertzel magnitude (normalised by length) at `frequency`.
pub fn goertzel(samples: &[f32], sample_rate: f64, frequency: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0f64;
    let mut previous2 = 0.0f64;
    for &sample in samples {
        let current = f64::from(sample) + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    let power = previous * previous + previous2 * previous2 - coefficient * previous * previous2;
    power.max(0.0).sqrt() / samples.len() as f64
}
