use std::f32::consts::TAU;

use super::*;
use crate::dab::SAMPLE_RATE;
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
        let estimated = receiver
            .estimate_frequency_offset(0)
            .expect("a clean guard correlates");
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
    let signal_metric = receiver.extract_prs_spectrum(0, 0.0);
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
    let noise_metric = noise.extract_prs_spectrum(0, 0.0);
    assert!(
        noise_metric < 0.1,
        "noise metric {noise_metric} should be far below the DAB score"
    );
}

/// The carrier tracker is damped (a quarter) and refuses implausible
/// jumps (over 250 Hz): an undamped tracker followed noisy guard estimates and wandered to +100 Hz while
/// the PRS score at the grid fell from 0.5 to 0.1.
///
/// Frames whose apparent offset alternates ±120 Hz are the deterministic
/// stand-in for that noisy estimate: the damped tracker stays in the tens
/// of Hz, an undamped one lands on ±120. The trailing +400 Hz frames
/// exercise the jump guard: damping alone would step 100 Hz towards them.
#[test]
fn the_offset_tracker_stays_bounded_on_jumpy_estimates() {
    use crate::dab::encoder::{EnsembleSpec, FicFrame, ServiceSpec};

    let spec = EnsembleSpec {
        eid: 0xF044,
        label: "TRACKER",
        services: vec![ServiceSpec {
            sid: 0x1001,
            label: "TONE",
            sub_channel: 0,
            ascty: 63,
        }],
        sub_channels: Vec::new(),
    };
    let base = FicFrame::new(&spec).iq_frame(0.2);
    let rotated = |offset_hz: f64| -> Vec<f32> {
        base.iter()
            .enumerate()
            .flat_map(|(i, sample)| {
                let phase = (f64::from(TAU) * offset_hz * i as f64 / SAMPLE_RATE) as f32;
                let value = *sample * Complex32::from_polar(1.0, phase);
                [value.re, value.im]
            })
            .collect()
    };

    let mut receiver = DabReceiver::new();
    for index in 0..9 {
        let offset = if index % 2 == 0 { 120.0 } else { -120.0 };
        receiver.push_iq(&rotated(offset));
    }
    // Enough trailing frames that the alternations and two of the
    // implausible frames are all decoded (sync costs a frame of lookahead).
    for _ in 0..3 {
        receiver.push_iq(&rotated(400.0));
    }
    assert!(
        receiver.freq_offset_hz.abs() < 60.0,
        "the tracker followed a jump: {} Hz",
        receiver.freq_offset_hz
    );
}

#[test]
fn empty_input_is_harmless() {
    let mut receiver = DabReceiver::new();
    assert_eq!(receiver.push_iq(&[]), 0);
    assert_eq!(receiver.push_iq(&[0.0, 0.0]), 0);
    let status = receiver.status();
    assert!(!status.locked);
    assert_eq!(status.frames, 0);
}
