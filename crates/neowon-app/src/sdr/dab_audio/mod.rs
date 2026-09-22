//! DAB audio playback (10.15.3): sub-channel bytes in, PCM out to the sink.
//!
//! While `sdr dab play` is in force, the selected service's MSC logical
//! frames are routed here (see [`super::dab::feed`]). DAB+ (`ASCTy` 63)
//! runs through the TS 102 563 super-frame transport and the HE-AAC v2
//! adapter; DAB classic (`ASCTy` 0) through the MPEG-1 Layer II adapter.
//! Both live in `neowon-codec`; this module owns only the app's side of
//! them: the stream spec resolved from the FIC, the sub-channel → decoder
//! routing, and the delivery to `neowon_audio`'s sink.
//!
//! **Decode runs off the frame loop.** A worker thread (the
//! `sdr/analysis.rs` pattern) owns the codec state; the app forwards each
//! logical frame over a bounded channel and drains finished PCM into the
//! sink each frame. A status shared through a mutex is what `get dab` and
//! the dock read, so the readout can only say what the worker actually
//! did — `starting` until the first block, `error` with the codec's own
//! message when a stream cannot be decoded (the DAB-G2 limitation
//! surfaces here rather than as silence).
//!
//! **Rate.** Both codings deliver 48 kHz and output devices normally run
//! at 48 kHz, so the converter below is identity in the common case. When
//! a device reports another rate — or a DAB+ stream's DAC rate is 32 kHz —
//! it is a stateful linear interpolator: 10.10's windowed-sinc resampler
//! is private to `neowon_dsp::demod`'s `Receiver`, which resamples IQ, not
//! codec PCM, and duplicating it here would fork the oracle. The sink is
//! still the one device; no second stream is opened.

use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};

use neowon_codec::aac::{AacDecoder, AudioSpecificConfig};

use neowon_dsp::dab::fec::{EepProfile, uep_profile};
use neowon_dsp::dab::{Protection, SubChannel};

use super::SdrState;

/// Bytes without an MP2 syncword before the same declaration. The decoder
/// resynchronises silently, so a stall is the only signal there is.
const MAX_MP2_BYTES_WITHOUT_SYNC: u64 = 64 * 1024;
/// Consecutive bad windows while "aligned" before the DAB+ sync search
/// restarts from the next logical frame.
const RESYNC_AFTER_BAD: u32 = 5;

/// The audio coding of a service, from its `ASCTy` (EN 300 401 clause
/// 8.1.14 / table 33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    /// `ASCTy` 63: DAB+ (HE-AAC v2), TS 102 563.
    DabPlus,
    /// `ASCTy` 0: MPEG-1 Layer II (DAB classic).
    Mp2,
}

impl Coding {
    /// The coding this build decodes, or `None` for one it does not —
    /// refused, never guessed (D27).
    #[must_use]
    pub fn from_ascty(ascty: Option<u8>) -> Option<Self> {
        match ascty {
            Some(0) => Some(Self::Mp2),
            Some(63) => Some(Self::DabPlus),
            _ => None,
        }
    }

    /// The codec backend this build links, for the readout.
    #[must_use]
    pub fn backend(self) -> &'static str {
        match self {
            Self::DabPlus => AacDecoder::BACKEND,
            // `neowon-codec` hides `oxideav-mp2` 0.0.10 behind its own
            // adapter and exposes no `BACKEND` constant for it; the name
            // here is that pinned dependency, not an invention.
            Self::Mp2 => "oxideav-mp2",
        }
    }
}

/// What the worker needs to decode one service's stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamSpec {
    pub coding: Coding,
    /// The MSC sub-channel the bytes arrive from.
    pub sub_channel: u8,
    /// DAB+: `subchannel_index` in 8 kbit/s units (TS 102 563 clause 5.1),
    /// 1..=24. The super-frame decoder validates it.
    pub subchannel_index: u8,
    /// DAB+ only: use this config instead of `for_dabplus(header)`.
    ///
    /// Real streams pass `None` and take the standard-derived config; the
    /// `rf-dab` sim scene's fixture is a 1024-line stream (no open encoder
    /// emits the mandated 960 transform) and passes its encoder's own
    /// config, which is the only way the playback path can be exercised
    /// without hardware. See `tests/fixtures/README.md`.
    pub asc_override: Option<AudioSpecificConfig>,
}

/// Where playback is, as `get dab` reports it. `off` is the absence of a
/// worker, not a state inside this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioState {
    /// The worker is running; nothing has been decoded yet.
    Starting,
    /// PCM has reached the sink.
    Playing,
    /// The stream cannot be decoded by the compiled backend; `reason` says
    /// which typed error (DAB-G2's `SbrUnsupportedFrameFamily`, a bad
    /// `subchannel_index`, no super-frame sync, …). Never a silent stream.
    Error,
}

impl AudioState {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Playing => "playing",
            Self::Error => "error",
        }
    }
}

/// The worker's own report: exactly what it decoded, never inferred.
#[derive(Debug, Clone)]
pub struct AudioStatus {
    pub state: AudioState,
    /// `"oxideav-aac"` / `"fdk-aac"` / `"oxideav-mp2"`.
    pub backend: &'static str,
    /// Decoder output rate, Hz, and channels; 0 until the first decode.
    pub rate: u32,
    pub channels: usize,
    /// Peak and RMS of the last block pushed to the sink (mono, after the
    /// downmix and any rate conversion).
    pub peak: f32,
    pub rms: f32,
    /// Blocks pushed to the sink; AUs (DAB+) or MP2 frames decoded; decode
    /// errors counted; logical frames dropped on a full worker queue.
    pub blocks: u64,
    pub decoded: u64,
    pub errors: u64,
    pub dropped: u64,
    /// The typed reason while `state == Error`.
    pub reason: String,
}

impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            state: AudioState::Starting,
            backend: "",
            rate: 0,
            channels: 0,
            peak: 0.0,
            rms: 0.0,
            blocks: 0,
            decoded: 0,
            errors: 0,
            dropped: 0,
            reason: String::new(),
        }
    }
}

/// Work handed to the thread. `Start` is not needed: the spec is fixed when
/// the worker is created, and `sdr dab play` on another service creates a
/// new worker (the app drops the old one, so its decoders reset with it).
enum Job {
    Frame {
        sub_channel: u8,
        bytes: Vec<u8>,
    },
    /// A splice restarted the MSC de-interleavers; the transport and codec
    /// state go with the seam (a click is honest, mixed history is not).
    Reset,
    /// The output device reported its real rate after the worker started.
    SinkRate(f64),
}

/// What the worker sends back: PCM for the sink, PAD regions for the DLS
/// parser (the same `PadParser` the raw sim carrier feeds, so there is one
/// DLS implementation).
enum Out {
    Pcm(Vec<f32>),
    Pad { sub_channel: u8, bytes: Vec<u8> },
}

/// A running playback worker. Dropping the handle ends the thread (its job
/// channel disconnects) and discards anything it had not handed back yet.
pub struct DabAudio {
    sid: u16,
    jobs: Sender<Job>,
    out: Receiver<Out>,
    status: Arc<Mutex<AudioStatus>>,
    sink_rate: f64,
}

impl DabAudio {
    /// Start decoding `spec` for service `sid` at the sink's current rate.
    #[must_use]
    pub fn start(sid: u16, spec: StreamSpec, sink_rate: f64) -> Self {
        let status = Arc::new(Mutex::new(AudioStatus {
            backend: spec.coding.backend(),
            ..Default::default()
        }));
        let (jobs, job_rx) = crossbeam_channel::bounded::<Job>(256);
        let (out_tx, out) = crossbeam_channel::bounded::<Out>(64);
        let shared = status.clone();
        let spawned = thread::Builder::new()
            .name("dab-audio".into())
            .spawn(move || worker(spec, sink_rate, job_rx, out_tx, shared));
        if spawned.is_err() {
            let mut s = status.lock().expect("status lock");
            s.state = AudioState::Error;
            s.reason = "cannot spawn the audio worker thread".into();
        }
        Self {
            sid,
            jobs,
            out,
            status,
            sink_rate,
        }
    }

    /// The service this worker decodes.
    #[must_use]
    pub fn sid(&self) -> u16 {
        self.sid
    }

    /// The worker's own report.
    #[must_use]
    pub fn status(&self) -> AudioStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Hand one logical frame to the worker. A full queue drops it (counted
    /// in the status): better a counted hole than a stalled feed path.
    pub fn push(&self, sub_channel: u8, bytes: &[u8]) {
        let job = Job::Frame {
            sub_channel,
            bytes: bytes.to_vec(),
        };
        if let Err(TrySendError::Full(_)) = self.jobs.try_send(job)
            && let Ok(mut s) = self.status.lock()
        {
            s.dropped += 1;
        }
    }

    /// Tell the worker the MSC de-interleavers were restarted.
    pub fn reset(&self) {
        let _ = self.jobs.try_send(Job::Reset);
    }

    /// Follow the output device's rate, if it reported one after startup.
    pub fn set_sink_rate(&mut self, rate: f64) {
        if rate > 0.0 && (rate - self.sink_rate).abs() > 0.5 {
            self.sink_rate = rate;
            let _ = self.jobs.try_send(Job::SinkRate(rate));
        }
    }

    /// Take finished PCM and PAD regions; non-blocking, called each frame.
    pub fn drain_into(&mut self, pcm: &mut Vec<f32>, pads: &mut Vec<(u8, Vec<u8>)>) {
        loop {
            match self.out.try_recv() {
                Ok(Out::Pcm(block)) => pcm.extend_from_slice(&block),
                Ok(Out::Pad { sub_channel, bytes }) => pads.push((sub_channel, bytes)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }
}

/// True while a service's audio transport is running.
#[must_use]
pub fn playing(sdr: &SdrState) -> bool {
    sdr.dab_audio.is_some()
}

/// The sub-channel the running playback is fed from, so `feed` can route
/// those logical frames to the worker instead of the raw PAD path.
#[must_use]
pub fn playing_sub_channel(sdr: &SdrState) -> Option<u8> {
    let _ = sdr.dab_audio.as_ref()?;
    let sid = sdr.dab_service?;
    let status = sdr.dab.as_ref()?.status();
    status.ensemble.services.get(&sid)?.sub_channel
}

/// Resolve the selected service into a stream this build can decode, or a
/// reason the operator can read. Every fact comes from the FIC; nothing is
/// assumed about the service.
pub fn spec_for(sdr: &SdrState) -> Result<(u16, StreamSpec), String> {
    let sid = sdr
        .dab_service
        .ok_or("dab play: no service selected (sdr dab service ...)")?;
    let rx = sdr
        .dab
        .as_ref()
        .ok_or("dab play: receiver is off (sdr dab on)")?;
    let status = rx.status();
    if !status.locked {
        return Err("dab play: no ensemble table yet".into());
    }
    let service = status
        .ensemble
        .services
        .get(&sid)
        .ok_or_else(|| format!("dab play: SId {sid} not in the locked table"))?;
    let sub_channel = service
        .sub_channel
        .ok_or_else(|| format!("dab play: service {sid:04X} has no audio sub-channel"))?;
    let sub = status
        .ensemble
        .sub_channels
        .get(&sub_channel)
        .ok_or_else(|| format!("dab play: sub-channel {sub_channel} is not in the table"))?;
    let coding = Coding::from_ascty(service.ascty).ok_or_else(|| {
        format!(
            "dab play: service {sid:04X} is ASCTy {}, which this build does not decode",
            service
                .ascty
                .map_or_else(|| "unknown".into(), |a| a.to_string())
        )
    })?;
    let (subchannel_index, asc_override) = match coding {
        Coding::DabPlus => (
            dabplus_index(sub)?,
            super::dab_scene::asc_override(status.ensemble.eid, sid)
                .map(AudioSpecificConfig::parse)
                .transpose()
                .map_err(|e| format!("dab play: the scene's ASC override is invalid: {e}"))?,
        ),
        Coding::Mp2 => (0, None),
    };
    Ok((
        sid,
        StreamSpec {
            coding,
            sub_channel,
            subchannel_index,
            asc_override,
        },
    ))
}

/// The DAB+ `subchannel_index` the FIC's sub-channel resolves to: TS 102 563
/// clause 5.1 makes a super frame `subchannel_index × 110` bytes carried in
/// five 24 ms logical frames, so one logical frame is `24 × index` bytes of
/// information — the same `info_bits` the MSC profile resolves to. The FIC
/// signals size and protection, not the index, so it is derived here exactly
/// as the MSC decoder resolves it, refusing what the standard does not define.
fn dabplus_index(sub: &SubChannel) -> Result<u8, String> {
    let info_bits = match sub.protection {
        Protection::Eep { option, level } => {
            let size = sub
                .size_cu
                .ok_or_else(|| "dab play: sub-channel size unknown".to_string())?;
            EepProfile::for_size(size, level, option).map(|p| p.info_bits())
        }
        Protection::Uep { table_index } => uep_profile(table_index).map(|p| p.info_bits()),
    }
    .ok_or_else(|| {
        format!(
            "dab play: sub-channel {} has an unresolved protection plan ({})",
            sub.id,
            sub.protection.label()
        )
    })?;
    let index = info_bits / 8 / 24;
    if !(1..=24).contains(&index) {
        return Err(format!(
            "dab play: sub-channel {} is {index} × 8 kbit/s useful, outside DAB+'s 1..=24",
            sub.id
        ));
    }
    Ok(index as u8)
}

/// Start playback of the selected service on the shared sink. Repeating
/// `play` for the already-playing service is a no-op; any other call starts
/// a fresh worker, which resets the decoders with it.
pub fn play(sdr: &mut SdrState) -> Result<(), String> {
    let (sid, spec) = spec_for(sdr)?;
    if let Some(audio) = &sdr.dab_audio
        && audio.sid() == sid
    {
        return Ok(());
    }
    let (volume, mute) = (sdr.volume, sdr.mute);
    let out = sdr
        .audio
        .get_or_insert_with(neowon_audio::sink::AudioOut::spawn);
    out.set_volume(volume);
    out.set_mute(mute);
    let sink_rate = out.rate();
    out.clear();
    sdr.dab_audio = Some(DabAudio::start(sid, spec, sink_rate));
    Ok(())
}

/// Stop playback: the worker is dropped (its decoders with it) and the
/// sink's queue is cleared, so no tail of the stopped service plays on.
pub fn stop(sdr: &mut SdrState) {
    sdr.dab_audio = None;
    if let Some(out) = &sdr.audio {
        out.clear();
    }
}

/// Per-frame tick: follow the device rate, move decoded PCM to the sink and
/// decoded PAD regions to the parsers. Called from `sdr::update`, so it
/// also runs on frames where no new IQ arrived.
pub fn drain(sdr: &mut SdrState) {
    let Some(audio) = sdr.dab_audio.as_mut() else {
        return;
    };
    if let Some(out) = &sdr.audio {
        audio.set_sink_rate(out.rate());
    }
    let mut pcm = Vec::new();
    let mut pads = Vec::new();
    audio.drain_into(&mut pcm, &mut pads);
    for (sub_channel, bytes) in pads {
        sdr.dab_pad
            .entry(sub_channel)
            .or_default()
            .push_pad_region(&bytes);
    }
    if !pcm.is_empty()
        && let Some(out) = &sdr.audio
    {
        out.push(&pcm);
    }
}

/// The worker loop: one stream at a time, publishing its status after every
/// job so a reader can never see a half-updated report.
fn worker(
    spec: StreamSpec,
    sink_rate: f64,
    jobs: Receiver<Job>,
    out: Sender<Out>,
    shared: Arc<Mutex<AudioStatus>>,
) {
    let mut status = AudioStatus {
        backend: spec.coding.backend(),
        ..Default::default()
    };
    let mut converter = RateConverter::new(48_000.0, sink_rate);
    let mut stream = match Stream::new(spec) {
        Ok(stream) => Some(stream),
        Err(reason) => {
            fail(&mut status, reason);
            None
        }
    };
    for job in jobs {
        match job {
            Job::Frame { sub_channel, bytes } => {
                if status.state == AudioState::Error || sub_channel != spec.sub_channel {
                    continue;
                }
                if let Some(stream) = stream.as_mut() {
                    stream.push(&bytes, &mut converter, &out, &mut status);
                }
            }
            Job::Reset => {
                status = AudioStatus {
                    backend: spec.coding.backend(),
                    ..Default::default()
                };
                converter.reset();
                match Stream::new(spec) {
                    Ok(s) => stream = Some(s),
                    Err(reason) => {
                        fail(&mut status, reason);
                        stream = None;
                    }
                }
            }
            Job::SinkRate(rate) => converter.set_output(rate),
        }
        if let Ok(mut s) = shared.lock() {
            *s = status.clone();
        }
    }
}

fn fail(status: &mut AudioStatus, reason: String) {
    status.state = AudioState::Error;
    status.reason = reason;
}

mod decode;
mod transport;

#[cfg(test)]
pub(crate) use transport::DabPlusSync;

use decode::Stream;
use transport::RateConverter;

#[cfg(test)]
mod tests;
