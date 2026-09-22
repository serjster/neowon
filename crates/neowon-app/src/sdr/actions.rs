//! SDR script actions: `sdr <verb> …` and `sim iq --seed <n>`, parsed here
//! and applied to `SdrState` (the UI injects the same actions).

use bevy::log::error;
use neowon_backend::{Command, SdrGain};

use super::{FFT_SIZES, SdrState};
use crate::Link;

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
    Detect(bool),
    /// Start a survey; `None` stops a running one.
    Survey(Option<SurveyRequest>),
    /// Run the modulation lab on the signal nearest the tuned frequency.
    Analyse(bool),
    /// The modulation the lab assumes; `None` picks it from cumulants.
    Modulation(Option<neowon_core::Modulation>),
    /// Detection threshold over the floor, dB.
    Threshold(f64),
    /// Display centre, Hz from the hardware centre (display only).
    Pan(f64),
    /// The hardware window centre, Hz (`sdr centre`); `sdr tune` moves the
    /// tuned frequency instead unless `follow` is on.
    Centre(f64),
    /// The hardware window follows the tuned frequency (`sdr follow`).
    Follow(bool),
    /// Channel width, Hz, around the tuned frequency; `None` takes it from
    /// the nearest detection's occupied bandwidth (`sdr width auto`).
    Width(Option<f64>),
    /// Audio demodulator; `None` is off (`sdr demod am|nfm|wfm|off`).
    Demod(Option<neowon_dsp::DemodMode>),
    /// Audio volume, 0..=1 (`sdr volume`).
    Volume(f32),
    /// Mute without dropping the demodulator (`sdr mute`).
    Mute(bool),
    /// Squelch threshold in dBFS, or `None` to open the gate (`sdr squelch`).
    Squelch(Option<f64>),
    /// Height of the dock's signal list, points.
    List(f32),
    /// `instrument sdr` (true) or `instrument scope`: switch instrument.
    Instrument(bool),
    /// DAB decoding (`sdr dab on|off|reset`).
    Dab(DabVerb),
    /// Raw IQ capture for offline replay (`sdr iqdump`).
    IqDump(IqDumpVerb),
}

/// `sdr iqdump <path> <seconds>` starts a capture of the complex frames the
/// streaming consumers receive; `sdr iqdump off` stops it early.
#[derive(Debug, Clone, PartialEq)]
pub enum IqDumpVerb {
    Start { path: String, seconds: f64 },
    Stop,
}

/// What `sdr dab` does. The receiver is a wideband consumer of raw IQ, so
/// it is switched on and off rather than set to a frequency: it decodes
/// whatever ensemble the hardware window covers.
#[derive(Debug, Clone, PartialEq)]
pub enum DabVerb {
    On,
    Off,
    Reset,
    /// Select a service (`sdr dab service …`).
    Service(DabService),
    /// Start audio transport (`sdr dab play`). The state is real; 10.15.3
    /// consumes it.
    Play,
    /// Stop audio transport (`sdr dab stop`).
    Stop,
    /// Land the hardware window on a Band III block (`sdr dab channel …`).
    Channel(DabChannel),
}

/// How `sdr dab channel` names its target: the raster step verbs, or a
/// block label (`11C`). The 38 block centres are `neowon-refdb`'s table;
/// the band plan decides which of them the operator's country allocates.
#[derive(Debug, Clone, PartialEq)]
pub enum DabChannel {
    Next,
    Prev,
    Label(String),
}

/// How `sdr dab service` names a service: by `SId`, or by 1-based position
/// in the locked table (written `#n`, since service numbers are 16-bit and
/// a bare number would be ambiguous).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DabService {
    Sid(u16),
    Index(u16),
}

/// `sdr survey <start> <stop> [cap N] [skip lo:hi]…`.
#[derive(Debug, Clone, PartialEq)]
pub struct SurveyRequest {
    pub start_hz: f64,
    pub stop_hz: f64,
    /// Most peaks kept per step.
    pub peak_cap: usize,
    /// Ranges not to scan.
    pub skip: Vec<(f64, f64)>,
}

/// The script line for an action. Every variant has one (the match is
/// exhaustive), and `parse` reads it back to the same action: the UI
/// injects these actions, so this is the script-parity rule by
/// construction.
impl std::fmt::Display for SdrAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let on = |b: bool| if b { "on" } else { "off" };
        match self {
            SdrAction::Tune(hz) => write!(f, "sdr tune {hz}"),
            SdrAction::Step(hz) => write!(f, "sdr step {hz}"),
            SdrAction::Rate(r) => write!(f, "sdr rate {r}"),
            SdrAction::Gain(None) => write!(f, "sdr gain auto"),
            SdrAction::Gain(Some(db)) => write!(f, "sdr gain {db}"),
            SdrAction::Agc(b) => write!(f, "sdr agc {}", on(*b)),
            SdrAction::Ppm(p) => write!(f, "sdr ppm {p}"),
            SdrAction::Span(hz) => write!(f, "sdr span {hz}"),
            SdrAction::Fft(n) => write!(f, "sdr fft {n}"),
            SdrAction::Level { ref_db, range_db } => write!(f, "sdr level {ref_db} {range_db}"),
            SdrAction::Run(b) => write!(f, "sdr run {}", on(*b)),
            SdrAction::Seed(s) => write!(f, "sim iq --seed {s}"),
            SdrAction::Detect(b) => write!(f, "sdr detect {}", on(*b)),
            SdrAction::Survey(None) => write!(f, "sdr survey stop"),
            SdrAction::Survey(Some(r)) => {
                write!(
                    f,
                    "sdr survey {} {} cap {}",
                    r.start_hz, r.stop_hz, r.peak_cap
                )?;
                r.skip
                    .iter()
                    .try_for_each(|(a, b)| write!(f, " skip {a}:{b}"))
            }
            SdrAction::Analyse(b) => write!(f, "sdr analyse {}", on(*b)),
            SdrAction::Modulation(None) => write!(f, "sdr modulation auto"),
            SdrAction::Modulation(Some(m)) => write!(f, "sdr modulation {}", m.label()),
            SdrAction::Threshold(db) => write!(f, "sdr threshold {db}"),
            SdrAction::Pan(hz) => write!(f, "sdr pan {hz}"),
            SdrAction::Centre(hz) => write!(f, "sdr centre {hz}"),
            SdrAction::Follow(b) => write!(f, "sdr follow {}", on(*b)),
            SdrAction::Width(None) => write!(f, "sdr width auto"),
            SdrAction::Width(Some(hz)) => write!(f, "sdr width {hz}"),
            SdrAction::Demod(None) => write!(f, "sdr demod off"),
            SdrAction::Demod(Some(m)) => write!(f, "sdr demod {}", m.verb()),
            SdrAction::Dab(DabVerb::On) => write!(f, "sdr dab on"),
            SdrAction::Dab(DabVerb::Off) => write!(f, "sdr dab off"),
            SdrAction::Dab(DabVerb::Reset) => write!(f, "sdr dab reset"),
            SdrAction::Dab(DabVerb::Play) => write!(f, "sdr dab play"),
            SdrAction::Dab(DabVerb::Stop) => write!(f, "sdr dab stop"),
            SdrAction::Dab(DabVerb::Service(DabService::Sid(sid))) => {
                write!(f, "sdr dab service {sid}")
            }
            SdrAction::Dab(DabVerb::Service(DabService::Index(n))) => {
                write!(f, "sdr dab service #{n}")
            }
            SdrAction::Dab(DabVerb::Channel(DabChannel::Next)) => {
                write!(f, "sdr dab channel next")
            }
            SdrAction::Dab(DabVerb::Channel(DabChannel::Prev)) => {
                write!(f, "sdr dab channel prev")
            }
            SdrAction::Dab(DabVerb::Channel(DabChannel::Label(label))) => {
                write!(f, "sdr dab channel {label}")
            }
            SdrAction::Volume(v) => write!(f, "sdr volume {v}"),
            SdrAction::Mute(b) => write!(f, "sdr mute {}", on(*b)),
            SdrAction::Squelch(None) => write!(f, "sdr squelch off"),
            SdrAction::Squelch(Some(db)) => write!(f, "sdr squelch {db}"),
            SdrAction::List(px) => write!(f, "sdr list {px}"),
            SdrAction::Instrument(sdr) => {
                write!(f, "instrument {}", if *sdr { "sdr" } else { "scope" })
            }
            SdrAction::IqDump(IqDumpVerb::Start { path, seconds }) => {
                write!(f, "sdr iqdump {path} {seconds}")
            }
            SdrAction::IqDump(IqDumpVerb::Stop) => write!(f, "sdr iqdump off"),
        }
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
pub fn apply(a: SdrAction, sdr: &mut SdrState, link: &mut Link) -> Result<(), String> {
    let caps = sdr.caps.clone();
    let in_range = |hz: f64| match &caps {
        Some(c) if !(c.freq_range_hz.0..=c.freq_range_hz.1).contains(&hz) => Err(format!(
            "{hz} Hz outside {}..={} Hz",
            c.freq_range_hz.0, c.freq_range_hz.1
        )),
        _ => Ok(hz),
    };
    match a {
        SdrAction::Tune(hz) => {
            let hz = in_range(hz)?;
            let before = sdr.config.centre_hz;
            sdr.set_tuned(hz);
            sdr.dirty |= sdr.config.centre_hz != before;
            return Ok(());
        }
        SdrAction::Step(hz) => {
            let hz = in_range(sdr.tuned_hz + hz)?;
            let before = sdr.config.centre_hz;
            sdr.set_tuned(hz);
            sdr.dirty |= sdr.config.centre_hz != before;
            return Ok(());
        }
        SdrAction::Rate(r) => {
            if let Some(c) = &caps
                && !c.sample_rates.iter().any(|&x| (x - r).abs() < 0.5)
            {
                return Err(format!("rate {r} not in {:?}", c.sample_rates));
            }
            if r <= 0.0 {
                return Err("rate must be positive".into());
            }
            let changed = sdr.config.sample_rate != r;
            sdr.config.sample_rate = r;
            sdr.clamp_pan();
            if changed {
                // Mode I is defined at exactly 2.048 MS/s: another rate is a
                // different signal, and the old table cannot be re-derived.
                sdr.dab_reset();
            }
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
        SdrAction::Run(on) => {
            if !on {
                // The switch stops the stream: there will be no frames to
                // re-derive the table from, so it dies with the stream (D27).
                sdr.dab_reset();
            }
            sdr.config.running = on;
        }
        SdrAction::Span(hz) => {
            if hz < 0.0 {
                return Err("span must be >= 0".into());
            }
            sdr.span_hz = hz;
            sdr.clamp_pan();
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
        SdrAction::Analyse(on) => {
            sdr.analyse_on = on;
            if !on {
                sdr.analysis = None;
            }
            return Ok(());
        }
        SdrAction::Modulation(m) => {
            sdr.modulation = m;
            sdr.analysis = None;
            return Ok(());
        }
        SdrAction::Survey(None) => {
            sdr.survey = None;
            return Ok(());
        }
        SdrAction::Survey(Some(r)) => {
            let plan = neowon_sdr::survey::SurveyPlan {
                start_hz: r.start_hz,
                stop_hz: r.stop_hz,
                sample_rate: sdr.config.sample_rate,
                peak_cap: r.peak_cap,
                skip: r.skip,
                ..Default::default()
            };
            return super::scan::start(sdr, plan);
        }
        SdrAction::Detect(on) => {
            sdr.detect_on = on;
            if !on {
                sdr.tracker.clear();
            }
            return Ok(());
        }
        SdrAction::Threshold(db) => {
            if !(3.0..=60.0).contains(&db) {
                return Err(format!("threshold {db} dB outside 3..=60"));
            }
            sdr.threshold_db = db;
            return Ok(());
        }
        SdrAction::Pan(hz) => {
            sdr.pan_hz = hz;
            sdr.clamp_pan();
            return Ok(());
        }
        SdrAction::Centre(hz) => sdr.set_centre(in_range(hz)?),
        SdrAction::Follow(on) => {
            sdr.follow = on;
            if on {
                let before = sdr.config.centre_hz;
                sdr.set_centre(sdr.tuned_hz);
                sdr.dirty |= sdr.config.centre_hz != before;
            }
            return Ok(());
        }
        SdrAction::Width(None) => {
            sdr.width_auto = true;
            return Ok(());
        }
        SdrAction::Width(Some(hz)) => {
            if !(1.0..=sdr.config.sample_rate).contains(&hz) {
                return Err(format!(
                    "width {hz} Hz outside 1..={} Hz",
                    sdr.config.sample_rate
                ));
            }
            sdr.width_hz = hz;
            sdr.width_auto = false;
            return Ok(());
        }
        SdrAction::Dab(verb) => match verb {
            // A fresh receiver: a retune means a different ensemble, and the
            // old table would be a different station's.
            DabVerb::On => {
                sdr.dab_reset();
                sdr.dab = Some(neowon_dsp::dab::DabReceiver::new());
            }
            DabVerb::Off => {
                sdr.dab_reset();
                sdr.dab = None;
            }
            DabVerb::Reset => sdr.dab_reset(),
            DabVerb::Service(choice) => {
                let Some(rx) = sdr.dab.as_ref() else {
                    return Err("dab service: receiver is off (sdr dab on)".into());
                };
                let status = rx.status();
                if !status.locked {
                    return Err("dab service: no ensemble table yet".into());
                }
                let sid = match choice {
                    DabService::Sid(sid) => {
                        if !status.ensemble.services.contains_key(&sid) {
                            return Err(format!("dab service: SId {sid} not in the ensemble"));
                        }
                        sid
                    }
                    DabService::Index(n) => {
                        let services: Vec<u16> = status.ensemble.services.keys().copied().collect();
                        let index = (n as usize)
                            .checked_sub(1)
                            .filter(|i| *i < services.len())
                            .ok_or_else(|| {
                                format!("dab service: index #{n} outside 1..={}", services.len())
                            })?;
                        services[index]
                    }
                };
                sdr.dab_service = Some(sid);
                sdr.dab_play_error = None;
                // Selecting a service while playing is the cue the audio
                // follows: the old worker stops (its decoders with it) and
                // a fresh one starts on the new stream. If the new stream
                // cannot be set up, playback stops and the reason surfaces.
                if super::dab_audio::playing(sdr) {
                    super::dab_audio::stop(sdr);
                    if let Err(reason) = super::dab_audio::play(sdr) {
                        sdr.dab_play_error = Some(reason.clone());
                        return Err(reason);
                    }
                }
                return Ok(());
            }
            DabVerb::Play => {
                match super::dab_audio::play(sdr) {
                    Ok(()) => sdr.dab_play_error = None,
                    Err(reason) => {
                        sdr.dab_play_error = Some(reason.clone());
                        return Err(reason);
                    }
                }
                return Ok(());
            }
            DabVerb::Stop => {
                super::dab_audio::stop(sdr);
                sdr.dab_play_error = None;
                return Ok(());
            }
            DabVerb::Channel(choice) => {
                let (label, hz) = super::dab::channel(sdr, &choice)?;
                // The status line is the script's receipt: it names the
                // block and the exact centre the hardware now runs.
                link.status = format!("DAB {label} {:.3} MHz", hz / 1e6);
                return Ok(());
            }
        },
        SdrAction::IqDump(verb) => {
            match verb {
                IqDumpVerb::Stop => {
                    super::iqdump::stop(sdr);
                }
                IqDumpVerb::Start { path, seconds } => {
                    sdr.iq_dump = Some(super::iqdump::IqDump::start(
                        &path,
                        seconds,
                        sdr.config.sample_rate,
                    )?);
                }
            }
            return Ok(());
        }
        SdrAction::Demod(m) => {
            if sdr.demod != m {
                // A mode change starts the channel clean; switching off
                // drops any queued audio rather than letting it finish.
                sdr.receiver = None;
                sdr.audio_buf.clear();
                sdr.audio_rms = 0.0;
                sdr.audio_squelched = false;
                if let Some(out) = &sdr.audio {
                    out.clear();
                }
            }
            sdr.demod = m;
            return Ok(());
        }
        SdrAction::Volume(v) => {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("volume {v} outside 0..=1"));
            }
            sdr.volume = v;
            if let Some(out) = &sdr.audio {
                out.set_volume(v);
            }
            return Ok(());
        }
        SdrAction::Mute(on) => {
            sdr.mute = on;
            if let Some(out) = &sdr.audio {
                out.set_mute(on);
            }
            return Ok(());
        }
        SdrAction::Squelch(db) => {
            sdr.squelch_db = db.unwrap_or(-120.0);
            return Ok(());
        }
        SdrAction::List(px) => {
            if !(40.0..=2000.0).contains(&px) {
                return Err(format!("list height {px} outside 40..=2000"));
            }
            sdr.list_px = px;
            return Ok(());
        }
        SdrAction::Instrument(to_sdr) => return super::instrument::switch(to_sdr, sdr, link),
        SdrAction::Seed(s) => {
            sdr.seed = s;
            let _ = link.sup.commands.send(Command::Seed(s));
            return Ok(());
        }
    }
    sdr.dirty = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdr::parse::{parse, parse_instrument, parse_sim};

    fn words<'a>(s: &'a str) -> impl FnMut() -> Result<&'a str, String> {
        let mut w = s.split_whitespace();
        move || w.next().ok_or_else(|| "missing argument".to_string())
    }

    /// Which variant an action is. Exhaustive, so a new variant fails to
    /// compile here until `every_action_round_trips` covers it.
    fn variant(a: &SdrAction) -> usize {
        match a {
            SdrAction::Tune(_) => 0,
            SdrAction::Step(_) => 1,
            SdrAction::Rate(_) => 2,
            SdrAction::Gain(_) => 3,
            SdrAction::Agc(_) => 4,
            SdrAction::Ppm(_) => 5,
            SdrAction::Span(_) => 6,
            SdrAction::Fft(_) => 7,
            SdrAction::Level { .. } => 8,
            SdrAction::Run(_) => 9,
            SdrAction::Seed(_) => 10,
            SdrAction::Detect(_) => 11,
            SdrAction::Survey(_) => 12,
            SdrAction::Analyse(_) => 13,
            SdrAction::Modulation(_) => 14,
            SdrAction::Threshold(_) => 15,
            SdrAction::Instrument(_) => 16,
            SdrAction::Pan(_) => 17,
            SdrAction::List(_) => 18,
            SdrAction::Centre(_) => 19,
            SdrAction::Follow(_) => 20,
            SdrAction::Width(_) => 21,
            SdrAction::Demod(_) => 22,
            SdrAction::Dab(_) => 26,
            SdrAction::IqDump(_) => 27,
            SdrAction::Volume(_) => 23,
            SdrAction::Mute(_) => 24,
            SdrAction::Squelch(_) => 25,
        }
    }

    /// Script parity: every action the UI can inject prints as a script
    /// line that parses back to the same action.
    #[test]
    fn every_action_round_trips() {
        let all = [
            SdrAction::Tune(99.412_345e6),
            SdrAction::Step(-1e5),
            SdrAction::Rate(2.048e6),
            SdrAction::Gain(None),
            SdrAction::Gain(Some(29.7)),
            SdrAction::Agc(true),
            SdrAction::Ppm(-1.3),
            SdrAction::Span(200e3),
            SdrAction::Fft(8192),
            SdrAction::Level {
                ref_db: -12.5,
                range_db: 90.0,
            },
            SdrAction::Run(false),
            SdrAction::Seed(7),
            SdrAction::Detect(true),
            SdrAction::Survey(None),
            SdrAction::Survey(Some(SurveyRequest {
                start_hz: 88e6,
                stop_hz: 108e6,
                peak_cap: 8,
                skip: vec![(99e6, 99.5e6), (101e6, 101.2e6)],
            })),
            SdrAction::Analyse(true),
            SdrAction::Modulation(None),
            SdrAction::Modulation(Some(neowon_core::Modulation::Psk8)),
            SdrAction::Threshold(9.5),
            SdrAction::Instrument(true),
            SdrAction::Instrument(false),
            SdrAction::Pan(-250e3),
            SdrAction::List(212.5),
            SdrAction::Centre(100e6),
            SdrAction::Follow(true),
            SdrAction::Width(None),
            SdrAction::Width(Some(15e3)),
            SdrAction::Demod(None),
            SdrAction::Demod(Some(neowon_dsp::DemodMode::Nfm)),
            SdrAction::Volume(0.85),
            SdrAction::Mute(true),
            SdrAction::Squelch(None),
            SdrAction::Squelch(Some(-30.0)),
            // `Dab` was the one variant with no sample here (review M8).
            SdrAction::Dab(DabVerb::On),
            SdrAction::Dab(DabVerb::Off),
            SdrAction::Dab(DabVerb::Reset),
            SdrAction::Dab(DabVerb::Service(DabService::Sid(0x1001))),
            SdrAction::Dab(DabVerb::Service(DabService::Index(2))),
            SdrAction::Dab(DabVerb::Play),
            SdrAction::Dab(DabVerb::Stop),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Next)),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Prev)),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Label("11C".into()))),
            SdrAction::IqDump(IqDumpVerb::Start {
                path: "tmp-inspiration/iq.f32".into(),
                seconds: 15.0,
            }),
            SdrAction::IqDump(IqDumpVerb::Stop),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for a in all {
            seen.insert(variant(&a));
            let line = a.to_string();
            let (head, rest) = line.split_once(' ').unwrap();
            let mut w = words(rest);
            let back = match head {
                "sdr" => parse(&mut w),
                "sim" => parse_sim(&mut w),
                "instrument" => parse_instrument(&mut w),
                _ => panic!("{line}"),
            };
            assert_eq!(back.unwrap(), a, "{line}");
        }
        assert_eq!(seen.len(), 28, "a variant has no round-trip sample");
    }
}
