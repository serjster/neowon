//! The output device, its one owner, and the demodulator's feed.
//!
//! Two producers can reach the sink in a single Update pass: the
//! demodulator (`feed`, below) and DAB playback (`dab_audio::drain`).
//! Interleaving them would put two streams in one device and let the
//! readouts disagree (`get audio` naming the demodulator while `get dab`
//! reports a playing transport).
//!
//! So the device has an **owner**: a running DAB transport takes it, the
//! demodulator has it otherwise, nobody holds it when neither is on. The
//! owner is [`SdrState::audio_owner`], *derived* from the state that
//! decides it rather than stored, so it cannot go stale; every write goes
//! through [`SdrState::push_audio`], which refuses a producer that does not
//! own the device; and [`SdrState::audio_state`] is the one place the
//! device's state is named, so `get audio` and `get dab`'s `audio.state`
//! print the same string.
//!
//! [`AudioDevice::push`] is private to this module, so
//! [`SdrState::push_audio`] is the only path to the sink that exists.

use neowon_audio::sink::{AudioOut, SinkState};

use super::{SdrState, dab_audio};

/// Who writes PCM to the output device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOwner {
    /// Neither the demodulator nor a DAB transport is running.
    Idle,
    /// The demodulator (`sdr demod …`).
    Demod,
    /// DAB playback (`sdr dab play`). It wins: a selected programme is an
    /// explicit act, and the demodulator's channel is still measured and
    /// reported, just not heard.
    Dab,
}

impl AudioOwner {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "none",
            Self::Demod => "demod",
            Self::Dab => "dab",
        }
    }
}

/// The output device, opened on first use.
///
/// Everything here is device state, not audio content: the content path is
/// [`SdrState::push_audio`], and `push` below is private to this module so
/// no second writer can exist.
#[derive(Default)]
pub struct AudioDevice {
    out: Option<AudioOut>,
}

impl AudioDevice {
    /// The sink's own state, or `None` while it has never been opened.
    #[must_use]
    pub fn state(&self) -> Option<SinkState> {
        self.out.as_ref().map(AudioOut::state)
    }

    /// The device name, empty while closed.
    #[must_use]
    pub fn device(&self) -> String {
        self.out.as_ref().map(AudioOut::device).unwrap_or_default()
    }

    /// The device's rate, 0 while closed.
    #[must_use]
    pub fn rate(&self) -> f64 {
        self.out.as_ref().map_or(0.0, AudioOut::rate)
    }

    #[must_use]
    pub fn available(&self) -> bool {
        self.out.as_ref().is_some_and(AudioOut::available)
    }

    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.out.as_ref().map_or(0, AudioOut::underruns)
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.out.as_ref().map_or(0, AudioOut::dropped)
    }

    pub fn set_volume(&self, v: f32) {
        if let Some(out) = &self.out {
            out.set_volume(v);
        }
    }

    pub fn set_mute(&self, m: bool) {
        if let Some(out) = &self.out {
            out.set_mute(m);
        }
    }

    /// Drop whatever is queued: an owner change, a stop or a mode switch
    /// must not let the old stream's tail play on.
    pub fn clear(&self) {
        if let Some(out) = &self.out {
            out.clear();
        }
    }

    /// Open the device if it is not open yet, and report the rate it runs
    /// at. The device opens on its own thread, so this is the sink's
    /// default until it reports the real one.
    pub fn open_rate(&mut self) -> f64 {
        self.out.get_or_insert_with(AudioOut::spawn).rate()
    }

    /// The only write to the sink. Private on purpose: it is reached solely
    /// through [`SdrState::push_audio`], which checks the caller owns the
    /// device, so a second producer cannot be added without going past that
    /// check.
    fn push(&mut self, pcm: &[f32]) {
        self.out.get_or_insert_with(AudioOut::spawn).push(pcm);
    }
}

impl SdrState {
    /// Who owns the output device right now. Derived, never stored: a
    /// running DAB transport owns it, the demodulator owns it otherwise.
    #[must_use]
    pub fn audio_owner(&self) -> AudioOwner {
        if self.dab.audio.is_some() {
            AudioOwner::Dab
        } else if self.demod.is_some() {
            AudioOwner::Demod
        } else {
            AudioOwner::Idle
        }
    }

    /// Push PCM to the device on behalf of `who`, and report whether it was
    /// taken. A producer that does not own the device is refused, so the
    /// two producers can never interleave into one stream.
    pub fn push_audio(&mut self, who: AudioOwner, pcm: &[f32]) -> bool {
        if pcm.is_empty() || self.audio_owner() != who {
            return false;
        }
        self.audio.push(pcm);
        true
    }

    /// The audio state the UI, `get audio` and `get dab` report — one
    /// source, so the readouts cannot disagree about the device.
    ///
    /// All of `off`, `no device`, `starting`, `muted` and `squelched` mean
    /// silence, so they are told apart by name. The sink opens off this
    /// thread, so `starting` is a state that can be caught in the
    /// act. A DAB stream this build cannot decode reports `error` whatever
    /// the device is doing: that is the more specific truth, and its typed
    /// reason is in `get dab`.
    #[must_use]
    pub fn audio_state(&self) -> &'static str {
        match self.audio_owner() {
            AudioOwner::Idle => "off",
            AudioOwner::Dab => {
                let worker = self.dab.audio.as_ref().map(|a| a.status().state);
                if worker == Some(dab_audio::AudioState::Error) {
                    return "error";
                }
                match self.audio.state() {
                    None | Some(SinkState::Starting) => "starting",
                    Some(SinkState::Unavailable) => "no device",
                    Some(SinkState::Ready) if self.mute => "muted",
                    Some(SinkState::Ready) => {
                        worker.map_or("starting", dab_audio::AudioState::label)
                    }
                }
            }
            AudioOwner::Demod => match self.audio.state() {
                None | Some(SinkState::Starting) => "starting",
                Some(SinkState::Unavailable) => "no device",
                Some(SinkState::Ready) if self.mute => "muted",
                Some(SinkState::Ready) if self.audio_squelched => "squelched",
                Some(SinkState::Ready) => "playing",
            },
        }
    }
}

/// Demodulate the tuned channel from `frame` and offer it to the device.
/// The receiver and sink persist; only the config changes frame to frame.
///
/// The channel is measured (`rms`, `channel_dbfs`, the squelch gate) even
/// while DAB owns the device: `get audio` keeps reporting the channel the
/// operator tuned, and only the sink write is withheld — the demodulator's
/// filters stay warm, so stopping DAB playback resumes mid-stream instead
/// of restarting the channel.
pub fn feed(sdr: &mut SdrState, frame: &neowon_core::CaptureFrame, mode: neowon_dsp::DemodMode) {
    // The device opens on its own thread: until it reports, the
    // receiver runs at the sink's default rate and `configure` below picks
    // the real one up on a later frame.
    let audio_rate = sdr.audio.open_rate();
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
    if !sdr.audio_squelched {
        // Silence while squelched, but keep the receiver's filters warm
        // (no reset). `push_audio` drops this when DAB owns the device.
        sdr.push_audio(AudioOwner::Demod, &audio);
    }
    sdr.audio_buf = audio;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdr::dab_audio::{Coding, DabAudio, StreamSpec};

    fn transport() -> DabAudio {
        DabAudio::start(
            0x1004,
            StreamSpec {
                coding: Coding::Mp2,
                sub_channel: 3,
                subchannel_index: 0,
                asc_override: None,
            },
            48_000.0,
        )
    }

    /// The device has exactly one owner, and it follows the state that
    /// decides it — nothing is stored, so nothing can go stale.
    #[test]
    fn one_owner_at_a_time() {
        let mut sdr = SdrState::default();
        assert_eq!(sdr.audio_owner(), AudioOwner::Idle);
        assert_eq!(sdr.audio_state(), "off");

        sdr.demod = Some(neowon_dsp::DemodMode::Nfm);
        assert_eq!(sdr.audio_owner(), AudioOwner::Demod);

        // A running transport takes the device from the demodulator.
        sdr.dab.audio = Some(transport());
        assert_eq!(sdr.audio_owner(), AudioOwner::Dab);

        // Stopping it hands the device straight back.
        sdr.dab.audio = None;
        assert_eq!(sdr.audio_owner(), AudioOwner::Demod);
        sdr.demod = None;
        assert_eq!(sdr.audio_owner(), AudioOwner::Idle);
    }

    /// A producer that does not own the device is refused. The device is
    /// never opened here: a refusal returns before the sink is touched.
    #[test]
    fn a_producer_that_does_not_own_the_device_is_refused() {
        let mut sdr = SdrState::default();
        let pcm = [0.1f32, -0.1];

        // Nobody owns it: both are refused.
        assert!(!sdr.push_audio(AudioOwner::Demod, &pcm));
        assert!(!sdr.push_audio(AudioOwner::Dab, &pcm));

        // DAB owns it, so the demodulator's push is dropped.
        sdr.demod = Some(neowon_dsp::DemodMode::Nfm);
        sdr.dab.audio = Some(transport());
        assert!(!sdr.push_audio(AudioOwner::Demod, &pcm));

        // An empty block is never a write, whoever asks.
        assert!(!sdr.push_audio(AudioOwner::Dab, &[]));
        // A closed device reports no rate: nothing above opened it.
        assert_eq!(sdr.audio.rate(), 0.0, "a refusal must not open the device");
    }

    /// While DAB owns the device, `get audio` reports the transport rather
    /// than calling itself `off`.
    #[test]
    fn the_state_readout_follows_the_owner() {
        let mut sdr = SdrState {
            demod: Some(neowon_dsp::DemodMode::Nfm),
            ..Default::default()
        };
        // No device open yet: the demodulator's sink is still starting.
        assert_eq!(sdr.audio_state(), "starting");

        sdr.dab.audio = Some(transport());
        // The worker reports `starting` until its first decoded block, and
        // the device is not open, so the device state agrees.
        assert_eq!(sdr.audio_state(), "starting");
        assert_eq!(sdr.audio_owner().label(), "dab");
    }
}
