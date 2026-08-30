//! Remote control plane: a line-oriented localhost socket that accepts
//! script-grammar commands and `get …` queries with JSON replies. This is
//! the general-purpose API every external transport (CLI attach, MCP,
//! future REST) translates into — no scope logic lives outside the app.
//!
//! Enabled by `NEOWON_CONTROL=<port>` (binds 127.0.0.1 only; off by
//! default). Protocol: one request per line; one JSON object per line
//! back. Commands are injected into the script queue and acked
//! immediately (`{"ok":true}`) — effects apply on the next frame.

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use crate::Link;
use crate::derived::{FftState, METRICS, MathState, MeasureState, PfState};
use crate::gpu::{Palette, Persistence, Phosphor, TraceMode};
use crate::record::{History, Recorder};
use crate::script::Script;
use crate::viz::three_d::Viz3dState;
use crate::viz::waterfall::WaterfallState;

pub struct Request {
    line: String,
    reply: Sender<String>,
}

#[derive(Resource)]
pub struct ControlServer {
    rx: Option<Receiver<Request>>,
}

/// Start the listener if `NEOWON_CONTROL` is set; otherwise an inert
/// resource (the poll system early-outs).
pub fn start_from_env() -> ControlServer {
    let Some(port) = std::env::var("NEOWON_CONTROL")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
    else {
        return ControlServer { rx: None };
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            error!("control: cannot bind 127.0.0.1:{port}: {e}");
            return ControlServer { rx: None };
        }
    };
    info!("control: listening on 127.0.0.1:{port}");
    let (tx, rx) = unbounded::<Request>();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut out = match conn.try_clone() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (reply_tx, reply_rx) = bounded(1);
                    if tx
                        .send(Request {
                            line,
                            reply: reply_tx,
                        })
                        .is_err()
                    {
                        break;
                    }
                    let reply = reply_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap_or_else(|_| r#"{"ok":false,"error":"timeout"}"#.into());
                    if writeln!(out, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    ControlServer { rx: Some(rx) }
}

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
    fx: Res<crate::effects::Effects>,
) {
    let Some(rx) = &server.rx else { return };
    let now = time.elapsed_secs_f64();
    for req in rx.try_iter() {
        let line = req.line.trim();
        let reply = match line.strip_prefix("get ") {
            Some("status") => status_json(&link, &rec, &hist),
            Some("config") => config_json(&link, &phosphor, &math, &fft, &pf, &wf, &viz, &fx),
            Some("measure") => measure_json(&meas),
            Some(other) => format!(
                r#"{{"ok":false,"error":"unknown query {}"}}"#,
                escape(other)
            ),
            None => match crate::script::parse(line) {
                Ok(actions) => {
                    for (dt, a) in actions {
                        script.inject_at(now + dt, a);
                    }
                    r#"{"ok":true}"#.into()
                }
                Err(e) => format!(r#"{{"ok":false,"error":"{}"}}"#, escape(&e)),
            },
        };
        let _ = req.reply.try_send(reply);
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A JSON number: finite floats as-is, everything else null.
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn status_json(link: &Link, rec: &Recorder, hist: &History) -> String {
    let (name, serial) = link
        .caps
        .as_ref()
        .map(|c| (c.name.clone(), c.serial.clone()))
        .unwrap_or_default();
    format!(
        concat!(
            r#"{{"ok":true,"running":{},"frames_seen":{},"backend":"{}","serial":"{}","#,
            r#""status":"{}","stimulus":"{}","recorder":{{"on":{},"frames":{}}},"#,
            r#""history":{},"last_export":{}}}"#
        ),
        link.config.running,
        link.frames_seen,
        escape(&name),
        escape(&serial),
        escape(&link.status),
        escape(&link.stimulus),
        rec.on,
        rec.frames.len(),
        hist.active.map_or("null".to_string(), |i| i.to_string()),
        rec.last_export
            .as_ref()
            .map_or("null".to_string(), |p| format!("\"{}\"", escape(p))),
    )
}

#[allow(clippy::too_many_arguments)]
fn config_json(
    link: &Link,
    phosphor: &Phosphor,
    math: &MathState,
    fft: &FftState,
    pf: &PfState,
    wf: &WaterfallState,
    viz: &Viz3dState,
    fx: &crate::effects::Effects,
) -> String {
    use neowon_core::{Coupling, Slope, TriggerKind};
    let c = &link.config;
    let mut channels = String::new();
    for (i, ch) in c.channels.iter().enumerate().take(2) {
        if i > 0 {
            channels.push(',');
        }
        let coup = match ch.coupling {
            Coupling::Dc => "dc",
            Coupling::Ac => "ac",
            Coupling::Gnd => "gnd",
        };
        channels.push_str(&format!(
            r#"{{"enabled":{},"volts_div":{},"coupling":"{coup}","probe":{},"offset":{}}}"#,
            ch.enabled,
            num(ch.volts_div),
            num(ch.probe),
            num(ch.offset),
        ));
    }
    let t = &c.trigger;
    let sweep = match t.sweep {
        neowon_core::Sweep::Auto => "auto",
        neowon_core::Sweep::Normal => "normal",
        neowon_core::Sweep::Single => "single",
    };
    let kind = match t.kind {
        TriggerKind::Edge { slope } => format!(
            r#""kind":"edge","slope":"{}""#,
            match slope {
                Slope::Rising => "rising",
                Slope::Falling => "falling",
            }
        ),
        TriggerKind::Pulse { condition, width } => {
            let (pol, cmp) = crate::session::condition_words(condition);
            format!(
                r#""kind":"pulse","condition":"{pol} {cmp}","width":{}"#,
                num(width)
            )
        }
        TriggerKind::Slope {
            condition,
            width,
            upper,
            lower,
        } => {
            let (pol, cmp) = crate::session::condition_words(condition);
            format!(
                r#""kind":"slope","condition":"{pol} {cmp}","width":{},"upper":{},"lower":{}"#,
                num(width),
                num(upper),
                num(lower)
            )
        }
        TriggerKind::Video { sync, line } => {
            let sync = match sync {
                neowon_core::VideoSync::Line => "line",
                neowon_core::VideoSync::Field => "field",
                neowon_core::VideoSync::OddField => "odd",
                neowon_core::VideoSync::EvenField => "even",
                neowon_core::VideoSync::LineNumber => "linenum",
            };
            format!(r#""kind":"video","sync":"{sync}","line":{line}"#)
        }
    };
    let acq = match c.acq {
        neowon_core::AcqMode::Sample => "sample".to_string(),
        neowon_core::AcqMode::Peak => "peak".to_string(),
        neowon_core::AcqMode::Average(n) => format!("avg{n}"),
    };
    let mode = match phosphor.mode {
        TraceMode::Vectors => "vectors",
        TraceMode::Dots => "dots",
        TraceMode::Xy => "xy",
    };
    let persist = match phosphor.persistence {
        Persistence::Off => "\"off\"".to_string(),
        Persistence::Infinite => "\"inf\"".to_string(),
        Persistence::Seconds(s) => format!("{s}"),
    };
    let palette = match phosphor.palette {
        Palette::Phosphor => "phosphor",
        Palette::Thermal => "thermal",
        Palette::Green => "green",
    };
    format!(
        concat!(
            r#"{{"ok":true,"sample_rate":{},"trigger_position":{},"acq":"{}","running":{},"#,
            r#""channels":[{}],"#,
            r#""trigger":{{"source":{},{},"level":{},"sweep":"{}","holdoff":{}}},"#,
            r#""display":{{"mode":"{}","persist":{},"gain":{},"crt":{},"palette":"{}","hview":[{},{}]}},"#,
            r#""math":{{"enabled":{}}},"fft":{{"enabled":{},"source":{}}},"#,
            r#""pf":{{"enabled":{},"source":{},"pass":{},"fail":{}}},"#,
            r#""viz":{{"waterfall":{},"mode":"{}","effect":{}}}}}"#
        ),
        num(c.sample_rate),
        num(c.position),
        acq,
        c.running,
        channels,
        t.source,
        kind,
        num(t.level),
        sweep,
        num(t.holdoff),
        mode,
        persist,
        num(phosphor.gain as f64),
        phosphor.crt,
        palette,
        num(phosphor.hview.0),
        num(phosphor.hview.1),
        math.enabled,
        fft.enabled,
        fft.source,
        pf.enabled,
        pf.source_slot,
        pf.pass,
        pf.fail,
        wf.on,
        viz.mode.name(),
        fx.active
            .as_ref()
            .map_or("null".to_string(), |n| format!("\"{}\"", escape(n))),
    )
}

fn measure_json(meas: &MeasureState) -> String {
    let mut slots = String::new();
    for slot in 0..crate::derived::SLOTS {
        if slot > 0 {
            slots.push(',');
        }
        let Some(m) = &meas.latest[slot] else {
            slots.push_str("null");
            continue;
        };
        let mut metrics = String::new();
        for (i, (name, get, _)) in METRICS.iter().enumerate() {
            if i > 0 {
                metrics.push(',');
            }
            let value = get(m).map_or("null".to_string(), num);
            let stats = meas
                .stats
                .get(slot)
                .map(|s| &s[i])
                .filter(|t| t.count > 0)
                .map_or("null".to_string(), |t| {
                    format!(
                        r#"{{"mean":{},"min":{},"max":{},"std":{},"n":{}}}"#,
                        num(t.mean),
                        num(t.min),
                        num(t.max),
                        num(t.std_dev()),
                        t.count
                    )
                });
            metrics.push_str(&format!(
                r#"{{"name":"{}","value":{value},"stats":{stats}}}"#,
                escape(name)
            ));
        }
        slots.push_str(&format!(r#"{{"metrics":[{metrics}]}}"#));
    }
    format!(
        r#"{{"ok":true,"slots":[{slots}],"sample_rate":{}}}"#,
        num(meas.sample_rate)
    )
}

#[cfg(test)]
mod tests {
    use super::{escape, num};

    #[test]
    fn json_escaping_and_numbers() {
        assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape("x\ny"), "x\\ny");
        assert_eq!(num(0.2), "0.2");
        assert_eq!(num(250e3), "250000");
        assert_eq!(num(f64::NAN), "null");
        assert_eq!(num(f64::INFINITY), "null");
    }
}
