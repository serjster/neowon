//! DAB audio playback: sub-channel bytes in, PCM out to the sink.
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
//! message when a stream cannot be decoded (an unsupported stream surfaces
//! here rather than as silence).
//!
//! **Playback starts with a cushion** (`decode::PRIME_SECONDS`): `playing`
//! means the device has audio queued behind its next sample, which is what
//! makes "a playing transport does not starve" an invariant and not a race.
//!
//! **Rate.** Both codings deliver 48 kHz and output devices normally run
//! at 48 kHz, so the converter below is identity in the common case. When
//! a device reports another rate — or a DAB+ stream's DAC rate is 32 kHz —
//! it is a stateful linear interpolator: the windowed-sinc resampler
//! is private to `neowon_dsp::demod`'s `Receiver`, which resamples IQ, not
//! codec PCM, and duplicating it here would fork the oracle. The sink is
//! still the one device; no second stream is opened.

use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};

use super::SdrState;

/// Bytes without an MP2 syncword before the same declaration. The decoder
/// resynchronises silently, so a stall is the only signal there is.
const MAX_MP2_BYTES_WITHOUT_SYNC: u64 = 64 * 1024;
/// Consecutive bad windows while "aligned" before the DAB+ sync search
/// restarts from the next logical frame.
const RESYNC_AFTER_BAD: u32 = 5;

/// Where playback is, as `get dab` reports it. `off` is the absence of a
/// worker, not a state inside this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioState {
    /// The worker is running; nothing has reached the device yet —
    /// either nothing has decoded, or the start-up cushion is still
    /// filling (`decode::PRIME_SECONDS`).
    Starting,
    /// PCM has reached the sink.
    Playing,
    /// The stream cannot be decoded by the compiled backend; `reason` says
    /// which typed error (`SbrUnsupportedFrameFamily`, a bad
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

    #[must_use]
    pub fn sid(&self) -> u16 {
        self.sid
    }

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
    sdr.dab.audio.is_some()
}

/// The sub-channel the running playback is fed from, so `feed` can route
/// those logical frames to the worker instead of the raw PAD path.
#[must_use]
pub fn playing_sub_channel(sdr: &SdrState) -> Option<u8> {
    let _ = sdr.dab.audio.as_ref()?;
    let sid = sdr.dab.service?;
    let status = sdr.dab.rx.as_ref()?.status();
    status.ensemble.services.get(&sid)?.sub_channel
}

/// Start playback of the selected service on the shared sink. Repeating
/// `play` for the already-playing service is a no-op; any other call starts
/// a fresh worker, which resets the decoders with it.
pub fn play(sdr: &mut SdrState) -> Result<(), String> {
    let (sid, spec) = spec_for(sdr)?;
    if let Some(audio) = &sdr.dab.audio
        && audio.sid() == sid
    {
        return Ok(());
    }
    // Starting a transport takes the device from the demodulator
    // (`SdrState::audio_owner`), so the queue is flushed here: whatever the
    // previous owner left must not play under the new stream.
    let sink_rate = sdr.audio.open_rate();
    sdr.audio.set_volume(sdr.volume);
    sdr.audio.set_mute(sdr.mute);
    sdr.audio.clear();
    sdr.dab.audio = Some(DabAudio::start(sid, spec, sink_rate));
    Ok(())
}

/// Stop playback: the worker is dropped (its decoders with it) and the
/// sink's queue is cleared, so no tail of the stopped service plays on.
/// The demodulator gets the device back on the next frame.
pub fn stop(sdr: &mut SdrState) {
    if sdr.dab.audio.take().is_some() {
        sdr.audio.clear();
    }
}

/// Per-frame tick: follow the device rate, move decoded PCM to the sink and
/// decoded PAD regions to the parsers. Called from `sdr::update`, so it
/// also runs on frames where no new IQ arrived.
pub fn drain(sdr: &mut SdrState) {
    let rate = sdr.audio.rate();
    let Some(audio) = sdr.dab.audio.as_mut() else {
        return;
    };
    audio.set_sink_rate(rate);
    let mut pcm = Vec::new();
    let mut pads = Vec::new();
    audio.drain_into(&mut pcm, &mut pads);
    for (sub_channel, bytes) in pads {
        sdr.dab
            .pad
            .entry(sub_channel)
            .or_default()
            .push_pad_region(&bytes);
    }
    // The device has one writer: while this transport runs it is the owner,
    // and `push_audio` is what makes that true rather than a convention.
    sdr.push_audio(super::AudioOwner::Dab, &pcm);
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
    let mut feed = SinkFeed::new(48_000.0, sink_rate);
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
                    stream.push(&bytes, &mut feed, &out, &mut status);
                }
            }
            Job::Reset => {
                status = AudioStatus {
                    backend: spec.coding.backend(),
                    ..Default::default()
                };
                feed.reset();
                match Stream::new(spec) {
                    Ok(s) => stream = Some(s),
                    Err(reason) => {
                        fail(&mut status, reason);
                        stream = None;
                    }
                }
            }
            Job::SinkRate(rate) => feed.set_output(rate),
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
mod spec;
mod transport;

#[cfg(test)]
use spec::dabplus_index;
pub use spec::{Coding, StreamSpec, service_spec, spec_for};

#[cfg(test)]
pub(crate) use transport::DabPlusSync;

use decode::{SinkFeed, Stream};

#[cfg(test)]
mod tests;
