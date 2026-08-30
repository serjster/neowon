//! View manipulation — zoom, pan, home reset. Shared by the dock toolbar,
//! pointer gestures (`ui/touch.rs`), keyboard shortcuts, and script actions:
//! one code path for every entry point.
//!
//! Zoom and pan act on acquisition parameters the way a bench scope does:
//! vertical zoom = volts/div, horizontal zoom = sample rate, vertical pan =
//! channel offset, horizontal pan = trigger position.

use neowon_backend::ScopeConfig;

use crate::Link;
use crate::ui::widgets::{FALLBACK_RATES, FALLBACK_VDIV};

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

fn rate_ladder(link: &Link) -> Vec<f64> {
    link.caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| FALLBACK_RATES.to_vec())
}

fn vdiv_ladder(link: &Link) -> Vec<f64> {
    link.caps
        .as_ref()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| FALLBACK_VDIV.to_vec())
}

/// Horizontal zoom one ladder rung; `inward` = faster rate (finer time).
pub fn zoom_rate(link: &mut Link, inward: bool) {
    let ladder = rate_ladder(link);
    link.config.sample_rate = step_ladder(&ladder, link.config.sample_rate, inward);
    link.dirty = true;
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

/// Pan one division: ±0.1 of the record (trigger position) or ±0.1 of full
/// scale (the selected channel's offset).
pub fn pan(link: &mut Link, dir: Pan) {
    const STEP: f64 = 0.1;
    match dir {
        Pan::Left => link.config.position = (link.config.position + STEP).min(1.0),
        Pan::Right => link.config.position = (link.config.position - STEP).max(0.0),
        Pan::Up | Pan::Down => {
            let sel = link.selected.min(1);
            let d = if dir == Pan::Up { STEP } else { -STEP };
            let off = (link.config.channels[sel].offset + d).clamp(-0.5, 0.5);
            link.config.channels[sel].offset = off;
        }
    }
    link.dirty = true;
}

/// Reset zoom and centring to the startup defaults (volts/div, sample rate,
/// offsets, trigger position). The trigger level is not view state and is
/// left alone.
pub fn home(link: &mut Link) {
    let d = startup_config();
    for ch in 0..2 {
        link.config.channels[ch].volts_div = d.channels[ch].volts_div;
        link.config.channels[ch].offset = 0.0;
    }
    link.config.sample_rate = d.sample_rate;
    link.config.position = d.position;
    link.dirty = true;
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
        zoom_rate(&mut link, true); // 250 kS/s -> faster
        assert_eq!(link.config.sample_rate, 2.5e6);
        assert!(link.dirty);
    }

    #[test]
    fn pan_moves_position_and_offset() {
        let mut link = link();
        assert_eq!(link.config.position, 0.5);
        pan(&mut link, Pan::Left);
        assert!((link.config.position - 0.6).abs() < 1e-9);
        pan(&mut link, Pan::Right);
        assert!((link.config.position - 0.5).abs() < 1e-9);
        // Waveform-follows-drag: right arrow slides it off the right edge.
        for _ in 0..20 {
            pan(&mut link, Pan::Right);
        }
        assert_eq!(link.config.position, 0.0);
        pan(&mut link, Pan::Up);
        assert!((link.config.channels[0].offset - 0.1).abs() < 1e-9);
        pan(&mut link, Pan::Down);
        pan(&mut link, Pan::Down);
        assert!((link.config.channels[0].offset + 0.1).abs() < 1e-9);
    }

    #[test]
    fn home_restores_startup_view() {
        let mut link = link();
        link.config.channels[0].volts_div = 5.0;
        link.config.channels[0].offset = 0.4;
        link.config.channels[1].offset = -0.3;
        link.config.sample_rate = 100e6;
        link.config.position = 0.9;
        link.config.trigger.level = 3.3;
        home(&mut link);
        let d = startup_config();
        assert_eq!(link.config.channels[0].volts_div, d.channels[0].volts_div);
        assert_eq!(link.config.channels[1].volts_div, d.channels[1].volts_div);
        assert_eq!(link.config.channels[0].offset, 0.0);
        assert_eq!(link.config.channels[1].offset, 0.0);
        assert_eq!(link.config.sample_rate, d.sample_rate);
        assert_eq!(link.config.position, d.position);
        // Trigger level is not view state.
        assert_eq!(link.config.trigger.level, 3.3);
        assert!(link.dirty);
    }
}
