//! Automatic peak detect at slow time bases.
//!
//! When the time base outruns the signal, plain sampling aliases: on the
//! attached VDS1022 a 1 kHz square at 1 s/div (500 S/s) collapses to a flat
//! 16 mV line, while peak detect on the same acquisition shows the full
//! 616 mV envelope, because the instrument keeps the min/max pair over each
//! decimation interval. Scopes offer peak detect for exactly this; this
//! module engages it for you and says so.
//!
//! The user's own choice of acquisition mode is kept in `user_acq` and is
//! what sessions persist — writing the engaged mode into `link.config.acq`
//! would bake "peak" into a saved setup with no way back.

use bevy::prelude::*;
use neowon_core::AcqMode;

use crate::Link;

/// Minimum time between mode changes. Each change is a `SET_PEAKMODE`
/// register write that restarts the acquisition, so the decision is allowed
/// to settle before it flips again.
const DWELL: f64 = 1.0;

#[derive(Resource)]
pub struct AutoPeak {
    /// Is the automatic rule active at all?
    pub on: bool,
    /// Has the rule engaged peak detect right now?
    pub engaged: bool,
    /// What the user actually selected — restored on release, saved in
    /// sessions, and never overwritten by the rule.
    pub user_acq: AcqMode,
    last_change: f64,
}

impl Default for AutoPeak {
    fn default() -> Self {
        Self {
            on: true,
            engaged: false,
            user_acq: AcqMode::Sample,
            last_change: f64::NEG_INFINITY,
        }
    }
}

impl AutoPeak {
    /// The user picked an acquisition mode: adopt it and re-evaluate.
    pub fn set_user(&mut self, acq: AcqMode) {
        self.user_acq = acq;
        self.engaged = false;
    }
}

/// The frequency the rule reasons about: the hardware meter, which counts
/// edges on the analog input and so is independent of the sample rate.
/// Measuring the record instead would be circular — once it aliases, the
/// estimate aliases with it and reports a comfortably low frequency.
fn meter_freq(link: &Link) -> Option<f64> {
    let frame = link.latest.as_ref()?;
    let src = link.config.trigger.source.min(1);
    frame
        .channels
        .iter()
        .find(|c| c.ch == src)
        .or_else(|| frame.channels.first())
        .and_then(|c| c.freq_meter)
}

pub fn update(time: Res<Time>, mut link: ResMut<Link>, mut ap: ResMut<AutoPeak>) {
    // Averaging is a deliberate choice about noise, not about resolution;
    // the rule never overrides it.
    if !ap.on || matches!(ap.user_acq, AcqMode::Average(_)) {
        if ap.engaged {
            ap.engaged = false;
            link.config.acq = ap.user_acq;
            link.dirty = true;
        }
        return;
    }
    let want = neowon_dsp::peak_advised(link.config.sample_rate, meter_freq(&link), ap.engaged);
    if want == ap.engaged {
        return;
    }
    let now = time.elapsed_secs_f64();
    if now - ap.last_change < DWELL {
        return;
    }
    ap.last_change = now;
    ap.engaged = want;
    link.config.acq = if want { AcqMode::Peak } else { ap.user_acq };
    link.dirty = true;
}
