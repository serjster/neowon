//! Bulk IQ streaming with several transfers in flight.
//!
//! The RTL2832 has a small FIFO; a gap between bulk reads overflows it and
//! the loss is invisible downstream. So `TRANSFERS` reads stay queued on
//! the endpoint at all times, and a dedicated thread only resubmits. The
//! stream owns nothing but the bulk endpoint: control transfers (tune,
//! gain) stay with `RtlSdr` and may be issued while samples flow.
//!
//! If the consumer falls `QUEUE` chunks behind, chunks are dropped rather
//! than stalling USB, and counted in `overflows`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, TrySendError, bounded};
use nusb::Interface;
use nusb::transfer::{Bulk, In};
use tracing::{debug, warn};

use super::{Error, Result};

const BULK_IN: u8 = 0x81;
/// Consecutive failed transfers taken as a disconnect.
const MAX_ERRORS: u32 = 5;

pub struct Stream {
    rx: Receiver<std::result::Result<Vec<u8>, String>>,
    stop: Arc<AtomicBool>,
    overflows: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Stream {
    pub(super) fn start(
        iface: &Interface,
        transfers: usize,
        transfer_len: usize,
        queue: usize,
    ) -> Result<Self> {
        let mut ep = iface.endpoint::<Bulk, In>(BULK_IN)?;
        let (tx, rx) = bounded(queue);
        let stop = Arc::new(AtomicBool::new(false));
        let overflows = Arc::new(AtomicU64::new(0));
        let (stop_t, over_t) = (stop.clone(), overflows.clone());
        let thread = std::thread::Builder::new()
            .name("neowon-rtl-stream".into())
            .spawn(move || {
                for _ in 0..transfers {
                    ep.submit(ep.allocate(transfer_len));
                }
                let mut errors = 0;
                while !stop_t.load(Ordering::Relaxed) {
                    let Some(c) = ep.wait_next_complete(Duration::from_millis(200)) else {
                        continue;
                    };
                    let mut buf = c.buffer;
                    match c.status {
                        Ok(()) => {
                            errors = 0;
                            match tx.try_send(Ok(buf[..c.actual_len].to_vec())) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    over_t.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(TrySendError::Disconnected(_)) => break,
                            }
                        }
                        Err(e) => {
                            errors += 1;
                            warn!("bulk transfer failed ({errors}/{MAX_ERRORS}): {e}");
                            if errors >= MAX_ERRORS {
                                let _ = tx.send(Err(format!("device lost: {e}")));
                                break;
                            }
                        }
                    }
                    buf.clear();
                    ep.submit(buf);
                }
                ep.cancel_all();
                while ep.pending() > 0 {
                    if ep.wait_next_complete(Duration::from_millis(200)).is_none() {
                        break;
                    }
                }
                debug!("stream thread exits");
            })
            .map_err(|e| Error::Invalid(format!("spawning the stream thread: {e}")))?;
        Ok(Self {
            rx,
            stop,
            overflows,
            thread: Some(thread),
        })
    }

    /// Next chunk of u8 offset-binary I,Q pairs; `Ok(None)` if none arrived
    /// within `timeout`. An error means the stream is dead.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        match self.rx.recv_timeout(timeout) {
            Ok(Ok(b)) => Ok(Some(b)),
            Ok(Err(e)) => Err(Error::Invalid(e)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(Error::Invalid("stream thread ended".into()))
            }
        }
    }

    /// Chunks dropped because the consumer was behind.
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
