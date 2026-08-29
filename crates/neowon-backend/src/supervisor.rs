//! Owns a `Backend` on a dedicated thread. Reconnects when the backend
//! reports a fatal error and replays the last requested configuration, so
//! unplug/replug is invisible to the UI beyond a status change.
//!
//! Backend-agnostic acquisition behavior also lives here: host-side
//! averaging, and the single-sweep auto-stop.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use neowon_core::{AcqMode, CaptureFrame, SharedFrame, Sweep};
use tracing::{info, warn};

use crate::{Backend, BackendError, Capabilities, ScopeConfig};

#[derive(Debug, Clone)]
pub enum Command {
    Apply(ScopeConfig),
    ForceTrigger,
    AutoSet,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected(Capabilities),
    Disconnected(String),
    Frame(SharedFrame),
    /// The effective config changed on the backend side (single-sweep stop,
    /// autoset); the UI should adopt it.
    ConfigUpdated(ScopeConfig),
    Error(String),
}

pub struct Supervisor {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Supervisor {
    pub fn apply(&self, cfg: ScopeConfig) {
        let _ = self.commands.send(Command::Apply(cfg));
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Spawn the acquisition thread. `factory` is invoked to (re)connect; it
/// returns a ready backend or a human-readable failure.
pub fn spawn<F>(mut factory: F) -> Supervisor
where
    F: FnMut() -> Result<Box<dyn Backend>, String> + Send + 'static,
{
    let (cmd_tx, cmd_rx) = unbounded::<Command>();
    // Bounded so a stalled UI applies backpressure instead of growing a queue;
    // frames are Arc-shared and cheap to drop.
    let (event_tx, event_rx) = bounded::<Event>(64);

    let handle = std::thread::Builder::new()
        .name("neowon-acq".into())
        .spawn(move || run(&mut factory, cmd_rx, event_tx))
        .expect("spawn acquisition thread");

    Supervisor { commands: cmd_tx, events: event_rx, handle: Some(handle) }
}

/// Host-side running average over the last N records, per channel.
#[derive(Default)]
struct Averager {
    n: u8,
    count: u32,
    acc: Vec<Vec<f32>>,
}

impl Averager {
    fn reset(&mut self, n: u8) {
        self.n = n;
        self.count = 0;
        self.acc.clear();
    }

    /// Fold `frame` in; returns the averaged replacement frame.
    fn fold(&mut self, frame: &CaptureFrame) -> CaptureFrame {
        if self.acc.len() != frame.channels.len()
            || frame
                .channels
                .iter()
                .zip(&self.acc)
                .any(|(c, a)| c.raw.len() != a.len())
        {
            self.acc = frame
                .channels
                .iter()
                .map(|c| c.raw.iter().map(|&r| r as f32).collect())
                .collect();
            self.count = 1;
        } else {
            self.count += 1;
            let k = self.count.min(self.n as u32) as f32;
            for (cap, acc) in frame.channels.iter().zip(&mut self.acc) {
                for (&r, a) in cap.raw.iter().zip(acc.iter_mut()) {
                    *a += (r as f32 - *a) / k;
                }
            }
        }
        let mut out = frame.clone();
        for (cap, acc) in out.channels.iter_mut().zip(&self.acc) {
            cap.raw = acc.iter().map(|&a| a.round().clamp(-128.0, 127.0) as i8).collect();
        }
        out.acq = AcqMode::Average(self.n);
        out
    }
}

fn run(
    factory: &mut dyn FnMut() -> Result<Box<dyn Backend>, String>,
    commands: Receiver<Command>,
    events: Sender<Event>,
) {
    let mut wanted: Option<ScopeConfig> = None;
    let mut averager = Averager::default();
    'outer: loop {
        // (Re)connect, absorbing commands while we wait.
        let mut backend = loop {
            match factory() {
                Ok(b) => break b,
                Err(e) => {
                    let _ = events.try_send(Event::Disconnected(e));
                    match commands.recv_timeout(Duration::from_secs(1)) {
                        Ok(Command::Apply(cfg)) => wanted = Some(cfg),
                        Ok(Command::Shutdown) => break 'outer,
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }
            }
        };
        let caps = backend.capabilities().clone();
        info!(name = %caps.name, serial = %caps.serial, "backend connected");
        let _ = events.send(Event::Connected(caps));

        if let Some(cfg) = &wanted
            && let Err(e) = backend.apply(cfg)
        {
            warn!("config replay failed: {e}");
            let _ = events.try_send(Event::Disconnected(e.to_string()));
            continue 'outer;
        }

        loop {
            // Drain pending commands; only the newest config matters.
            let mut newest: Option<ScopeConfig> = None;
            let mut do_force = false;
            let mut do_autoset = false;
            loop {
                match commands.try_recv() {
                    Ok(Command::Apply(cfg)) => newest = Some(cfg),
                    Ok(Command::ForceTrigger) => do_force = true,
                    Ok(Command::AutoSet) => do_autoset = true,
                    Ok(Command::Shutdown) => break 'outer,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }
            if let Some(cfg) = newest {
                if let AcqMode::Average(n) = cfg.acq {
                    if averager.n != n {
                        averager.reset(n);
                    }
                } else {
                    averager.reset(0);
                }
                wanted = Some(cfg.clone());
                match backend.apply(&cfg) {
                    Ok(()) => {}
                    Err(BackendError::Fatal(e)) => {
                        let _ = events.try_send(Event::Disconnected(e));
                        continue 'outer;
                    }
                    Err(BackendError::Transient(e)) => {
                        let _ = events.try_send(Event::Error(e));
                    }
                }
            }
            if do_force && let Err(e) = backend.force_trigger() {
                let _ = events.try_send(Event::Error(e.to_string()));
            }
            if do_autoset {
                match backend.autoset() {
                    Ok(Some(cfg)) => {
                        if let AcqMode::Average(n) = cfg.acq {
                            averager.reset(n);
                        } else {
                            averager.reset(0);
                        }
                        wanted = Some(cfg.clone());
                        let _ = events.send(Event::ConfigUpdated(cfg));
                    }
                    Ok(None) => {
                        let _ = events.try_send(Event::Error("autoset: no signal".into()));
                    }
                    Err(BackendError::Fatal(e)) => {
                        let _ = events.try_send(Event::Disconnected(e));
                        continue 'outer;
                    }
                    Err(BackendError::Transient(e)) => {
                        let _ = events.try_send(Event::Error(e));
                    }
                }
            }

            let running = wanted.as_ref().is_none_or(|c| c.running);
            if running {
                match backend.poll_frame(Duration::from_millis(100)) {
                    Ok(Some(frame)) => {
                        let frame = match wanted.as_ref().map(|c| c.acq) {
                            Some(AcqMode::Average(_)) => Arc::new(averager.fold(&frame)),
                            _ => frame,
                        };
                        // Prefer dropping frames over blocking acquisition.
                        let _ = events.try_send(Event::Frame(frame));

                        // Single sweep: one record, then stop.
                        if let Some(cfg) = &mut wanted
                            && cfg.trigger.sweep == Sweep::Single
                            && cfg.running
                        {
                            cfg.running = false;
                            let cfg = cfg.clone();
                            if let Err(e) = backend.apply(&cfg) {
                                let _ = events.try_send(Event::Error(e.to_string()));
                            }
                            let _ = events.send(Event::ConfigUpdated(cfg));
                        }
                    }
                    Ok(None) => {}
                    Err(BackendError::Fatal(e)) => {
                        warn!("backend lost: {e}");
                        let _ = events.try_send(Event::Disconnected(e));
                        continue 'outer;
                    }
                    Err(BackendError::Transient(e)) => {
                        let _ = events.try_send(Event::Error(e));
                    }
                }
            } else {
                if let Err(BackendError::Fatal(e)) = backend.idle() {
                    let _ = events.try_send(Event::Disconnected(e));
                    continue 'outer;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    info!("acquisition thread exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_core::ChannelCapture;

    fn frame(vals: &[i8]) -> CaptureFrame {
        CaptureFrame {
            seq: 0,
            sample_rate: 1.0,
            acq: AcqMode::Sample,
            channels: vec![ChannelCapture {
                ch: 0,
                raw: vals.to_vec(),
                volts_per_lsb: 1.0,
                zero_volts: 0.0,
                clipped: false,
                freq_meter: None,
            }],
        }
    }

    #[test]
    fn averager_converges() {
        let mut avg = Averager::default();
        avg.reset(4);
        let a = avg.fold(&frame(&[100, 0]));
        assert_eq!(a.channels[0].raw, vec![100, 0]);
        // Fold in an opposite frame repeatedly: converges toward the mean of
        // the last window, never oscillates outside bounds.
        let b = avg.fold(&frame(&[0, 100]));
        assert_eq!(b.channels[0].raw, vec![50, 50]);
        let c = avg.fold(&frame(&[0, 100]));
        assert!(c.channels[0].raw[0] < 50 && c.channels[0].raw[1] > 50);
        assert_eq!(c.acq, AcqMode::Average(4));
    }
}
