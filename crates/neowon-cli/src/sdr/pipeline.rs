//! The SDR smoke pipeline: tune → capture → detect → classify → decode →
//! verdict, over any SDR `Backend`. The source (the simulator or the RTL
//! dongle) is chosen in `super::open`; nothing here knows which it got, so
//! the sim run exercises exactly the code the hardware run uses.
//!
//! Only engine-free libraries: `neowon_dsp::{detect, classify, demod,
//! modlab}` — the same calls the app's SDR mode makes (the app is a binary
//! crate, so its glue is mirrored here, not shared).

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use neowon_backend::{Backend, Capabilities, InstrumentConfig, SdrConfig, SdrGain};
use neowon_core::SignalObservation;
use neowon_dsp::classify::{Class, Classification, classify, features};
use neowon_dsp::demod::{DemodMode, Receiver, ReceiverConfig};
use neowon_dsp::modlab::{recover, select, symbol_rate};
use neowon_dsp::{DetectConfig, Window, detect, spectrum};

/// Bins either side of DC the detector ignores on the dongle (the RTL2832's
/// zero-IF spike, `docs/protocol-rtlsdr.md`); the app's `sdr::DC_GUARD`.
pub const RTL_DC_GUARD: usize = 4;
/// Detection FFT size: the app's default, so RBW matches its spectrum.
pub const NFFT: usize = 4096;
/// The contract's thresholds (`docs/tasks/phase10-sdr-spec.md`, Testing
/// strategy): a peak within ±2 RBW, confidence at least 0.70.
pub const PEAK_TOL_RBW: f64 = 2.0;
pub const MIN_CONFIDENCE: f64 = 0.70;
/// Pairs the classifier sees (one app frame at 2.048 MS/s).
const CLASSIFY_PAIRS: usize = 64 * 1024;
const AUDIO_RATE: f64 = 48e3;
/// Recovered audio below this RMS is silence, not a decode.
const AUDIO_FLOOR_RMS: f64 = 1e-3;
/// The RRC roll-off the app's modulation lab assumes.
const ROLLOFF: f64 = 0.35;
/// Wide FM when the occupied band is wider than this.
const WFM_MIN_OBW_HZ: f64 = 50e3;

#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// The frequency under test, Hz.
    pub freq_hz: f64,
    /// The hardware centre sits this far below `freq_hz`, so the signal is
    /// clear of the zero-IF DC spike.
    pub lo_offset_hz: f64,
    pub sample_rate: f64,
    pub gain: SdrGain,
    pub pairs: usize,
    /// Frames discarded after tuning (settling).
    pub settle_frames: usize,
    /// Wall-clock limit on the capture (pacing only; never the signal).
    pub timeout: Duration,
}

impl Params {
    pub fn new(freq_hz: f64) -> Self {
        Self {
            freq_hz,
            lo_offset_hz: 250e3,
            sample_rate: 2.048e6,
            gain: SdrGain::Auto,
            pairs: 256 * 1024,
            settle_frames: 2,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn centre_hz(&self) -> f64 {
        self.freq_hz - self.lo_offset_hz
    }

    /// Resolution bandwidth: one detection bin.
    pub fn rbw_hz(&self) -> f64 {
        self.sample_rate / NFFT as f64
    }
}

/// The smoke readout. The contract's fields are `dongle_serial, tune_hz,
/// peak_hz, peak_tol_hz, class, confidence, decode, snr_db`; `source`,
/// `centre_hz` and `failures` are this implementation's additions.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    /// `"sim"` or `"rtl"`: a sim readout is never the hardware one.
    pub source: &'static str,
    pub dongle_serial: String,
    pub tune_hz: f64,
    pub centre_hz: f64,
    /// Where the detected signal nearest `tune_hz` sits: see [`peak_hz`].
    pub peak_hz: Option<f64>,
    pub peak_tol_hz: f64,
    /// A preset class label, `unknown`, or `none` when nothing was detected.
    pub class: String,
    pub confidence: f64,
    pub decode: String,
    pub snr_db: Option<f64>,
    pub failures: Vec<Failure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    NoPeak,
    ClassUnknown,
    LowConfidence,
    EmptyDecode,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::NoPeak => "no-peak",
            Rule::ClassUnknown => "class-unknown",
            Rule::LowConfidence => "low-confidence",
            Rule::EmptyDecode => "empty-decode",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub rule: Rule,
    pub detail: String,
}

/// The contract's FAIL rules over a readout, in the contract's order.
pub fn verdict(r: &Readout) -> Vec<Failure> {
    let mut out = Vec::new();
    let mut fail = |rule, detail: String| out.push(Failure { rule, detail });
    match r.peak_hz {
        Some(p) if (p - r.tune_hz).abs() <= r.peak_tol_hz => {}
        Some(p) => fail(
            Rule::NoPeak,
            format!(
                "nearest peak {p:.0} Hz is {:.0} Hz from {:.0} Hz (tolerance ±{:.0} Hz)",
                (p - r.tune_hz).abs(),
                r.tune_hz,
                r.peak_tol_hz
            ),
        ),
        None => fail(Rule::NoPeak, "no signal detected".into()),
    }
    if r.class == "unknown" || r.class == "none" {
        fail(Rule::ClassUnknown, format!("class {}", r.class));
    }
    if r.confidence < MIN_CONFIDENCE {
        fail(
            Rule::LowConfidence,
            format!("confidence {:.3} < {MIN_CONFIDENCE:.2}", r.confidence),
        );
    }
    if r.decode.is_empty() {
        fail(
            Rule::EmptyDecode,
            format!("no decode for class {}", r.class),
        );
    }
    out
}

pub fn run(backend: &mut dyn Backend, source: &'static str, p: &Params) -> Result<Readout> {
    let Capabilities::Sdr(caps) = backend.capabilities() else {
        bail!("not an SDR backend");
    };
    let serial = caps.serial.clone();
    // The simulator has no DC spike; the dongle does (the app's rule).
    let dc_guard = if caps.tuner == "sim" { 0 } else { RTL_DC_GUARD };
    let cfg = SdrConfig {
        centre_hz: p.centre_hz(),
        sample_rate: p.sample_rate,
        gain: p.gain,
        running: true,
        ..SdrConfig::default()
    };
    backend
        .apply(&InstrumentConfig::Sdr(cfg))
        .map_err(|e| anyhow::anyhow!("tuning: {e}"))?;
    let iq = capture(backend, p)?;
    Ok(analyse(&iq, source, serial, dc_guard, p))
}

/// `p.pairs` contiguous pairs after `p.settle_frames` discarded frames. A
/// frame that reports a gap restarts the collection, so the analysis never
/// sees a splice.
fn capture(backend: &mut dyn Backend, p: &Params) -> Result<Vec<f32>> {
    let until = Instant::now() + p.timeout;
    let (mut iq, mut seen) = (Vec::with_capacity(2 * p.pairs), 0usize);
    while iq.len() < 2 * p.pairs {
        if Instant::now() > until {
            bail!(
                "capture timed out: {} of {} pairs in {:?}",
                iq.len() / 2,
                p.pairs,
                p.timeout
            );
        }
        let Some(frame) = backend
            .poll_frame(Duration::from_millis(200))
            .map_err(|e| anyhow::anyhow!("capture: {e}"))?
        else {
            continue;
        };
        seen += 1;
        if seen <= p.settle_frames {
            continue;
        }
        if frame.dropped_before() > 0 {
            iq.clear();
        }
        iq.extend_from_slice(&frame.channels[0].data);
    }
    iq.truncate(2 * p.pairs);
    Ok(iq)
}

/// A detected signal's frequency: the middle of its 99 % occupied band.
/// Not the strongest bin — an FM signal's strongest Bessel line can sit
/// deviation-far from its carrier — and not the power-weighted centroid,
/// which measured 5 kHz off a symmetric ±30 kHz FM tone in the simulator.
pub fn peak_hz(o: &SignalObservation) -> f64 {
    (o.lo_hz + o.hi_hz) / 2.0
}

pub fn analyse(
    iq: &[f32],
    source: &'static str,
    dongle_serial: String,
    dc_guard: usize,
    p: &Params,
) -> Readout {
    let (centre, rate) = (p.centre_hz(), p.sample_rate);
    let det = DetectConfig {
        nfft: NFFT,
        blocks: (iq.len() / 2 / NFFT).max(1),
        dc_guard,
        ..Default::default()
    };
    let obs = detect(iq, rate, centre, 0.0, &det);
    let peak = obs
        .iter()
        .min_by(|a, b| {
            (peak_hz(a) - p.freq_hz)
                .abs()
                .total_cmp(&(peak_hz(b) - p.freq_hz).abs())
        })
        .cloned();
    let head = &iq[..iq.len().min(2 * CLASSIFY_PAIRS)];
    let class = peak.as_ref().and_then(|o| {
        features(head, rate, o.centre_hz - centre, o.bandwidth_hz()).map(|f| classify(&f))
    });
    let decode = match (&peak, &class) {
        (Some(o), Some(c)) if !c.unknown => decode(iq, rate, centre, o, c),
        _ => String::new(),
    };
    let mut r = Readout {
        source,
        dongle_serial,
        tune_hz: p.freq_hz,
        centre_hz: centre,
        peak_hz: peak.as_ref().map(peak_hz),
        peak_tol_hz: PEAK_TOL_RBW * p.rbw_hz(),
        class: match &class {
            None => "none".into(),
            Some(c) if c.unknown => "unknown".into(),
            Some(c) => c.class.label().into(),
        },
        confidence: class.as_ref().map_or(0.0, |c| c.confidence),
        decode,
        snr_db: peak.as_ref().map(|o| o.snr_db),
        failures: Vec::new(),
    };
    r.failures = verdict(&r);
    r
}

/// What the signal carries, per class: demodulated audio for AM/FM, the
/// recovered symbol labels for a digital signal. A CW carrier or noise
/// carries nothing to decode, so they decode to nothing and trip
/// `empty-decode`.
fn decode(iq: &[f32], rate: f64, centre: f64, o: &SignalObservation, c: &Classification) -> String {
    let offset = o.centre_hz - centre;
    let obw = o.bandwidth_hz();
    match c.class {
        Class::Am => audio(iq, rate, DemodMode::Am, offset, obw),
        Class::Fm if obw > WFM_MIN_OBW_HZ => audio(iq, rate, DemodMode::Wfm, offset, obw),
        Class::Fm => audio(iq, rate, DemodMode::Nfm, offset, obw),
        Class::Digital(m) => symbols(iq, rate, offset, obw, m, c.features.symbol_rate_hz),
        Class::Cw | Class::Noise => String::new(),
    }
}

/// Demodulate the channel and describe the audio; empty when it is silent.
fn audio(iq: &[f32], rate: f64, mode: DemodMode, offset_hz: f64, obw_hz: f64) -> String {
    let width_hz = match mode {
        DemodMode::Wfm => mode.default_width_hz(),
        _ => mode.default_width_hz().max(1.2 * obw_hz),
    };
    let mut rx = Receiver::new(ReceiverConfig {
        offset_hz,
        width_hz,
        deemphasis_tau_s: None,
        ..ReceiverConfig::new(mode, rate, AUDIO_RATE)
    });
    let mut out = Vec::new();
    rx.process(iq, &mut out);
    // The first quarter holds the filters' start-up.
    let a = &out[out.len() / 4..];
    if a.is_empty() {
        return String::new();
    }
    let mean = a.iter().map(|&x| x as f64).sum::<f64>() / a.len() as f64;
    let rms = (a.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / a.len() as f64).sqrt();
    if rms.is_nan() || rms <= AUDIO_FLOOR_RMS {
        return String::new();
    }
    let tone = dominant_hz(a)
        .map(|f| format!(", dominant {f:.0} Hz"))
        .unwrap_or_default();
    format!(
        "{} audio: {} samples at {AUDIO_RATE:.0} Hz, rms {:.1} dBFS{tone}",
        mode.verb(),
        a.len(),
        20.0 * rms.log10()
    )
}

/// The strongest audio line above a few bins of DC, Hz (bin resolution).
fn dominant_hz(a: &[f32]) -> Option<f64> {
    let s = spectrum(a, 1.0, AUDIO_RATE, Window::Hann, a.len())?;
    let (k, _) = s
        .amplitude
        .iter()
        .enumerate()
        .skip(3)
        .max_by(|x, y| x.1.total_cmp(y.1))?;
    Some(k as f64 * s.bin_hz)
}

/// Recover the symbols (the app's modulation-lab chain) and list the first
/// Gray labels; empty when recovery fails.
fn symbols(
    iq: &[f32],
    rate: f64,
    offset_hz: f64,
    obw_hz: f64,
    m: neowon_core::Modulation,
    rs: Option<f64>,
) -> String {
    let data = &iq[..iq.len().min(2 * CLASSIFY_PAIRS)];
    let obw = obw_hz.max(rate / 1000.0);
    let taps = ((8.0 * rate / obw) as usize).clamp(65, 401);
    let chan = select(data, rate, offset_hz, 0.8 * obw, taps);
    let Some(rs) = rs.or_else(|| symbol_rate(&chan, rate, 0.4 * obw, 1.2 * obw)) else {
        return String::new();
    };
    let Some(r) = recover(&chan, rate, rs, m, ROLLOFF) else {
        return String::new();
    };
    if r.labels.is_empty() {
        return String::new();
    }
    let first: Vec<String> = r.labels.iter().take(16).map(|l| l.to_string()).collect();
    format!(
        "{} symbols at {rs:.0} Bd: {} recovered, EVM {:.1} %, MER {:.1} dB, first labels {}",
        neowon_dsp::classify::Class::Digital(m).label(),
        r.labels.len(),
        r.evm_rms_pct,
        r.mer_db,
        first.join(" ")
    )
}
