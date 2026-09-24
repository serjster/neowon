//! SDR mode: state and the frame → spectrum / waterfall /
//! constellation / detection pipeline. Script actions live in `actions`,
//! control-socket readouts in `readout`.
//!
//! The mode follows the connected backend: `Capabilities::Sdr` switches the
//! app into it, and complex frames are routed here rather than to the scope
//! consumers (phosphor, measurements, recorder), which then simply idle.
//! Every display is derived from `neowon_dsp::iq_spectrum`, the oracle, and
//! notched at DC by `display::mask_dc` before the trace and the waterfall
//! read it, so the two can never disagree. The notch is the receiver's DC
//! offset, not a measurement; DSP consumers keep the unmasked spectrum.

use bevy::prelude::*;
use neowon_backend::{InstrumentConfig, SdrCaps, SdrConfig};
use neowon_core::{CaptureFrame, SampleLayout, SharedFrame};
use neowon_dsp::{DetectConfig, IqSpectrum, Tracker, TrackerConfig, Window, detect, iq_spectrum};

use crate::Link;

mod actions;
pub mod analysis;
mod audio;
pub mod dab;
pub mod dab_audio;
mod dab_gone;
pub mod dab_scene;
mod dab_state;
mod display;
mod instrument;
pub mod iqdump;
mod parse;
mod readout;
pub mod scan;
pub mod zoom;

use crate::viz::waterfall::thermal;
pub use actions::{DabChannel, DabService, DabVerb, IqDumpVerb, SdrAction, run};
pub use audio::{AudioDevice, AudioOwner};
pub use dab_state::{DabState, Gone, GoneCause};
use display::wf_level;
pub use display::{columns, mask_dc};
pub use parse::{parse, parse_hz, parse_instrument, parse_sim};
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

#[derive(Resource)]
pub struct SdrState {
    /// True once an SDR backend connected; the UI and flush follow it.
    /// The instrument's capabilities are not duplicated here: the link
    /// carries the one `Option<Capabilities>` and `Link::sdr_caps()`
    /// answers with this half of it.
    pub active: bool,
    pub config: SdrConfig,
    pub dirty: bool,
    /// Last seed sent to a generating backend.
    pub seed: u64,
    pub latest: Option<SharedFrame>,
    pub frames_seen: u64,
    /// I/Q pairs the backend reported lost before the frames it delivered,
    /// cumulative for the session (`CaptureFrame::dropped_before`): a USB
    /// overflow, or the chunk discarded after a retune. This is what makes
    /// the live-continuity claim falsifiable — without it a drop is
    /// invisible everywhere downstream.
    pub dropped_pairs: u64,
    /// How many frames arrived after such a gap. Two numbers, because one
    /// long stall and forty short ones are different faults.
    pub drop_events: u64,
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
    /// Everything the DAB consumer derived from the signal — receiver,
    /// timing grid, selection, PAD parsers, playback — under one owner,
    /// cleared only through [`SdrState::dab_reset`] and its siblings so no
    /// site can forget half of it. See [`dab_state`].
    pub dab: DabState,
    /// Active raw-IQ capture (`sdr iqdump`): frames are written as they
    /// arrive, so a hardware session can be replayed offline.
    pub iq_dump: Option<iqdump::IqDump>,
    /// The output device, opened on first use. One producer writes to it at
    /// a time; see [`SdrState::audio_owner`] and [`SdrState::push_audio`].
    pub audio: AudioDevice,
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
    /// Latest spectrum from the oracle, unmasked: the floor readout and the
    /// measurements use it. The displays read `columns`, whose DC spike is
    /// notched out (`display::mask_dc`).
    pub spectrum: Option<IqSpectrum>,
    /// Display columns of the latest masked spectrum (peak of the bins each
    /// covers), dBFS — what both the trace and the waterfall row show.
    pub columns: Vec<f64>,
    /// Palettized waterfall, row-major RGBA, newest row first.
    pub waterfall: Vec<[u8; 4]>,
    /// Rows pushed so far; the UI re-uploads the texture when it changes.
    pub wf_rows: u64,
    pub iq: Vec<[f32; 2]>,
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
    /// The lab's latest result and the classifier's verdict from the same
    /// run. Each carries its target's track id; readouts reach them only
    /// through `analysis_of` / `classification_of`, never bare.
    pub analysis: Option<analysis::Analysis>,
    pub classification: Option<analysis::Classified>,
    /// The lab's worker thread, started on first use.
    lab: Option<analysis::Lab>,
    /// A survey in progress, and the last completed ones (oldest first).
    pub survey: Option<neowon_dsp::survey::Survey>,
    pub surveys: Vec<neowon_dsp::survey::SurveyResult>,
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
            config: SdrConfig::default(),
            dirty: false,
            seed: 1,
            latest: None,
            frames_seen: 0,
            dropped_pairs: 0,
            drop_events: 0,
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
            dab: DabState::default(),
            iq_dump: None,
            audio: AudioDevice::default(),
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

    /// Account for one arriving IQ frame: it is one more frame seen, and
    /// whatever the backend says was lost before it is a gap in the stream.
    /// The counting lives here rather than in the ingest system so there is
    /// one place a drop can be forgotten, instead of one per consumer.
    pub fn note_frame(&mut self, frame: &CaptureFrame, now: f64) {
        self.frames_seen += 1;
        let dropped = frame.dropped_before();
        if dropped > 0 {
            self.dropped_pairs += dropped;
            self.drop_events += 1;
            tracing::warn!(
                pairs = dropped,
                total = self.dropped_pairs,
                "IQ stream gap: samples lost before this frame"
            );
        }
        self.dab.last_frame_at = Some(now);
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

    /// The lab's result for track `id`, or `None` when its latest run
    /// measured another signal (or none): a readout joins a lab result to a
    /// detection by identity, so one signal's EVM is never shown beside
    /// another's centre and power.
    pub fn analysis_of(&self, id: u64) -> Option<&analysis::Analysis> {
        self.analysis.as_ref().filter(|a| a.track == id)
    }

    /// The classifier's verdict on track `id`, joined the same way.
    pub fn classification_of(&self, id: u64) -> Option<&analysis::Classified> {
        self.classification.as_ref().filter(|c| c.track == id)
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
    ///
    /// A move is a new signal: the DAB receiver forgets its lock and table,
    /// because a table decoded from the old window would name a station that
    /// may not be at the new one.
    pub fn set_centre(&mut self, hz: f64) {
        let moved = self.config.centre_hz != hz;
        self.config.centre_hz = hz;
        self.pan_hz = 0.0;
        if self.follow {
            self.tuned_hz = hz;
        }
        if moved {
            self.dab_reset();
        }
    }

    /// Forget the DAB receiver's lock, table, timing, selection and
    /// playback: the hardware window moved, the stream stopped or the
    /// operator reset it.
    ///
    /// **The one entry point.** Every site that moves the hardware window,
    /// stops the stream, switches instrument or cycles the receiver calls
    /// this; `DabState` does the clearing and this adds the half it does
    /// not own — flushing the device so a stopped service cannot play on.
    pub fn dab_reset(&mut self) {
        if self.dab.reset() {
            self.audio.clear();
        }
        debug_assert!(!self.dab.holds_derived(), "a reset left DAB state behind");
    }

    /// The frame stream stopped, so the lock, table and timing grid
    /// describe a stream that is no longer there (`dab::NO_INPUT_TIMEOUT_S`).
    pub fn dab_no_input(&mut self) {
        if self.dab.no_input() {
            self.audio.clear();
        }
        debug_assert!(!self.dab.holds_derived(), "no_input left DAB state behind");
    }

    /// The receiver's table expired under a selection: drop the selection,
    /// its parsers and playback, and leave the running receiver alone.
    pub fn dab_forget_selection(&mut self) {
        if self.dab.forget_selection() {
            self.audio.clear();
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
}

/// Fold newly arrived frames into the displays.
pub fn update(mut sdr: ResMut<SdrState>, link: Res<Link>) {
    // Decoded DAB audio and its PAD reach the sink and the parsers every
    // frame, including frames with no new IQ.
    dab_audio::drain(&mut sdr);
    take_lab_result(&mut sdr);
    let Some(frame) = sdr.latest.clone() else {
        return;
    };
    if !sdr.active || sdr.last_seq == Some(frame.seq) || frame.layout() != SampleLayout::Complex {
        return;
    }
    sdr.last_seq = Some(frame.seq);
    let data = &frame.channels[0].data;
    let Some(spec) = iq_spectrum(data, frame.sample_rate, Window::Hann, sdr.fft_size) else {
        return;
    };
    // The displays read a copy with the receiver's DC spike notched out
    // (display::mask_dc); `spec` itself stays raw for the floor readout and
    // every DSP consumer. One masked spectrum feeds both the trace and the
    // waterfall through the same `cols`.
    let masked = mask_dc(&spec, DC_GUARD);
    let cols = columns(&masked, sdr.pan_hz, sdr.span(), WF_W);
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
        track(&mut sdr, &frame, link.sdr_caps());
    }
    // The lab is costly (a channel filter and two recoveries), so it runs
    // on its own thread, a few times a second, on the signal nearest the
    // tuned frequency; `centre` is still the hardware window, the
    // spectrum's zero.
    if sdr.analyse_on {
        let (centre, setting) = (sdr.config.centre_hz, sdr.modulation);
        match sdr.nearest_track().cloned() {
            Some(target) => {
                // The target moved to another signal: what the lab holds
                // belongs to the old one and is dropped, not relabelled.
                if sdr.analysis.as_ref().is_some_and(|a| a.track != target.id) {
                    sdr.analysis = None;
                }
                if sdr
                    .classification
                    .as_ref()
                    .is_some_and(|c| c.track != target.id)
                {
                    sdr.classification = None;
                }
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
        audio::feed(&mut sdr, &frame, mode);
    }
}

/// Adopt the lab's finished run, unless the operator has since turned the
/// lab off, retuned the hardware, changed the modulation setting, or moved
/// the cursor to another signal (the run's target is no longer the one
/// nearest the tuned frequency).
fn take_lab_result(sdr: &mut SdrState) {
    if let Some(r) = sdr.lab.as_mut().and_then(|l| l.poll()) {
        adopt(sdr, r);
    }
}

fn adopt(sdr: &mut SdrState, r: analysis::LabResult) {
    if sdr.active
        && sdr.analyse_on
        && r.centre_hz == sdr.config.centre_hz
        && r.setting == sdr.modulation
        && sdr.nearest_track().map(|t| t.id) == Some(r.track)
    {
        sdr.analysis = r.analysis;
        sdr.classification = r.classification;
    }
}

/// Detect in the frame (one detection frame per SDR frame) and fold the
/// observations into the tracker.
fn track(sdr: &mut SdrState, frame: &neowon_core::CaptureFrame, caps: Option<&SdrCaps>) {
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
        dc_guard: if caps.is_some_and(|c| c.tuner == "sim") {
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
pub(crate) mod join_tests;

#[cfg(test)]
mod tests {
    use super::*;

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
            track(&mut sdr, &frame, None);
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
}
