//! Phase 10.4: survey diff over the simulated SDR (fixed order).
//!
//! Two surveys of 100.0–104.9 MHz in three 1.64 MHz steps. Between them:
//! - step 1: a tone is removed (`gone`), one is raised 12 dB (`stronger`),
//!   one is added (`new`), one is unchanged (`same`);
//! - step 2: two strong tones and four weak ones; the second survey keeps
//!   only 3 peaks per step, so the three weakest are `unknown` (truncated
//!   below the kept floor), not `gone`;
//! - step 3: scanned before, skipped after: everything there is `unknown`.
//!
//! `cargo test -p neowon-sdr --test survey_diff -- --nocapture`

use std::time::Duration;

use neowon_backend::{Backend, InstrumentConfig, SdrConfig};
use neowon_sdr::survey::{Change, Survey, SurveyPlan, SurveyResult, diff};
use neowon_sim::{RfScene, SimSdrBackend, em};

fn run(backend: &mut SimSdrBackend, plan: SurveyPlan) -> SurveyResult {
    let mut s = Survey::new(plan);
    while let Some(centre) = s.next_centre() {
        backend
            .apply(&InstrumentConfig::Sdr(SdrConfig {
                centre_hz: centre,
                sample_rate: s.plan().sample_rate,
                ..Default::default()
            }))
            .unwrap();
        loop {
            if let Some(f) = backend.poll_frame(Duration::from_millis(200)).unwrap()
                && s.feed(&f)
            {
                break;
            }
        }
    }
    s.finish()
}

fn plan(peak_cap: usize, skip: Vec<(f64, f64)>) -> SurveyPlan {
    SurveyPlan {
        start_hz: 100.0e6,
        stop_hz: 104.9e6,
        peak_cap,
        skip,
        ..Default::default()
    }
}

#[test]
fn survey_diff_classifies_every_change_honestly() {
    let step2 = [
        em(102.0e6, 0.3),
        em(102.3e6, 0.3),
        em(101.9e6, 0.030),
        em(102.6e6, 0.025),
        em(102.8e6, 0.020),
        em(103.0e6, 0.015),
    ];
    let before_scene = RfScene {
        emitters: [
            &[em(100.2e6, 0.1), em(100.5e6, 0.1), em(101.0e6, 0.05)][..],
            &step2,
            &[em(103.8e6, 0.1), em(104.4e6, 0.1)],
        ]
        .concat(),
        noise_rms: 0.01,
    };
    let after_scene = RfScene {
        emitters: [
            &[em(100.2e6, 0.1), em(100.7e6, 0.1), em(101.0e6, 0.2)][..],
            &step2,
            &[em(103.8e6, 0.1), em(104.4e6, 0.1)],
        ]
        .concat(),
        noise_rms: 0.01,
    };
    let before = run(
        &mut SimSdrBackend::with_scene(before_scene),
        plan(32, vec![]),
    );
    // The after survey skips step 3 and keeps 3 peaks per step.
    let after = run(
        &mut SimSdrBackend::with_scene(after_scene),
        plan(3, vec![(103.4e6, 105.0e6)]),
    );
    assert_eq!(before.coverage.len(), 3);
    assert!(before.coverage.iter().all(|c| c.scanned && !c.truncated));
    assert!(after.coverage[1].truncated && !after.coverage[2].scanned);

    let rows = diff(&before, &after, 2e3, 3.0);
    for r in &rows {
        println!(
            r#"{{"mhz":{:.4},"change":"{}","before_dbfs":{},"after_dbfs":{}}}"#,
            r.centre_hz / 1e6,
            r.change.label(),
            r.before
                .as_ref()
                .map_or("null".into(), |p| format!("{:.1}", p.power_dbfs)),
            r.after
                .as_ref()
                .map_or("null".into(), |p| format!("{:.1}", p.power_dbfs)),
        );
    }
    let got: Vec<(f64, &str)> = rows
        .iter()
        .map(|r| ((r.centre_hz / 1e5).round() / 10.0, r.change.label()))
        .collect();
    let want = vec![
        (100.2, "same"),
        (100.5, "gone"),
        (100.7, "new"),
        (101.0, "stronger"),
        (101.9, "same"), // the strongest weak tone is within the cap
        (102.0, "same"),
        (102.3, "same"),
        (102.6, "unknown"),
        (102.8, "unknown"),
        (103.0, "unknown"),
        (103.8, "unknown"),
        (104.4, "unknown"),
    ];
    assert_eq!(got, want);
    // Nothing the after survey could not have seen is called gone or new.
    for r in &rows {
        let p = r.before.as_ref().or(r.after.as_ref()).unwrap().power_dbfs;
        let visible = after
            .band_of(r.centre_hz)
            .is_some_and(|c| c.scanned && (!c.truncated || p >= c.retained_power_floor_dbfs));
        if !visible {
            assert!(!matches!(r.change, Change::Gone | Change::New), "{r:?}");
        }
    }
}
