//! SDR mode (Phase 10.0–10.1): state and the frame → spectrum / waterfall /
//! constellation / detection pipeline. Script actions live in `actions`,
//! control-socket readouts in `readout`.
//!
//! The mode follows the connected backend: `Capabilities::Sdr` switches the
//! app into it, and complex frames are routed here rather than to the scope
//! consumers (phosphor, measurements, recorder), which then simply idle.
//! Every display is derived from `neowon_dsp::iq_spectrum`, the oracle, so
//! the trace and the waterfall can never disagree.

use bevy::prelude::*;
use neowon_backend::{InstrumentConfig, SdrCaps, SdrConfig};
use neowon_core::{SampleLayout, SharedFrame};
use neowon_dsp::{DetectConfig, IqSpectrum, Tracker, TrackerConfig, Window, detect, iq_spectrum};

use crate::Link;

mod actions;
mod readout;

use crate::viz::waterfall::thermal;
pub use actions::{SdrAction, parse, parse_hz, parse_sim, run};
pub use readout::{detections_json, iq_json, modmeas_json, sdr_json};

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
    /// Run detection on each frame.
    pub detect_on: bool,
    /// Detection threshold over the floor, dB.
    pub threshold_db: f64,
    pub tracker: Tracker,
    /// (centre, rate) the tracker's tracks belong to; a retune clears it.
    tracked: (f64, f64),
    last_seq: Option<u64>,
}

/// Live tracking: active after 0.25 s, forgotten after 1 s unseen.
fn tracker() -> Tracker {
    Tracker::new(TrackerConfig {
        min_duration_s: 0.25,
        hold_s: 1.0,
        assoc_hz: 0.0,
    })
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
            detect_on: true,
            threshold_db: DetectConfig::default().threshold_db,
            tracker: tracker(),
            tracked: (0.0, 0.0),
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
    if sdr.detect_on {
        track(&mut sdr, &frame);
    }
}

/// Detect in the frame (one detection frame per SDR frame) and fold the
/// observations into the tracker.
fn track(sdr: &mut SdrState, frame: &neowon_core::CaptureFrame) {
    let (centre, rate) = (sdr.config.centre_hz, frame.sample_rate);
    if sdr.tracked != (centre, rate) {
        sdr.tracker.clear();
        sdr.tracked = (centre, rate);
    }
    let pairs = frame.channels[0].data.len() / 2;
    let cfg = DetectConfig {
        nfft: sdr.fft_size,
        blocks: (pairs / sdr.fft_size).max(1),
        threshold_db: sdr.threshold_db,
        // The RTL2832's DC spike is not a signal; the simulator has none.
        dc_guard: if sdr.caps.as_ref().is_some_and(|c| c.tuner == "sim") {
            0
        } else {
            DC_GUARD
        },
        ..Default::default()
    };
    let t0 = frame.t_start();
    let obs = detect(&frame.channels[0].data, rate, centre, t0, &cfg);
    sdr.tracker.cfg.assoc_hz = 2.0 * rate / sdr.fft_size as f64;
    sdr.tracker.update(&obs, t0 + frame.duration());
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

#[cfg(test)]
mod tests {
    use neowon_sim::IqScene;

    use super::*;

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
