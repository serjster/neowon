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
    /// Sticky SI band per slot × metric, shared by the measure table and
    /// the on-plot readout badges so both render identically.
    pub bands: Vec<[Band; N_METRICS]>,
    /// Which slot the statistics columns show.
    pub stats_slot: usize,
    /// Draw measurement guide lines on the plot while the Measure dialog is
    /// open.
    pub guides: bool,
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
    /// Zoomed frequency view as fractions of Nyquist (0..1).
    pub view: (f64, f64),
    /// dB axis range.
    pub db: (f32, f32),
    /// Sticky SI bands for the peak readout (amplitude, frequency).
    pub peak_bands: (Band, Band),
}

impl Default for FftState {
    fn default() -> Self {
        Self {
            enabled: false,
            source: 0,
            window: Window::Hann,
            spectrum: None,
            view: (0.0, 1.0),
            db: (-100.0, 20.0),
            peak_bands: (Band::default(), Band::default()),
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
    ("Vrms", |m| m.vrms, Unit::Volt),
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
    if meas.bands.len() != SLOTS {
        meas.bands = vec![[Band::default(); N_METRICS]; SLOTS];
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
    // Envelope records (hardware peak detect) store interleaved min/max
    // pairs, not successive samples: measuring them as a waveform yields
    // confident nonsense, so only what survives decimation is reported and
    // the rest shows as absent.
    let envelope = frame.acq == neowon_core::AcqMode::Peak;
    for (slot, cap) in slot_caps.iter().enumerate() {
        let m = cap.and_then(|c| {
            if envelope {
                neowon_dsp::measure_envelope(c)
            } else {
                measure(c, frame.sample_rate)
            }
        });
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

/// Sticky SI band for one live readout: the power-of-1000 exponent last
/// used to display it (1 = kilo, -1 = milli, …). With hysteresis a value
/// hovering at a band boundary (999.9 Hz ↔ 1000.4 Hz) keeps its band, so
/// the string layout — digit columns, decimal point, prefix, unit — stays
/// put frame after frame instead of flapping between ` 999.9  Hz` and
/// ` 1.000 kHz`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Band(Option<i8>);

fn nominal_band(a: f64) -> i8 {
    if a >= 1e9 {
        3
    } else if a >= 1e6 {
        2
    } else if a >= 1e3 {
        1
    } else if a >= 1.0 || a == 0.0 {
        0
    } else if a >= 1e-3 {
        -1
    } else if a >= 1e-6 {
        -2
    } else {
        -3
    }
}

fn band_scale(b: i8) -> f64 {
    1000f64.powi(-b as i32)
}

fn band_prefix(b: i8) -> &'static str {
    match b {
        3 => "G",
        2 => "M",
        1 => "k",
        -1 => "m",
        -2 => "µ",
        -3 => "n",
        _ => " ",
    }
}

/// Mantissa right-aligned in a fixed six-character field, four significant
/// digits when they fit, dropping decimals as needed so the field NEVER
/// widens (`" 999.9"`, `" 1.000"`, `"1000.4"`, `" -1000"`).
fn mantissa6(m: f64) -> String {
    let mut decimals = if m.abs() >= 1000.0 {
        usize::from(m >= 0.0)
    } else if m.abs() >= 100.0 {
        1
    } else if m.abs() >= 10.0 {
        2
    } else {
        3
    };
    loop {
        let s = format!("{m:>6.decimals$}");
        if s.chars().count() <= 6 || decimals == 0 {
            return s;
        }
        decimals -= 1;
    }
}

/// `fmt_si` with band hysteresis: keeps the previous band while the
/// mantissa stays within [0.5, 1050), so boundary-hovering values cannot
/// alternate layouts. Pass the same `Band` cell every frame.
pub fn fmt_si_sticky(v: f64, unit: &str, band: &mut Band) -> String {
    let a = v.abs();
    let b = match band.0 {
        // Zero carries no magnitude information — keep the current band.
        Some(b) if a == 0.0 => b,
        Some(b) if (0.5..1050.0).contains(&(a * band_scale(b))) => b,
        _ => nominal_band(a),
    };
    band.0 = Some(b);
    format!("{} {}{unit}", mantissa6(v * band_scale(b)), band_prefix(b))
}

/// Engineering formatting: value with SI prefix per unit.
pub fn fmt(v: f64, unit: Unit) -> String {
    fmt_sticky(v, unit, &mut Band::default())
}

/// `fmt` with band hysteresis (see [`fmt_si_sticky`]).
pub fn fmt_sticky(v: f64, unit: Unit, band: &mut Band) -> String {
    match unit {
        Unit::Percent => format!("{:>6.1} %", v * 100.0),
        Unit::Volt => fmt_si_sticky(v, "V", band),
        Unit::Second => fmt_si_sticky(v, "s", band),
        Unit::Hertz => fmt_si_sticky(v, "Hz", band),
    }
}

/// Character width of every `fmt` output for `unit` (`fmt_si` is
/// constant-width per unit, so this is exact, not a maximum).
pub fn fmt_width(unit: Unit) -> usize {
    match unit {
        Unit::Percent => 8,             // "  50.0 %"
        Unit::Volt | Unit::Second => 9, // " 200.0 mV"
        Unit::Hertz => 10,              // " 1.000 kHz"
    }
}

/// `fmt` with band hysteresis; `None` renders as a dash padded to the
/// exact same width and keeps the band untouched, so the layout is
/// unchanged when the value comes back.
pub fn fmt_opt_sticky(v: Option<f64>, unit: Unit, band: &mut Band) -> String {
    match v {
        Some(v) => fmt_sticky(v, unit, band),
        None => format!("{:^width$}", "—", width = fmt_width(unit)),
    }
}

/// Engineering notation the way a scope front panel writes it: an SI
/// prefix, four significant digits, the mantissa right-aligned in a
/// six-character field, and the prefix padded to one column —
/// ` 200.0 mV`, ` 1.000 kHz`, ` 999.9  Hz`. In a monospace font every
/// value of a given unit renders at a constant width, whatever its
/// magnitude or sign. For per-frame readouts use [`fmt_si_sticky`] so the
/// band cannot flap at a boundary either.
pub fn fmt_si(v: f64, unit: &str) -> String {
    fmt_si_sticky(v, unit, &mut Band::default())
}

#[cfg(test)]
mod fmt_tests {
    use super::{Band, Unit, fmt, fmt_opt_sticky, fmt_si, fmt_si_sticky, fmt_width};

    #[test]
    fn band_hysteresis_pins_layout_at_boundaries() {
        // Entered from below: a value hovering at 1 kHz stays in the Hz
        // band, and the decimal point stays in the same column.
        let mut b = Band::default();
        assert_eq!(fmt_si_sticky(999.9, "Hz", &mut b), " 999.9  Hz");
        assert_eq!(fmt_si_sticky(1000.4, "Hz", &mut b), "1000.4  Hz");
        assert_eq!(fmt_si_sticky(999.8, "Hz", &mut b), " 999.8  Hz");
        // Entered from above: stays in kHz just below the boundary, with
        // an identical string layout.
        let mut b = Band::default();
        assert_eq!(fmt_si_sticky(1000.4, "Hz", &mut b), " 1.000 kHz");
        assert_eq!(fmt_si_sticky(999.9, "Hz", &mut b), " 1.000 kHz");
        // A genuine decade move re-bands.
        let mut b = Band::default();
        fmt_si_sticky(999.9, "Hz", &mut b);
        assert_eq!(fmt_si_sticky(5.0e6, "Hz", &mut b), " 5.000 MHz");
        // Zero carries no magnitude — the band holds.
        let mut b = Band::default();
        fmt_si_sticky(0.002, "V", &mut b);
        assert_eq!(fmt_si_sticky(0.0, "V", &mut b), " 0.000 mV");
        // The mantissa field never widens, even where rounding overflows
        // a decimal ("-999.96" would round to "-1000.0").
        assert_eq!(fmt_si(-999.96, "V").chars().count(), 9);
    }

    #[test]
    fn fmt_width_is_exact_and_covers_none() {
        for unit in [Unit::Volt, Unit::Second, Unit::Hertz, Unit::Percent] {
            let w = fmt_width(unit);
            // Percent values are fractions and stay below 1000 % in
            // practice (duty, overshoot); SI units cover any magnitude.
            let values: &[f64] = if matches!(unit, Unit::Percent) {
                &[0.0, 0.123, 0.5, -0.5, 4.56, 9.99]
            } else {
                &[0.0, 0.123, 4.56, 999.9, 1234.5, -0.5]
            };
            for &v in values {
                assert_eq!(fmt(v, unit).chars().count(), w, "{v} {w}");
                let mut b = Band::default();
                assert_eq!(fmt_opt_sticky(Some(v), unit, &mut b).chars().count(), w);
                assert_eq!(fmt_opt_sticky(None, unit, &mut b).chars().count(), w);
            }
        }
    }

    #[test]
    fn constant_width_engineering_numbers() {
        assert_eq!(fmt_si(0.2, "V"), " 200.0 mV");
        assert_eq!(fmt_si(1000.0, "Hz"), " 1.000 kHz");
        assert_eq!(fmt_si(999.9, "Hz"), " 999.9  Hz");
        assert_eq!(fmt_si(0.0009999, "s"), " 999.9 µs");
        assert_eq!(fmt_si(250e3, "S/s"), " 250.0 kS/s");
        assert_eq!(fmt_si(1.032, "V"), " 1.032  V");
        assert_eq!(fmt_si(0.0, "V"), " 0.000  V");
        assert_eq!(fmt_si(-0.2, "V"), "-200.0 mV");
        // The whole point: width is constant across magnitude bands and
        // sign, so a value hovering at 1 kHz cannot flicker the layout.
        for (a, b) in [
            (999.9, 1000.4),
            (0.999, 1.001),
            (-0.5, 0.5),
            (0.000_999, 0.001_001),
        ] {
            assert_eq!(
                fmt_si(a, "Hz").chars().count(),
                fmt_si(b, "Hz").chars().count(),
                "{a} vs {b}"
            );
        }
    }
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
