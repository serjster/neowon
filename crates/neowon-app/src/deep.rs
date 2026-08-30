//! Deep view: the display as a window onto the acquisition *timeline*
//! rather than onto one record.
//!
//! The instrument hands over short records — 5000 samples on the VDS1022 —
//! separated by dead time it spends transferring and re-arming. Showing one
//! record means the time you can see is `record_len / sample_rate`, so
//! zooming out has to slow the sample rate and eventually aliases the signal
//! away. Showing the *timeline* instead keeps the rate and spans as much
//! history as the scrollback holds, with the dead time drawn as gaps.
//!
//! Nothing here is specific to a small-buffer instrument: a streaming source
//! produces back-to-back segments and simply never yields a gap. The
//! reduction itself is `neowon_dsp::timeline`, which is engine-free and
//! unit-tested; this module only decides *which* window to show and feeds
//! the result to the renderer.
//!
//! The trace goes to `Phosphor.deep` and nowhere else. Measurements, math,
//! FFT and pass/fail all read `link.latest`, so they keep seeing one real
//! record — per-acquisition, as on a bench scope — and cannot be corrupted
//! by a reduced envelope.

use bevy::prelude::*;
use neowon_dsp::timeline::{self, Segment};

use crate::Link;
use crate::gpu::{CHANNELS, DeepFrame, Phosphor};
use crate::record::Recorder;

/// Display columns the timeline is reduced to. One pair per plot column;
/// the wave buffer holds 5000 values, so 1000 columns (2000 values) plus the
/// gap mask fits with room to spare.
pub const COLUMNS: usize = 1000;

/// Window durations the zoom steps through, seconds — a 1-2-5 ladder, the
/// same progression a time base uses.
pub const SPAN_LADDER: [f64; 13] = [
    0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0,
];

#[derive(Resource)]
pub struct DeepView {
    /// Is the display showing the timeline rather than a single record?
    pub on: bool,
    /// Window duration in seconds.
    pub span: f64,
    /// Window end on the session clock; `None` follows live acquisition.
    pub anchor: Option<f64>,
    /// Fraction of the last built window that was actually acquired.
    pub coverage: f64,
    /// Discontinuities in the last built window.
    pub gap_count: usize,
    /// Records that contributed to the last built window.
    pub records: usize,
    rev: u64,
}

impl Default for DeepView {
    fn default() -> Self {
        Self {
            on: false,
            span: 1.0,
            anchor: None,
            coverage: 0.0,
            gap_count: 0,
            records: 0,
            rev: 0,
        }
    }
}

impl DeepView {
    /// Seconds per division of the displayed window.
    pub fn seconds_per_div(&self) -> f64 {
        self.span / crate::ui::layout::H_DIVS as f64
    }

    /// Fraction of the window with no acquisition behind it.
    pub fn lost(&self) -> f64 {
        (1.0 - self.coverage).clamp(0.0, 1.0)
    }
}

/// Turn the timeline view on or off. Turning it on clears the within-record
/// zoom window: the two are different ways of choosing what the x axis
/// means, and stacking them would make both readouts lie.
pub fn set_on(deep: &mut DeepView, phosphor: &mut Phosphor, on: bool) {
    deep.on = on;
    if on {
        crate::view::hview_home(phosphor);
    } else {
        deep.anchor = None;
        phosphor.deep = None;
    }
}

/// Step the window one rung of the ladder. Returns false when already at the
/// end, which is how the caller knows to hand back to the record view.
pub fn span_step(deep: &mut DeepView, wider: bool) -> bool {
    let idx = SPAN_LADDER
        .iter()
        .position(|&s| s >= deep.span - 1e-12)
        .unwrap_or(0);
    let next = if wider { idx + 1 } else { idx.wrapping_sub(1) };
    match SPAN_LADDER.get(next) {
        Some(&s) if wider || idx > 0 => {
            deep.span = s;
            true
        }
        _ => false,
    }
}

/// Scroll the window through history by a fraction of its own width.
/// Reaching the live end drops the anchor so the view follows again.
pub fn pan(deep: &mut DeepView, newest: f64, dfrac: f64) {
    let anchor = deep.anchor.unwrap_or(newest);
    let next = anchor - dfrac * deep.span;
    if next >= newest {
        deep.anchor = None;
    } else {
        deep.anchor = Some(next);
    }
}

/// Rebuild the displayed timeline from the scrollback ring.
///
/// Runs after `record_frames` so the newest record is already in the ring.
pub fn build(
    rec: Res<Recorder>,
    link: Res<Link>,
    mut deep: ResMut<DeepView>,
    mut phosphor: ResMut<Phosphor>,
) {
    if !deep.on {
        if phosphor.deep.is_some() {
            phosphor.deep = None;
        }
        return;
    }
    let Some(newest) = rec.frames.last().map(|f| f.t_start() + f.duration()) else {
        return;
    };
    let t1 = deep.anchor.unwrap_or(newest);
    let window = (t1 - deep.span, t1);

    // Only the records overlapping the window; the ring is time-ordered, so
    // this could binary-search, but a linear scan over a few thousand Arc
    // headers is not what costs here.
    let mut used = 0usize;
    let mut pairs: [Vec<i8>; CHANNELS] = Default::default();
    let mut enabled = [false; CHANNELS];
    let mut gaps: Vec<u32> = Vec::new();
    let mut coverage = 0.0;
    for ch in 0..CHANNELS {
        let mut segs: Vec<Segment<'_>> = Vec::new();
        for f in &rec.frames {
            let t0 = f.t_start();
            if t0 + f.duration() <= window.0 || t0 >= window.1 {
                continue;
            }
            if let Some(cap) = f.channels.iter().find(|c| c.ch == ch) {
                segs.push(Segment {
                    t0,
                    sample_rate: f.sample_rate,
                    raw: &cap.raw,
                    tiles: None,
                });
            }
        }
        if segs.is_empty() {
            pairs[ch] = vec![timeline::NO_DATA; COLUMNS * 2];
            continue;
        }
        enabled[ch] = true;
        used = used.max(segs.len());
        let r = timeline::reduce(&segs, window, COLUMNS);
        // Gaps and coverage are a property of the acquisition, not of a
        // channel, so take them from whichever channel is on.
        if r.coverage > coverage || gaps.is_empty() {
            coverage = r.coverage;
            gaps = r.gaps.clone();
        }
        pairs[ch] = r.pairs;
    }
    // The math trace has no timeline of its own.
    let _ = &link;

    deep.coverage = coverage;
    deep.gap_count = gaps.len();
    deep.records = used;
    deep.rev = deep.rev.wrapping_add(1);
    phosphor.deep = Some(std::sync::Arc::new(DeepFrame {
        rev: deep.rev,
        columns: COLUMNS,
        pairs,
        enabled,
        gaps,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_steps_the_ladder_and_reports_the_end() {
        let mut d = DeepView {
            span: SPAN_LADDER[0],
            ..Default::default()
        };
        assert!(!span_step(&mut d, false), "already at the narrowest");
        assert!(span_step(&mut d, true));
        assert_eq!(d.span, SPAN_LADDER[1]);
        d.span = *SPAN_LADDER.last().unwrap();
        assert!(!span_step(&mut d, true), "already at the widest");
    }

    #[test]
    fn panning_back_anchors_and_panning_forward_follows_live() {
        let mut d = DeepView {
            span: 2.0,
            ..Default::default()
        };
        // Positive means "drag the waveform right", i.e. look further back.
        pan(&mut d, 100.0, 0.5);
        assert_eq!(d.anchor, Some(99.0));
        pan(&mut d, 100.0, 0.5);
        assert_eq!(d.anchor, Some(98.0));
        // Scrolling forward past the newest data releases the anchor, so the
        // view resumes following live acquisition.
        pan(&mut d, 100.0, -5.0);
        assert_eq!(d.anchor, None);
    }

    #[test]
    fn lost_fraction_is_the_complement_of_coverage() {
        let d = DeepView {
            coverage: 0.71,
            ..Default::default()
        };
        assert!((d.lost() - 0.29).abs() < 1e-9);
    }
}
