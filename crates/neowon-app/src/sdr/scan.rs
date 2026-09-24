//! Surveys in SDR mode: the `neowon_dsp::survey` state
//! machine driven through the supervisor. Each completed step retunes to
//! the next; the last two results are kept so `get surveydiff` can
//! compare them.

use neowon_core::CaptureFrame;
use neowon_dsp::survey::{Survey, SurveyPlan, SurveyResult, diff};

use super::SdrState;

/// Results kept (the latest two are diffed).
const KEEP: usize = 2;

/// Start a survey: tune to its first step.
pub fn start(sdr: &mut SdrState, plan: SurveyPlan) -> Result<(), String> {
    if plan.stop_hz <= plan.start_hz {
        return Err("survey: stop must be above start".into());
    }
    let s = Survey::new(plan);
    let Some(first) = s.next_centre() else {
        return Err("survey: every step is skipped".into());
    };
    sdr.config.centre_hz = first;
    sdr.config.sample_rate = s.plan().sample_rate;
    sdr.dirty = true;
    sdr.dab_reset();
    sdr.survey = Some(s);
    Ok(())
}

/// Feed a frame to the running survey; retune or finish as it asks.
pub fn feed(sdr: &mut SdrState, frame: &CaptureFrame) {
    let Some(s) = sdr.survey.as_mut() else { return };
    // Frames tuned elsewhere (in flight before the retune) are not this
    // step's; the survey's own settle frames cover the rest.
    let Some(centre) = s.next_centre() else {
        return;
    };
    if (sdr.config.centre_hz - centre).abs() > 1.0 {
        return;
    }
    if !s.feed(frame) {
        return;
    }
    match s.next_centre() {
        Some(next) => {
            sdr.config.centre_hz = next;
            sdr.dirty = true;
            // The survey moved the hardware: the DAB table was decoded from
            // the previous step's window.
            sdr.dab_reset();
        }
        None => {
            let done = sdr.survey.take().expect("running").finish();
            sdr.surveys.push(done);
            if sdr.surveys.len() > KEEP {
                sdr.surveys.remove(0);
            }
        }
    }
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn result_json(r: &SurveyResult) -> String {
    let cov: Vec<String> = r
        .coverage
        .iter()
        .map(|c| {
            format!(
                concat!(
                    r#"{{"lo_hz":{},"hi_hz":{},"scanned":{},"truncated":{},"peak_cap":{},"#,
                    r#""retained_power_floor_dbfs":{}}}"#
                ),
                num(c.lo_hz),
                num(c.hi_hz),
                c.scanned,
                c.truncated,
                c.peak_cap,
                // The not-truncated / unscanned sentinels read as null.
                num(if c.retained_power_floor_dbfs.abs() > 1e300 {
                    f64::NAN
                } else {
                    c.retained_power_floor_dbfs
                }),
            )
        })
        .collect();
    let peaks: Vec<String> = r
        .peaks
        .iter()
        .map(|p| {
            format!(
                r#"{{"centre_hz":{},"bandwidth_hz":{},"power_dbfs":{},"snr_db":{}}}"#,
                num(p.centre_hz),
                num(p.bandwidth_hz),
                num(p.power_dbfs),
                num(p.snr_db)
            )
        })
        .collect();
    format!(
        r#"{{"coverage":[{}],"peaks":[{}]}}"#,
        cov.join(","),
        peaks.join(",")
    )
}

/// `get survey`: progress of a running survey and the latest result.
pub fn survey_json(sdr: &SdrState) -> String {
    format!(
        r#"{{"ok":true,"running":{},"progress":{},"completed":{},"last":{}}}"#,
        sdr.survey.is_some(),
        num(sdr.survey.as_ref().map_or(1.0, Survey::progress)),
        sdr.surveys.len(),
        sdr.surveys.last().map_or("null".into(), result_json)
    )
}

/// `get surveydiff`: the last two surveys compared (2 kHz, 3 dB).
pub fn diff_json(sdr: &SdrState) -> String {
    let [a, b] = match sdr.surveys.as_slice() {
        [.., a, b] => [a, b],
        _ => return r#"{"ok":false,"error":"need two completed surveys"}"#.into(),
    };
    let rows: Vec<String> = diff(a, b, 2e3, 3.0)
        .iter()
        .map(|r| {
            let p = |x: &Option<neowon_dsp::survey::Peak>| {
                x.as_ref().map_or("null".into(), |p| num(p.power_dbfs))
            };
            format!(
                r#"{{"centre_hz":{},"change":"{}","before_dbfs":{},"after_dbfs":{}}}"#,
                num(r.centre_hz),
                r.change.label(),
                p(&r.before),
                p(&r.after)
            )
        })
        .collect();
    format!(r#"{{"ok":true,"rows":[{}]}}"#, rows.join(","))
}
