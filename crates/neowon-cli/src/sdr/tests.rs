//! The smoke pipeline on the simulator: one passing run per source kind
//! of signal, and one run per FAIL rule that trips it. Every backend here
//! is a `SimSdrBackend` built directly — nothing calls `open`, so no test
//! can reach the dongle. Scenes and seeds are fixed, so the readouts are
//! deterministic.

use clap::Parser;
use neowon_sim::sdr::{Emitter, EmitterKind};

use super::pipeline::{Failure, MIN_CONFIDENCE, Rule, verdict};
use super::*;

const SEED: u64 = 1;

fn preset(scene: &str, freq_hz: f64) -> Readout {
    let mut b = sim_backend(scene, SEED).unwrap();
    pipeline::run(&mut b, "sim", &Params::new(freq_hz)).unwrap()
}

/// The pipeline on a one-emitter scene at 100.1 MHz.
fn custom(kind: EmitterKind, amplitude: f64, noise_rms: f64) -> Readout {
    let scene = RfScene {
        emitters: vec![Emitter {
            freq_hz: 100.1e6,
            amplitude,
            kind,
        }],
        noise_rms,
        buffer: None,
    };
    let mut b = SimSdrBackend::with_scene(scene);
    b.set_seed(SEED).unwrap();
    pipeline::run(&mut b, "sim", &Params::new(100.1e6)).unwrap()
}

fn rules(r: &Readout) -> Vec<Rule> {
    r.failures.iter().map(|f| f.rule).collect()
}

#[test]
fn an_am_station_passes_every_rule() {
    let r = preset("rf-am", 100.1e6);
    assert_eq!(rules(&r), [], "{}", to_json(&r));
    assert_eq!((r.source, r.dongle_serial.as_str()), ("sim", "sim-sdr-0"));
    assert_eq!(r.class, "am");
    assert!(r.confidence >= MIN_CONFIDENCE, "{}", to_json(&r));
    assert!((r.peak_hz.unwrap() - 100.1e6).abs() <= r.peak_tol_hz);
    assert_eq!(r.peak_tol_hz, 1000.0, "±2 RBW at 2.048 MS/s / 4096 bins");
    // The decode is the demodulated audio: the scene's 1 kHz tone, to the
    // audio spectrum's bin (48 kHz / 4096 ≈ 12 Hz).
    assert!(r.decode.starts_with("am audio:"), "{}", r.decode);
    assert!(r.decode.contains("dominant 996 Hz"), "{}", r.decode);
    assert!(r.snr_db.unwrap() > 40.0, "{}", to_json(&r));
}

#[test]
fn a_digital_signal_passes_with_its_symbols_decoded() {
    let r = preset("rf-digital", 100.3e6);
    assert_eq!(rules(&r), [], "{}", to_json(&r));
    assert_eq!(r.class, "qpsk");
    assert!(r.decode.starts_with("qpsk symbols at "), "{}", r.decode);
    assert!(r.decode.contains("first labels "), "{}", r.decode);
}

#[test]
fn a_wide_fm_station_passes_through_the_wfm_demodulator() {
    let fm = EmitterKind::Fm {
        deviation_hz: 75e3,
        tone_hz: 1000.0,
    };
    let r = custom(fm, 0.5, 0.05);
    assert_eq!(rules(&r), [], "{}", to_json(&r));
    assert_eq!(r.class, "fm");
    assert!(r.decode.starts_with("wfm audio:"), "{}", r.decode);
    assert!(r.decode.contains("dominant 996 Hz"), "{}", r.decode);
}

/// Rule 1, a signal but not at `--freq`: 5 kHz from rf-am's carrier is
/// five tolerances away. The carrier is still classified and decoded, so
/// no other rule trips.
#[test]
fn no_peak_trips_when_the_signal_is_off_frequency() {
    let r = preset("rf-am", 100.105e6);
    assert_eq!(rules(&r), [Rule::NoPeak], "{}", to_json(&r));
    assert!((r.peak_hz.unwrap() - 100.1e6).abs() < 100.0);
}

/// Rule 1, nothing on the air at all: every rule trips, `no-peak` first.
#[test]
fn no_peak_trips_on_an_empty_band() {
    let r = preset("rf-noise", 100.1e6);
    assert_eq!(
        rules(&r),
        [
            Rule::NoPeak,
            Rule::ClassUnknown,
            Rule::LowConfidence,
            Rule::EmptyDecode
        ],
        "{}",
        to_json(&r)
    );
    assert_eq!((r.peak_hz, r.class.as_str()), (None, "none"));
}

/// Rule 2: the rf-fm preset (±3 kHz narrowband FM) is detected on
/// frequency, but the DSP classifier will not name it. An unnamed signal
/// has no decoder, so `empty-decode` trips with it; confidence below 0.5
/// is what makes it unknown, so `low-confidence` does too.
#[test]
fn class_unknown_trips_on_a_signal_the_classifier_will_not_name() {
    let r = preset("rf-fm", 100.1e6);
    assert_eq!(r.class, "unknown", "{}", to_json(&r));
    assert!(rules(&r).contains(&Rule::ClassUnknown), "{}", to_json(&r));
    assert!(!rules(&r).contains(&Rule::NoPeak), "{}", to_json(&r));
}

/// Rule 3 alone: a strong AM carrier at 20 % depth is named `am` but only
/// at ~0.58, and its audio still decodes.
#[test]
fn low_confidence_trips_below_0_70() {
    let am = EmitterKind::Am {
        depth: 0.2,
        tone_hz: 1000.0,
    };
    let r = custom(am, 0.5, 0.05);
    assert_eq!(rules(&r), [Rule::LowConfidence], "{}", to_json(&r));
    assert_eq!(r.class, "am");
    assert!((0.5..MIN_CONFIDENCE).contains(&r.confidence));
}

/// Rule 4 alone: rf-reference is an unmodulated carrier, confidently `cw`,
/// with nothing to decode.
#[test]
fn empty_decode_trips_on_a_bare_carrier() {
    let r = preset("rf-reference", 100.1e6);
    assert_eq!(rules(&r), [Rule::EmptyDecode], "{}", to_json(&r));
    assert_eq!(r.class, "cw");
    assert!(r.confidence >= MIN_CONFIDENCE);
}

/// The rules' edges: exactly ±2 RBW and exactly 0.70 pass.
#[test]
fn the_thresholds_are_inclusive() {
    let mut r = preset("rf-am", 100.1e6);
    r.peak_hz = Some(r.tune_hz + r.peak_tol_hz);
    r.confidence = MIN_CONFIDENCE;
    assert_eq!(verdict(&r), Vec::<Failure>::new());
    r.peak_hz = Some(r.tune_hz - r.peak_tol_hz - 1.0);
    assert_eq!(verdict(&r)[0].rule, Rule::NoPeak);
}

#[test]
fn the_sim_readout_is_deterministic() {
    assert_eq!(preset("rf-am", 100.1e6), preset("rf-am", 100.1e6));
}

#[test]
fn the_json_carries_the_contract_fields_and_the_source() {
    let json = to_json(&preset("rf-reference", 100.1e6));
    for key in [
        "dongle_serial",
        "tune_hz",
        "peak_hz",
        "peak_tol_hz",
        "class",
        "confidence",
        "decode",
        "snr_db",
        "source",
    ] {
        assert!(json.contains(&format!("\"{key}\":")), "{key}: {json}");
    }
    assert!(json.contains(r#""source":"sim""#), "{json}");
    assert!(json.contains(r#""pass":false"#), "{json}");
    assert!(json.contains(r#""rule":"empty-decode""#), "{json}");
    assert_eq!(quote("a\"b\\c\n"), r#""a\"b\\c\n""#);
}

/// Argument parsing picks the source and stops there: nothing is opened.
#[test]
fn without_sim_the_source_is_the_dongle_and_parsing_opens_nothing() {
    let parse = |args: &[&str]| {
        let cli = crate::Cli::try_parse_from(args).unwrap();
        let crate::Cmd::Sdr {
            cmd: SdrCmd::Smoke(a),
        } = cli.cmd
        else {
            panic!("not sdr smoke");
        };
        a
    };
    let hw = parse(&["neowon", "sdr", "smoke", "--freq", "100.1e6"]);
    assert_eq!(hw.source(), Source::Rtl { serial: None });
    let sim = parse(&[
        "neowon", "sdr", "smoke", "--freq", "100.1e6", "--sim", "rf-am", "--seed", "3",
    ]);
    assert_eq!(
        sim.source(),
        Source::Sim {
            scene: "rf-am".into(),
            seed: 3
        }
    );
    assert_eq!(sim.params().centre_hz(), 100.1e6 - 250e3);
    // A serial only names a dongle; it cannot be combined with the sim.
    assert!(
        crate::Cli::try_parse_from([
            "neowon", "sdr", "smoke", "--freq", "1e8", "--sim", "rf-am", "--serial", "x"
        ])
        .is_err()
    );
}

#[test]
fn an_unknown_scene_is_refused_with_the_list() {
    let err = sim_backend("rf-nope", SEED).err().unwrap().to_string();
    assert!(err.contains("rf-reference"), "{err}");
}

#[test]
fn civil_dates() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(11_016), "2000-02-29");
    assert_eq!(civil_date(20_719), "2026-09-23");
}
