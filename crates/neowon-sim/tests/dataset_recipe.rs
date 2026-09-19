//! Phase 10.8: dataset recipes are reproducible, and their metadata is
//! the truth about the samples.
//!
//! - the same recipe builds bit-identical SigMF bytes; another seed does
//!   not; an example does not depend on the ones built before it;
//! - every recorded value is checked against the samples it describes:
//!   signal power (SNR), tone frequency, DC offset, IQ imbalance (inverting
//!   the recorded transform recovers the clean example);
//! - occupancy rectangles follow their definitions, and hold the signal's
//!   energy (Carson's rule for FM holds ~98% by construction).
//!
//! `cargo test -p neowon-sim --test dataset_recipe`

use neowon_dsp::Window;
use neowon_dsp::iq::stft;
use neowon_sim::dataset::{Parts, Recipe, build, sigmf, validate};
use neowon_sim::iq::{cos_sin_turns, fnv1a64};

fn recipe() -> Recipe {
    Recipe {
        name: "test".into(),
        seed: 42,
        samples: 16 * 1024,
        examples: 16,
        signals_per_example: (1, 3),
        ..Default::default()
    }
}

const CLEAN: Parts = Parts {
    noise: false,
    impairments: false,
};

fn power(iq: &[f32]) -> f64 {
    iq.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / (iq.len() / 2) as f64
}

#[test]
fn same_recipe_same_bytes() {
    let r = recipe();
    validate(&r).unwrap();
    let (d1, m1) = sigmf(&r).unwrap();
    let (d2, m2) = sigmf(&r).unwrap();
    assert_eq!(d1, d2);
    assert_eq!(m1, m2);
    assert_eq!(d1.len(), r.examples * r.samples * 8);
    // Pinned like the D8 fixture: every platform builds these bytes, and
    // a change here is a change to every dataset already built.
    assert_eq!(fnv1a64(&d1), 0x4d97_5060_f969_0347, "dataset bytes moved");

    let other = sigmf(&Recipe {
        seed: 43,
        ..recipe()
    })
    .unwrap();
    assert_ne!(d1, other.0);

    // Example 5 alone is the sixth slot of the recording.
    let ex = build(&r, 5, Parts::ALL);
    let slot = &d1[5 * r.samples * 8..6 * r.samples * 8];
    let bytes: Vec<u8> = ex.iq.iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(bytes, slot);

    // The metadata is valid SigMF-shaped JSON with one annotation per signal.
    let meta: serde_json::Value = serde_json::from_slice(&m1).unwrap();
    assert_eq!(meta["global"]["core:datatype"], "cf32_le");
    let signals: usize = (0..r.examples)
        .map(|i| build(&r, i, CLEAN).meta.signals.len())
        .sum();
    assert_eq!(meta["annotations"].as_array().unwrap().len(), signals);
    assert!(
        signals > r.examples,
        "compositing drew more than one signal"
    );
}

#[test]
fn recipes_are_validated() {
    let r = Recipe::from_json(r#"{"seed": 7, "classes": ["qpsk", "fm"]}"#).unwrap();
    assert_eq!((r.seed, r.examples), (7, Recipe::default().examples));
    assert!(Recipe::from_json(r#"{"classes": ["ofdm"]}"#).is_err());
    assert!(
        validate(&Recipe {
            classes: vec!["ofdm".into()],
            ..recipe()
        })
        .is_err()
    );
    assert!(
        validate(&Recipe {
            signals_per_example: (2, 1),
            ..recipe()
        })
        .is_err()
    );
}

#[test]
fn metadata_is_the_truth() {
    let r = Recipe {
        signals_per_example: (1, 1),
        examples: 40,
        ..recipe()
    };
    let noise_p = r.noise_rms * r.noise_rms;
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..r.examples {
        let clean = build(&r, i, CLEAN);
        let full = build(&r, i, Parts::ALL);
        let noisy = build(
            &r,
            i,
            Parts {
                noise: true,
                impairments: false,
            },
        );
        let s = &clean.meta.signals[0];
        seen.insert(s.label.clone());
        assert_eq!(clean.meta, full.meta, "parts must not change the draws");

        // SNR: the signal's power over the noise's.
        let sig_p = power(&clean.iq);
        let snr = 10.0 * (sig_p / noise_p).log10();
        let tol = if s.symbol_rate_hz.is_some() {
            0.5
        } else {
            0.05
        };
        assert!(
            (snr - s.snr_db).abs() < tol,
            "#{i} {}: snr {snr:.2} recorded {:.2}",
            s.label,
            s.snr_db
        );
        let n: Vec<f32> = noisy.iq.iter().zip(&clean.iq).map(|(a, b)| a - b).collect();
        let np = power(&n);
        assert!((np / noise_p - 1.0).abs() < 0.05, "#{i}: noise power {np}");

        // Tone frequency: the recorded centre is where the energy is.
        if s.label == "cw" {
            let at = |f: f64| {
                let (mut re, mut im) = (0.0, 0.0);
                for (k, p) in clean.iq.as_chunks::<2>().0.iter().enumerate() {
                    let (c, sn) = cos_sin_turns(-f * k as f64 / r.sample_rate);
                    re += p[0] as f64 * c - p[1] as f64 * sn;
                    im += p[0] as f64 * sn + p[1] as f64 * c;
                }
                (re * re + im * im).sqrt() / r.samples as f64
            };
            let bin = r.sample_rate / r.samples as f64;
            assert!((at(s.centre_hz) - s.amplitude).abs() < 1e-5 * s.amplitude.max(1.0));
            assert!(at(s.centre_hz + 3.0 * bin) < 0.2 * s.amplitude);
        }

        // Impairments: inverting the recorded transform recovers the
        // unimpaired samples; the DC is the recorded one.
        let m = &full.meta.impairments;
        let (c, sn) = cos_sin_turns(m.iq_phase_rad / std::f64::consts::TAU);
        let mut err = 0.0f64;
        let (mut mi, mut mq) = (0.0, 0.0);
        for (f, u) in full
            .iq
            .as_chunks::<2>()
            .0
            .iter()
            .zip(noisy.iq.as_chunks::<2>().0.iter())
        {
            let i0 = (f[0] as f64 - m.dc_i) / m.iq_gain;
            let q0 = (f[1] as f64 - m.dc_q - sn * i0) / c;
            err = err.max((i0 - u[0] as f64).abs().max((q0 - u[1] as f64).abs()));
            mi += f[0] as f64;
            mq += f[1] as f64;
        }
        assert!(err < 1e-5, "#{i}: inverse error {err}");
        let (mi, mq) = (mi / r.samples as f64, mq / r.samples as f64);
        let (ci, cq) = clean
            .iq
            .as_chunks::<2>()
            .0
            .iter()
            .fold((0.0, 0.0), |a, p| (a.0 + p[0] as f64, a.1 + p[1] as f64));
        let sig_dc = (ci / r.samples as f64).abs() + (cq / r.samples as f64).abs();
        let tol = 4.0 * r.noise_rms / (r.samples as f64).sqrt() + 2.0 * sig_dc;
        assert!(
            (mi - m.dc_i).abs() < tol,
            "#{i}: dc_i {mi} recorded {}",
            m.dc_i
        );
        assert!(
            (mq - m.dc_q).abs() < tol,
            "#{i}: dc_q {mq} recorded {}",
            m.dc_q
        );
    }
    assert_eq!(
        seen.len(),
        8,
        "40 examples should cover every class: {seen:?}"
    );
}

#[test]
fn occupancy_rectangles_are_exact() {
    let r = Recipe {
        signals_per_example: (1, 3),
        examples: 24,
        ..recipe()
    };
    let n = 1024;
    let bin = r.sample_rate / n as f64;
    for i in 0..r.examples {
        let ex = build(&r, i, CLEAN);
        // Rectangles: from the definitions, disjoint, over the whole example.
        let duration = r.samples as f64 / r.sample_rate;
        for s in &ex.meta.signals {
            let half = match s.label.as_str() {
                "cw" => 0.0,
                "am" => s.tone_hz.unwrap(),
                "fm" => s.deviation_hz.unwrap() + s.tone_hz.unwrap(),
                _ => s.symbol_rate_hz.unwrap() * (1.0 + s.rolloff.unwrap()) / 2.0,
            };
            assert_eq!((s.lo_hz, s.hi_hz), (s.centre_hz - half, s.centre_hz + half));
            assert_eq!((s.t_start_s, s.t_stop_s), (0.0, duration));
        }
        for (a, b) in ex
            .meta
            .signals
            .iter()
            .flat_map(|a| ex.meta.signals.iter().map(move |b| (a, b)))
        {
            if !std::ptr::eq(a, b) {
                assert!(a.hi_hz < b.lo_hz || b.hi_hz < a.lo_hz, "#{i}: overlap");
            }
        }

        // Energy: each rectangle (plus the window's main lobe) holds its
        // signal; together they hold (nearly) everything.
        let blocks = stft(&ex.iq, Window::Blackman, n, n / 2).unwrap();
        let spec: Vec<f64> = (0..n).map(|k| blocks.iter().map(|b| b[k]).sum()).collect();
        let total: f64 = spec.iter().sum();
        let inside = |lo: f64, hi: f64| -> f64 {
            (0..n)
                .filter(|&k| {
                    let f = (k as f64 - (n / 2) as f64) * bin;
                    f >= lo - 4.0 * bin && f <= hi + 4.0 * bin
                })
                .map(|k| spec[k])
                .sum()
        };
        let held: f64 = ex
            .meta
            .signals
            .iter()
            .map(|s| inside(s.lo_hz, s.hi_hz))
            .sum();
        let want = if ex.meta.signals.iter().any(|s| s.label == "fm") {
            0.97
        } else {
            0.995
        };
        assert!(
            held / total > want,
            "#{i}: rectangles hold {:.4} of the energy ({:?})",
            held / total,
            ex.meta
                .signals
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>()
        );
    }
}
