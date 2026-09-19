//! The modulation lab in SDR mode: analyse the tracked signal nearest the
//! tuned frequency — its symbol rate, modulation (set, or picked by nearest
//! cumulants), recovered constellation, EVM and MER.
//!
//! The lab costs a channel filter, a symbol-rate search and two recoveries
//! — tens of milliseconds — so it runs on its own thread (`Lab`), never on
//! the frame loop: a result arrives a run later, and the display keeps its
//! frame rate meanwhile.

use crossbeam_channel::{Receiver, Sender, TryRecvError};

use neowon_core::{CaptureFrame, Modulation, SharedFrame};
use neowon_dsp::Track;
use neowon_dsp::classify::{Classification, classify as dsp_classify, features};
use neowon_dsp::modlab::{Cumulants, cumulants, recover, select, symbol_rate};

/// Frames between lab runs (a few runs a second at the sim's frame rate).
const EVERY: u64 = 8;

/// Roll-off the lab assumes (the common RRC choice; also the simulator's).
pub const ROLLOFF: f64 = 0.35;
/// Pairs of a frame the lab looks at (bounds its cost; ≥ 800 symbols at
/// the rates the simulator uses).
const PAIRS: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct Analysis {
    pub track: u64,
    pub centre_hz: f64,
    pub symbol_rate_hz: f64,
    pub modulation: Modulation,
    /// The modulation was picked from cumulants, not set.
    pub auto: bool,
    pub evm_rms_pct: f64,
    pub mer_db: f64,
    pub cumulants: Cumulants,
    /// Recovered decision-point samples (up to 2048), for display.
    pub symbols: Vec<[f32; 2]>,
}

/// The modulation whose ideal (C42, |C40|) is nearest the measured ones;
/// both are rotation-invariant, so a blind front end is enough.
pub fn nearest(c: &Cumulants) -> Modulation {
    let ideal = |m: Modulation| match m {
        Modulation::Bpsk => (-2.0, 2.0),
        Modulation::Qpsk => (-1.0, 1.0),
        Modulation::Psk8 => (-1.0, 0.0),
        Modulation::Qam16 => (-0.68, 0.68),
        Modulation::Qam64 => (-0.619, 0.619),
    };
    Modulation::ALL
        .into_iter()
        .min_by(|&a, &b| {
            let d = |m| {
                let (c42, c40) = ideal(m);
                (c.c42 - c42).powi(2) + (c.c40.norm() - c40).powi(2)
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(Modulation::Qpsk)
}

/// Analyse `track` in `frame` (tuned to `centre_hz`), with `setting` the
/// user's modulation or `None` to pick one.
pub fn analyse(
    frame: &CaptureFrame,
    centre_hz: f64,
    track: &Track,
    setting: Option<Modulation>,
) -> Option<Analysis> {
    let rate = frame.sample_rate;
    let data = &frame.channels[0].data;
    let data = &data[..data.len().min(2 * PAIRS)];
    let obw = track.last.bandwidth_hz().max(rate / 1000.0);
    // Channel: the occupied band plus margin. An RRC signal reaches
    // ±Rs(1 + β)/2 ≈ ±OBW/2, so the cutoff sits well outside it, with taps
    // enough for the transition band to clear the signal's edge.
    let taps = ((8.0 * rate / obw) as usize).clamp(65, 401);
    let chan = select(
        data,
        rate,
        track.last.centre_hz - centre_hz,
        0.8 * obw,
        taps,
    );
    // OBW(99%) of an RRC signal is about Rs·(1 + β): search around it.
    let rs = symbol_rate(&chan, rate, 0.4 * obw, 1.2 * obw)?;
    let blind = recover(&chan, rate, rs, Modulation::Qpsk, ROLLOFF)?;
    let syms: Vec<f32> = blind
        .symbols
        .iter()
        .flat_map(|z| [z.re as f32, z.im as f32])
        .collect();
    let c = cumulants(&syms)?;
    let modulation = setting.unwrap_or_else(|| nearest(&c));
    let r = recover(&chan, rate, rs, modulation, ROLLOFF)?;
    Some(Analysis {
        track: track.id,
        centre_hz: track.last.centre_hz,
        symbol_rate_hz: rs,
        modulation,
        auto: setting.is_none(),
        evm_rms_pct: r.evm_rms_pct,
        mer_db: r.mer_db,
        cumulants: c,
        symbols: r
            .symbols
            .iter()
            .take(2048)
            .map(|z| [z.re as f32, z.im as f32])
            .collect(),
    })
}

/// The DSP classifier's verdict on `track`.
pub fn classify(frame: &CaptureFrame, centre_hz: f64, track: &Track) -> Option<Classification> {
    let data = &frame.channels[0].data;
    let data = &data[..data.len().min(2 * PAIRS)];
    let f = features(
        data,
        frame.sample_rate,
        track.last.centre_hz - centre_hz,
        track.last.bandwidth_hz(),
    )?;
    Some(dsp_classify(&f))
}

/// One lab run: the frame, the hardware centre it was taken at (the
/// spectrum's zero), the target and the user's modulation setting.
struct Job {
    frame: SharedFrame,
    centre_hz: f64,
    track: Track,
    setting: Option<Modulation>,
}

/// What a run found, with the inputs it ran on so the caller can drop a
/// result the operator has since moved away from.
pub struct LabResult {
    pub centre_hz: f64,
    pub setting: Option<Modulation>,
    pub analysis: Option<Analysis>,
    pub classification: Option<Classification>,
}

/// The lab's worker thread. At most one run is in flight: a frame that
/// arrives while one runs is skipped, not queued, so a slow run never
/// builds a backlog. The thread ends when the `Lab` is dropped.
pub struct Lab {
    jobs: Sender<Job>,
    done: Receiver<LabResult>,
    busy: bool,
    next_seq: u64,
}

impl Lab {
    pub fn spawn() -> Self {
        let (jobs, rx) = crossbeam_channel::bounded::<Job>(1);
        let (tx, done) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sdr-lab".into())
            .spawn(move || {
                for job in rx {
                    let r = LabResult {
                        centre_hz: job.centre_hz,
                        setting: job.setting,
                        classification: classify(&job.frame, job.centre_hz, &job.track),
                        analysis: analyse(&job.frame, job.centre_hz, &job.track, job.setting),
                    };
                    if tx.send(r).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the lab thread");
        Self {
            jobs,
            done,
            busy: false,
            next_seq: 0,
        }
    }

    /// Start a run on `frame` if none is in flight and `EVERY` frames have
    /// passed since the last one. Returns whether it started.
    pub fn submit(
        &mut self,
        frame: &SharedFrame,
        centre_hz: f64,
        track: Track,
        setting: Option<Modulation>,
    ) -> bool {
        if self.busy || frame.seq < self.next_seq {
            return false;
        }
        let job = Job {
            frame: frame.clone(),
            centre_hz,
            track,
            setting,
        };
        self.busy = self.jobs.try_send(job).is_ok();
        if self.busy {
            self.next_seq = frame.seq + EVERY;
        }
        self.busy
    }

    /// The finished run, if one finished. A worker that died (a panic in
    /// the DSP) is replaced, so the lab recovers on the next submit.
    pub fn poll(&mut self) -> Option<LabResult> {
        match self.done.try_recv() {
            Ok(r) => {
                self.busy = false;
                Some(r)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                bevy::log::error!("sdr: the lab thread died; restarting it");
                *self = Lab::spawn();
                None
            }
        }
    }
}
