//! Owns a `Backend` on a dedicated thread. Reconnects when the backend
//! reports a fatal error and replays the last requested configuration, so
//! unplug/replug is invisible to the UI beyond a status change.

use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use neowon_core::SharedFrame;
use tracing::{info, warn};

use crate::{Backend, BackendError, Capabilities, ScopeConfig};

#[derive(Debug, Clone)]
pub enum Command {
    Apply(ScopeConfig),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected(Capabilities),
    Disconnected(String),
    Frame(SharedFrame),
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

fn run(
    factory: &mut dyn FnMut() -> Result<Box<dyn Backend>, String>,
    commands: Receiver<Command>,
    events: Sender<Event>,
) {
    let mut wanted: Option<ScopeConfig> = None;
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
            loop {
                match commands.try_recv() {
                    Ok(Command::Apply(cfg)) => newest = Some(cfg),
                    Ok(Command::Shutdown) => break 'outer,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }
            if let Some(cfg) = newest {
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

            let running = wanted.as_ref().is_none_or(|c| c.running);
            if running {
                match backend.poll_frame(Duration::from_millis(100)) {
                    Ok(Some(frame)) => {
                        // Prefer dropping frames over blocking acquisition.
                        let _ = events.try_send(Event::Frame(frame));
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
