//! Phase 10.10 golden: the demodulators against `neowon-sim`'s exact AM/FM
//! generators. Expectations come from the scene definition, not a recording:
//! AM `amplitude·(1 + depth·cos)` has envelope depth `depth`; FM's
//! instantaneous frequency `offset + deviation·cos` is recovered as
//! `deviation`, scaled by half the channel width.
//!
//!   cargo test -p neowon-dsp --test demod_golden

use neowon_dsp::demod::{DemodMode, Receiver, ReceiverConfig};
use neowon_sim::{IqComponent, IqScene};

const FS: f64 = 2.048e6;
const AUDIO: f64 = 48e3;
const PAIRS: usize = 409_600; // 0.2 s
const CHUNK: usize = 16_384;
const SEED: u64 = 7;

/// Demodulate `component` (at `offset_hz`) with `mode`/`width`.
fn run(mode: DemodMode, component: IqComponent, offset_hz: f64, width_hz: f64) -> Vec<f32> {
    let scene = IqScene {
        sample_rate: FS,
        components: vec![component],
        noise_rms: 0.0,
    };
    let mut rx = Receiver::new(ReceiverConfig {
        mode,
        offset_hz,
        width_hz,
        sample_rate: FS,
        audio_rate: AUDIO,
        deemphasis_tau_s: None,
    });
    let mut audio = Vec::new();
    let mut iq = Vec::new();
    let mut start = 0usize;
    while start < PAIRS {
        let n = CHUNK.min(PAIRS - start);
        iq.clear();
        iq.extend_from_slice(&scene.samples(SEED, start as u64, n));
        rx.process(&iq, &mut audio);
        start += n;
    }
    audio
}

/// Frequency of the recovered tone by zero crossings over the middle half.
fn tone_hz(a: &[f32]) -> f64 {
    let a = &a[a.len() / 4..a.len() * 3 / 4];
    let mean = a.iter().sum::<f32>() / a.len() as f32;
    let mut cross = 0u64;
    let mut prev = a[0] - mean;
    for &x in &a[1..] {
        let y = x - mean;
        if (prev < 0.0) != (y < 0.0) {
            cross += 1;
        }
        prev = y;
    }
    cross as f64 / 2.0 / (a.len() as f64 / AUDIO)
}

/// Peak magnitude over the middle half (DC removed).
fn amp(a: &[f32]) -> f64 {
    let a = &a[a.len() / 4..a.len() * 3 / 4];
    let mx = a.iter().cloned().fold(f32::MIN, f32::max) as f64;
    let mn = a.iter().cloned().fold(f32::MAX, f32::min) as f64;
    (mx - mn) / 2.0
}

#[test]
fn am_recovers_the_tone_and_its_depth() {
    let audio = run(
        DemodMode::Am,
        IqComponent::Am {
            offset_hz: 100e3,
            amplitude: 0.5,
            depth: 0.3,
            tone_hz: 1000.0,
        },
        100e3,
        10e3,
    );
    let f = tone_hz(&audio);
    assert!((f - 1000.0).abs() < 10.0, "AM tone {f:.1} Hz");
    // AGC normalises the mean envelope to 1, so the AC peak is the depth.
    let depth = amp(&audio);
    assert!((depth - 0.3).abs() < 0.015, "AM depth {depth:.3}");
}

#[test]
fn nfm_recovers_the_tone_and_deviation() {
    let width = 12.5e3;
    let audio = run(
        DemodMode::Nfm,
        IqComponent::Fm {
            offset_hz: 100e3,
            amplitude: 1.0,
            deviation_hz: 3000.0,
            tone_hz: 1000.0,
        },
        100e3,
        width,
    );
    let f = tone_hz(&audio);
    assert!((f - 1000.0).abs() < 10.0, "NFM tone {f:.1} Hz");
    let want = 3000.0 / (width / 2.0);
    let got = amp(&audio);
    assert!(
        (got - want).abs() < 0.05 * want,
        "NFM deviation {got:.3} vs {want:.3}"
    );
}

#[test]
fn wfm_recovers_the_tone_and_deviation() {
    let width = 180e3;
    let audio = run(
        DemodMode::Wfm,
        IqComponent::Fm {
            offset_hz: 0.0,
            amplitude: 1.0,
            deviation_hz: 30_000.0,
            tone_hz: 1000.0,
        },
        0.0,
        width,
    );
    let f = tone_hz(&audio);
    assert!((f - 1000.0).abs() < 10.0, "WFM tone {f:.1} Hz");
    let want = 30_000.0 / (width / 2.0);
    let got = amp(&audio);
    assert!(
        (got - want).abs() < 0.05 * want,
        "WFM deviation {got:.3} vs {want:.3}"
    );
}
