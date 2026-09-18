//! SDR mode (Phase 10.0): state, the frame → spectrum / waterfall /
//! constellation pipeline, script actions, and the control-socket readouts.
//!
//! The mode follows the connected backend: `Capabilities::Sdr` switches the
//! app into it, and complex frames are routed here rather than to the scope
//! consumers (phosphor, measurements, recorder), which then simply idle.
//! Every display is derived from `neowon_dsp::iq_spectrum`, the oracle, so
//! the trace and the waterfall can never disagree.

use bevy::prelude::*;
use neowon_backend::{Command, InstrumentConfig, SdrCaps, SdrConfig, SdrGain};
use neowon_core::{SampleLayout, SharedFrame};
use neowon_dsp::{IqSpectrum, Window, iq_spectrum};

use crate::Link;
use crate::viz::waterfall::thermal;

/// Waterfall texture: display columns × history rows (newest on top).
pub const WF_W: usize = 1024;
pub const WF_H: usize = 320;
pub const FFT_SIZES: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
/// Constellation points kept from each frame.
pub const IQ_POINTS: usize = 2048;
/// Bins either side of DC ignored by the peak readout (the RTL2832's DC
/// spike would win otherwise).
pub const DC_GUARD: usize = 4;

#[derive(Resource)]
pub struct SdrState {
    /// True once an SDR backend connected; the UI and flush follow it.
    pub active: bool,
    pub caps: Option<SdrCaps>,
    pub config: SdrConfig,
    pub dirty: bool,
    /// Last seed sent to a generating backend.
    pub seed: u64,
    pub latest: Option<SharedFrame>,
    pub frames_seen: u64,
    pub fft_size: usize,
    /// Displayed span, Hz, centred on the tuned frequency; 0 = the whole
    /// sample rate.
    pub span_hz: f64,
    /// Top of the display and its depth, dBFS / dB.
    pub ref_db: f64,
    pub range_db: f64,
    pub spectrum: Option<IqSpectrum>,
    /// Display columns of the latest spectrum (peak of the bins each
    /// covers), dBFS — what both the trace and the waterfall row show.
    pub columns: Vec<f64>,
    /// Palettized waterfall, row-major RGBA, newest row first.
    pub waterfall: Vec<[u8; 4]>,
    /// Rows pushed so far; the UI re-uploads the texture when it changes.
    pub wf_rows: u64,
    pub iq: Vec<[f32; 2]>,
    last_seq: Option<u64>,
}

impl Default for SdrState {
    fn default() -> Self {
        Self {
            active: false,
            caps: None,
            config: SdrConfig::default(),
            dirty: false,
            seed: 1,
            latest: None,
            frames_seen: 0,
            fft_size: 4096,
            span_hz: 0.0,
            ref_db: 0.0,
            range_db: 100.0,
            spectrum: None,
            columns: Vec::new(),
            waterfall: vec![[0, 0, 0, 255]; WF_W * WF_H],
            wf_rows: 0,
            iq: Vec::new(),
            last_seq: None,
        }
    }
}

impl SdrState {
    /// `active` when the app was launched on an SDR backend.
    pub fn new(active: bool) -> Self {
        Self {
            active,
            ..Default::default()
        }
    }

    /// The span actually shown, Hz.
    pub fn span(&self) -> f64 {
        let rate = self.config.sample_rate;
        if self.span_hz > 0.0 {
            self.span_hz.min(rate)
        } else {
            rate
        }
    }

    /// Strongest displayed signal: `(absolute Hz, dBFS)`.
    pub fn peak(&self) -> Option<(f64, f64)> {
        let s = self.spectrum.as_ref()?;
        let half = self.span() / 2.0;
        let dc = s.len() / 2;
        s.power_db
            .iter()
            .enumerate()
            .filter(|(k, _)| k.abs_diff(dc) > DC_GUARD && s.offset_hz(*k).abs() <= half)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, &p)| (self.config.centre_hz + s.offset_hz(k), p))
    }

    /// dBFS → 0..1 display position (1 = the reference level).
    pub fn level(&self, db: f64) -> f32 {
        (1.0 - (self.ref_db - db) / self.range_db).clamp(0.0, 1.0) as f32
    }
}

/// `cols` display columns across `span` Hz centred on DC, each the peak of
/// the bins it covers (peak-preserving: a narrow carrier never vanishes
/// between columns).
pub fn columns(s: &IqSpectrum, span: f64, cols: usize) -> Vec<f64> {
    (0..cols)
        .map(|c| {
            let lo = (c as f64 / cols as f64 - 0.5) * span;
            let hi = ((c + 1) as f64 / cols as f64 - 0.5) * span;
            let (a, b) = (s.bin_of(lo), s.bin_of(hi));
            s.power_db[a.min(b)..=a.max(b)]
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect()
}

/// Fold newly arrived frames into the displays.
pub fn update(mut sdr: ResMut<SdrState>) {
    let Some(frame) = sdr.latest.clone() else {
        return;
    };
    if !sdr.active || sdr.last_seq == Some(frame.seq) || frame.layout != SampleLayout::Complex {
        return;
    }
    sdr.last_seq = Some(frame.seq);
    let data = &frame.channels[0].data;
    let Some(spec) = iq_spectrum(data, frame.sample_rate, Window::Hann, sdr.fft_size) else {
        return;
    };
    let cols = columns(&spec, sdr.span(), WF_W);
    // Scroll down one row and paint the new one on top.
    sdr.waterfall.copy_within(0..WF_W * (WF_H - 1), WF_W);
    for (c, db) in cols.iter().enumerate() {
        let px = thermal(sdr.level(*db));
        sdr.waterfall[c] = px;
    }
    sdr.wf_rows += 1;
    let pairs = data.len() / 2;
    let step = (pairs / IQ_POINTS).max(1);
    sdr.iq = (0..pairs)
        .step_by(step)
        .map(|i| [data[2 * i], data[2 * i + 1]])
        .collect();
    sdr.columns = cols;
    sdr.spectrum = Some(spec);
}

/// In SDR mode the SDR config is what goes to the instrument; scope edits
/// (keys, dock) are dropped rather than sent to a backend that refuses them.
/// Runs before the scope's `flush`.
pub fn flush(mut sdr: ResMut<SdrState>, mut link: ResMut<Link>) {
    if !sdr.active {
        return;
    }
    link.dirty = false;
    if sdr.dirty {
        sdr.dirty = false;
        link.sup.apply(InstrumentConfig::Sdr(sdr.config.clone()));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SdrAction {
    Tune(f64),
    /// Relative retune, Hz.
    Step(f64),
    Rate(f64),
    /// `None` = tuner AGC.
    Gain(Option<f64>),
    Agc(bool),
    Ppm(f64),
    Span(f64),
    Fft(usize),
    Level {
        ref_db: f64,
        range_db: f64,
    },
    Run(bool),
    Seed(u64),
}

/// `99.4M`, `100k`, `1.2G` or plain Hz.
pub fn parse_hz(s: &str) -> Result<f64, String> {
    let (num, mul) = match s.chars().last() {
        Some('k' | 'K') => (&s[..s.len() - 1], 1e3),
        Some('M') => (&s[..s.len() - 1], 1e6),
        Some('G' | 'g') => (&s[..s.len() - 1], 1e9),
        _ => (s, 1.0),
    };
    num.parse::<f64>()
        .map(|v| v * mul)
        .map_err(|_| format!("bad frequency {s:?}"))
}

fn on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" | "1" => Ok(true),
        "off" | "0" => Ok(false),
        _ => Err(format!("expected on/off, got {s:?}")),
    }
}

/// `sdr <verb> …` (the words after `sdr`).
pub fn parse<'a>(next: &mut dyn FnMut() -> Result<&'a str, String>) -> Result<SdrAction, String> {
    let num = |s: &str| s.parse::<f64>().map_err(|_| format!("bad number {s:?}"));
    Ok(match next()? {
        "tune" => SdrAction::Tune(parse_hz(next()?)?),
        "step" => SdrAction::Step(parse_hz(next()?)?),
        "rate" => SdrAction::Rate(parse_hz(next()?)?),
        "gain" => match next()? {
            "auto" => SdrAction::Gain(None),
            db => SdrAction::Gain(Some(num(db)?)),
        },
        "agc" => SdrAction::Agc(on_off(next()?)?),
        "ppm" => SdrAction::Ppm(num(next()?)?),
        "span" => SdrAction::Span(parse_hz(next()?)?),
        "fft" => SdrAction::Fft(next()?.parse().map_err(|_| "bad FFT size".to_string())?),
        "level" => SdrAction::Level {
            ref_db: num(next()?)?,
            range_db: num(next()?)?,
        },
        "run" => SdrAction::Run(on_off(next()?)?),
        other => return Err(format!("unknown sdr verb {other:?}")),
    })
}

/// `sim iq --seed <s>` (the words after `sim`).
pub fn parse_sim<'a>(
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<SdrAction, String> {
    match (next()?, next()?) {
        ("iq", "--seed") => Ok(SdrAction::Seed(
            next()?.parse().map_err(|_| "bad seed".to_string())?,
        )),
        _ => Err("expected: sim iq --seed <n>".into()),
    }
}

/// Run a script action, reporting a refusal in the log and the status
/// line (what `get status` returns).
pub fn run(a: SdrAction, sdr: &mut SdrState, link: &mut Link) {
    if let Err(e) = apply(a, sdr, link) {
        error!("script: sdr: {e}");
        link.status = format!("error: {e}");
    }
}

/// Apply a script action. Invalid values are refused, not clamped, so a
/// script learns it asked for something the instrument cannot do.
pub fn apply(a: SdrAction, sdr: &mut SdrState, link: &Link) -> Result<(), String> {
    let caps = sdr.caps.clone();
    let in_range = |hz: f64| match &caps {
        Some(c) if !(c.freq_range_hz.0..=c.freq_range_hz.1).contains(&hz) => Err(format!(
            "{hz} Hz outside {}..={} Hz",
            c.freq_range_hz.0, c.freq_range_hz.1
        )),
        _ => Ok(hz),
    };
    match a {
        SdrAction::Tune(hz) => sdr.config.centre_hz = in_range(hz)?,
        SdrAction::Step(hz) => sdr.config.centre_hz = in_range(sdr.config.centre_hz + hz)?,
        SdrAction::Rate(r) => {
            if let Some(c) = &caps
                && !c.sample_rates.iter().any(|&x| (x - r).abs() < 0.5)
            {
                return Err(format!("rate {r} not in {:?}", c.sample_rates));
            }
            if r <= 0.0 {
                return Err("rate must be positive".into());
            }
            sdr.config.sample_rate = r;
        }
        SdrAction::Gain(None) => sdr.config.gain = SdrGain::Auto,
        SdrAction::Gain(Some(db)) => {
            // Snap to the tuner's table so the readout says what it runs.
            let db = caps
                .as_ref()
                .and_then(|c| {
                    c.gains_db
                        .iter()
                        .copied()
                        .min_by(|a, b| (a - db).abs().total_cmp(&(b - db).abs()))
                })
                .unwrap_or(db);
            sdr.config.gain = SdrGain::Manual(db);
        }
        SdrAction::Agc(on) => sdr.config.agc = on,
        SdrAction::Ppm(p) => sdr.config.ppm = p,
        SdrAction::Run(on) => sdr.config.running = on,
        SdrAction::Span(hz) => {
            if hz < 0.0 {
                return Err("span must be >= 0".into());
            }
            sdr.span_hz = hz;
            return Ok(());
        }
        SdrAction::Fft(n) => {
            if !FFT_SIZES.contains(&n) {
                return Err(format!("FFT size {n} not in {FFT_SIZES:?}"));
            }
            sdr.fft_size = n;
            return Ok(());
        }
        SdrAction::Level { ref_db, range_db } => {
            if range_db <= 0.0 {
                return Err("range must be positive".into());
            }
            (sdr.ref_db, sdr.range_db) = (ref_db, range_db);
            return Ok(());
        }
        SdrAction::Seed(s) => {
            sdr.seed = s;
            let _ = link.sup.commands.send(Command::Seed(s));
            return Ok(());
        }
    }
    sdr.dirty = true;
    Ok(())
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

/// `get iq`: identity of the latest IQ frame. `start` is its first sample
/// index (t_start × rate), so a test can regenerate the same bytes from
/// the D8 generator and compare `bytes_fnv`.
pub fn iq_json(sdr: &SdrState) -> String {
    let Some(f) = &sdr.latest else {
        return r#"{"ok":false,"error":"no IQ frame yet"}"#.into();
    };
    let bytes = neowon_sim::iq::to_le_bytes(&f.channels[0].data);
    format!(
        r#"{{"ok":true,"seed":{},"n":{},"start":{},"layout":"complex","bytes_fnv":{}}}"#,
        sdr.seed,
        f.channels[0].unit_count(f.layout),
        (f.t_start() * f.sample_rate).round(),
        neowon_sim::iq::fnv1a64(&bytes)
    )
}

/// `get sdr`: the SDR mode's settings and live readouts.
pub fn sdr_json(sdr: &SdrState) -> String {
    let c = &sdr.config;
    let gain = match c.gain {
        SdrGain::Auto => r#""auto""#.to_string(),
        SdrGain::Manual(db) => num(db),
    };
    let (name, serial, tuner) = sdr
        .caps
        .as_ref()
        .map(|c| (c.name.as_str(), c.serial.as_str(), c.tuner.as_str()))
        .unwrap_or_default();
    let (peak_hz, peak_db) = sdr.peak().unwrap_or((f64::NAN, f64::NAN));
    let floor = sdr.spectrum.as_ref().map_or(f64::NAN, |s| s.median_db());
    format!(
        concat!(
            r#"{{"ok":true,"active":{},"backend":"{}","serial":"{}","tuner":"{}","#,
            r#""centre_hz":{},"sample_rate":{},"gain_db":{},"agc":{},"ppm":{},"running":{},"#,
            r#""span_hz":{},"fft":{},"ref_db":{},"range_db":{},"frames_seen":{},"#,
            r#""peak_hz":{},"peak_dbfs":{},"floor_dbfs":{}}}"#
        ),
        sdr.active,
        name,
        serial,
        tuner,
        num(c.centre_hz),
        num(c.sample_rate),
        gain,
        c.agc,
        num(c.ppm),
        c.running,
        num(sdr.span()),
        sdr.fft_size,
        num(sdr.ref_db),
        num(sdr.range_db),
        sdr.frames_seen,
        num(peak_hz),
        num(peak_db),
        num(floor),
    )
}

#[cfg(test)]
mod tests {
    use neowon_sim::IqScene;

    use super::*;

    fn words<'a>(s: &'a str) -> impl FnMut() -> Result<&'a str, String> {
        let mut w = s.split_whitespace();
        move || w.next().ok_or_else(|| "missing argument".to_string())
    }

    #[test]
    fn frequencies_take_suffixes() {
        assert_eq!(parse_hz("99.4M").unwrap(), 99.4e6);
        assert_eq!(parse_hz("100k").unwrap(), 100e3);
        assert_eq!(parse_hz("1.2G").unwrap(), 1.2e9);
        assert_eq!(parse_hz("2048000").unwrap(), 2.048e6);
        assert!(parse_hz("fast").is_err());
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(
            parse(&mut words("tune 99.4M")).unwrap(),
            SdrAction::Tune(99.4e6)
        );
        assert_eq!(
            parse(&mut words("gain auto")).unwrap(),
            SdrAction::Gain(None)
        );
        assert_eq!(
            parse(&mut words("gain 29.7")).unwrap(),
            SdrAction::Gain(Some(29.7))
        );
        assert_eq!(parse(&mut words("agc on")).unwrap(), SdrAction::Agc(true));
        assert_eq!(
            parse(&mut words("level -10 80")).unwrap(),
            SdrAction::Level {
                ref_db: -10.0,
                range_db: 80.0
            }
        );
        assert!(parse(&mut words("warp 9")).is_err());
        assert_eq!(
            parse_sim(&mut words("iq --seed 7")).unwrap(),
            SdrAction::Seed(7)
        );
        assert!(parse_sim(&mut words("iq 7")).is_err());
    }

    #[test]
    fn columns_keep_a_narrow_carrier() {
        // One 4096-bin spectrum squeezed into 1024 columns: the carrier's
        // bin must survive as its column's peak.
        let scene = IqScene {
            sample_rate: 2.048e6,
            components: vec![neowon_sim::IqComponent::Tone {
                offset_hz: 300e3,
                amplitude: 0.5,
                phase: 0.0,
            }],
            noise_rms: 0.01,
        };
        let s = iq_spectrum(&scene.samples(1, 0, 8192), 2.048e6, Window::Hann, 4096).unwrap();
        let cols = columns(&s, 2.048e6, WF_W);
        let (c, db) = cols
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!((db + 6.02).abs() < 0.1, "{db}");
        // Column c spans (c/1024 - 0.5) * 2.048 MHz: 300 kHz is column 662.
        assert_eq!(c, 662);
    }

    #[test]
    fn level_maps_reference_and_depth() {
        let s = SdrState::default();
        assert_eq!(s.level(0.0), 1.0);
        assert_eq!(s.level(-50.0), 0.5);
        assert_eq!(s.level(-150.0), 0.0);
    }
}
