//! Test/automation scripting. `NEOWON_SCRIPT=<path>` runs a plain-text
//! action script against the live app — the same state mutations the UI
//! performs, so anything the panel can do a script can do.
//!
//! One action per line (`#` comments allowed):
//! ```text
//! wait <seconds>
//! stimulus <name>                       # sim scenarios, e.g. xy-circle
//! rate <S/s>
//! vdiv <ch> <volts>
//! enable <ch> <0|1>
//! coupling <ch> <dc|ac|gnd>
//! probe <ch> <factor>
//! offset <ch> <fraction>
//! trigger <ch> <rising|falling> <level_volts> <auto|normal|single>
//! acq <sample|peak|avg4|avg16|avg64>
//! mode <vectors|dots|xy>
//! persist <off|inf|SECONDS>
//! gain <float>
//! math <off|add|sub|mul|div|diff|integ>
//! run <0|1>
//! shot <path.ppm> [x y w h]             # plot-texture region (default: all)
//! quit
//! ```
//! `wait` advances a cumulative timeline; other actions fire when their
//! time comes. `quit` waits for outstanding shots, then exits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use neowon_backend::Command;
use neowon_core::{AcqMode, Coupling, Slope, Sweep, TriggerKind};
use neowon_dsp::MathOp;

use crate::Link;
use crate::derived::MathState;
use crate::gpu::{PLOT_H, PLOT_W, Persistence, Phosphor, TraceMode};

/// Shots in flight (readback observers decrement).
static PENDING_SHOTS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
pub enum Action {
    Stimulus(String),
    Rate(f64),
    Vdiv(usize, f64),
    Enable(usize, bool),
    CouplingSet(usize, Coupling),
    Probe(usize, f64),
    Offset(usize, f64),
    Trigger {
        ch: usize,
        slope: Slope,
        level: f64,
        sweep: Sweep,
    },
    Acq(AcqMode),
    Mode(TraceMode),
    Persist(Persistence),
    Gain(f32),
    Math(Option<MathOp>),
    Run(bool),
    Shot {
        path: String,
        roi: Option<(u32, u32, u32, u32)>,
    },
    Quit,
}

#[derive(Resource, Default)]
pub struct Script {
    /// (due time in seconds since startup, action)
    queue: VecDeque<(f64, Action)>,
}

pub fn load_from_env() -> Script {
    let Some(path) = std::env::var_os("NEOWON_SCRIPT") else {
        return Script::default();
    };
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("NEOWON_SCRIPT {path:?}: {e}"));
    match parse(&text) {
        Ok(queue) => {
            info!("script: {} actions from {path:?}", queue.len());
            Script { queue }
        }
        Err(e) => panic!("NEOWON_SCRIPT parse error: {e}"),
    }
}

fn parse(text: &str) -> Result<VecDeque<(f64, Action)>, String> {
    let mut t = 0.0f64;
    let mut out = VecDeque::new();
    for (ln, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err = |m: &str| format!("line {}: {m}: {line:?}", ln + 1);
        let mut w = line.split_whitespace();
        let cmd = w.next().unwrap();
        let mut rest = || w.next().ok_or_else(|| err("missing argument"));
        let action = match cmd {
            "wait" => {
                t += rest()?.parse::<f64>().map_err(|_| err("bad number"))?;
                continue;
            }
            "stimulus" => Action::Stimulus(rest()?.to_string()),
            "rate" => Action::Rate(rest()?.parse().map_err(|_| err("bad rate"))?),
            "vdiv" => Action::Vdiv(
                rest()?.parse().map_err(|_| err("bad ch"))?,
                rest()?.parse().map_err(|_| err("bad volts"))?,
            ),
            "enable" => Action::Enable(rest()?.parse().map_err(|_| err("bad ch"))?, rest()? == "1"),
            "coupling" => Action::CouplingSet(
                rest()?.parse().map_err(|_| err("bad ch"))?,
                match rest()? {
                    "dc" => Coupling::Dc,
                    "ac" => Coupling::Ac,
                    "gnd" => Coupling::Gnd,
                    _ => return Err(err("bad coupling")),
                },
            ),
            "probe" => Action::Probe(
                rest()?.parse().map_err(|_| err("bad ch"))?,
                rest()?.parse().map_err(|_| err("bad factor"))?,
            ),
            "offset" => Action::Offset(
                rest()?.parse().map_err(|_| err("bad ch"))?,
                rest()?.parse().map_err(|_| err("bad fraction"))?,
            ),
            "trigger" => Action::Trigger {
                ch: rest()?.parse().map_err(|_| err("bad ch"))?,
                slope: match rest()? {
                    "rising" => Slope::Rising,
                    "falling" => Slope::Falling,
                    _ => return Err(err("bad slope")),
                },
                level: rest()?.parse().map_err(|_| err("bad level"))?,
                sweep: match rest()? {
                    "auto" => Sweep::Auto,
                    "normal" => Sweep::Normal,
                    "single" => Sweep::Single,
                    _ => return Err(err("bad sweep")),
                },
            },
            "acq" => Action::Acq(match rest()? {
                "sample" => AcqMode::Sample,
                "peak" => AcqMode::Peak,
                "avg4" => AcqMode::Average(4),
                "avg16" => AcqMode::Average(16),
                "avg64" => AcqMode::Average(64),
                _ => return Err(err("bad acq mode")),
            }),
            "mode" => Action::Mode(match rest()? {
                "vectors" => TraceMode::Vectors,
                "dots" => TraceMode::Dots,
                "xy" => TraceMode::Xy,
                _ => return Err(err("bad trace mode")),
            }),
            "persist" => Action::Persist(match rest()? {
                "off" => Persistence::Off,
                "inf" => Persistence::Infinite,
                s => Persistence::Seconds(s.parse().map_err(|_| err("bad persistence"))?),
            }),
            "gain" => Action::Gain(rest()?.parse().map_err(|_| err("bad gain"))?),
            "math" => Action::Math(match rest()? {
                "off" => None,
                "add" => Some(MathOp::Add),
                "sub" => Some(MathOp::Sub),
                "mul" => Some(MathOp::Mul),
                "div" => Some(MathOp::Div),
                "diff" => Some(MathOp::Diff),
                "integ" => Some(MathOp::Integ),
                _ => return Err(err("bad math op")),
            }),
            "run" => Action::Run(rest()? == "1"),
            "shot" => {
                let path = rest()?.to_string();
                let roi = match w.next() {
                    None => None,
                    Some(x) => {
                        let p = |s: Option<&str>| -> Result<u32, String> {
                            s.ok_or_else(|| err("roi needs x y w h"))?
                                .parse()
                                .map_err(|_| err("bad roi number"))
                        };
                        Some((
                            x.parse().map_err(|_| err("bad roi number"))?,
                            p(w.next())?,
                            p(w.next())?,
                            p(w.next())?,
                        ))
                    }
                };
                Action::Shot { path, roi }
            }
            "quit" => Action::Quit,
            _ => return Err(err("unknown action")),
        };
        out.push_back((t, action));
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn run_script(
    time: Res<Time>,
    mut script: ResMut<Script>,
    mut commands: Commands,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut math: ResMut<MathState>,
) {
    let now = time.elapsed_secs_f64();
    while let Some((due, _)) = script.queue.front() {
        if *due > now {
            return;
        }
        let (_, action) = script.queue.pop_front().unwrap();
        debug!("script: {action:?}");
        match action {
            Action::Stimulus(name) => {
                let _ = link.sup.commands.send(Command::Stimulus(name));
            }
            Action::Rate(r) => {
                link.config.sample_rate = r;
                link.dirty = true;
            }
            Action::Vdiv(ch, v) => {
                link.config.channels[ch].volts_div = v;
                link.dirty = true;
            }
            Action::Enable(ch, on) => {
                link.config.channels[ch].enabled = on;
                link.dirty = true;
            }
            Action::CouplingSet(ch, c) => {
                link.config.channels[ch].coupling = c;
                link.dirty = true;
            }
            Action::Probe(ch, p) => {
                link.config.channels[ch].probe = p;
                link.dirty = true;
            }
            Action::Offset(ch, o) => {
                link.config.channels[ch].offset = o;
                link.dirty = true;
            }
            Action::Trigger {
                ch,
                slope,
                level,
                sweep,
            } => {
                link.config.trigger.source = ch;
                link.config.trigger.kind = TriggerKind::Edge { slope };
                link.config.trigger.level = level;
                link.config.trigger.sweep = sweep;
                link.dirty = true;
            }
            Action::Acq(a) => {
                link.config.acq = a;
                link.dirty = true;
            }
            Action::Mode(m) => phosphor.mode = m,
            Action::Persist(p) => phosphor.persistence = p,
            Action::Gain(g) => phosphor.gain = g,
            Action::Math(op) => match op {
                None => math.enabled = false,
                Some(op) => {
                    math.enabled = true;
                    math.op = op;
                    math.rescale = true;
                }
            },
            Action::Run(r) => {
                link.config.running = r;
                link.dirty = true;
            }
            Action::Shot { path, roi } => {
                PENDING_SHOTS.fetch_add(1, Ordering::SeqCst);
                commands
                    .spawn(bevy::render::gpu_readback::Readback::texture(
                        phosphor.display_image.clone(),
                    ))
                    .observe(
                        move |event: On<bevy::render::gpu_readback::ReadbackComplete>,
                              mut cmd: Commands| {
                            write_shot(&event.data, &path, roi);
                            PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
                            cmd.entity(event.entity).despawn();
                        },
                    );
            }
            Action::Quit => {
                if PENDING_SHOTS.load(Ordering::SeqCst) > 0 {
                    // Re-arm shortly; shots still in flight.
                    script.queue.push_front((now + 0.05, Action::Quit));
                    return;
                }
                info!("script: done");
                std::process::exit(0);
            }
        }
    }
}

/// Write a (possibly cropped) region of the plot texture as binary PPM.
/// Readback rows are 256-byte aligned; the stride strips that.
fn write_shot(rgba: &[u8], path: &str, roi: Option<(u32, u32, u32, u32)>) {
    let stride = rgba.len() / PLOT_H as usize;
    let (x0, y0, w, h) = roi.unwrap_or((0, 0, PLOT_W, PLOT_H));
    let (x0, y0) = (x0.min(PLOT_W - 1), y0.min(PLOT_H - 1));
    let w = w.min(PLOT_W - x0);
    let h = h.min(PLOT_H - y0);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for row in y0..y0 + h {
        let base = row as usize * stride + x0 as usize * 4;
        for px in rgba[base..base + w as usize * 4].chunks_exact(4) {
            ppm.extend_from_slice(&px[..3]);
        }
    }
    match std::fs::write(path, &ppm) {
        Ok(()) => info!("script: wrote {path} ({w}x{h})"),
        Err(e) => error!("script: cannot write {path}: {e}"),
    }
}
