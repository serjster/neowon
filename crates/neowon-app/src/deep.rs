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
/// Window durations the zoom steps through, seconds. It reaches down into
/// microseconds so that engaging the timeline at a fast time base does not
/// jump straight to a window thousands of times longer than the record,
/// which is what made the display almost entirely gap.
pub const SPAN_LADDER: [f64; 25] = [
    1e-5, 2e-5, 5e-5, 1e-4, 2e-4, 5e-4, 1e-3, 2e-3, 5e-3, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0,
    2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0,
];

/// How the window tracks live acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Follow {
    /// Fill a fixed page, then turn to the next one. The content stays put
    /// while the page fills, so nothing moves horizontally — a strip chart.
    #[default]
    Page,
    /// The window ends at the newest data and slides as it arrives. Shows
    /// the most recent data at the right edge always, at the cost of the
    /// whole trace marching left: at a short time base each record advances
    /// it by many columns, and the records at the edges come and go.
    Slide,
}

impl Follow {
    pub fn name(self) -> &'static str {
        match self {
            Follow::Page => "page",
            Follow::Slide => "slide",
        }
    }
}

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
    /// Discontinuities in the window — breaks in the signal, not blank
    /// columns. Widening the window shows more dead time but gives each
    /// interval fewer columns, so a column count moves the wrong way.
    pub gap_count: usize,
    /// Records that contributed to the last built window.
    pub records: usize,
    /// Lay the segments end to end and mark each join with a single column,
    /// instead of giving dead time the screen width its duration deserves.
    /// The x axis is then not time, so time readouts are suppressed.
    pub collapse: bool,
    /// How the window tracks live acquisition.
    pub follow: Follow,
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
            collapse: false,
            follow: Follow::Page,
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
        // The zoom window magnifies *inside one record*; the timeline is a
        // different axis entirely. Leaving both on drew the record's zoomed
        // trace over the timeline's gap markers.
        crate::view::set_zoom(phosphor, false);
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
    // Belt and braces with the greyed-out Zoom group: the reduced trace is
    // not a record, so a record-space zoom window would re-map x without
    // re-mapping the gap mask and the markers would part company with the
    // signal.
    if phosphor.zoom_on || phosphor.hview != (0.5, 1.0) {
        crate::view::set_zoom(&mut phosphor, false);
    }
    let col_dt = deep.span / COLUMNS as f64;
    let t1 = match deep.anchor {
        // Parked in history: exactly where the user left it.
        Some(a) => a,
        // Page: the window is a fixed slice of the session clock, so it does
        // not move at all while it fills and then turns over in one step.
        // Sliding it to end at the newest sample instead means every record
        // shifts the whole trace left — by many columns at a short time base
        // — and the records at either edge come and go, which reads as the
        // display jittering.
        None => match deep.follow {
            Follow::Page => (newest / deep.span).floor() * deep.span + deep.span,
            Follow::Slide => newest,
        },
    };
    // Snap to a whole column either way: a fractional edge shifts every
    // column on each rebuild and makes the gap markers shimmer.
    let t1 = (t1 / col_dt).floor() * col_dt;
    let window = (t1 - deep.span, t1);
    // The part of a page that has not happened yet is not a gap in the
    // capture, so it must not be marked as one or counted against coverage.
    let live = newest.min(window.1);
    let live_col = (((live - window.0) / col_dt).ceil().max(0.0) as usize).min(COLUMNS);

    // Only the records overlapping the window; the ring is time-ordered, so
    // this could binary-search, but a linear scan over a few thousand Arc
    // headers is not what costs here.
    let mut used = 0usize;
    let mut pairs: [Vec<i8>; CHANNELS] = Default::default();
    let mut enabled = [false; CHANNELS];
    let mut gaps: Vec<u32> = Vec::new();
    let mut breaks = 0usize;
    let mut coverage = 0.0;
    // Only the frames overlapping the window, found by search rather than
    // by scanning the ring: with a large scrollback the scan alone cost more
    // than the reduction did.
    let from = rec.first_after(window.0);
    for ch in 0..CHANNELS {
        let mut segs: Vec<Segment<'_>> = Vec::new();
        for (i, f) in rec.frames.iter().enumerate().skip(from) {
            let t0 = f.t_start();
            if t0 >= window.1 {
                break;
            }
            if let Some((k, cap)) = f.channels.iter().enumerate().find(|(_, c)| c.ch == ch) {
                segs.push(Segment {
                    t0,
                    sample_rate: f.sample_rate,
                    raw: &cap.raw,
                    // Summaries make a wide window affordable: a column
                    // spanning thousands of samples reads a handful of
                    // tiles instead of every one of them.
                    tiles: rec.tiles.get(i).and_then(|t| t.get(k)),
                });
            }
        }
        if segs.is_empty() {
            pairs[ch] = vec![timeline::NO_DATA; COLUMNS * 2];
            continue;
        }
        enabled[ch] = true;
        used = used.max(segs.len());
        #[allow(clippy::let_and_return)]
        let r = if deep.collapse {
            timeline::reduce_collapsed(&segs, window, COLUMNS)
        } else {
            timeline::reduce(&segs, window, COLUMNS)
        };
        // Gaps and coverage are a property of the acquisition, not of a
        // channel, so take them from whichever channel is on.
        // Coverage is over what has actually elapsed, not over the whole
        // page: the future is not dead time.
        let elapsed = (live - window.0).max(0.0);
        let scale = if elapsed > 0.0 {
            (window.1 - window.0) / elapsed
        } else {
            0.0
        };
        let cov = (r.coverage * scale).clamp(0.0, 1.0);
        let trimmed: Vec<u32> = r
            .gaps
            .iter()
            .copied()
            .filter(|&c| (c as usize) < live_col)
            .collect();
        if cov > coverage || gaps.is_empty() {
            coverage = cov;
            breaks = trimmed.windows(2).filter(|w| w[1] != w[0] + 1).count()
                + usize::from(!trimmed.is_empty());
            gaps = trimmed;
        }
        pairs[ch] = r.pairs;
    }
    // The math trace has no timeline of its own.
    let _ = &link;

    deep.coverage = coverage;
    deep.gap_count = breaks;
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
