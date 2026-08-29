//! Derived data computed per record: math trace, automatic measurements with
//! running statistics, and the FFT spectrum.

use bevy::prelude::*;
use neowon_backend::Command;
use neowon_core::ChannelCapture;
use neowon_dsp::{
    MathOp, Measurements, Spectrum, StatTrack, Window, math_trace, measure, spectrum,
};

use crate::Link;

/// Trace slots: CH1, CH2, math.
pub const SLOTS: usize = 3;
pub const SLOT_NAMES: [&str; SLOTS] = ["CH1", "CH2", "M"];

pub const N_METRICS: usize = 18;

/// Per-sample pass/fail envelope, in raw counts.
#[derive(Debug, Clone)]
pub struct PfMask {
    pub lo: Vec<i8>,
    pub hi: Vec<i8>,
}

/// Build the envelope from a captured reference trace: dilate horizontally by
/// `h_div` divisions (min/max over the window), then pad vertically by
/// `v_div` divisions of raw counts (250 counts = 10 divs), saturating.
pub fn build_pf_mask(reference: &[i8], h_div: f64, v_div: f64) -> PfMask {
    let len = reference.len();
    let win = ((h_div / 20.0) * len as f64).round().max(1.0) as usize;
    let half = win / 2;
    let pad = ((v_div / 10.0) * 250.0).round() as i16;
    let mut lo = Vec::with_capacity(len);
    let mut hi = Vec::with_capacity(len);
    for i in 0..len {
        let from = i.saturating_sub(half);
        let to = (i + half + 1).min(len);
        let mut mn = reference[from];
        let mut mx = reference[from];
        for &r in &reference[from..to] {
            mn = mn.min(r);
            mx = mx.max(r);
        }
        lo.push((mn as i16 - pad).clamp(-128, 127) as i8);
        hi.push((mx as i16 + pad).clamp(-128, 127) as i8);
    }
    PfMask { lo, hi }
}

/// True when every sample of `samples` stays within `[lo, hi]`.
pub fn evaluate_pf(mask: &PfMask, samples: &[i8]) -> bool {
    let n = mask.lo.len().min(samples.len());
    (0..n).all(|i| samples[i] >= mask.lo[i] && samples[i] <= mask.hi[i])
}

/// Pass/fail rule engine state.
#[derive(Resource)]
pub struct PfState {
    pub enabled: bool,
    pub source_slot: usize,
    /// Time tolerance, in horizontal divisions.
    pub h_div: f64,
    /// Voltage tolerance, in vertical divisions.
    pub v_div: f64,
    pub mask: Option<PfMask>,
    pub pass: u64,
    pub fail: u64,
    pub stop_on_fail: bool,
    pub output_multi: bool,
}

impl Default for PfState {
    fn default() -> Self {
        Self {
            enabled: false,
            source_slot: 0,
            h_div: 1.0,
            v_div: 0.5,
            mask: None,
            pass: 0,
            fail: 0,
            stop_on_fail: false,
            output_multi: false,
        }
    }
}

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
        Self {
            enabled: false,
            source: 0,
            window: Window::Hann,
            spectrum: None,
        }
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

/// One auto-measurement row: label, accessor, display unit.
pub type Metric = (&'static str, fn(&Measurements) -> Option<f64>, Unit);

pub const METRICS: [Metric; N_METRICS] = [
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
    mut link: ResMut<Link>,
    mut math: ResMut<MathState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
    mut pf: ResMut<PfState>,
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
        let src = if math.op.needs_b() {
            a.zip(b).map(|(a, _)| a)
        } else {
            a
        };
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

    // Pass/fail: compare the source slot's trace against the mask.
    let pf_result = if pf.enabled {
        slot_caps[pf.source_slot.min(SLOTS - 1)]
            .zip(pf.mask.as_ref())
            .map(|(cap, mask)| evaluate_pf(mask, &cap.raw))
    } else {
        None
    };
    if let Some(passed) = pf_result {
        if passed {
            pf.pass += 1;
        } else {
            pf.fail += 1;
        }
        if pf.output_multi {
            let _ = link.sup.commands.send(Command::PassFail(passed));
        }
        if pf.stop_on_fail && !passed {
            link.config.running = false;
            link.dirty = true;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pf_mask_vertical_pad() {
        // Zero horizontal tolerance, one division of vertical pad
        // (1 div = 25 counts).
        let m = build_pf_mask(&[10, -10], 0.0, 1.0);
        assert_eq!(m.lo, vec![-15, -35]);
        assert_eq!(m.hi, vec![35, 15]);
    }

    #[test]
    fn pf_mask_saturates() {
        let m = build_pf_mask(&[120, -120], 0.0, 2.0);
        assert_eq!(m.hi[0], 127);
        assert_eq!(m.lo[1], -128);
    }

    #[test]
    fn pf_mask_horizontal_dilation() {
        // 20-sample record; h_div = 20 divs -> window of the full record, so
        // every position sees the central min/max spikes.
        let mut reference = vec![0i8; 20];
        reference[9] = -40;
        reference[10] = 50;
        let m = build_pf_mask(&reference, 20.0, 0.0);
        assert!(m.lo.iter().all(|&l| l == -40));
        assert!(m.hi.iter().all(|&h| h == 50));

        // No horizontal tolerance: the envelope follows the reference.
        let m = build_pf_mask(&reference, 0.0, 0.0);
        assert_eq!(m.lo, reference);
        assert_eq!(m.hi, reference);
    }

    #[test]
    fn pf_evaluate() {
        let m = build_pf_mask(&[0, 10, -10], 0.0, 1.0);
        assert!(evaluate_pf(&m, &[0, 10, -10]));
        assert!(evaluate_pf(&m, &[20, 10, -10])); // within +25 pad
        assert!(!evaluate_pf(&m, &[0, 10, 50])); // outside envelope
    }
}
