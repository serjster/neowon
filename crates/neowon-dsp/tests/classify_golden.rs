//! Golden, statistical: the DSP classifier over every class,
//! seed 42, N trials per class (`NEOWON_CLASSIFY_N`, default 500).
//!
//! Each trial is a fresh capture from the sim IQ generator at 1 MS/s, 8 Ki
//! pairs (1024 symbols of a digital signal), with the trial's own seed, a random offset (±5 kHz) and phase.
//! The pipeline is the app's: detect the signal, then classify the band
//! detection reports. SNR is signal power over the capture's total noise
//! power. Digital signals are 125 ksym/s RRC (roll-off 0.35); AM is depth
//! 0.5 on a 1 kHz tone; FM deviates 25 kHz at 1 kHz.
//!
//! Readout: per class precision and recall, and the confusion matrix,
//! as JSON. `unknown` answers count against recall, not precision. The run
//! files it at `target/tmp/readouts/classify.json` (the path is
//! printed as `readout: …`); see `tests/common/mod.rs`.
//!
//! `cargo test --release -p neowon-dsp --test classify_golden -- --nocapture`
//!
//! **Both SNR rows are asserted.** The bounds below are read off that
//! command's own output, not chosen: see [`Bounds`].

mod common;

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

/// Floors and ceilings one SNR row must clear.
///
/// Every number here was **read off the run**, not picked: at the default
/// seed 42 / N 500 the command in the header measured
///
/// | SNR | macro precision | macro recall | unknown | worst class P | worst class R |
/// | --- | --- | --- | --- | --- | --- |
/// | 10 dB | 0.99947 | 0.95667 | 4.29 % | 0.99523 (16qam) | 0.776 (64qam) |
/// | 20 dB | 0.99945 | 0.95800 | 4.16 % | 0.99505 (64qam) | 0.804 (64qam) |
///
/// and the bounds sit a few binomial standard errors under (over) those —
/// for N 500 a per-class rate has σ ≈ 0.019 and the 9-class macro σ ≈ 0.006,
/// so each bound is ≥ 3σ from the measured value: wide enough not to flake
/// when `NEOWON_CLASSIFY_N` changes, tight enough that one class collapsing
/// fails the run (a single class falling to zero costs the macro figures
/// 1/9 = 0.111).
///
/// The 10 dB row is asserted with the same force as 20 dB.
struct Bounds {
    /// Mean per-class precision over the nine classes.
    macro_precision: f64,
    /// Mean per-class recall; an `unknown` answer counts against it.
    macro_recall: f64,
    /// Fraction of all trials answered `unknown`.
    unknown_rate: f64,
    /// No single class may fall below these.
    class_precision: f64,
    class_recall: f64,
}

const BOUNDS: Bounds = Bounds {
    macro_precision: 0.99,
    macro_recall: 0.94,
    unknown_rate: 0.06,
    class_precision: 0.98,
    class_recall: 0.70,
};

#[test]
fn dsp_classifier_precision_per_class() {
    let n: u64 = std::env::var("NEOWON_CLASSIFY_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let mut documents = Vec::new();
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
        let mut recalls = Vec::new();
        let mut unknowns = 0u64;
        for (ci, c) in Class::ALL.iter().enumerate() {
            let tp = conf[ci][ci] as f64;
            let predicted: u64 = conf.iter().map(|r| r[ci]).sum();
            let precision = if predicted > 0 {
                tp / predicted as f64
            } else {
                0.0
            };
            let recall = tp / n as f64;
            let unknown = conf[ci][Class::ALL.len()];
            precisions.push(precision);
            recalls.push(recall);
            unknowns += unknown;
            rows.push(format!(
                r#"{{"class":"{}","precision":{precision:.4},"recall":{recall:.4},"unknown":{unknown}}}"#,
                c.label()
            ));
        }
        let macro_p = precisions.iter().sum::<f64>() / precisions.len() as f64;
        let macro_r = recalls.iter().sum::<f64>() / recalls.len() as f64;
        let unknown_rate = unknowns as f64 / (n * Class::ALL.len() as u64) as f64;
        let document = format!(
            r#"{{"snr_db":{snr},"n_per_class":{n},"macro_precision":{macro_p:.4},"macro_recall":{macro_r:.4},"unknown_rate":{unknown_rate:.4},"classes":[{}],"confusion":{:?}}}"#,
            rows.join(","),
            conf
        );
        println!("{document}");
        documents.push(document);
        file_rows(&documents);

        // Asserted after the row is printed and filed, so a failing run
        // still leaves the numbers that failed it.
        //
        // Per class first, so one collapsing class is named rather than
        // averaged away by the eight that still work.
        for ((c, &precision), &recall) in Class::ALL.iter().zip(&precisions).zip(&recalls) {
            assert!(
                precision >= BOUNDS.class_precision,
                "{} precision {precision:.4} at {snr} dB (floor {})",
                c.label(),
                BOUNDS.class_precision
            );
            assert!(
                recall >= BOUNDS.class_recall,
                "{} recall {recall:.4} at {snr} dB (floor {})",
                c.label(),
                BOUNDS.class_recall
            );
        }

        // The row is asserted at 10 dB exactly as at 20 dB — recall and
        // `unknown` beside precision, because a classifier that answers
        // `unknown` to everything has perfect precision.
        assert!(
            macro_p >= BOUNDS.macro_precision,
            "macro precision {macro_p:.4} at {snr} dB (floor {})",
            BOUNDS.macro_precision
        );
        assert!(
            macro_r >= BOUNDS.macro_recall,
            "macro recall {macro_r:.4} at {snr} dB (floor {})",
            BOUNDS.macro_recall
        );
        assert!(
            unknown_rate <= BOUNDS.unknown_rate,
            "unknown rate {unknown_rate:.4} at {snr} dB (ceiling {})",
            BOUNDS.unknown_rate
        );
    }
    let _ = Modulation::Qpsk;
}

fn file_rows(documents: &[String]) {
    common::file_readout(
        "classify",
        &format!(
            r#"{{"run":"classify_golden","seed":42,"rows":[{}]}}"#,
            documents.join(",")
        ),
    );
}
