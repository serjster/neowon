//! Owns a `Backend` on a dedicated thread. Reconnects when the backend
//! reports a fatal error and replays the last requested configuration, so
//! unplug/replug is invisible to the UI beyond a status change.
//!
//! Backend-agnostic acquisition behavior also lives here: host-side
//! averaging, and the single-sweep auto-stop.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded, unbounded};
use neowon_core::{AcqMode, CaptureFrame, SharedFrame, Sweep};
use tracing::{info, warn};

use crate::{Backend, BackendError, Capabilities, InstrumentConfig, MultiMode};

#[derive(Debug, Clone)]
pub enum Command {
    Apply(InstrumentConfig),
    ForceTrigger,
    AutoSet,
    Multi(MultiMode),
    PassFail(bool),
    /// Select a named stimulus on generating backends (sim, AWG).
    Stimulus(String),
    /// Reseed the generator on generating backends (sim).
    Seed(u64),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected(Capabilities),
    Disconnected(String),
    Frame(SharedFrame),
    /// The effective config changed on the backend side (single-sweep stop,
    /// autoset); the UI should adopt it.
    ConfigUpdated(InstrumentConfig),
    Error(String),
}

pub struct Supervisor {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    /// Frames the acquisition thread captured but could not hand over
    /// because the consumer was behind. Dropping is the right call — never
    /// stall acquisition — but it is coverage lost, so it is counted rather
    /// than silent.
    pub dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Supervisor {
    pub fn apply(&self, cfg: impl Into<InstrumentConfig>) {
        let _ = self.commands.send(Command::Apply(cfg.into()));
    }

    /// Stop the acquisition thread and wait for it, so the backend has
    /// released its device before this returns. Idempotent (drop calls it).
    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
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
    // Deep enough that a brief UI stall (shader compile, window resize)
    // does not cost coverage at the ~131 frames/s the device can serve.
    let (event_tx, event_rx) = bounded::<Event>(256);
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dropped_tx = dropped.clone();

    let handle = std::thread::Builder::new()
        .name("neowon-acq".into())
        .spawn(move || run(&mut factory, cmd_rx, event_tx, dropped_tx))
        .expect("spawn acquisition thread");

    Supervisor {
        commands: cmd_tx,
        events: event_rx,
        dropped,
        handle: Some(handle),
    }
}

/// Host-side running average over the last N records, per channel.
#[derive(Default)]
struct Averager {
    n: u8,
    count: u32,
    acc: Vec<Vec<f32>>,
    /// The connected instrument's sample grid, from its `Capabilities`
    /// ([`Capabilities::count_range`]): `Some` for an instrument whose
    /// frames carry ADC counts, `None` for one carrying real-valued
    /// full-scale samples. The supervisor is instrument-agnostic, so it
    /// must not assume the scope's i8 range here.
    counts: Option<(f32, f32)>,
}

impl Averager {
    fn reset(&mut self, n: u8) {
        self.n = n;
        self.count = 0;
        self.acc.clear();
    }

    /// Fold `frame` in; returns the averaged replacement frame, or `None`
    /// where the record has no averaged form (a complex stream frame)
    /// and the original must pass through untouched.
    fn fold(&mut self, frame: &CaptureFrame) -> Option<CaptureFrame> {
        if self.acc.len() != frame.channels.len()
            || frame
                .channels
                .iter()
                .zip(&self.acc)
                .any(|(c, a)| c.data.len() != a.len())
        {
            self.acc = frame.channels.iter().map(|c| c.data.clone()).collect();
            self.count = 1;
        } else {
            self.count += 1;
            let k = self.count.min(self.n as u32) as f32;
            for (cap, acc) in frame.channels.iter().zip(&mut self.acc) {
                for (&r, a) in cap.data.iter().zip(acc.iter_mut()) {
                    *a += (r - *a) / k;
                }
            }
        }
        let mut channels = frame.channels.clone();
        for (cap, acc) in channels.iter_mut().zip(&self.acc) {
            cap.data = match self.counts {
                // Counts stay on the instrument's grid.
                Some((lo, hi)) => acc.iter().map(|&a| a.round().clamp(lo, hi)).collect(),
                // Real-valued samples are already in their own units:
                // rounding them to a count grid would erase the signal.
                None => acc.clone(),
            };
        }
        frame.with_acq(AcqMode::Average(self.n), channels).ok()
    }
}

/// The host-side averaging depth `cfg` asks for; only scopes average.
fn averaging(cfg: &InstrumentConfig) -> Option<u8> {
    match cfg.scope()?.acq {
        AcqMode::Average(n) => Some(n),
        _ => None,
    }
}

/// Units (samples, or I/Q pairs) a frame holds.
fn units_of(frame: &CaptureFrame) -> u64 {
    frame
        .channels
        .first()
        .map_or(0, |c| c.unit_count(frame.layout())) as u64
}

/// Add `carry` units of loss to what `frame` already reports.
///
/// Dropping a frame rather than stalling acquisition is the right call, but
/// the samples in it are gone: unless a later frame says so, the consumer
/// sees a hole it cannot know about — and a hole it cannot know about is the
/// whole reason splice detection could not be exact. So the loss rides
/// forward on the next delivered frame, the same mechanism a backend uses
/// for a USB overflow, applied one stage later. `carry == 0` is the normal
/// case and touches nothing.
fn carry_loss(frame: SharedFrame, carry: u64) -> SharedFrame {
    if carry == 0 {
        return frame;
    }
    let owed = frame.dropped_before() + carry;
    // Unwrap when we hold the only reference (the usual case straight from a
    // backend), so recovering from a drop does not copy the samples.
    let f = Arc::try_unwrap(frame).unwrap_or_else(|a| (*a).clone());
    Arc::new(f.with_dropped_before(owed))
}

/// Auto-set is a scope function: it answers with a `ScopeConfig`. Asked of
/// any other instrument it is refused by name, rather than reported as the
/// scope's "no signal" — which read as a scope fault on a radio.
fn autoset_refusal(caps: &Capabilities) -> Option<&'static str> {
    match caps {
        Capabilities::Scope(_) => None,
        Capabilities::Sdr(_) => Some("autoset: scope only, not available on an SDR"),
    }
}

fn run(
    factory: &mut dyn FnMut() -> Result<Box<dyn Backend>, String>,
    commands: Receiver<Command>,
    events: Sender<Event>,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let mut wanted: Option<InstrumentConfig> = None;
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
        // Whatever bounds averaged samples comes from the instrument, not
        // from an assumption about which instrument it is.
        averager.counts = caps.count_range();
        info!(name = %caps.name(), serial = %caps.serial(), "backend connected");
        let _ = events.send(Event::Connected(caps.clone()));
        // Units owed to the consumer: what a frame we could not hand over
        // took with it. Reset by the next frame that gets through, and per
        // connection — a reconnect restarts the clock, so a carry from the
        // old link would be nonsense on the new one.
        let mut carry: u64 = 0;

        if let Some(cfg) = &wanted
            && let Err(e) = backend.apply(cfg)
        {
            warn!("config replay failed: {e}");
            let _ = events.try_send(Event::Disconnected(e.to_string()));
            continue 'outer;
        }

        loop {
            // Drain pending commands; only the newest config matters.
            let mut newest: Option<InstrumentConfig> = None;
            let mut do_force = false;
            let mut do_autoset = false;
            let mut multi: Option<MultiMode> = None;
            let mut pass_fail: Option<bool> = None;
            let mut stimulus: Option<String> = None;
            let mut seed: Option<u64> = None;
            loop {
                match commands.try_recv() {
                    Ok(Command::Apply(cfg)) => newest = Some(cfg),
                    Ok(Command::ForceTrigger) => do_force = true,
                    Ok(Command::AutoSet) => do_autoset = true,
                    Ok(Command::Multi(m)) => multi = Some(m),
                    Ok(Command::PassFail(level)) => pass_fail = Some(level),
                    Ok(Command::Stimulus(name)) => stimulus = Some(name),
                    Ok(Command::Seed(s)) => seed = Some(s),
                    Ok(Command::Shutdown) => break 'outer,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }
            if let Some(cfg) = newest {
                match averaging(&cfg) {
                    Some(n) if averager.n == n => {}
                    n => averager.reset(n.unwrap_or(0)),
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
            if let Some(mode) = multi
                && let Err(e) = backend.set_multi(mode)
            {
                let _ = events.try_send(Event::Error(e.to_string()));
            }
            if let Some(level) = pass_fail
                && let Err(e) = backend.set_pass_fail_output(level)
            {
                let _ = events.try_send(Event::Error(e.to_string()));
            }
            if let Some(name) = stimulus {
                match backend.set_stimulus(&name) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = events.try_send(Event::Error(format!("unknown stimulus {name:?}")));
                    }
                    Err(e) => {
                        let _ = events.try_send(Event::Error(e.to_string()));
                    }
                }
            }
            if let Some(s) = seed {
                match backend.set_seed(s) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = events.try_send(Event::Error("this backend has no seed".into()));
                    }
                    Err(e) => {
                        let _ = events.try_send(Event::Error(e.to_string()));
                    }
                }
            }
            if do_autoset && let Some(why) = autoset_refusal(&caps) {
                let _ = events.try_send(Event::Error(why.into()));
            } else if do_autoset {
                match backend.autoset() {
                    Ok(Some(cfg)) => {
                        let cfg = InstrumentConfig::Scope(cfg);
                        averager.reset(averaging(&cfg).unwrap_or(0));
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

            let running = wanted.as_ref().is_none_or(|c| c.running());
            if running {
                match backend.poll_frame(Duration::from_millis(100)) {
                    Ok(Some(frame)) => {
                        let frame = match wanted.as_ref().and_then(averaging) {
                            Some(_) => averager.fold(&frame).map_or(frame, Arc::new),
                            None => frame,
                        };
                        // Prefer dropping frames over blocking acquisition,
                        // but account for what was dropped: the frame we
                        // could not deliver is a hole, and the next one we do
                        // deliver has to say so.
                        let frame = carry_loss(frame, carry);
                        let owed = frame.dropped_before() + units_of(&frame);
                        if events.try_send(Event::Frame(frame)).is_err() {
                            dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            carry = owed;
                        } else {
                            carry = 0;
                        }

                        // Single sweep: one record, then stop.
                        if let Some(InstrumentConfig::Scope(cfg)) = &mut wanted
                            && cfg.trigger.sweep == Sweep::Single
                            && cfg.running
                        {
                            cfg.running = false;
                            let cfg = InstrumentConfig::Scope(cfg.clone());
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
    use neowon_core::{Acquisition, ChannelCapture, IqCal, SampleLayout};

    fn frame(vals: &[f32]) -> CaptureFrame {
        CaptureFrame::new(
            0,
            None,
            1.0,
            AcqMode::Sample,
            Acquisition::Record {
                samples: vals.len(),
            },
            SampleLayout::Real,
            vec![ChannelCapture {
                ch: 0,
                data: vals.to_vec(),
                cal: IqCal::real(1.0, 0.0),
                clipped: false,
                freq_meter: None,
            }],
        )
        .expect("a real record is a valid frame")
    }

    fn scope_caps(acquisition: Acquisition) -> Capabilities {
        Capabilities::Scope(neowon_core::ScopeCaps {
            name: "test".into(),
            serial: "0".into(),
            channels: 1,
            sample_rates: vec![1.0],
            volts_div: vec![1.0],
            probes: vec![1.0],
            acquisition,
            hardware_trigger: false,
        })
    }

    #[test]
    fn averager_converges() {
        let mut avg = Averager {
            counts: scope_caps(Acquisition::Record { samples: 2 }).count_range(),
            ..Default::default()
        };
        avg.reset(4);
        let a = avg.fold(&frame(&[100.0, 0.0])).unwrap();
        assert_eq!(a.channels[0].data, vec![100.0, 0.0]);
        // Fold in an opposite frame repeatedly: converges toward the mean of
        // the last window, never oscillates outside bounds.
        let b = avg.fold(&frame(&[0.0, 100.0])).unwrap();
        assert_eq!(b.channels[0].data, vec![50.0, 50.0]);
        let c = avg.fold(&frame(&[0.0, 100.0])).unwrap();
        assert!(c.channels[0].data[0] < 50.0 && c.channels[0].data[1] > 50.0);
        assert_eq!(c.acq(), AcqMode::Average(4));
    }

    /// The clamp is the instrument's, not the scope's. A scope's i8
    /// counts keep their grid and their rails; a streaming backend's
    /// full-scale samples must survive averaging untouched — rounding
    /// them to integers would erase a ±1.0 signal outright.
    #[test]
    fn averaged_samples_follow_the_instrument_range() {
        let scope = scope_caps(Acquisition::Record { samples: 2 });
        assert_eq!(scope.count_range(), Some((-128.0, 127.0)));
        let mut avg = Averager {
            counts: scope.count_range(),
            ..Default::default()
        };
        avg.reset(2);
        // Off-grid and past the rails: rounded onto the count grid, held
        // at the i8 limits.
        let out = avg.fold(&frame(&[0.4, 200.0, -200.0])).unwrap();
        assert_eq!(out.channels[0].data, vec![0.0, 127.0, -128.0]);

        let sdr = Capabilities::Sdr(neowon_core::SdrCaps {
            name: "test".into(),
            serial: "0".into(),
            tuner: "none".into(),
            freq_range_hz: (1.0, 2.0),
            sample_rates: vec![1.0],
            gains_db: vec![0.0],
            acquisition: Acquisition::Stream { chunk: 2 },
        });
        assert_eq!(sdr.count_range(), None);
        let mut avg = Averager {
            counts: sdr.count_range(),
            ..Default::default()
        };
        avg.reset(2);
        let out = avg.fold(&frame(&[0.4, 0.9, -0.9])).unwrap();
        assert_eq!(out.channels[0].data, vec![0.4, 0.9, -0.9]);
        // …and the running mean of two full-scale frames is the mean, not
        // a rounded, clipped ghost of it.
        let out = avg.fold(&frame(&[0.6, 0.1, -0.1])).unwrap();
        assert_eq!(out.channels[0].data, vec![0.5, 0.5, -0.5]);
    }

    /// A complex stream frame has no averaged form: the supervisor
    /// passes it through instead of minting an illegal record.
    #[test]
    fn complex_frames_are_not_averaged() {
        let complex = CaptureFrame::new(
            0,
            None,
            1.0,
            AcqMode::Sample,
            Acquisition::Stream { chunk: 1 },
            SampleLayout::Complex,
            vec![ChannelCapture {
                ch: 0,
                data: vec![0.5, -0.5],
                cal: IqCal::real(1.0, 0.0),
                clipped: false,
                freq_meter: None,
            }],
        )
        .unwrap();
        let mut avg = Averager::default();
        avg.reset(4);
        assert!(avg.fold(&complex).is_none());
    }

    /// A frame the consumer could not take is coverage lost, and the loss has
    /// to reach the consumer on the next frame — otherwise the hole is
    /// invisible and every continuity claim downstream is unfalsifiable.
    #[test]
    fn a_dropped_frames_loss_rides_on_the_next_one() {
        // Nothing owed: the same Arc goes through untouched.
        let f = Arc::new(frame(&[1.0, 2.0, 3.0]));
        let ptr = Arc::as_ptr(&f);
        let out = carry_loss(f, 0);
        assert_eq!(Arc::as_ptr(&out), ptr, "no carry must not copy the samples");
        assert_eq!(out.dropped_before(), 0);

        // Three units owed from a dropped frame: the next frame says so.
        let out = carry_loss(Arc::new(frame(&[1.0, 2.0])), 3);
        assert_eq!(out.dropped_before(), 3);

        // And it adds to what the backend itself reported, never replaces it.
        let reported = Arc::new(frame(&[1.0, 2.0]).with_dropped_before(10));
        assert_eq!(carry_loss(reported, 3).dropped_before(), 13);

        // What a dropped frame owes is its own samples plus its own gap.
        let f = frame(&[1.0, 2.0, 3.0, 4.0]).with_dropped_before(7);
        assert_eq!(units_of(&f), 4, "a real frame's units are its samples");
        assert_eq!(f.dropped_before() + units_of(&f), 11);
    }

    #[test]
    fn autoset_is_refused_by_name_on_an_sdr() {
        assert_eq!(
            autoset_refusal(&scope_caps(Acquisition::Record { samples: 2 })),
            None
        );
        let sdr = Capabilities::Sdr(neowon_core::SdrCaps {
            name: "test".into(),
            serial: "0".into(),
            tuner: "none".into(),
            freq_range_hz: (1.0, 2.0),
            sample_rates: vec![1.0],
            gains_db: vec![0.0],
            acquisition: Acquisition::Stream { chunk: 2 },
        });
        let why = autoset_refusal(&sdr).expect("an SDR has no autoset");
        assert!(why.contains("SDR") && !why.contains("no signal"), "{why}");
    }

    #[test]
    fn units_of_counts_pairs_for_a_complex_frame() {
        let complex = CaptureFrame::new(
            0,
            None,
            1.0,
            AcqMode::Sample,
            Acquisition::Stream { chunk: 2 },
            SampleLayout::Complex,
            vec![ChannelCapture {
                ch: 0,
                data: vec![0.5, -0.5, 0.25, -0.25],
                cal: IqCal::real(1.0, 0.0),
                clipped: false,
                freq_meter: None,
            }],
        )
        .unwrap();
        assert_eq!(units_of(&complex), 2);
    }
}
