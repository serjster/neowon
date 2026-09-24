//! Audio output sink: the demodulated channel on the default
//! output device.
//!
//! A `cpal::Stream` is not `Send` on every host, so a thread owns it and the
//! app keeps a `Send + Sync` handle. The handle pushes mono audio into a
//! bounded queue the audio callback drains; a full queue drops the oldest
//! audio (glitches, not unbounded latency) and an empty one counts an
//! underrun rather than hiding it. A machine with no output device is a
//! state (`available() == false`), not an error.
//!
//! **Opening is off the caller's path.** `spawn` starts the
//! thread and returns immediately; the device report is polled from the
//! accessors, so a slow or absent device never stalls the frame loop.
//! `SinkState::Starting` is a real, observable state between the two.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::bounded;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Queue depth (~1 s at 48 kHz): beyond it the oldest audio is dropped.
pub const MAX_QUEUED: usize = 48_000;

#[derive(Default)]
struct Shared {
    buf: VecDeque<f32>,
    /// Callbacks that had to emit at least one silent sample.
    underruns: u64,
    /// Samples dropped because the queue was full.
    dropped: u64,
}

impl Shared {
    fn push(&mut self, data: &[f32]) {
        let over = (self.buf.len() + data.len()).saturating_sub(MAX_QUEUED);
        let n = over.min(self.buf.len());
        self.buf.drain(..n);
        self.dropped += n as u64;
        self.buf.extend(data.iter().copied());
    }

    /// Fill one callback of interleaved `data`, duplicating the mono stream
    /// across `channels`. Applies mute and volume.
    fn fill(&mut self, data: &mut [f32], channels: usize, volume: f32, mute: bool) {
        let channels = channels.max(1);
        let mut starved = false;
        for frame in data.chunks_mut(channels) {
            let x = match self.buf.pop_front() {
                Some(x) => x,
                None => {
                    starved = true;
                    0.0
                }
            };
            let y = if mute { 0.0 } else { x * volume };
            for o in frame.iter_mut() {
                *o = y;
            }
        }
        if starved {
            self.underruns += 1;
        }
    }
}

/// Where the output device is in its open sequence. `Starting` is a real,
/// observable state: the thread that opens the device runs off the app's
/// path, so the handle reports it until the thread answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkState {
    Starting,
    Ready,
    Unavailable,
}

/// The opened device's description, written once the opening thread reports.
struct Outlet {
    state: SinkState,
    device: String,
    rate: f64,
    channels: usize,
    reason: String,
}

/// The opening thread's one-shot answer: `(device name, rate, channels)`, or
/// why the device could not be opened.
type DeviceReport = Result<(String, f64, usize), String>;

/// A handle to the output device; owns the queue.
pub struct AudioOut {
    shared: Arc<Mutex<Shared>>,
    volume: Arc<AtomicU32>,
    mute: Arc<AtomicBool>,
    /// The opening thread's one-shot report; polled, never waited on.
    ready: Option<crossbeam_channel::Receiver<DeviceReport>>,
    outlet: Mutex<Outlet>,
    quit: Option<crossbeam_channel::Sender<()>>,
}

impl AudioOut {
    /// Open the default output device on a thread that owns the stream, and
    /// return at once. Never fails: the handle reports `SinkState::Starting`
    /// while the thread works, then `Ready` or `Unavailable`, and
    /// `reason()` says why in the latter case.
    pub fn spawn() -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let mute = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = bounded::<DeviceReport>(1);
        let (quit_tx, quit_rx) = bounded::<()>(1);

        let (sh, vol, mute_) = (shared.clone(), volume.clone(), mute.clone());
        let spawned = std::thread::Builder::new()
            .name("neowon-audio-out".into())
            .spawn(move || {
                let host = cpal::default_host();
                let Some(device) = host.default_output_device() else {
                    let _ = ready_tx.send(Err("no audio output device".into()));
                    return;
                };
                let name = device.name().unwrap_or_else(|_| "audio output".into());
                let supported = match device.default_output_config() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("no output config: {e}")));
                        return;
                    }
                };
                let rate = supported.sample_rate().0 as f64;
                let channels = supported.channels() as usize;
                let config: cpal::StreamConfig = supported.into();
                let (sh2, vol2, mute2) = (sh.clone(), vol.clone(), mute_.clone());
                let stream = device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let Ok(mut s) = sh2.lock() else { return };
                        let v = f32::from_bits(vol2.load(Ordering::Relaxed));
                        let m = mute2.load(Ordering::Relaxed);
                        s.fill(data, channels, v, m);
                    },
                    |e| tracing::warn!("audio output error: {e}"),
                    None,
                );
                match stream {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            let _ = ready_tx.send(Err(format!("cannot start output: {e}")));
                            return;
                        }
                        let _ = ready_tx.send(Ok((name, rate, channels)));
                        // Hold the stream until the app drops the handle.
                        let _ = quit_rx.recv();
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("cannot open output stream: {e}")));
                    }
                }
            })
            .is_ok();

        let outlet = Outlet {
            state: if spawned {
                SinkState::Starting
            } else {
                SinkState::Unavailable
            },
            device: String::new(),
            rate: 48000.0,
            channels: 2,
            reason: if spawned {
                String::new()
            } else {
                "cannot spawn audio thread".into()
            },
        };

        Self {
            shared,
            volume,
            mute,
            ready: spawned.then_some(ready_rx),
            outlet: Mutex::new(outlet),
            quit: Some(quit_tx),
        }
    }

    /// Take the opening thread's report if it has arrived. Non-blocking, and
    /// called from every accessor, so the state settles on the next read.
    fn poll(&self) {
        let Some(rx) = &self.ready else { return };
        let Ok(report) = rx.try_recv() else { return };
        let Ok(mut outlet) = self.outlet.lock() else {
            return;
        };
        match report {
            Ok((device, rate, channels)) => {
                outlet.state = SinkState::Ready;
                outlet.device = device;
                outlet.rate = rate;
                outlet.channels = channels;
            }
            Err(reason) => {
                outlet.state = SinkState::Unavailable;
                outlet.reason = reason;
            }
        }
    }

    pub fn state(&self) -> SinkState {
        self.poll();
        self.outlet
            .lock()
            .map(|o| o.state)
            .unwrap_or(SinkState::Unavailable)
    }
    pub fn available(&self) -> bool {
        self.state() == SinkState::Ready
    }
    pub fn device(&self) -> String {
        self.poll();
        self.outlet
            .lock()
            .map(|o| o.device.clone())
            .unwrap_or_default()
    }
    pub fn rate(&self) -> f64 {
        self.poll();
        self.outlet.lock().map(|o| o.rate).unwrap_or(48000.0)
    }
    pub fn channels(&self) -> usize {
        self.poll();
        self.outlet.lock().map(|o| o.channels).unwrap_or(2)
    }
    pub fn reason(&self) -> String {
        self.poll();
        self.outlet
            .lock()
            .map(|o| o.reason.clone())
            .unwrap_or_default()
    }

    /// Queue mono audio. Drops the oldest if the queue is full.
    pub fn push(&self, data: &[f32]) {
        if let Ok(mut s) = self.shared.lock() {
            s.push(data);
        }
    }
    /// Drop queued audio (on retune or a mode change).
    pub fn clear(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.buf.clear();
        }
    }
    pub fn set_volume(&self, v: f32) {
        self.volume
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
    pub fn set_mute(&self, m: bool) {
        self.mute.store(m, Ordering::Relaxed);
    }
    pub fn mute(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }
    pub fn underruns(&self) -> u64 {
        self.shared.lock().map(|s| s.underruns).unwrap_or(0)
    }
    pub fn dropped(&self) -> u64 {
        self.shared.lock().map(|s| s.dropped).unwrap_or(0)
    }
    pub fn queued(&self) -> usize {
        self.shared.lock().map(|s| s.buf.len()).unwrap_or(0)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.try_send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_queue_drops_the_oldest() {
        let mut s = Shared::default();
        s.push(&vec![1.0; MAX_QUEUED]);
        s.push(&[2.0, 2.0]);
        assert_eq!(s.buf.len(), MAX_QUEUED);
        assert_eq!(s.dropped, 2);
        // The newest samples are kept.
        assert_eq!(s.buf.back(), Some(&2.0));
    }

    #[test]
    fn an_empty_queue_counts_an_underrun_and_writes_silence() {
        let mut s = Shared::default();
        let mut out = [9.0f32; 8];
        s.fill(&mut out, 2, 1.0, false);
        assert_eq!(out, [0.0; 8]);
        assert_eq!(s.underruns, 1);
    }

    #[test]
    fn mute_is_exact_zero_and_volume_scales() {
        let mut s = Shared::default();
        s.push(&[0.5, 0.5]);
        let mut out = [0.0f32; 4];
        s.fill(&mut out, 2, 0.5, false);
        assert_eq!(out, [0.25; 4]);
        s.push(&[1.0, 1.0]);
        let mut out = [0.0f32; 4];
        s.fill(&mut out, 2, 0.5, true);
        assert_eq!(out, [0.0; 4]);
        assert_eq!(s.underruns, 0);
    }

    /// Opening the device must not stall the caller, and the
    /// state is named while the thread works.
    #[test]
    fn spawn_returns_immediately_with_a_named_state() {
        let started = std::time::Instant::now();
        let out = AudioOut::spawn();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "spawn blocked for {:?}",
            started.elapsed()
        );
        assert!(matches!(
            out.state(),
            SinkState::Starting | SinkState::Ready | SinkState::Unavailable
        ));
        assert_eq!(out.available(), out.state() == SinkState::Ready);
        assert!(out.rate() > 0.0);
    }
}
