//! Derived data computed per record: math trace, automatic measurements with
//! running statistics, and the FFT spectrum.

use bevy::prelude::*;
use neowon_core::ChannelCapture;
use neowon_dsp::{math_trace, measure, spectrum, MathOp, Measurements, Spectrum, StatTrack, Window};

use crate::Link;

/// Trace slots: CH1, CH2, math.
pub const SLOTS: usize = 3;
pub const SLOT_NAMES: [&str; SLOTS] = ["CH1", "CH2", "M"];

pub const N_METRICS: usize = 18;

#[derive(Resource)]
pub struct MathState {
    pub enabled: bool,
    pub op: MathOp,
    /// Full-scale of the math trace (10 divisions), in the op's unit.
    pub full_scale: f64,
    /// Re-autoscale on the next record.
    pub rescale: bool,
    pub trace: Option<ChannelCapture>,
}

impl Default for MathState {
    fn default() -> Self {
        Self {
            enabled: false,
            op: MathOp::Add,
            full_scale: 10.0,
            rescale: true,
            trace: None,
        }
    }
}

#[derive(Resource, Default)]
pub struct MeasureState {
    pub last_seq: u64,
    pub latest: [Option<Measurements>; SLOTS],
    pub stats: Vec<[StatTrack; N_METRICS]>,
    /// Which slot the statistics columns show.
    pub stats_slot: usize,
    pub sample_rate: f64,
}

impl MeasureState {
    pub fn reset_stats(&mut self) {
        for s in &mut self.stats {
            for t in s.iter_mut() {
                t.reset();
            }
        }
    }
}

#[derive(Resource)]
pub struct FftState {
    pub enabled: bool,
    /// Source slot index.
    pub source: usize,
    pub window: Window,
    pub spectrum: Option<Spectrum>,
}

impl Default for FftState {
    fn default() -> Self {
        Self { enabled: false, source: 0, window: Window::Hann, spectrum: None }
    }
}

/// Metric table: label, extractor, unit.
#[derive(Clone, Copy)]
pub enum Unit {
    Volt,
    Second,
    Hertz,
    Percent,
}

pub const METRICS: [(&str, fn(&Measurements) -> Option<f64>, Unit); N_METRICS] = [
    ("Freq", |m| m.freq, Unit::Hertz),
    ("Period", |m| m.period, Unit::Second),
    ("Vpp", |m| Some(m.vpp), Unit::Volt),
    ("Vmax", |m| Some(m.vmax), Unit::Volt),
    ("Vmin", |m| Some(m.vmin), Unit::Volt),
    ("Vtop", |m| Some(m.vtop), Unit::Volt),
    ("Vbase", |m| Some(m.vbase), Unit::Volt),
    ("Vamp", |m| Some(m.vamp), Unit::Volt),
    ("Vavg", |m| Some(m.vavg), Unit::Volt),
    ("Vrms", |m| Some(m.vrms), Unit::Volt),
    ("Rise", |m| m.rise, Unit::Second),
    ("Fall", |m| m.fall, Unit::Second),
    ("+Width", |m| m.pwidth, Unit::Second),
    ("-Width", |m| m.nwidth, Unit::Second),
    ("+Duty", |m| m.pduty, Unit::Percent),
    ("-Duty", |m| m.nduty, Unit::Percent),
    ("Overshoot", |m| m.overshoot, Unit::Percent),
    ("Preshoot", |m| m.preshoot, Unit::Percent),
];

pub fn compute_derived(
    link: Res<Link>,
    mut math: ResMut<MathState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
) {
    let Some(frame) = &link.latest else { return };
    if frame.seq == meas.last_seq {
        return;
    }
    meas.last_seq = frame.seq;
    meas.sample_rate = frame.sample_rate;
    if meas.stats.len() != SLOTS {
        meas.stats = vec![[StatTrack::default(); N_METRICS]; SLOTS];
    }

    // Math trace.
    math.trace = None;
    if math.enabled {
        let a = frame.channels.iter().find(|c| c.ch == 0);
        let b = frame.channels.iter().find(|c| c.ch == 1);
        let src = if math.op.needs_b() { a.zip(b).map(|(a, _)| a) } else { a };
        if let Some(a) = src {
            let fs = (!math.rescale).then_some(math.full_scale);
            let (trace, fs) = math_trace(a, b, math.op, frame.sample_rate, fs);
            math.full_scale = fs;
            math.rescale = false;
            math.trace = Some(trace);
        }
    }

    // Measurements + stats per slot.
    let mut slot_caps: [Option<&ChannelCapture>; SLOTS] = [None; SLOTS];
    for cap in &frame.channels {
        if cap.ch < 2 {
            slot_caps[cap.ch] = Some(cap);
        }
    }
    slot_caps[2] = math.trace.as_ref();
    for (slot, cap) in slot_caps.iter().enumerate() {
        let m = cap.and_then(|c| measure(c, frame.sample_rate));
        meas.latest[slot] = m;
        if let Some(m) = m {
            for (i, (_, get, _)) in METRICS.iter().enumerate() {
                if let Some(v) = get(&m) {
                    meas.stats[slot][i].update(v);
                }
            }
        }
    }

    // Spectrum.
    fft.spectrum = if fft.enabled {
        slot_caps[fft.source.min(SLOTS - 1)]
            .and_then(|c| spectrum(&c.raw, c.volts_per_lsb, frame.sample_rate, fft.window, 4096))
    } else {
        None
    };
}

/// Engineering formatting: value with SI prefix per unit.
pub fn fmt(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Percent => format!("{:.1} %", v * 100.0),
        Unit::Volt => fmt_si(v, "V"),
        Unit::Second => fmt_si(v, "s"),
        Unit::Hertz => fmt_si(v, "Hz"),
    }
}

pub fn fmt_si(v: f64, unit: &str) -> String {
    let a = v.abs();
    let (scale, prefix) = if a >= 1e6 {
        (1e-6, "M")
    } else if a >= 1e3 {
        (1e-3, "k")
    } else if a >= 1.0 || a == 0.0 {
        (1.0, "")
    } else if a >= 1e-3 {
        (1e3, "m")
    } else if a >= 1e-6 {
        (1e6, "µ")
    } else {
        (1e9, "n")
    };
    format!("{:.4} {}{}", v * scale, prefix, unit)
}
