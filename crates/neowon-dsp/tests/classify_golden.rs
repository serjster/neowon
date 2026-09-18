//! Phase 10.5 golden, statistical: the DSP classifier over every class,
//! seed 42, N trials per class (`NEOWON_CLASSIFY_N`, default 500).
//!
//! Each trial is a fresh capture from the D8 generator at 1 MS/s, 8 Ki
//! pairs (1024 symbols of a digital signal), with the trial's own seed, a random offset (±5 kHz) and phase.
//! The pipeline is the app's: detect the signal, then classify the band
//! detection reports. SNR is signal power over the capture's total noise
//! power. Digital signals are 125 ksym/s RRC (roll-off 0.35); AM is depth
//! 0.5 on a 1 kHz tone; FM deviates 25 kHz at 1 kHz.
//!
//! Readout: per class precision and recall, and the confusion matrix,
//! as JSON. `unknown` answers count against recall, not precision.
//!
//! `cargo test --release -p neowon-dsp --test classify_golden -- --nocapture`

use neowon_core::Modulation;
use neowon_dsp::classify::{Class, classify, features};
use neowon_dsp::{DetectConfig, detect};
use neowon_sim::iq::splitmix64;
use neowon_sim::{IqComponent, IqScene};

const RATE: f64 = 1e6;
const PAIRS: usize = 8 * 1024;
const NOISE: f64 = 0.05;

fn unit(seed: u64, i: u64) -> f64 {
    (splitmix64(seed, i) >> 11) as f64 / (1u64 << 53) as f64
}

/// A capture of `class` at `snr_db` for trial `seed`.
fn capture(class: Class, snr_db: f64, seed: u64) -> Vec<f32> {
    let offset = (unit(seed, 1) - 0.5) * 10e3;
    let phase = unit(seed, 2);
    let noise_power = NOISE * NOISE;
    let p = noise_power * 10f64.powf(snr_db / 10.0);
    let comp = match class {
        Class::Noise => None,
        Class::Cw => Some(IqComponent::Tone {
            offset_hz: offset,
            amplitude: p.sqrt(),
            phase,
        }),
        // Mean power of A(1 + m cos) is A²(1 + m²/2).
        Class::Am => Some(IqComponent::Am {
            offset_hz: offset,
            amplitude: (p / 1.125).sqrt(),
            depth: 0.5,
            tone_hz: 1e3,
        }),
        Class::Fm => Some(IqComponent::Fm {
            offset_hz: offset,
            amplitude: p.sqrt(),
            deviation_hz: 25e3,
            tone_hz: 1e3,
        }),
        // Per-sample power of the digital source is amplitude² / sps.
        Class::Digital(m) => Some(IqComponent::Digital {
            modulation: m,
            symbol_rate: 125e3,
            offset_hz: offset,
            amplitude: (p * 8.0).sqrt(),
            rolloff: 0.35,
        }),
    };
    IqScene {
        sample_rate: RATE,
        components: comp.into_iter().collect(),
        noise_rms: NOISE,
    }
    .samples(seed, 0, PAIRS)
}

/// The pipeline: strongest detection's band, then classify; no detection
/// is noise.
fn predict(iq: &[f32]) -> Option<Class> {
    let cfg = DetectConfig {
        nfft: 1024,
        blocks: PAIRS / 1024,
        ..Default::default()
    };
    let obs = detect(iq, RATE, 0.0, 0.0, &cfg);
    let Some(o) = obs
        .iter()
        .max_by(|a, b| a.power_dbfs.total_cmp(&b.power_dbfs))
    else {
        return Some(Class::Noise);
    };
    let f = features(iq, RATE, o.centre_hz, o.bandwidth_hz())?;
    let c = classify(&f);
    (!c.unknown).then_some(c.class)
}

#[test]
fn dsp_classifier_precision_per_class() {
    let n: u64 = std::env::var("NEOWON_CLASSIFY_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    for snr in [10.0, 20.0] {
        let mut conf = vec![vec![0u64; Class::ALL.len() + 1]; Class::ALL.len()];
        for (ti, &truth) in Class::ALL.iter().enumerate() {
            for trial in 0..n {
                let seed = splitmix64(42, (ti as u64) << 32 | trial);
                let col = match predict(&capture(truth, snr, seed)) {
                    Some(p) => Class::ALL.iter().position(|&c| c == p).unwrap(),
                    None => Class::ALL.len(), // unknown
                };
                conf[ti][col] += 1;
            }
        }
        let mut rows = Vec::new();
        let mut precisions = Vec::new();
        for (ci, c) in Class::ALL.iter().enumerate() {
            let tp = conf[ci][ci] as f64;
            let predicted: u64 = conf.iter().map(|r| r[ci]).sum();
            let precision = if predicted > 0 {
                tp / predicted as f64
            } else {
                0.0
            };
            let recall = tp / n as f64;
            precisions.push(precision);
            rows.push(format!(
                r#"{{"class":"{}","precision":{precision:.3},"recall":{recall:.3},"unknown":{}}}"#,
                c.label(),
                conf[ci][Class::ALL.len()]
            ));
        }
        let macro_p = precisions.iter().sum::<f64>() / precisions.len() as f64;
        println!(
            r#"{{"snr_db":{snr},"n_per_class":{n},"macro_precision":{macro_p:.3},"classes":[{}],"confusion":{:?}}}"#,
            rows.join(","),
            conf
        );
        if snr >= 20.0 {
            assert!(macro_p >= 0.9, "macro precision {macro_p} at {snr} dB");
        }
    }
    let _ = Modulation::Qpsk;
}
