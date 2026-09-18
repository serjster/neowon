//! Scanning and survey (Phase 10.4): sweep a frequency range in tuning
//! steps, keep the peaks each step detects, and record exactly what was
//! and was not looked at, so a later survey can be compared honestly.
//!
//! The engine is a state machine fed frames (`next_centre`, `feed`,
//! `finish`), so the same logic runs over a `Backend` directly (tests,
//! CLI) or through the app's supervisor.
//!
//! The honesty rule: a signal missing from a band that was not scanned,
//! or whose band was truncated at a peak cap with the signal below what
//! was kept, is `unknown`, never `gone`; likewise nothing there is `new`.

use neowon_catalog::BandCoverage;
use neowon_core::CaptureFrame;
use neowon_dsp::{DetectConfig, detect};

#[derive(Debug, Clone, PartialEq)]
pub struct SurveyPlan {
    pub start_hz: f64,
    pub stop_hz: f64,
    /// IQ rate per step, pairs/s.
    pub sample_rate: f64,
    /// Fraction of each step's band used (the edges roll off in the
    /// tuner's filter); steps advance by this much of the rate.
    pub usable: f64,
    /// Frames discarded after each retune (settling), then frames kept.
    pub settle_frames: usize,
    pub dwell_frames: usize,
    /// Most peaks kept per step; beyond it the step is truncated.
    pub peak_cap: usize,
    pub threshold_db: f64,
    pub nfft: usize,
    /// Bands deliberately not scanned (excluded ranges), Hz.
    pub skip: Vec<(f64, f64)>,
}

impl Default for SurveyPlan {
    fn default() -> Self {
        Self {
            start_hz: 88e6,
            stop_hz: 108e6,
            sample_rate: 2.048e6,
            usable: 0.8,
            settle_frames: 1,
            dwell_frames: 2,
            peak_cap: 32,
            threshold_db: 12.0,
            nfft: 4096,
            skip: Vec::new(),
        }
    }
}

/// A signal a survey kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Peak {
    pub centre_hz: f64,
    pub bandwidth_hz: f64,
    pub power_dbfs: f64,
    pub snr_db: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurveyResult {
    pub plan: SurveyPlan,
    /// One entry per step, in frequency order.
    pub coverage: Vec<BandCoverage>,
    /// Kept peaks, in frequency order.
    pub peaks: Vec<Peak>,
}

impl SurveyResult {
    /// The step band containing `hz`, if any.
    pub fn band_of(&self, hz: f64) -> Option<&BandCoverage> {
        self.coverage
            .iter()
            .find(|c| (c.lo_hz..c.hi_hz).contains(&hz))
    }
}

/// Step centres covering the plan's range.
pub fn steps(plan: &SurveyPlan) -> Vec<f64> {
    let width = plan.sample_rate * plan.usable;
    let mut v = Vec::new();
    let mut c = plan.start_hz + width / 2.0;
    while c - width / 2.0 < plan.stop_hz {
        v.push(c);
        c += width;
    }
    v
}

pub struct Survey {
    plan: SurveyPlan,
    centres: Vec<f64>,
    step: usize,
    /// Frames seen in the current step (settling included).
    seen: usize,
    /// IQ gathered for the current step.
    iq: Vec<f32>,
    coverage: Vec<BandCoverage>,
    peaks: Vec<Peak>,
}

impl Survey {
    pub fn new(plan: SurveyPlan) -> Self {
        let centres = steps(&plan);
        let mut s = Self {
            plan,
            centres,
            step: 0,
            seen: 0,
            iq: Vec::new(),
            coverage: Vec::new(),
            peaks: Vec::new(),
        };
        s.skip_unscanned();
        s
    }

    pub fn plan(&self) -> &SurveyPlan {
        &self.plan
    }

    /// Fraction of steps done.
    pub fn progress(&self) -> f64 {
        self.step as f64 / self.centres.len().max(1) as f64
    }

    /// Where to tune next; `None` when the survey is complete.
    pub fn next_centre(&self) -> Option<f64> {
        self.centres.get(self.step).copied()
    }

    fn band(&self, centre: f64) -> (f64, f64) {
        let half = self.plan.sample_rate * self.plan.usable / 2.0;
        (centre - half, centre + half)
    }

    /// Record steps that fall in a skipped range as unscanned, without
    /// tuning to them.
    fn skip_unscanned(&mut self) {
        while let Some(c) = self.next_centre() {
            let (lo, hi) = self.band(c);
            if !self.plan.skip.iter().any(|&(a, b)| lo < b && hi > a) {
                break;
            }
            self.coverage.push(BandCoverage {
                lo_hz: lo,
                hi_hz: hi,
                scanned: false,
                bins: 0,
                threshold_db: self.plan.threshold_db,
                truncated: false,
                peak_cap: self.plan.peak_cap,
                selection_rule: "not scanned".into(),
                retained_power_floor_dbfs: f64::MAX,
            });
            self.step += 1;
        }
    }

    /// Feed one frame captured at the current step's centre. Returns true
    /// when the step is complete (the caller then tunes to `next_centre`).
    pub fn feed(&mut self, frame: &CaptureFrame) -> bool {
        let Some(centre) = self.next_centre() else {
            return false;
        };
        self.seen += 1;
        if self.seen <= self.plan.settle_frames {
            return false;
        }
        self.iq.extend_from_slice(&frame.channels[0].data);
        if self.seen < self.plan.settle_frames + self.plan.dwell_frames {
            return false;
        }
        self.close_step(centre, frame.sample_rate);
        true
    }

    fn close_step(&mut self, centre: f64, rate: f64) {
        let (lo, hi) = self.band(centre);
        let pairs = self.iq.len() / 2;
        let cfg = DetectConfig {
            nfft: self.plan.nfft,
            blocks: (pairs / self.plan.nfft).max(1),
            threshold_db: self.plan.threshold_db,
            ..Default::default()
        };
        let mut found: Vec<Peak> = detect(&self.iq, rate, centre, 0.0, &cfg)
            .into_iter()
            .filter(|o| (lo..hi).contains(&o.centre_hz))
            .map(|o| Peak {
                centre_hz: o.centre_hz,
                bandwidth_hz: o.bandwidth_hz(),
                power_dbfs: o.power_dbfs,
                snr_db: o.snr_db,
            })
            .collect();
        // Strongest first; keep the cap and say where the cut fell.
        found.sort_by(|a, b| b.power_dbfs.total_cmp(&a.power_dbfs));
        let truncated = found.len() > self.plan.peak_cap;
        found.truncate(self.plan.peak_cap);
        // Nothing was cut: every detected power was kept (a finite
        // sentinel; catalog values must survive JSON).
        let floor = if truncated {
            found.last().map_or(f64::MAX, |p| p.power_dbfs)
        } else {
            f64::MIN
        };
        self.coverage.push(BandCoverage {
            lo_hz: lo,
            hi_hz: hi,
            scanned: true,
            bins: self.plan.nfft,
            threshold_db: self.plan.threshold_db,
            truncated,
            peak_cap: self.plan.peak_cap,
            selection_rule: "strongest".into(),
            retained_power_floor_dbfs: floor,
        });
        self.peaks.extend(found);
        self.iq.clear();
        self.seen = 0;
        self.step += 1;
        self.skip_unscanned();
    }

    pub fn finish(mut self) -> SurveyResult {
        self.peaks
            .sort_by(|a, b| a.centre_hz.total_cmp(&b.centre_hz));
        SurveyResult {
            plan: self.plan,
            coverage: self.coverage,
            peaks: self.peaks,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    New,
    Gone,
    Stronger,
    Weaker,
    Same,
    /// The other survey could not have seen it: its band was not scanned,
    /// or was truncated with this signal below the kept floor.
    Unknown,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::New => "new",
            Change::Gone => "gone",
            Change::Stronger => "stronger",
            Change::Weaker => "weaker",
            Change::Same => "same",
            Change::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub centre_hz: f64,
    pub change: Change,
    pub before: Option<Peak>,
    pub after: Option<Peak>,
}

/// Whether `survey` could have seen a signal of `power` at `hz`.
fn could_see(survey: &SurveyResult, hz: f64, power: f64) -> bool {
    match survey.band_of(hz) {
        Some(c) => c.scanned && (!c.truncated || power >= c.retained_power_floor_dbfs),
        None => false,
    }
}

/// Compare two surveys, in frequency order. Peaks match when their
/// centres are within `freq_tol_hz`; a matched pair differing by more than
/// `power_tol_db` is stronger or weaker.
pub fn diff(
    before: &SurveyResult,
    after: &SurveyResult,
    freq_tol_hz: f64,
    power_tol_db: f64,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut used = vec![false; after.peaks.len()];
    for b in &before.peaks {
        let m = after
            .peaks
            .iter()
            .enumerate()
            .filter(|(i, a)| !used[*i] && (a.centre_hz - b.centre_hz).abs() <= freq_tol_hz)
            .min_by(|x, y| {
                let d = |p: &Peak| (p.centre_hz - b.centre_hz).abs();
                d(x.1).total_cmp(&d(y.1))
            })
            .map(|(i, _)| i);
        let (change, after_peak) = match m {
            Some(i) => {
                used[i] = true;
                let a = &after.peaks[i];
                let d = a.power_dbfs - b.power_dbfs;
                let c = if d > power_tol_db {
                    Change::Stronger
                } else if d < -power_tol_db {
                    Change::Weaker
                } else {
                    Change::Same
                };
                (c, Some(a.clone()))
            }
            None if could_see(after, b.centre_hz, b.power_dbfs) => (Change::Gone, None),
            None => (Change::Unknown, None),
        };
        rows.push(DiffRow {
            centre_hz: b.centre_hz,
            change,
            before: Some(b.clone()),
            after: after_peak,
        });
    }
    for (i, a) in after.peaks.iter().enumerate() {
        if used[i] {
            continue;
        }
        let change = if could_see(before, a.centre_hz, a.power_dbfs) {
            Change::New
        } else {
            Change::Unknown
        };
        rows.push(DiffRow {
            centre_hz: a.centre_hz,
            change,
            before: None,
            after: Some(a.clone()),
        });
    }
    rows.sort_by(|x, y| x.centre_hz.total_cmp(&y.centre_hz));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_cover_the_range() {
        let p = SurveyPlan {
            start_hz: 100e6,
            stop_hz: 104e6,
            ..Default::default()
        };
        let s = steps(&p);
        let w = p.sample_rate * p.usable;
        assert_eq!(s.first().copied(), Some(100e6 + w / 2.0));
        assert!(s.last().unwrap() + w / 2.0 >= 104e6);
        assert!(s.windows(2).all(|x| (x[1] - x[0] - w).abs() < 1e-6));
    }
}
