//! Golden: the demodulators against `neowon-sim`'s exact AM/FM
//! generators. Expectations come from the scene definition, not a recording:
//! AM `amplitude·(1 + depth·cos)` has envelope depth `depth`; FM's
//! instantaneous frequency `offset + deviation·cos` is recovered as
//! `deviation`, scaled by half the channel width.
//!
//! Each row files a JSON readout at
//! `target/tmp/readouts/demod-<row>.json` (the path is printed as
//! `readout: …`); see `tests/common/mod.rs`.
//!
//!   cargo test -p neowon-dsp --test demod_golden -- --nocapture

mod common;

use neowon_dsp::demod::{DemodMode, Receiver, ReceiverConfig};
use neowon_sim::{IqComponent, IqScene};

const FS: f64 = 2.048e6;
const AUDIO: f64 = 48e3;
const PAIRS: usize = 409_600; // 0.2 s
const CHUNK: usize = 16_384;
const SEED: u64 = 7;

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

/// File the row: what the scene says the answer is, what came back, and the
/// tolerance the assertion below uses. `level` is the depth (AM) or the
/// normalised deviation (FM).
#[allow(clippy::too_many_arguments)]
fn readout(
    row: &str,
    tone: f64,
    tone_truth: f64,
    tone_tol: f64,
    level: f64,
    level_truth: f64,
    level_tol: f64,
) {
    let document = format!(
        r#"{{"row":"{row}","tone_hz":{tone:.2},"tone_truth_hz":{tone_truth:.2},"tone_tolerance_hz":{tone_tol:.2},"level":{level:.4},"level_truth":{level_truth:.4},"level_tolerance":{level_tol:.4}}}"#
    );
    println!("{document}");
    common::file_readout(&format!("demod-{row}"), &document);
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
    // AGC normalises the mean envelope to 1, so the AC peak is the depth.
    let depth = amp(&audio);
    readout("am", f, 1000.0, 10.0, depth, 0.3, 0.015);
    assert!((f - 1000.0).abs() < 10.0, "AM tone {f:.1} Hz");
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
    let want = 3000.0 / (width / 2.0);
    let got = amp(&audio);
    readout("nfm", f, 1000.0, 10.0, got, want, 0.05 * want);
    assert!((f - 1000.0).abs() < 10.0, "NFM tone {f:.1} Hz");
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
    let want = 30_000.0 / (width / 2.0);
    let got = amp(&audio);
    readout("wfm", f, 1000.0, 10.0, got, want, 0.05 * want);
    assert!((f - 1000.0).abs() < 10.0, "WFM tone {f:.1} Hz");
    assert!(
        (got - want).abs() < 0.05 * want,
        "WFM deviation {got:.3} vs {want:.3}"
    );
}
