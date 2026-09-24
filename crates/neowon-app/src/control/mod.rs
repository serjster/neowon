//! Remote control plane: a line-oriented localhost socket that accepts
//! script-grammar commands and `get …` queries with JSON replies. This is
//! the general-purpose API every external transport (CLI attach, MCP,
//! future REST) translates into — no scope logic lives outside the app.
//!
//! The socket binds 127.0.0.1 only. It is **on by default** at port 7777 so
//! an app launched from the desktop (no environment to set) is still
//! reachable by the live-development loop and the MCP server;
//! `NEOWON_CONTROL=<port>` picks another port and `NEOWON_CONTROL=off` or
//! `NEOWON_NO_CONTROL=1` turns it off. Protocol: one request per line; one
//! JSON object per line back. Commands are injected into the script queue
//! and acked immediately (`{"ok":true}`) — effects apply on the next frame.
//!
//! Because it is on by default, verbs that leave the process — writing a
//! file, reading one the caller named, reaching the network, ending the
//! process — need the connection to have sent `auth <token>` first. What
//! needs it is decided in [`privilege`]; how a client gets
//! the token is in [`conn`]. Queries and instrument control stay open.
//!
//! Test and tooling launches set `NEOWON_ORPHAN_EXIT=<seconds>` so a harness
//! that is killed cannot leave the app behind: the [`orphan`] watchdog ends
//! the process once no client has been live for that long.

pub mod conn;
mod json;
mod orphan;
mod privilege;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::net::TcpListener;
use std::sync::Arc;

use crate::Link;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::Phosphor;
use crate::record::{History, Recorder};
use crate::script::Script;
use crate::viz::three_d::Viz3dState;
use crate::viz::waterfall::WaterfallState;
pub(crate) use json::escape;
use json::{config_json, decode_json, measure_json, status_json};

pub struct Request {
    line: String,
    /// Whether this connection has sent a good `auth <token>`. Trust is
    /// per-connection, so it travels with the request rather than sitting
    /// in a resource every connection would share.
    authed: bool,
    reply: Sender<String>,
}

#[derive(Resource)]
pub struct ControlServer {
    rx: Option<Receiver<Request>>,
    /// The process token, so a refusal can say where to find it. `None`
    /// when no socket was opened.
    auth: Option<Arc<conn::Auth>>,
}

/// The control socket's port: `NEOWON_CONTROL` when set (`off`/`0`/`none`
/// disables), otherwise the default localhost port unless
/// `NEOWON_NO_CONTROL` is set.
#[must_use]
pub fn configured_port() -> Option<u16> {
    const DEFAULT_PORT: u16 = 7777;
    match std::env::var("NEOWON_CONTROL") {
        Ok(v) => match v.trim() {
            "0" | "off" | "none" => None,
            v => v.parse::<u16>().ok(),
        },
        Err(_) => (std::env::var_os("NEOWON_NO_CONTROL").is_none()).then_some(DEFAULT_PORT),
    }
}

/// Start the listener on [`configured_port`]; otherwise an inert resource
/// (the poll system early-outs). `NEOWON_ORPHAN_EXIT` starts the orphan
/// watchdog either way (see [`orphan`]).
pub fn start_from_env() -> ControlServer {
    let guard = orphan::OrphanGuard::from_env();
    let inert = |guard: Option<Arc<orphan::OrphanGuard>>| {
        // A scripted launch can ask for the guard with no socket at all:
        // the watchdog then just watches the start clock.
        if let Some(g) = guard {
            g.watch();
        }
        ControlServer {
            rx: None,
            auth: None,
        }
    };
    let Some(port) = configured_port() else {
        return inert(guard);
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            error!("control: cannot bind 127.0.0.1:{port}: {e}");
            return inert(guard);
        }
    };
    info!("control: listening on 127.0.0.1:{port}");
    if let Some(g) = &guard {
        g.watch();
    }
    let auth = conn::Auth::for_port(port);
    let (tx, rx) = unbounded::<Request>();
    let served = Arc::clone(&auth);
    std::thread::spawn(move || conn::accept_loop(listener, tx, guard, served));
    ControlServer {
        rx: Some(rx),
        auth: Some(auth),
    }
}

/// Later-phase resources bundled into one system param (Bevy caps systems
/// at 16 parameters).
type ExtraState<'w> = (
    Res<'w, crate::effects::Effects>,
    Res<'w, crate::autopeak::AutoPeak>,
    Res<'w, crate::deep::DeepView>,
    Res<'w, crate::decode::DecodeState>,
    Res<'w, crate::sdr::SdrState>,
    Res<'w, crate::catalog::CatalogState>,
    Res<'w, crate::uitree::UiTree>,
    Res<'w, crate::ui::layout::Layout>,
    Res<'w, crate::ui::layout::UiRects>,
    Res<'w, crate::refmap::RefMap>,
);

/// Drain pending requests. Runs before `run_script` so injected commands
/// land in the same frame.
#[allow(clippy::too_many_arguments)]
pub fn poll(
    server: Res<ControlServer>,
    time: Res<Time>,
    mut script: ResMut<Script>,
    link: Res<Link>,
    meas: Res<MeasureState>,
    math: Res<MathState>,
    fft: Res<FftState>,
    pf: Res<PfState>,
    phosphor: Res<Phosphor>,
    rec: Res<Recorder>,
    hist: Res<History>,
    wf: Res<WaterfallState>,
    viz: Res<Viz3dState>,
    extra: ExtraState,
) {
    let (fx, ap, deep, dec, sdr, cat) =
        (&extra.0, &extra.1, &extra.2, &extra.3, &extra.4, &extra.5);
    let Some(rx) = &server.rx else { return };
    let now = time.elapsed_secs_f64();
    for req in rx.try_iter() {
        let line = req.line.trim();
        let reply = match line.strip_prefix("get ") {
            Some("status") => status_json(&link, &rec, &hist),
            Some("config") => {
                config_json(&link, &phosphor, &math, &fft, &pf, &wf, &viz, fx, ap, deep)
            }
            Some("measure") => measure_json(&meas),
            Some("decode") => decode_json(dec),
            Some("sdr") => crate::sdr::sdr_json(sdr, link.sdr_caps()),
            Some("audio") => crate::sdr::audio_json(sdr),
            Some("iq") => crate::sdr::iq_json(sdr),
            Some("detections") => crate::sdr::detections_json(sdr),
            Some("modmeas") => crate::sdr::modmeas_json(sdr),
            Some("classify") => crate::sdr::classify_json(sdr),
            Some("dab") => crate::sdr::dab_json(sdr, &extra.9, now),
            Some("survey") => crate::sdr::survey_json(sdr),
            Some("surveydiff") => crate::sdr::survey_diff_json(sdr),
            Some("catalog") => crate::catalog::catalog_json(cat),
            Some("bands") => crate::refmap::bands_json(&extra.9, sdr),
            Some("location") => crate::refmap::location_json(&extra.9),
            Some("refdb") => crate::refmap::refdb_json(&extra.9),
            Some(q) if q == "stations" || q.starts_with("stations ") => {
                crate::refmap::stations_query(
                    &extra.9,
                    sdr,
                    q.trim_start_matches("stations").trim(),
                )
            }
            Some("uitree") => extra.6.json(&extra.7, &extra.8).unwrap_or_else(|| {
                r#"{"ok":false,"error":"no UI tree yet (one frame after enabling)"}"#.into()
            }),
            Some(q) if q.starts_with("history ") => {
                crate::catalog::history_json(cat, q[8..].trim())
            }
            Some(other) => format!(
                r#"{{"ok":false,"error":"unknown query {}"}}"#,
                escape(other)
            ),
            // Not a query: a script line. Everything that leaves the
            // process needs the connection's token first.
            None => match server
                .auth
                .as_deref()
                .map(|auth| conn::authorize(line, req.authed, auth))
            {
                Some(Ok(actions)) => {
                    for (dt, a) in actions {
                        script.inject_at(now + dt, a);
                    }
                    r#"{"ok":true}"#.into()
                }
                Some(Err(reply)) => reply,
                // Unreachable: no `auth` means no listener means no request.
                None => r#"{"ok":false,"error":"control socket is off"}"#.into(),
            },
        };
        let _ = req.reply.try_send(reply);
    }
}
