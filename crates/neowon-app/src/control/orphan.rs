//! Orphan guard: `NEOWON_ORPHAN_EXIT=<seconds>` makes the app exit by
//! itself once no control-socket client has been live for that long.
//!
//! Test and tooling launches set it because they spawn the app as a child
//! and rely on the harness reaping it; when the harness itself is killed
//! (`timeout`, SIGKILL) the app is reparented to launchd and would live
//! forever with its window on screen. The harness holds one control-socket
//! connection for the test's lifetime, so the kill closes it — the watchdog
//! then ends the process. The start clock covers the window before the
//! first client ever connects, and a launch without the variable starts no
//! watchdog.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Live-client bookkeeping for one control server, plus the watchdog that
/// ends an unclaimed process.
pub struct OrphanGuard {
    timeout: Duration,
    live: AtomicUsize,
    last_activity: Mutex<Instant>,
}

impl OrphanGuard {
    /// The guard `NEOWON_ORPHAN_EXIT` asks for, or `None` when the
    /// variable is unset, empty, zero or not a number.
    #[must_use]
    pub fn from_env() -> Option<Arc<Self>> {
        let secs = parse_seconds(std::env::var("NEOWON_ORPHAN_EXIT").ok().as_deref())?;
        Some(Arc::new(Self {
            timeout: Duration::from_secs(secs),
            live: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        }))
    }

    /// Start the watchdog. It exits the process — hard, because the point
    /// is to survive a wedged frame loop — once the live count is zero and
    /// the last client activity is `timeout` old. With no client ever
    /// connected that is `timeout` after the guard was created.
    pub fn watch(self: &Arc<Self>) {
        let guard = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if guard.live.load(Ordering::SeqCst) > 0 {
                    continue;
                }
                let idle = guard.last_activity.lock().unwrap().elapsed();
                // Re-check after reading the clock: a client that has just
                // connected wins the race.
                if idle >= guard.timeout && guard.live.load(Ordering::SeqCst) == 0 {
                    eprintln!(
                        "control: no live client for {:.1}s (NEOWON_ORPHAN_EXIT); exiting",
                        idle.as_secs_f32()
                    );
                    std::process::exit(0);
                }
            }
        });
    }

    /// Register a live connection; drop the returned token when the
    /// connection ends, on any exit path.
    pub fn connect(self: &Arc<Self>) -> LiveClient {
        self.live.fetch_add(1, Ordering::SeqCst);
        self.touch();
        LiveClient(Arc::clone(self))
    }

    fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }
}

/// RAII token for one live connection: dropping it unregisters and
/// restarts the idle clock (a panic in the connection thread included).
pub struct LiveClient(Arc<OrphanGuard>);

impl Drop for LiveClient {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
    }
}

fn parse_seconds(v: Option<&str>) -> Option<u64> {
    v?.trim().parse::<u64>().ok().filter(|s| *s > 0)
}

#[cfg(test)]
mod tests {
    use super::{AtomicUsize, OrphanGuard, parse_seconds};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn env_seconds_are_parsed_strictly() {
        assert_eq!(parse_seconds(Some("30")), Some(30));
        assert_eq!(parse_seconds(Some(" 3 ")), Some(3));
        assert_eq!(parse_seconds(Some("0")), None);
        assert_eq!(parse_seconds(Some("-1")), None);
        assert_eq!(parse_seconds(Some("x")), None);
        assert_eq!(parse_seconds(Some("")), None);
        assert_eq!(parse_seconds(None), None);
    }

    #[test]
    fn connections_register_and_deregister() {
        let guard = Arc::new(OrphanGuard {
            timeout: Duration::from_secs(3600),
            live: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        });
        let a = guard.connect();
        let b = guard.connect();
        assert_eq!(guard.live.load(Ordering::SeqCst), 2);
        drop(a);
        assert_eq!(guard.live.load(Ordering::SeqCst), 1);
        drop(b);
        assert_eq!(guard.live.load(Ordering::SeqCst), 0);
    }
}
