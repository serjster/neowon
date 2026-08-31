//! View manipulation — zoom, pan, home reset. Shared by the dock toolbar,
//! pointer gestures (`ui/touch.rs`), keyboard shortcuts, and script actions:
//! one code path for every entry point.
//!
//! The horizontal controls follow the bench-scope model (docs/tasks/
//! phase78-lab-semantics-spec.md §1):
//!
//! - **Time base** (s/div) is the primary horizontal control. The record
//!   always spans the graticule's 10 divisions, so `s/div = record_len /
//!   (sample_rate x 10)`; turning it slower steps down the sample-rate
//!   ladder, which is how a scope reaches seconds per division.
//! - **Horizontal position** (trigger delay) slides the trigger point
//!   through the record — the acquisition control, not a display pan.
//! - **Zoom** (delayed sweep) is a *secondary* magnified window into the
//!   already-acquired record (`Phosphor.hview`), off by default. While it
//!   is on, the horizontal zoom gestures drive the window instead of the
//!   time base, exactly like a scope's Zoom mode.
//!
//! Vertical: zoom = volts/div, pan = channel offset.

use neowon_backend::ScopeConfig;

use crate::Link;
use crate::gpu::Phosphor;
use crate::ui::layout::H_DIVS;
use crate::ui::widgets::FALLBACK_VDIV;

/// Narrowest horizontal zoom window (100x).
pub const HVIEW_MIN_SPAN: f64 = 0.01;

/// Samples per record before capabilities arrive (VDS1022/sim shape).
pub const FALLBACK_RECORD_LEN: usize = 5000;

/// Record length of the attached instrument.
pub fn record_len(link: &Link) -> usize {
    link.caps
        .as_ref()
        .map(|c| c.record_len())
        .unwrap_or(FALLBACK_RECORD_LEN)
}

/// Seconds per division for a sample rate: the record spans the graticule.
pub fn s_per_div(rate: f64, record_len: usize) -> f64 {
    record_len as f64 / rate.max(1e-12) / H_DIVS as f64
}

/// The sample rate that puts `s_div` on one division.
pub fn rate_for_s_per_div(s_div: f64, record_len: usize) -> f64 {
    record_len as f64 / (s_div.max(1e-15) * H_DIVS as f64)
}

/// The sample-rate ladder the instrument offers.
pub fn rate_ladder(link: &Link) -> Vec<f64> {
    link.caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| crate::ui::widgets::FALLBACK_RATES.to_vec())
}

/// The achievable time-base ladder in s/div, fastest (smallest) first.
pub fn timebase_ladder(link: &Link) -> Vec<f64> {
    let n = record_len(link);
    let mut l: Vec<f64> = rate_ladder(link).iter().map(|&r| s_per_div(r, n)).collect();
    l.sort_by(f64::total_cmp);
    l
}

/// Current time base, s/div.
pub fn timebase(link: &Link) -> f64 {
    s_per_div(link.config.sample_rate, record_len(link))
}

/// Set the time base to the ladder rung nearest `s_div`.
pub fn set_timebase(link: &mut Link, s_div: f64) {
    let n = record_len(link);
    let want = rate_for_s_per_div(s_div, n);
    let ladder = rate_ladder(link);
    let rate = ladder
        .iter()
        .copied()
        .min_by(|a, b| {
            let da = (a.ln() - want.max(1e-12).ln()).abs();
            let db = (b.ln() - want.max(1e-12).ln()).abs();
            da.total_cmp(&db)
        })
        .unwrap_or(want);
    if rate != link.config.sample_rate {
        link.config.sample_rate = rate;
        link.dirty = true;
    }
}

/// Step the time base one rung. `slower` = more seconds per division (a
/// wider time window), which means a slower sample rate.
pub fn timebase_step(link: &mut Link, slower: bool) {
    let ladder = rate_ladder(link);
    let rate = step_ladder(&ladder, link.config.sample_rate, !slower);
    if rate != link.config.sample_rate {
        link.config.sample_rate = rate;
        link.dirty = true;
    }
}

/// Below this sample rate the VDS1022 runs in roll mode (docs/protocol-
/// vds1022.md) — on a 5000-point record that is 200 ms/div, the same
/// threshold Rigol's MSO5000 uses.
pub const ROLL_RATE: f64 = 2500.0;

/// Does this sample rate put the instrument in roll mode?
pub fn is_roll(rate: f64) -> bool {
    rate < ROLL_RATE
}

/// Is the zoom (delayed-sweep) window active? Off = the window is the whole
/// record, which is the plain main time base.
pub fn zoom_active(p: &Phosphor) -> bool {
    p.zoom_on
}

/// Turn the zoom window on (half the record) or off (whole record).
pub fn set_zoom(p: &mut Phosphor, on: bool) {
    p.zoom_on = on;
    p.hview = if on {
        hview_clamp(p.hview.0, p.hview.1.min(0.5))
    } else {
        (0.5, 1.0)
    };
}

/// The horizontal zoom control, with a scope's mode split: the time base
/// when the zoom window is off, the zoom window when it is on. `anchor` is
/// a record fraction (e.g. under the pointer) and only matters when zoomed.
///
/// While the acquisition is stopped the time base has nothing to re-acquire,
/// so it zooms the stored record instead — the InfiniiVision rule: "When
/// running, adjusting the horizontal scale knob changes the sample rate.
/// When stopped, adjusting the horizontal scale knob lets you zoom into
/// acquired data."
pub fn hzoom(link: &mut Link, p: &mut Phosphor, anchor: f64, inward: bool) {
    if zoom_active(p) || !link.config.running {
        hview_zoom(p, anchor, inward);
    } else {
        timebase_step(link, !inward);
    }
}

/// The horizontal zoom control including the timeline, in three bands from
/// most zoomed-in to most zoomed-out:
///
/// | state | zoom in | zoom out |
/// |---|---|---|
/// | timeline on | narrower window; hand back to the record once it fits | wider window |
/// | zoom window on | widen toward the whole record | widen; at the record, engage the timeline |
/// | whole record | narrow the zoom window | engage the timeline |
///
/// The sample rate is deliberately absent: zooming is a display choice, and
/// the time base stays the acquisition control it is on a bench scope. That
/// is the whole point — you can span seconds without giving up resolution.
pub fn hzoom_timeline(
    link: &mut Link,
    p: &mut Phosphor,
    deep: &mut crate::deep::DeepView,
    anchor: f64,
    inward: bool,
) {
    let record_s = record_len(link) as f64 / link.config.sample_rate.max(1e-12);
    if deep.on {
        if inward {
            // Narrower, until the window fits inside one record — then the
            // record itself is the better view, at full resolution.
            if !crate::deep::span_step(deep, false) || deep.span <= record_s {
                crate::deep::set_on(deep, p, false);
            }
        } else {
            crate::deep::span_step(deep, true);
        }
        return;
    }
    if inward || zoom_active(p) {
        hview_zoom(p, anchor, inward);
        return;
    }
    // Zooming out from the whole record: rather than slowing the sample rate
    // and aliasing the signal away, span more time using history.
    crate::deep::set_on(deep, p, true);
    deep.span = crate::deep::SPAN_LADDER
        .iter()
        .copied()
        .find(|&s| s > record_s * 1.5)
        .unwrap_or(record_s * 2.0);
}

/// The horizontal position control including the timeline: scroll the window
/// through history while it is on, otherwise the trigger delay / zoom window.
pub fn hposition_timeline(
    link: &mut Link,
    p: &mut Phosphor,
    deep: &mut crate::deep::DeepView,
    newest: f64,
    dfrac: f64,
) {
    if deep.on {
        crate::deep::pan(deep, newest, dfrac);
    } else {
        hposition(link, p, dfrac);
    }
}

/// The horizontal position control, with the same mode split: the trigger
/// delay when the zoom window is off (the acquisition control), the zoom
/// window's position when it is on. `dfrac` is a fraction of the visible
/// width, positive = the waveform moves right.
pub fn hposition(link: &mut Link, p: &mut Phosphor, dfrac: f64) {
    if zoom_active(p) {
        hview_pan(p, -dfrac * p.hview.1);
    } else {
        let pos = (link.config.position + dfrac).clamp(0.0, 1.0);
        if pos != link.config.position {
            link.config.position = pos;
            link.dirty = true;
        }
    }
}

/// Clamp a (center, span) window inside the record.
pub fn hview_clamp(center: f64, span: f64) -> (f64, f64) {
    let span = span.clamp(HVIEW_MIN_SPAN, 1.0);
    let center = center.clamp(span / 2.0, 1.0 - span / 2.0);
    (center, span)
}

/// Zoom the horizontal view around `anchor` (record fraction, e.g. under
/// the pointer); `inward` = a narrower window. The anchor stays put on
/// screen, like the spectrum window's zoom.
pub fn hview_zoom(p: &mut Phosphor, anchor: f64, inward: bool) {
    let (c, s) = p.hview;
    let k = if inward { 0.5 } else { 2.0 };
    let new_span = (s * k).clamp(HVIEW_MIN_SPAN, 1.0);
    let anchor = anchor.clamp(0.0, 1.0);
    let frac = if s > 1e-9 {
        ((anchor - (c - s / 2.0)) / s).clamp(0.0, 1.0)
    } else {
        0.5
    };
    p.hview = hview_clamp(anchor - frac * new_span + new_span / 2.0, new_span);
}

/// Pan the horizontal view by `dcenter` record fractions (clamped).
pub fn hview_pan(p: &mut Phosphor, dcenter: f64) {
    let (c, s) = p.hview;
    p.hview = hview_clamp(c + dcenter, s);
}

/// Reset the horizontal view to the whole record.
pub fn hview_home(p: &mut Phosphor) {
    p.hview = (0.5, 1.0);
}

/// The configuration the app boots with — also what `home` restores the
/// view geometry to. Defaults matched to the 1 kHz probe-comp signal
/// through a x10 probe.
pub fn startup_config() -> ScopeConfig {
    let mut config = ScopeConfig::default();
    config.channels[0].volts_div = 0.2;
    config.trigger.level = 0.25;
    config
}

/// Nearest-rung ladder step (log distance), clamped at both ends.
pub fn step_ladder(ladder: &[f64], current: f64, up: bool) -> f64 {
    let mut idx = 0;
    let mut best = f64::MAX;
    for (i, &v) in ladder.iter().enumerate() {
        let d = (v.ln() - current.max(1e-12).ln()).abs();
        if d < best {
            best = d;
            idx = i;
        }
    }
    let idx = if up {
        (idx + 1).min(ladder.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    ladder[idx]
}

fn vdiv_ladder(link: &Link) -> Vec<f64> {
    link.caps
        .as_ref()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| FALLBACK_VDIV.to_vec())
}

/// Vertical zoom one ladder rung on `ch`; `inward` = finer volts/div.
pub fn zoom_channel(link: &mut Link, ch: usize, inward: bool) {
    let ladder = vdiv_ladder(link);
    let v = link.config.channels[ch].volts_div;
    link.config.channels[ch].volts_div = step_ladder(&ladder, v, !inward);
    link.dirty = true;
}

/// Pan direction — the arrows move the waveform as if dragged that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pan {
    Left,
    Right,
    Up,
    Down,
}

/// Pan one step: left/right are the horizontal position control (trigger
/// delay, or the zoom window while zoomed) with the waveform following the
/// arrow; up/down move the selected channel's offset by a tenth of full
/// scale.
pub fn pan(link: &mut Link, phosphor: &mut Phosphor, dir: Pan) {
    const STEP: f64 = 0.1;
    match dir {
        // Content follows the arrow, like dragging the waveform that way:
        // pan left slides it left, revealing later samples.
        Pan::Left => hposition(link, phosphor, -STEP),
        Pan::Right => hposition(link, phosphor, STEP),
        Pan::Up | Pan::Down => {
            let sel = link.selected.min(1);
            let d = if dir == Pan::Up { STEP } else { -STEP };
            let off = (link.config.channels[sel].offset + d).clamp(-0.5, 0.5);
            link.config.channels[sel].offset = off;
            link.dirty = true;
        }
    }
}

/// Reset zoom and centring to the startup defaults (volts/div, sample rate,
/// offsets, trigger position) plus the horizontal zoom window. The trigger
/// level is not view state and is left alone.
pub fn home(link: &mut Link, phosphor: &mut Phosphor) {
    let d = startup_config();
    for ch in 0..2 {
        link.config.channels[ch].volts_div = d.channels[ch].volts_div;
        link.config.channels[ch].offset = 0.0;
    }
    link.config.sample_rate = d.sample_rate;
    link.config.position = d.position;
    link.dirty = true;
    hview_home(phosphor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_backend::Backend;

    fn link() -> Link {
        let sup = neowon_backend::spawn(|| -> Result<Box<dyn Backend>, String> {
            Err("no hardware in tests".into())
        });
        Link {
            sup,
            caps: None,
            status: String::new(),
            latest: None,
            config: startup_config(),
            dirty: false,
            frames_seen: 0,
            multi: neowon_backend::MultiMode::TriggerOut,
            last_frame_at: 0.0,
            arrived: Vec::new(),
            stimulus: String::new(),
            selected: 0,
        }
    }

    #[test]
    fn timebase_is_record_over_rate_over_divisions() {
        // Rigol's MDepth = SRate x TScale x HDivs, solved for the scale.
        assert!((s_per_div(250e3, 5000) - 2e-3).abs() < 1e-12);
        assert!((s_per_div(2.5, 5000) - 200.0).abs() < 1e-9);
        assert!((rate_for_s_per_div(2e-3, 5000) - 250e3).abs() < 1e-6);
    }

    #[test]
    fn timebase_zooms_out_into_seconds_per_division() {
        // The complaint this phase fixes: at 250 kS/s the horizontal control
        // stopped at the 20 ms record. Stepping the time base slower walks
        // the rate ladder down to whole seconds per division.
        let mut link = link();
        link.config.sample_rate = 250e3;
        assert!((timebase(&link) - 2e-3).abs() < 1e-12);
        let mut p = Phosphor::default();
        // "I remember setting the zoom to about 5 seconds": the ladder's
        // neighbouring rungs are 4 s/div (125 S/s) and 10 s/div (50 S/s).
        let mut rungs = Vec::new();
        for _ in 0..24 {
            hzoom(&mut link, &mut p, 0.5, false);
            rungs.push(timebase(&link));
        }
        assert!(
            rungs.iter().any(|&t| (t - 4.0).abs() < 1e-9),
            "seconds-per-division is not reachable by zooming out: {rungs:?}"
        );
        // Bottoms out at the slowest rate, 200 s/div on a 5000-point record.
        assert_eq!(link.config.sample_rate, 2.5);
        assert!((timebase(&link) - 200.0).abs() < 1e-9);
        // The zoom window stayed out of it: this is the acquisition control.
        assert_eq!(p.hview, (0.5, 1.0));
    }

    #[test]
    fn set_timebase_snaps_to_a_reachable_rung() {
        let mut link = link();
        set_timebase(&mut link, 5.0); // 5 s/div -> 100 S/s wanted
        // Nearest rung on the ladder (125 S/s -> 4 s/div).
        assert!(rate_ladder(&link).contains(&link.config.sample_rate));
        assert!(timebase(&link) > 1.0 && timebase(&link) < 20.0);
        assert!(link.dirty);
    }

    #[test]
    fn zoom_window_takes_over_the_horizontal_zoom() {
        let mut link = link();
        link.config.sample_rate = 250e3;
        let mut p = Phosphor::default();
        assert!(!zoom_active(&p));
        set_zoom(&mut p, true);
        assert!(zoom_active(&p));
        let rate = link.config.sample_rate;
        hzoom(&mut link, &mut p, 0.5, true);
        // While zoomed the time base is untouched — the window narrows.
        assert_eq!(link.config.sample_rate, rate);
        assert!((p.hview.1 - 0.25).abs() < 1e-9);
        // Widening the window back to the whole record must not switch the
        // mode off under the user — that unchecked the box mid-drag.
        p.hview = hview_clamp(0.5, 1.0);
        assert!(zoom_active(&p), "zoom at 1x is still zoom mode");
        set_zoom(&mut p, false);
        assert!(!zoom_active(&p));
        assert_eq!(p.hview, (0.5, 1.0));
    }

    #[test]
    fn stopped_acquisition_zooms_the_stored_record() {
        // InfiniiVision rule: running = sample rate, stopped = zoom memory.
        let mut link = link();
        link.config.running = false;
        let rate = link.config.sample_rate;
        let mut p = Phosphor::default();
        hzoom(&mut link, &mut p, 0.5, true);
        assert_eq!(link.config.sample_rate, rate);
        assert!((p.hview.1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn horizontal_position_is_the_trigger_delay_until_zoomed() {
        let mut link = link();
        let mut p = Phosphor::default();
        let pos = link.config.position;
        hposition(&mut link, &mut p, 0.1);
        assert!((link.config.position - (pos + 0.1)).abs() < 1e-9);
        assert_eq!(p.hview, (0.5, 1.0), "not a display pan while unzoomed");
        // Zoomed, the same control moves the window and leaves the
        // acquisition alone.
        set_zoom(&mut p, true);
        let pos = link.config.position;
        hposition(&mut link, &mut p, 0.1);
        assert_eq!(link.config.position, pos);
        assert!((p.hview.0 - 0.45).abs() < 1e-9);
    }

    #[test]
    fn roll_threshold_matches_the_instrument() {
        assert!(is_roll(250.0));
        assert!(!is_roll(2500.0));
        // 2.5 kS/s on a 5000-point record is 200 ms/div.
        assert!((s_per_div(ROLL_RATE, 5000) - 0.2).abs() < 1e-12);
    }

    #[test]
    fn ladder_steps_and_clamps() {
        let l = [0.1, 0.2, 0.5, 1.0];
        assert_eq!(step_ladder(&l, 0.2, true), 0.5);
        assert_eq!(step_ladder(&l, 0.2, false), 0.1);
        assert_eq!(step_ladder(&l, 1.0, true), 1.0);
        assert_eq!(step_ladder(&l, 0.1, false), 0.1);
        // Snaps to the nearest rung first (log distance).
        assert_eq!(step_ladder(&l, 0.24, true), 0.5);
    }

    #[test]
    fn zoom_steps_the_ladders() {
        let mut link = link();
        zoom_channel(&mut link, 0, true); // 0.2 -> finer
        assert_eq!(link.config.channels[0].volts_div, 0.1);
        zoom_channel(&mut link, 0, false);
        zoom_channel(&mut link, 0, false); // back to coarser
        assert_eq!(link.config.channels[0].volts_div, 0.5);
        assert!(link.dirty);
    }

    #[test]
    fn pan_moves_window_and_offset() {
        let mut link = link();
        // Zoomed to the middle half: pans slide the window, clamped inside.
        // Zoom is an explicit mode, so the flag is part of that state.
        let mut p = Phosphor {
            hview: (0.5, 0.5),
            zoom_on: true,
            ..Default::default()
        };
        pan(&mut link, &mut p, Pan::Left);
        assert!((p.hview.0 - 0.55).abs() < 1e-9);
        pan(&mut link, &mut p, Pan::Right);
        assert!((p.hview.0 - 0.5).abs() < 1e-9);
        // Content follows the arrow: repeated left pans slide the window
        // toward the record's end and clamp there.
        for _ in 0..40 {
            pan(&mut link, &mut p, Pan::Left);
        }
        assert!((p.hview.0 + p.hview.1 / 2.0 - 1.0).abs() < 1e-9);
        // Vertical pans move the selected channel offset.
        pan(&mut link, &mut p, Pan::Up);
        assert!((link.config.channels[0].offset - 0.1).abs() < 1e-9);
        pan(&mut link, &mut p, Pan::Down);
        pan(&mut link, &mut p, Pan::Down);
        assert!((link.config.channels[0].offset + 0.1).abs() < 1e-9);
    }

    #[test]
    fn home_restores_startup_view() {
        let mut link = link();
        let mut p = Phosphor::default();
        link.config.channels[0].volts_div = 5.0;
        link.config.channels[0].offset = 0.4;
        link.config.channels[1].offset = -0.3;
        link.config.sample_rate = 100e6;
        link.config.position = 0.9;
        link.config.trigger.level = 3.3;
        p.hview = (0.2, 0.1);
        home(&mut link, &mut p);
        let d = startup_config();
        assert_eq!(link.config.channels[0].volts_div, d.channels[0].volts_div);
        assert_eq!(link.config.channels[1].volts_div, d.channels[1].volts_div);
        assert_eq!(link.config.channels[0].offset, 0.0);
        assert_eq!(link.config.channels[1].offset, 0.0);
        assert_eq!(link.config.sample_rate, d.sample_rate);
        assert_eq!(link.config.position, d.position);
        // Trigger level is not view state.
        assert_eq!(link.config.trigger.level, 3.3);
        assert_eq!(p.hview, (0.5, 1.0));
        assert!(link.dirty);
    }

    #[test]
    fn hview_zoom_keeps_the_anchor_and_clamps() {
        let mut p = Phosphor::default();
        // Zoom in around the record center: window halves, stays centred.
        hview_zoom(&mut p, 0.5, true);
        assert!((p.hview.1 - 0.5).abs() < 1e-9);
        assert!((p.hview.0 - 0.5).abs() < 1e-9);
        // Anchor at the visible left edge: it stays at the left edge.
        let left = p.hview.0 - p.hview.1 / 2.0;
        hview_zoom(&mut p, left, true);
        assert!((p.hview.0 - p.hview.1 / 2.0 - left).abs() < 1e-9);
        // Zoom out from full view clamps at the whole record.
        hview_home(&mut p);
        hview_zoom(&mut p, 0.5, false);
        assert_eq!(p.hview, (0.5, 1.0));
        // Repeated zoom-in bottoms out at the minimum span, still centred.
        for _ in 0..40 {
            hview_zoom(&mut p, 0.5, true);
        }
        assert_eq!(p.hview.1, HVIEW_MIN_SPAN);
    }

    #[test]
    fn hview_pan_stays_inside_the_record() {
        let mut p = Phosphor::default();
        hview_zoom(&mut p, 0.5, true); // span 0.5
        hview_pan(&mut p, -10.0);
        let (c, s) = p.hview;
        assert!((c - s / 2.0).abs() < 1e-9, "clamped at the left edge");
        hview_pan(&mut p, 10.0);
        let (c, s) = p.hview;
        assert!(
            (c + s / 2.0 - 1.0).abs() < 1e-9,
            "clamped at the right edge"
        );
    }
}
