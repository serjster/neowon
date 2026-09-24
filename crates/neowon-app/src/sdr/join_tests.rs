//! A measurement is only ever reported beside the identity of the
//! signal it was measured on. The `rf-digital` scene has two tracks: 16QAM
//! at 99.6 MHz (the one nearest the cursor here) and QPSK at 100.3 MHz,
//! whose channel power is the higher of the two. Each test puts the lab's
//! result on one of them and asserts every readout that joins it to a
//! detection joins by track id — or says the ids differ.

use neowon_backend::Backend;

use super::analysis::{self, LabResult};
use super::*;

/// The scene tracked, the cursor on 16QAM. Returns the state (with the
/// spectrum `get modmeas` needs), the last frame, and the tracks
/// (16QAM, QPSK).
pub(crate) fn two_signals() -> (SdrState, SharedFrame, neowon_dsp::Track, neowon_dsp::Track) {
    let mut b = neowon_sim::SimSdrBackend::new();
    b.apply(&InstrumentConfig::Sdr(SdrConfig::default()))
        .unwrap();
    assert!(b.set_stimulus("rf-digital").unwrap());
    let mut sdr = SdrState {
        active: true,
        analyse_on: true,
        tuned_hz: 99.6e6,
        ..Default::default()
    };
    let mut next = || loop {
        if let Some(f) = b.poll_frame(std::time::Duration::from_millis(100)).unwrap() {
            return f;
        }
    };
    for _ in 0..100 {
        let frame = next();
        track(&mut sdr, &frame, None);
        if sdr.tracker.active().count() == 2 {
            let spec = iq_spectrum(
                &frame.channels[0].data,
                frame.sample_rate,
                Window::Hann,
                sdr.fft_size,
            );
            sdr.spectrum = spec;
            let near = |hz: f64| {
                sdr.tracker
                    .active()
                    .find(|t| (t.last.centre_hz - hz).abs() < 50e3)
                    .cloned()
                    .expect("both signals tracked")
            };
            let (qam, qpsk) = (near(99.6e6), near(100.3e6));
            return (sdr, frame, qam, qpsk);
        }
    }
    panic!("rf-digital never gave two active tracks");
}

/// The lab's result for `t`, as its worker would return it.
fn run(frame: &SharedFrame, t: &neowon_dsp::Track) -> LabResult {
    LabResult {
        centre_hz: 100e6,
        track: t.id,
        setting: None,
        analysis: analysis::analyse(frame, 100e6, t, None),
        classification: analysis::classify(frame, 100e6, t),
    }
}

/// A top-level field of a flat JSON reply, as text.
fn raw<'a>(json: &'a str, key: &str) -> &'a str {
    let k = format!("\"{key}\":");
    let at = json
        .find(&k)
        .unwrap_or_else(|| panic!("{key} missing: {json}"))
        + k.len();
    let rest = &json[at..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim_matches('"')
}

/// The id inside the `lab` block, if the block is present.
fn lab_track(json: &str) -> Option<u64> {
    let at = json.find(r#""lab":{"track":"#)?;
    raw(&json[at + 7..], "track").parse().ok()
}

#[test]
fn the_premise_holds_the_stronger_signal_is_not_the_one_tuned() {
    // Without this the tests below could not tell "nearest" from
    // "strongest".
    let (sdr, _, qam, qpsk) = two_signals();
    let strongest = sdr
        .tracker
        .active()
        .max_by(|a, b| a.last.power_dbfs.total_cmp(&b.last.power_dbfs))
        .unwrap();
    assert_eq!(strongest.id, qpsk.id);
    assert_eq!(sdr.nearest_track().unwrap().id, qam.id);
}

#[test]
fn modmeas_reports_the_lab_beside_the_signal_it_measured() {
    let (mut sdr, frame, qam, _) = two_signals();
    adopt(&mut sdr, run(&frame, &qam));
    let m = readout::modmeas_json(&sdr);
    assert_eq!(raw(&m, "id"), qam.id.to_string(), "{m}");
    assert_eq!(lab_track(&m), Some(qam.id), "{m}");
    assert_eq!(raw(&m, "lab_other_track"), "null", "{m}");
    assert_eq!(raw(&m, "modulation"), "16QAM", "{m}");
    let c: f64 = raw(&m, "centre_hz").parse().unwrap();
    assert!((c - 99.6e6).abs() < 5e3, "{m}");
}

#[test]
fn a_result_on_another_signal_is_withheld_and_named() {
    let (mut sdr, frame, qam, qpsk) = two_signals();
    // The lab's result is on QPSK (it was the target before the cursor
    // moved): held in the state, as `update` would hold it for a frame.
    let r = run(&frame, &qpsk);
    sdr.analysis = r.analysis;
    sdr.classification = r.classification;

    let m = readout::modmeas_json(&sdr);
    assert_eq!(raw(&m, "id"), qam.id.to_string(), "{m}");
    assert_eq!(lab_track(&m), None, "QPSK's numbers beside 16QAM: {m}");
    assert!(m.contains(r#""lab":null"#), "{m}");
    assert_eq!(raw(&m, "evm_rms_pct"), "null", "{m}");
    assert_eq!(raw(&m, "lab_other_track"), qpsk.id.to_string(), "{m}");

    let k = readout::classify_json(&sdr);
    assert!(
        k.contains(r#""ok":false"#),
        "QPSK's verdict as 16QAM's: {k}"
    );
    assert!(k.contains(&format!("track {}", qpsk.id)), "{k}");

    // Adopting a finished run on the signal the cursor has left: dropped.
    let (mut fresh, frame, _, qpsk) = two_signals();
    adopt(&mut fresh, run(&frame, &qpsk));
    assert!(fresh.analysis.is_none() && fresh.classification.is_none());
}

#[test]
fn classify_names_the_track_it_judged() {
    let (mut sdr, frame, qam, _) = two_signals();
    adopt(&mut sdr, run(&frame, &qam));
    let k = readout::classify_json(&sdr);
    assert!(k.contains(r#""ok":true"#), "{k}");
    assert_eq!(raw(&k, "track"), qam.id.to_string(), "{k}");
}

#[test]
fn every_lab_run_measures_the_scene_evm() {
    // A lab run on a 32 K-pair window reads 16QAM's EVM as 5–13 % against
    // the closed-form 4 % (symbol SNR (1.25 / 0.05)²), so the window is the
    // whole frame. Every run, on every frame of the deterministic scene,
    // must be within the suite's 1 % tolerance.
    let mut b = neowon_sim::SimSdrBackend::new();
    b.apply(&InstrumentConfig::Sdr(SdrConfig::default()))
        .unwrap();
    assert!(b.set_stimulus("rf-digital").unwrap());
    let mut sdr = SdrState {
        tuned_hz: 99.6e6,
        ..Default::default()
    };
    let mut runs = 0;
    for _ in 0..40 {
        let frame = loop {
            if let Some(f) = b.poll_frame(std::time::Duration::from_millis(100)).unwrap() {
                break f;
            }
        };
        track(&mut sdr, &frame, None);
        let Some(t) = sdr.nearest_track().cloned() else {
            continue;
        };
        let a = analysis::analyse(&frame, 100e6, &t, Some(neowon_core::Modulation::Qam16))
            .expect("analysed");
        assert!(
            (a.evm_rms_pct - 100.0 * 0.05 / 1.25).abs() < 1.0,
            "frame {}: EVM {:.2} % (MER {:.2} dB, {:.1} sym/s)",
            frame.seq,
            a.evm_rms_pct,
            a.mer_db,
            a.symbol_rate_hz
        );
        runs += 1;
    }
    assert!(runs >= 30, "only {runs} runs");
}
