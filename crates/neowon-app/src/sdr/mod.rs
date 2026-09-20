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
pub mod analysis;
mod instrument;
mod readout;
pub mod scan;

use crate::viz::waterfall::thermal;
pub use actions::{DabVerb, SdrAction, parse, parse_hz, parse_instrument, parse_sim, run};
pub use readout::{
    audio_json, classify_json, dab_json, detections_json, iq_json, modmeas_json, sdr_json,
};
pub use scan::{diff_json as survey_diff_json, survey_json};

/// Waterfall texture: display columns × history rows (newest on top).
pub const WF_W: usize = 1024;
pub const WF_H: usize = 320;
pub const FFT_SIZES: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
/// Constellation points kept from each frame.
pub const IQ_POINTS: usize = 2048;
/// Bins either side of DC ignored by the peak readout (the RTL2832's DC
/// spike would win otherwise).
pub const DC_GUARD: usize = 4;
/// How far from a centre, as a fraction of its width, a tuned frequency may
/// sit and still count as inside: the band edges roll off, so the outer 10%
/// of the IQ band retunes the hardware and of a zoomed view pans it.
const TUNE_REACH: f64 = 0.45;
/// Waterfall black sits this far under the measured noise floor, dB.
const WF_UNDER_FLOOR_DB: f64 = 5.0;

/// Waterfall intensity of `db`: 0 at `black`, 1 at `white` (the spectrum's
/// reference level); a floor above the reference still gets 20 dB of room.
fn wf_level(db: f64, black: f64, white: f64) -> f32 {
    let white = white.max(black + 20.0);
    ((db - black) / (white - black)).clamp(0.0, 1.0) as f32
}

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
    /// Displayed span, Hz; 0 = the whole sample rate. The view sits
    /// `pan_hz` off the hardware centre, which is also where it sits by
    /// default.
    pub span_hz: f64,
    /// Where the displayed span sits, Hz from the hardware centre: the
    /// view pans inside the IQ band without retuning (`sdr pan`).
    pub pan_hz: f64,
    /// The tuned frequency: the channel the operator monitors, absolute
    /// Hz. Distinct from `config.centre_hz`, the hardware window that sets
    /// what the IQ band covers; tuning inside the band moves this and
    /// leaves the hardware alone, tuning beyond it recentres the hardware
    /// (`sdr tune`).
    pub tuned_hz: f64,
    /// The hardware window follows the tuned frequency (`sdr follow`).
    pub follow: bool,
    /// Channel width, Hz, used when it is not taken from a detection
    /// (`sdr width <hz>`).
    pub width_hz: f64,
    /// Channel width follows the nearest detection's occupied bandwidth
    /// (`sdr width auto`).
    pub width_auto: bool,
    /// Audio demodulation; `None` is off (`sdr demod`).
    pub demod: Option<neowon_dsp::DemodMode>,
    pub volume: f32,
    pub mute: bool,
    /// Squelch threshold on the channel power, dBFS (`sdr squelch`); a very
    /// low value leaves the gate open.
    pub squelch_db: f64,
    /// The streaming demodulator, built while a mode is on.
    receiver: Option<neowon_dsp::Receiver>,
    /// First sample index the next DAB frame should carry: frames whose
    /// timestamps do not continue from it are spliced, not contiguous.
    pub dab_next_sample: Option<i64>,
    /// The DAB receiver (10.15.1), built while `sdr dab on` is in force.
    /// It is fed the raw IQ frames, not the demodulated channel: DAB wants
    /// the whole 1.536 MHz ensemble, so it is a wideband consumer sitting
    /// beside the demodulator, not a mode of it.
    pub dab: Option<neowon_dsp::dab::DabReceiver>,
    /// Output device, opened on first use.
    pub audio: Option<neowon_audio::sink::AudioOut>,
    /// Scratch audio buffer, kept to avoid a per-frame allocation.
    audio_buf: Vec<f32>,
    /// Audio of the last frame: RMS, channel power, and whether the squelch
    /// held it back.
    pub audio_rms: f32,
    pub audio_channel_dbfs: f64,
    pub audio_squelched: bool,
    /// Height of the dock's signal list, points (`sdr list`).
    pub list_px: f32,
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
    /// Run the modulation lab on the signal nearest the tuned frequency.
    pub analyse_on: bool,
    /// The modulation to assume, or `None` to pick it from cumulants.
    pub modulation: Option<neowon_core::Modulation>,
    pub analysis: Option<analysis::Analysis>,
    /// The DSP classifier's verdict on the same signal.
    pub classification: Option<neowon_dsp::classify::Classification>,
    /// The lab's worker thread, started on first use.
    lab: Option<analysis::Lab>,
    /// A survey in progress, and the last completed ones (oldest first).
    pub survey: Option<neowon_sdr::survey::Survey>,
    pub surveys: Vec<neowon_sdr::survey::SurveyResult>,
    last_seq: Option<u64>,
    /// The instruments `instrument scope|sdr` switches between.
    pub launch: crate::launch::Launch,
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
            pan_hz: 0.0,
            tuned_hz: 100e6,
            follow: false,
            width_hz: 12.5e3,
            width_auto: true,
            demod: None,
            volume: 0.7,
            mute: false,
            squelch_db: -120.0,
            receiver: None,
            dab_next_sample: None,
            dab: None,
            audio: None,
            audio_buf: Vec::new(),
            audio_rms: 0.0,
            audio_channel_dbfs: f64::NEG_INFINITY,
            audio_squelched: false,
            list_px: 160.0,
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
            analyse_on: false,
            modulation: None,
            analysis: None,
            classification: None,
            lab: None,
            survey: None,
            surveys: Vec::new(),
            last_seq: None,
            launch: Default::default(),
        }
    }
}

impl SdrState {
    /// `active` when the app was launched on an SDR backend.
    pub fn new(launch: crate::launch::Launch) -> Self {
        Self {
            active: launch.sdr(),
            launch,
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

    /// Centre of the display, Hz.
    pub fn view_centre(&self) -> f64 {
        self.config.centre_hz + self.pan_hz
    }

    /// Keep the displayed span inside the IQ band.
    pub fn clamp_pan(&mut self) {
        let room = (self.config.sample_rate - self.span()).max(0.0) / 2.0;
        self.pan_hz = self.pan_hz.clamp(-room, room);
    }

    /// The active detection nearest the tuned frequency, if any.
    pub fn nearest_track(&self) -> Option<&neowon_dsp::Track> {
        self.tracker.active().min_by(|a, b| {
            let d = |t: &&neowon_dsp::Track| (t.last.centre_hz - self.tuned_hz).abs();
            d(a).total_cmp(&d(b))
        })
    }

    /// The channel width shown and filed: the nearest detection's measured
    /// 99% bandwidth when auto and one is present, else the manual value.
    pub fn channel_width(&self) -> f64 {
        if self.width_auto {
            self.nearest_track()
                .map(|t| t.last.bandwidth_hz())
                .filter(|w| *w > 0.0)
                .unwrap_or(self.width_hz)
        } else {
            self.width_hz
        }
    }

    /// Tune: move the channel cursor (`sdr tune`, `sdr follow`). Inside the
    /// IQ band the hardware stays put and the view pans only if the cursor
    /// left it; a target the band cannot see recentres the hardware on it,
    /// as does `follow`.
    pub fn set_tuned(&mut self, hz: f64) {
        self.tuned_hz = hz;
        let reach = self.config.sample_rate * TUNE_REACH;
        if self.follow || (hz - self.config.centre_hz).abs() > reach {
            self.set_centre(hz);
        } else if (hz - self.view_centre()).abs() > self.span() * TUNE_REACH {
            self.pan_hz = hz - self.config.centre_hz;
            self.clamp_pan();
        }
    }

    /// Move the hardware window to `hz` and drop the display pan (`sdr
    /// centre`, right-drag on the canvas). With `follow` on the tuned
    /// cursor rides along, so the window still holds it centred.
    pub fn set_centre(&mut self, hz: f64) {
        self.config.centre_hz = hz;
        self.pan_hz = 0.0;
        if self.follow {
            self.tuned_hz = hz;
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
            .filter(|(k, _)| {
                k.abs_diff(dc) > DC_GUARD && (s.offset_hz(*k) - self.pan_hz).abs() <= half
            })
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, &p)| (self.config.centre_hz + s.offset_hz(k), p))
    }

    /// dBFS → 0..1 display position (1 = the reference level).
    pub fn level(&self, db: f64) -> f32 {
        (1.0 - (self.ref_db - db) / self.range_db).clamp(0.0, 1.0) as f32
    }

    /// The audio state the UI and `get audio` report. All of `off`,
    /// `no device`, `starting`, `muted` and `squelched` mean silence, so
    /// they are told apart by name.
    pub fn audio_state(&self) -> &'static str {
        if self.demod.is_none() {
            return "off";
        }
        match &self.audio {
            None => "starting",
            Some(a) if !a.available() => "no device",
            Some(_) if self.mute => "muted",
            Some(_) if self.audio_squelched => "squelched",
            Some(_) => "playing",
        }
    }
}

/// `cols` display columns across `span` Hz centred `pan` Hz from DC, each the peak of
/// the bins it covers (peak-preserving: a narrow carrier never vanishes
/// between columns).
pub fn columns(s: &IqSpectrum, pan: f64, span: f64, cols: usize) -> Vec<f64> {
    (0..cols)
        .map(|c| {
            let lo = pan + (c as f64 / cols as f64 - 0.5) * span;
            let hi = pan + ((c + 1) as f64 / cols as f64 - 0.5) * span;
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
    take_lab_result(&mut sdr);
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
    let cols = columns(&spec, sdr.pan_hz, sdr.span(), WF_W);
    // Scroll down one row and paint the new one on top. Black sits just
    // under the measured floor and white at the reference level, so the
    // noise reads dark and a signal a few dB above it already shows.
    sdr.waterfall.copy_within(0..WF_W * (WF_H - 1), WF_W);
    let black = spec.median_db() - WF_UNDER_FLOOR_DB;
    for (c, db) in cols.iter().enumerate() {
        let px = thermal(wf_level(*db, black, sdr.ref_db));
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
    scan::feed(&mut sdr, &frame);
    if sdr.detect_on {
        track(&mut sdr, &frame);
    }
    // The lab is costly (a channel filter and two recoveries), so it runs
    // on its own thread, a few times a second, on the signal nearest the
    // tuned frequency; `centre` is still the hardware window, the
    // spectrum's zero.
    if sdr.analyse_on {
        let (centre, setting) = (sdr.config.centre_hz, sdr.modulation);
        match sdr.nearest_track().cloned() {
            Some(target) => {
                sdr.lab
                    .get_or_insert_with(analysis::Lab::spawn)
                    .submit(&frame, centre, target, setting);
            }
            None => {
                sdr.analysis = None;
                sdr.classification = None;
            }
        }
    }
    if let Some(mode) = sdr.demod {
        feed_audio(&mut sdr, &frame, mode);
    }
}

/// A coarse upper bound on one DAB transmission frame, in samples at 2.048 MS/s,
/// used only to size the splice tolerance.
pub const FRAME_SAMPLES_HINT: i64 = 196_608;

/// Hand one frame's IQ to the DAB receiver.
///
/// Called from `ingest`, where **every** frame arrives, not from the display
/// path: that one is latest-wins by design (it only needs the newest frame to
/// paint), and a decoder fed from it sees a stream with holes in it. Measured on
/// air before this moved: 110 frames decoded out of ~3 700 received, i.e. about
/// one frame in six, because the receiver spent the rest of the time re-finding
/// the null symbol. Frames are `Arc`-shared and never copied for a consumer, and
/// the receiver keeps its own buffer, so a ragged chunk is normal input.
pub fn feed_dab(sdr: &mut SdrState, frame: &neowon_core::CaptureFrame) {
    let rate = frame.sample_rate;
    let start = (frame.t_start() * rate).round() as i64;
    let pairs = frame.channels[0].unit_count(frame.layout) as i64;
    // Tolerance is deliberately coarse. `CaptureFrame::t_start` is derived from
    // *arrival* time ("biased late by up to one poll"), so a tight bound fires
    // on ordinary jitter — which is how this check cost a real air session ~87%
    // of its attempts. It is a safety net for a stall or a retune, not splice
    // detection: that needs a dropped-sample counter from the backend, and it is
    // recorded as an open item in docs/protocol-dab.md.
    const JITTER_TOLERANCE_SAMPLES: i64 = 4 * crate::sdr::FRAME_SAMPLES_HINT;
    let spliced = matches!(sdr.dab_next_sample, Some(expected) if (start - expected).abs() > JITTER_TOLERANCE_SAMPLES);
    sdr.dab_next_sample = Some(start + pairs);
    let Some(rx) = sdr.dab.as_mut() else {
        return;
    };
    if spliced {
        rx.discard_buffer();
    }
    rx.push_iq(&frame.channels[0].data);
}

/// Adopt the lab's finished run, unless the operator has since turned the
/// lab off, retuned the hardware or changed the modulation setting.
fn take_lab_result(sdr: &mut SdrState) {
    let Some(r) = sdr.lab.as_mut().and_then(|l| l.poll()) else {
        return;
    };
    if sdr.active
        && sdr.analyse_on
        && r.centre_hz == sdr.config.centre_hz
        && r.setting == sdr.modulation
    {
        sdr.analysis = r.analysis;
        sdr.classification = r.classification;
    }
}

/// Demodulate the tuned channel from `frame` and push it to the sink. The
/// receiver and sink persist; only the config changes frame to frame.
fn feed_audio(sdr: &mut SdrState, frame: &neowon_core::CaptureFrame, mode: neowon_dsp::DemodMode) {
    let audio_rate = sdr
        .audio
        .get_or_insert_with(neowon_audio::sink::AudioOut::spawn)
        .rate();
    let cfg = neowon_dsp::ReceiverConfig {
        mode,
        offset_hz: sdr.tuned_hz - sdr.config.centre_hz,
        width_hz: sdr.channel_width().clamp(200.0, 0.9 * frame.sample_rate),
        sample_rate: frame.sample_rate,
        audio_rate,
        deemphasis_tau_s: matches!(
            mode,
            neowon_dsp::DemodMode::Nfm | neowon_dsp::DemodMode::Wfm
        )
        .then_some(75e-6),
    };
    let mut audio = std::mem::take(&mut sdr.audio_buf);
    {
        let rx = sdr
            .receiver
            .get_or_insert_with(|| neowon_dsp::Receiver::new(cfg));
        rx.configure(cfg);
        audio.clear();
        rx.process(&frame.channels[0].data, &mut audio);
        sdr.audio_channel_dbfs = rx.channel_dbfs();
    }
    sdr.audio_rms = if audio.is_empty() {
        0.0
    } else {
        (audio.iter().map(|x| x * x).sum::<f32>() / audio.len() as f32).sqrt()
    };
    sdr.audio_squelched = sdr.audio_channel_dbfs < sdr.squelch_db;
    if sdr.audio_squelched {
        // Silence, but keep the receiver's filters warm (no reset).
    } else if let Some(out) = &sdr.audio {
        out.push(&audio);
    }
    sdr.audio_buf = audio;
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
        let cols = columns(&s, 0.0, 2.048e6, WF_W);
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

    #[test]
    fn tuning_outside_the_band_moves_the_hardware() {
        let mut s = SdrState::default();
        s.config.centre_hz = 100e6;
        s.config.sample_rate = 2.048e6;
        // Inside the band: the cursor moves, the hardware stays.
        s.set_tuned(100.3e6);
        assert_eq!((s.tuned_hz, s.config.centre_hz), (100.3e6, 100e6));
        // Beyond it: the hardware recentres on the target.
        s.set_tuned(145.5e6);
        assert_eq!((s.tuned_hz, s.config.centre_hz), (145.5e6, 145.5e6));
        // Into the rolled-off edge: recentred too.
        s.set_tuned(145.5e6 + 0.95e6);
        assert_eq!(s.config.centre_hz, 146.45e6);
    }

    #[test]
    fn tuning_outside_a_zoomed_view_pans_it() {
        let mut s = SdrState::default();
        s.config.centre_hz = 100e6;
        s.config.sample_rate = 2.048e6;
        s.span_hz = 200e3;
        s.set_tuned(100.6e6);
        assert_eq!(s.config.centre_hz, 100e6);
        assert_eq!(s.view_centre(), 100.6e6);
        // A step that stays in view leaves the pan alone.
        s.set_tuned(100.62e6);
        assert_eq!(s.view_centre(), 100.6e6);
    }

    #[test]
    fn the_lab_runs_off_thread_and_skips_while_busy() {
        use neowon_backend::Backend;
        let mut b = neowon_sim::SimSdrBackend::new();
        b.apply(&InstrumentConfig::Sdr(SdrConfig::default()))
            .unwrap();
        assert!(b.set_stimulus("rf-digital").unwrap());
        let mut sdr = SdrState {
            tuned_hz: 100.3e6,
            ..Default::default()
        };
        let next = |b: &mut neowon_sim::SimSdrBackend| loop {
            if let Some(f) = b.poll_frame(std::time::Duration::from_millis(100)).unwrap() {
                return f;
            }
        };
        let mut frame = next(&mut b);
        for _ in 0..100 {
            track(&mut sdr, &frame);
            if sdr.nearest_track().is_some() {
                break;
            }
            frame = next(&mut b);
        }
        let target = sdr
            .nearest_track()
            .cloned()
            .expect("the QPSK signal tracked");

        let mut lab = analysis::Lab::spawn();
        assert!(lab.submit(&frame, 100e6, target.clone(), None));
        // In flight: the next frame is skipped, not queued.
        assert!(!lab.submit(&frame, 100e6, target, None));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let r = loop {
            if let Some(r) = lab.poll() {
                break r;
            }
            assert!(std::time::Instant::now() < deadline, "no lab result");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(r.centre_hz, 100e6);
        let a = r.analysis.expect("analysed");
        assert_eq!(a.modulation, neowon_core::Modulation::Qpsk);
        assert!(r.classification.is_some());
    }

    #[test]
    fn the_waterfall_floor_is_dark() {
        // Floor at -60 dBFS: the noise lands near black, a signal 10 dB
        // up is visibly lit, the reference level is white.
        let black = -60.0 - WF_UNDER_FLOOR_DB;
        assert!(wf_level(-60.0, black, 0.0) < 0.1);
        assert!(wf_level(-50.0, black, 0.0) > 0.2);
        assert_eq!(wf_level(0.0, black, 0.0), 1.0);
        assert_eq!(wf_level(-80.0, black, -90.0), 0.0);
    }
}
