//! The instrument link: the supervisor handle with the scope-side state it
//! feeds, and the per-frame systems that drain its events and push config back.

use bevy::prelude::*;
use neowon_backend::{Capabilities, Event, InstrumentConfig, MultiMode, ScopeConfig, Supervisor};
use neowon_core::SharedFrame;

use crate::sdr;

#[derive(Resource)]
pub struct Link {
    pub sup: Supervisor,
    /// The connected instrument's capabilities, `None` while disconnected.
    /// The variant *is* the instrument, so there is one of these, not
    /// one `Option` per mode (readers: `Link::scope_caps`/`sdr_caps`).
    pub caps: Option<Capabilities>,
    pub status: String,
    pub latest: Option<SharedFrame>,
    pub config: ScopeConfig,
    pub dirty: bool,
    pub frames_seen: u64,
    pub multi: MultiMode,
    /// Elapsed time when the last frame arrived — the WAIT indicator in the
    /// menu bar compares against it (starved Normal/Single trigger).
    pub last_frame_at: f64,
    /// Every frame that arrived this update, oldest first. `latest` is the
    /// newest of them; consumers that need one record read that, while the
    /// recorder takes them all — otherwise the scrollback is capped at the
    /// render rate no matter how fast the instrument captures.
    pub arrived: Vec<SharedFrame>,
    /// Name of the active stimulus (generating backends only).
    pub stimulus: String,
    /// The channel pointer gestures and scroll steps act on.
    pub selected: usize,
    /// Path of the last written screenshot (`shot`/`shotplot`), for the
    /// status line and `get status`.
    pub last_shot: Option<String>,
}

impl Link {
    /// The link as the app starts it: connecting, nothing acquired yet.
    pub fn new(sup: Supervisor, config: ScopeConfig) -> Self {
        Self {
            sup,
            caps: None,
            status: "connecting…".into(),
            latest: None,
            config,
            dirty: false,
            frames_seen: 0,
            multi: MultiMode::TriggerOut,
            last_frame_at: 0.0,
            arrived: Vec::new(),
            stimulus: "probe-comp".into(),
            selected: 0,
            last_shot: None,
        }
    }
}

pub(crate) fn ingest(time: Res<Time>, mut link: ResMut<Link>, mut sdr: ResMut<sdr::SdrState>) {
    link.arrived.clear();
    while let Ok(event) = link.sup.events.try_recv() {
        match event {
            Event::Connected(caps) => {
                link.status = match &caps {
                    Capabilities::Scope(c) => format!("{} {}", c.name, c.serial),
                    Capabilities::Sdr(c) => format!("{} {} ({})", c.name, c.serial, c.tuner),
                };
                // The mode follows the instrument that answered.
                sdr.active |= caps.sdr().is_some();
                link.caps = Some(caps);
            }
            Event::Disconnected(e) => {
                link.status = format!("disconnected: {e}");
                link.caps = None;
            }
            // Complex frames are SDR data; the scope consumers never see them.
            Event::Frame(f) if f.layout() == neowon_core::SampleLayout::Complex => {
                link.last_frame_at = time.elapsed_secs_f64();
                sdr.note_frame(&f, time.elapsed_secs_f64());
                // Streaming consumers get every frame here; the display path
                // only ever sees the latest one.
                sdr::dab::feed(&mut sdr, &f);
                sdr::iqdump::write(&mut sdr, &f);
                sdr.latest = Some(f);
            }
            Event::Frame(f) => {
                link.frames_seen += 1;
                link.last_frame_at = time.elapsed_secs_f64();
                link.arrived.push(f.clone());
                if link.frames_seen == 1 || link.frames_seen.is_multiple_of(500) {
                    tracing::info!(frames = link.frames_seen, "acquiring");
                }
                link.latest = Some(f);
            }
            Event::ConfigUpdated(InstrumentConfig::Scope(cfg)) => {
                link.config = cfg;
            }
            Event::ConfigUpdated(InstrumentConfig::Sdr(cfg)) => sdr.config = cfg,
            Event::Error(e) => link.status = format!("error: {e}"),
        }
    }
    // A DAB receiver whose stream has stopped is looking at nothing. The
    // hardware-moving actions reset at once; this catches a stopped backend,
    // a disconnect or a stalled link, where no frame will arrive to carry the
    // receiver's own expiry.
    let stalled = |at: f64| time.elapsed_secs_f64() - at > sdr::dab::NO_INPUT_TIMEOUT_S;
    if sdr.dab.on() && sdr.dab.last_frame_at.is_some_and(stalled) {
        sdr.dab_no_input();
    }
}

pub(crate) fn flush(mut link: ResMut<Link>) {
    if link.dirty {
        link.dirty = false;
        let cfg = link.config.clone();
        link.sup.apply(cfg);
    }
}
