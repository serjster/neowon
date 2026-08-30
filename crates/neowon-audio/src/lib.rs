//! Audio-input backend: the machine's sound card as a two-channel,
//! DC-blocked, ~±1 V, 48 kS/s oscilloscope.
//!
//! It exists as much for the architecture as for the feature. Everything
//! else in this workspace is shaped like the VDS1022 — a fixed 5000-sample
//! record, a hardware trigger, a switchable analogue front end — and the
//! simulator was deliberately built to mirror it, so it validates none of
//! those assumptions. A sound card breaks all three: it *streams*, so there
//! is no record and no dead time; it has no trigger hardware, so the host
//! has to find edges itself; and its input range is fixed. Porting it is
//! what proves the abstraction is about instruments rather than about one
//! instrument.
//!
//! It is also useful on its own: audio-band signals, and something anyone
//! can run without owning a scope.
//!
//! **macOS permission.** Querying an input device blocks until the OS has
//! decided about microphone access, and a plain CLI binary cannot raise the
//! prompt — it simply never returns. If `--audio` sits at "connecting…",
//! grant microphone access to the terminal (or to the app bundle) in System
//! Settings > Privacy & Security > Microphone. Linux and Windows have no
//! such gate.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use neowon_backend::{Acquisition, Backend, BackendError, Capabilities, ScopeConfig};
use neowon_core::{AcqMode, CaptureFrame, ChannelCapture, SharedFrame, Slope, Sweep, TriggerKind};

/// Samples per delivered frame. Not a hardware record — the stream is
/// continuous — just the granularity the display and the timeline see.
pub const CHUNK: usize = 4096;

/// Ring of captured samples, filled by the audio callback thread.
#[derive(Default)]
struct Shared {
    /// Interleaved by channel: `[ch0, ch1, ch0, ch1, ...]`.
    samples: Vec<f32>,
    channels: usize,
    /// Frames dropped because the consumer fell behind.
    overruns: u64,
}

/// How much audio to keep buffered before the consumer is declared behind:
/// a couple of seconds is plenty and bounds memory.
const MAX_BUFFERED: usize = 48_000 * 2 * 2;

pub struct AudioBackend {
    caps: Capabilities,
    cfg: ScopeConfig,
    shared: Arc<Mutex<Shared>>,
    /// Held to keep the input stream alive; cpal stops it on drop.
    _stream: cpal::Stream,
    rate: f64,
    channels: usize,
    /// Total samples per channel consumed so far — the capture clock. A
    /// streaming source knows exactly when each sample was taken, which is
    /// the one thing the USB scope cannot tell us.
    consumed: u64,
    seq: u64,
    /// Samples not yet emitted, de-interleaved per channel.
    pending: Vec<Vec<f32>>,
}

// cpal's Stream is not Send on some hosts; the backend is owned by the
// acquisition thread and never moved after construction, and the stream is
// only ever dropped there.
unsafe impl Send for AudioBackend {}

impl AudioBackend {
    /// Open the default input device.
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no audio input device".to_string())?;
        let name = device.name().unwrap_or_else(|_| "audio input".into());
        // On macOS this blocks until microphone access has been decided,
        // and a bare CLI binary cannot raise the prompt — say so before
        // going quiet, or it looks like a hang with no explanation.
        tracing::info!(device = %name, "opening audio input (may await microphone permission)");
        let supported = device
            .default_input_config()
            .map_err(|e| format!("no default input config: {e}"))?;
        let rate = supported.sample_rate().0 as f64;
        let channels = supported.channels().min(2) as usize;
        let config: cpal::StreamConfig = supported.clone().into();

        let shared = Arc::new(Mutex::new(Shared {
            channels: supported.channels() as usize,
            ..Default::default()
        }));
        let sink = shared.clone();
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let Ok(mut s) = sink.lock() else { return };
                    if s.samples.len() > MAX_BUFFERED {
                        // Never block the audio thread: drop the oldest and
                        // count it, the same bargain the USB path makes.
                        let drop = s.samples.len() - MAX_BUFFERED / 2;
                        s.samples.drain(..drop);
                        s.overruns += 1;
                    }
                    s.samples.extend_from_slice(data);
                },
                |e| tracing::warn!("audio input error: {e}"),
                None,
            )
            .map_err(|e| format!("cannot open input stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("cannot start input stream: {e}"))?;

        tracing::info!(device = %name, rate, channels, "audio input open");
        Ok(Self {
            caps: Capabilities {
                name: "Audio input".into(),
                serial: name,
                channels,
                sample_rates: vec![rate],
                // The input range is not adjustable; the vertical control
                // scales what we do with it, as it does for a x1 probe.
                volts_div: Vec::new(),
                probes: vec![1.0],
                acquisition: Acquisition::Stream { chunk: CHUNK },
                hardware_trigger: false,
            },
            cfg: ScopeConfig::default(),
            shared,
            _stream: stream,
            rate,
            channels,
            consumed: 0,
            seq: 0,
            pending: vec![Vec::new(); 2],
        })
    }

    /// Move everything the callback has collected into the per-channel
    /// pending buffers.
    fn drain(&mut self) {
        let Ok(mut s) = self.shared.lock() else {
            return;
        };
        let stride = s.channels.max(1);
        for frame in s.samples.chunks_exact(stride) {
            for ch in 0..self.channels {
                self.pending[ch].push(frame[ch.min(stride - 1)]);
            }
        }
        s.samples.clear();
    }

    /// Volts represented by one ADC count, for the configured vertical
    /// scale. Full scale of the input (±1.0) maps to the ±5-division
    /// encoding, so the vertical knob behaves as it does everywhere else.
    fn scale(&self, ch: usize) -> (f64, f64) {
        let c = self.cfg.channels.get(ch).copied().unwrap_or_default();
        let range = c.volts_div * 10.0 * c.probe;
        (range / 250.0, -c.offset * range)
    }

    fn quantize(&self, ch: usize, v: f32) -> i8 {
        let (lsb, _) = self.scale(ch);
        let c = self.cfg.channels.get(ch).copied().unwrap_or_default();
        let pos0 = (250.0 * c.offset).round();
        ((v as f64 / lsb.max(1e-12)).round() + pos0).clamp(-125.0, 125.0) as i8
    }

    /// Host-side edge trigger: the index in `pending[src]` where the level
    /// is crossed with the requested slope, if any.
    fn find_edge(&self, src: usize, slope: Slope, level: f64) -> Option<usize> {
        let (lsb, zero) = self.scale(src);
        let buf = self.pending.get(src)?;
        let volts = |v: f32| v as f64 * (250.0 * lsb) / 250.0 + zero;
        buf.windows(2).position(|w| {
            let (a, b) = (volts(w[0]), volts(w[1]));
            match slope {
                Slope::Rising => a < level && b >= level,
                Slope::Falling => a > level && b <= level,
            }
        })
    }

    /// Take `CHUNK` samples per channel starting at `from`, as a frame.
    fn take_frame(&mut self, from: usize) -> CaptureFrame {
        self.seq += 1;
        // The first sample's time, exactly: a streaming source counts.
        let t_capture = (self.consumed + from as u64) as f64 / self.rate;
        let channels = (0..self.channels)
            .filter(|&ch| self.cfg.channels.get(ch).is_some_and(|c| c.enabled))
            .map(|ch| {
                let raw: Vec<i8> = self.pending[ch][from..from + CHUNK]
                    .iter()
                    .map(|&v| self.quantize(ch, v))
                    .collect();
                let (lsb, zero) = self.scale(ch);
                ChannelCapture {
                    ch,
                    clipped: raw.iter().any(|&r| r.abs() >= 125),
                    freq_meter: None,
                    raw,
                    volts_per_lsb: lsb,
                    zero_volts: zero,
                }
            })
            .collect();
        let drop_to = from + CHUNK;
        for buf in self.pending.iter_mut() {
            if buf.len() >= drop_to {
                buf.drain(..drop_to);
            } else {
                buf.clear();
            }
        }
        self.consumed += drop_to as u64;
        CaptureFrame {
            seq: self.seq,
            t_capture: Some(t_capture),
            sample_rate: self.rate,
            acq: AcqMode::Sample,
            channels,
        }
    }

    /// Frames dropped because the host could not keep up.
    pub fn overruns(&self) -> u64 {
        self.shared.lock().map(|s| s.overruns).unwrap_or(0)
    }
}

impl Backend for AudioBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &ScopeConfig) -> Result<(), BackendError> {
        // Nothing to drive: the device has one rate and one input range, so
        // the config only changes how the host interprets the stream.
        self.cfg = cfg.clone();
        Ok(())
    }

    fn poll_frame(
        &mut self,
        budget: std::time::Duration,
    ) -> Result<Option<SharedFrame>, BackendError> {
        self.drain();
        let available = self.pending.first().map_or(0, |b| b.len());
        if available < CHUNK {
            // Wait roughly as long as the missing audio takes to arrive.
            let short = (CHUNK - available) as f64 / self.rate;
            std::thread::sleep(budget.min(std::time::Duration::from_secs_f64(short.min(0.05))));
            return Ok(None);
        }
        // No trigger hardware, so the host does it. Auto free-runs, which is
        // also what a sound card is usually wanted for.
        let start = match (self.cfg.trigger.sweep, self.cfg.trigger.kind) {
            (Sweep::Auto, _) | (_, TriggerKind::Pulse { .. }) => 0,
            (_, TriggerKind::Edge { slope }) => {
                let src = self.cfg.trigger.source.min(1);
                match self.find_edge(src, slope, self.cfg.trigger.level) {
                    // Centre the crossing, as the trigger position asks.
                    Some(i) => {
                        let want = (self.cfg.position * CHUNK as f64) as usize;
                        i.saturating_sub(want)
                    }
                    None => {
                        // Starve like a scope in Normal sweep, but do not
                        // let the buffer grow without bound.
                        if available > CHUNK * 4 {
                            for buf in self.pending.iter_mut() {
                                let d = buf.len() - CHUNK * 2;
                                buf.drain(..d);
                            }
                            self.consumed += (available - CHUNK * 2) as u64;
                        }
                        return Ok(None);
                    }
                }
            }
            _ => 0,
        };
        if start + CHUNK > available {
            return Ok(None);
        }
        Ok(Some(Arc::new(self.take_frame(start))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_declares_itself_as_one() {
        // The capability, not the device: constructing a real stream needs
        // an input device, which CI does not have.
        let a = Acquisition::Stream { chunk: CHUNK };
        assert!(a.is_stream());
        assert_eq!(a.frame_len(), CHUNK);
        let r = Acquisition::Record { samples: 5000 };
        assert!(!r.is_stream());
        assert_eq!(r.frame_len(), 5000);
    }
}
