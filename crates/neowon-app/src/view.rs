//! View manipulation — zoom, pan, home reset. Shared by the dock toolbar,
//! pointer gestures (`ui/touch.rs`), keyboard shortcuts, and script actions:
//! one code path for every entry point.
//!
//! Zoom and pan act on acquisition parameters the way a bench scope does:
//! vertical zoom = volts/div, horizontal zoom = sample rate, vertical pan =
//! channel offset, horizontal pan = trigger position.

use neowon_backend::ScopeConfig;

use crate::Link;
use crate::gpu::Phosphor;
use crate::ui::widgets::FALLBACK_VDIV;

/// Narrowest horizontal zoom window (100x).
pub const HVIEW_MIN_SPAN: f64 = 0.01;

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

/// Pan one step: left/right slide the horizontal zoom window (content
/// follows the arrow), up/down move the selected channel's offset by a
/// tenth of full scale.
pub fn pan(link: &mut Link, phosphor: &mut Phosphor, dir: Pan) {
    const STEP: f64 = 0.1;
    match dir {
        // Content follows the arrow, like dragging the waveform that way:
        // pan left slides it left, revealing later samples.
        Pan::Left => hview_pan(phosphor, STEP * phosphor.hview.1),
        Pan::Right => hview_pan(phosphor, -STEP * phosphor.hview.1),
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
            stimulus: String::new(),
            selected: 0,
        }
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
        let mut p = Phosphor {
            hview: (0.5, 0.5),
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
