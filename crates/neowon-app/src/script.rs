//! Test/automation scripting. `NEOWON_SCRIPT=<path>` runs a plain-text
//! action script against the live app — the same state mutations the UI
//! performs, so anything the panel can do a script can do.
//!
//! One action per line (`#` comments allowed). Every UI control is
//! reachable here (AGENTS.md script-parity rule).
//!
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
//! trigpulse <ch> <pos|neg> <gt|eq|lt> <width_us> <auto|normal|single>
//! trigslope <ch> <pos|neg> <gt|eq|lt> <width_us> <upper_v> <lower_v> <sweep>
//! trigvideo <line|field|odd|even|linenum> <line#> <sweep>
//! holdoff <seconds>
//! autoset
//! force
//! acq <sample|peak|avg4|avg16|avg64>
//! mode <vectors|dots|xy>
//! persist <off|inf|SECONDS>
//! gain <float>
//! math <off|add|sub|mul|div|diff|integ>
//! run <0|1>
//! multi <trigout|pfout|trigin>
//! pfout <0|1>
//! cursor <time|amp> <on|off>
//! stats <slot>
//! statsreset
//! fft <on|off>
//! fftsrc <slot>
//! fftwnd <rectangle|hamming|hann|blackman|flattop|triangular>
//! pf <on|off>
//! pfsrc <slot>
//! pftol <h_div> <v_div>
//! pfcapture
//! pfreset
//! menu <channel <ch>|horizontal|trigger|acquire|display|measure|math|cursor|utility|none>
//! markers <0|1>                         # on-graph drag handles
//! palette <phosphor|thermal|green>
//! window <W>x<H>                        # resize (layout tests)
//! layout <path.json>                    # named-ROI map + open menu
//! shot <path.ppm> [x y w h]             # plot-texture region (default: all)
//! quit
//! ```
//! `wait` advances a cumulative timeline; other actions fire when their
//! time comes. `quit` waits for outstanding shots, then exits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use neowon_backend::{Command, MultiMode};
use neowon_core::{AcqMode, Coupling, PulseCondition, Slope, Sweep, TriggerKind, VideoSync};
use neowon_dsp::{MathOp, Window};

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::{PLOT_H, PLOT_W, Persistence, Phosphor, TraceMode};
use crate::ui::layout::dump_json;
use crate::ui::{Menu, MenuState};

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
    TrigPulse {
        ch: usize,
        cond: PulseCondition,
        width: f64,
        sweep: Sweep,
    },
    TrigSlope {
        ch: usize,
        cond: PulseCondition,
        width: f64,
        upper: f64,
        lower: f64,
        sweep: Sweep,
    },
    TrigVideo {
        sync: VideoSync,
        line: u16,
        sweep: Sweep,
    },
    Holdoff(f64),
    AutoSet,
    Force,
    Acq(AcqMode),
    Mode(TraceMode),
    Persist(Persistence),
    Gain(f32),
    Crt(bool),
    Select(usize),
    Guides(bool),
    Markers(bool),
    PaletteSet(crate::gpu::Palette),
    WindowSize(f32, f32),
    Math(Option<MathOp>),
    Run(bool),
    Multi(MultiMode),
    PfOut(bool),
    Cursor {
        amp: bool,
        on: bool,
    },
    Stats(usize),
    StatsReset,
    Fft(bool),
    FftSrc(usize),
    FftWnd(Window),
    Pf(bool),
    PfSrc(usize),
    PfTol(f64, f64),
    PfCapture,
    PfReset,
    Menu(Option<Menu>),
    Layout(String),
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

fn parse_sweep(s: &str) -> Result<Sweep, ()> {
    match s {
        "auto" => Ok(Sweep::Auto),
        "normal" => Ok(Sweep::Normal),
        "single" => Ok(Sweep::Single),
        _ => Err(()),
    }
}

fn parse_condition(polarity: &str, cmp: &str) -> Result<PulseCondition, ()> {
    use PulseCondition::*;
    let c = match (polarity, cmp) {
        ("pos", "gt") => PositiveGreater,
        ("pos", "eq") => PositiveEqual,
        ("pos", "lt") => PositiveLess,
        ("neg", "gt") => NegativeGreater,
        ("neg", "eq") => NegativeEqual,
        ("neg", "lt") => NegativeLess,
        _ => return Err(()),
    };
    Ok(c)
}

fn parse_window(s: &str) -> Result<Window, ()> {
    match s {
        "rectangle" => Ok(Window::Rectangle),
        "hamming" => Ok(Window::Hamming),
        "hann" => Ok(Window::Hann),
        "blackman" => Ok(Window::Blackman),
        "flattop" => Ok(Window::Flattop),
        "triangular" => Ok(Window::Triangular),
        _ => Err(()),
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
                sweep: parse_sweep(rest()?).map_err(|_| err("bad sweep"))?,
            },
            "trigpulse" => Action::TrigPulse {
                ch: rest()?.parse().map_err(|_| err("bad ch"))?,
                cond: parse_condition(rest()?, rest()?).map_err(|_| err("bad condition"))?,
                width: rest()?.parse::<f64>().map_err(|_| err("bad width"))? * 1e-6,
                sweep: parse_sweep(rest()?).map_err(|_| err("bad sweep"))?,
            },
            "trigslope" => Action::TrigSlope {
                ch: rest()?.parse().map_err(|_| err("bad ch"))?,
                cond: parse_condition(rest()?, rest()?).map_err(|_| err("bad condition"))?,
                width: rest()?.parse::<f64>().map_err(|_| err("bad width"))? * 1e-6,
                upper: rest()?.parse().map_err(|_| err("bad upper"))?,
                lower: rest()?.parse().map_err(|_| err("bad lower"))?,
                sweep: parse_sweep(rest()?).map_err(|_| err("bad sweep"))?,
            },
            "trigvideo" => Action::TrigVideo {
                sync: match rest()? {
                    "line" => VideoSync::Line,
                    "field" => VideoSync::Field,
                    "odd" => VideoSync::OddField,
                    "even" => VideoSync::EvenField,
                    "linenum" => VideoSync::LineNumber,
                    _ => return Err(err("bad video sync")),
                },
                line: rest()?.parse().map_err(|_| err("bad line"))?,
                sweep: parse_sweep(rest()?).map_err(|_| err("bad sweep"))?,
            },
            "holdoff" => Action::Holdoff(rest()?.parse().map_err(|_| err("bad holdoff"))?),
            "autoset" => Action::AutoSet,
            "force" => Action::Force,
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
            "crt" => Action::Crt(rest()? == "1"),
            "select" => Action::Select(rest()?.parse().map_err(|_| err("bad ch"))?),
            "guides" => Action::Guides(rest()? == "1"),
            "markers" => Action::Markers(rest()? == "1"),
            "palette" => Action::PaletteSet(match rest()? {
                "phosphor" => crate::gpu::Palette::Phosphor,
                "thermal" => crate::gpu::Palette::Thermal,
                "green" => crate::gpu::Palette::Green,
                _ => return Err(err("bad palette")),
            }),
            "window" => {
                let arg = rest()?;
                let (w, h) = arg.split_once('x').ok_or_else(|| err("expected WxH"))?;
                Action::WindowSize(
                    w.parse().map_err(|_| err("bad width"))?,
                    h.parse().map_err(|_| err("bad height"))?,
                )
            }
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
            "multi" => Action::Multi(match rest()? {
                "trigout" => MultiMode::TriggerOut,
                "pfout" => MultiMode::PassFailOut,
                "trigin" => MultiMode::TriggerIn,
                _ => return Err(err("bad multi mode")),
            }),
            "pfout" => Action::PfOut(rest()? == "1"),
            "cursor" => Action::Cursor {
                amp: match rest()? {
                    "time" => false,
                    "amp" => true,
                    _ => return Err(err("bad cursor kind")),
                },
                on: rest()? == "on",
            },
            "stats" => Action::Stats(rest()?.parse().map_err(|_| err("bad slot"))?),
            "statsreset" => Action::StatsReset,
            "fft" => Action::Fft(rest()? == "on"),
            "fftsrc" => Action::FftSrc(rest()?.parse().map_err(|_| err("bad slot"))?),
            "fftwnd" => Action::FftWnd(parse_window(rest()?).map_err(|_| err("bad window"))?),
            "pf" => Action::Pf(rest()? == "on"),
            "pfsrc" => Action::PfSrc(rest()?.parse().map_err(|_| err("bad slot"))?),
            "pftol" => Action::PfTol(
                rest()?.parse().map_err(|_| err("bad h"))?,
                rest()?.parse().map_err(|_| err("bad v"))?,
            ),
            "pfcapture" => Action::PfCapture,
            "pfreset" => Action::PfReset,
            "menu" => Action::Menu(match rest()? {
                "none" => None,
                "channel" => {
                    let ch: usize = rest()?.parse().map_err(|_| err("bad ch"))?;
                    Some(Menu::Channel(ch))
                }
                "horizontal" => Some(Menu::Horizontal),
                "trigger" => Some(Menu::Trigger),
                "acquire" => Some(Menu::Acquire),
                "display" => Some(Menu::Display),
                "measure" => Some(Menu::Measure),
                "math" => Some(Menu::Math),
                "cursor" => Some(Menu::Cursor),
                "utility" => Some(Menu::Utility),
                _ => return Err(err("bad menu")),
            }),
            "layout" => Action::Layout(rest()?.to_string()),
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
    layout: Res<crate::ui::layout::Layout>,
    mut windows: Query<&mut bevy::window::Window>,
    mut script: ResMut<Script>,
    mut commands: Commands,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut math: ResMut<MathState>,
    mut menus: ResMut<MenuState>,
    mut cur: ResMut<CursorState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
    mut pf: ResMut<PfState>,
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
                let _ = link.sup.commands.send(Command::Stimulus(name.clone()));
                link.stimulus = name;
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
            Action::TrigPulse {
                ch,
                cond,
                width,
                sweep,
            } => {
                link.config.trigger.source = ch;
                link.config.trigger.kind = TriggerKind::Pulse {
                    condition: cond,
                    width,
                };
                link.config.trigger.sweep = sweep;
                link.dirty = true;
            }
            Action::TrigSlope {
                ch,
                cond,
                width,
                upper,
                lower,
                sweep,
            } => {
                link.config.trigger.source = ch;
                link.config.trigger.kind = TriggerKind::Slope {
                    condition: cond,
                    width,
                    upper,
                    lower,
                };
                link.config.trigger.sweep = sweep;
                link.dirty = true;
            }
            Action::TrigVideo { sync, line, sweep } => {
                link.config.trigger.kind = TriggerKind::Video { sync, line };
                link.config.trigger.sweep = sweep;
                link.dirty = true;
            }
            Action::Holdoff(h) => {
                link.config.trigger.holdoff = h;
                link.dirty = true;
            }
            Action::AutoSet => {
                let _ = link.sup.commands.send(Command::AutoSet);
            }
            Action::Force => {
                let _ = link.sup.commands.send(Command::ForceTrigger);
            }
            Action::Acq(a) => {
                link.config.acq = a;
                link.dirty = true;
            }
            Action::Mode(m) => phosphor.mode = m,
            Action::Persist(p) => phosphor.persistence = p,
            Action::Gain(g) => phosphor.gain = g,
            Action::Crt(on) => phosphor.crt = on,
            Action::Select(ch) => link.selected = ch.min(1),
            Action::Guides(on) => meas.guides = on,
            Action::Markers(on) => cur.markers = on,
            Action::PaletteSet(p) => phosphor.palette = p,
            Action::WindowSize(w, h) => {
                if let Ok(mut window) = windows.single_mut() {
                    window.resolution.set(w, h);
                }
            }
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
            Action::Multi(m) => {
                link.multi = m;
                let _ = link.sup.commands.send(Command::Multi(m));
            }
            Action::PfOut(level) => {
                let _ = link.sup.commands.send(Command::PassFail(level));
            }
            Action::Cursor { amp, on } => {
                if amp {
                    cur.amp_on = on;
                } else {
                    cur.time_on = on;
                }
            }
            Action::Stats(slot) => meas.stats_slot = slot,
            Action::StatsReset => meas.reset_stats(),
            Action::Fft(on) => fft.enabled = on,
            Action::FftSrc(slot) => fft.source = slot,
            Action::FftWnd(w) => fft.window = w,
            Action::Pf(on) => pf.enabled = on,
            Action::PfSrc(slot) => {
                pf.source_slot = slot;
                pf.mask = None;
            }
            Action::PfTol(h, v) => {
                pf.h_div = h;
                pf.v_div = v;
            }
            Action::PfCapture => {
                let raw: Option<Vec<i8>> = if pf.source_slot < 2 {
                    link.latest
                        .as_ref()
                        .and_then(|f| f.channels.iter().find(|c| c.ch == pf.source_slot))
                        .map(|c| c.raw.clone())
                } else {
                    math.trace.as_ref().map(|c| c.raw.clone())
                };
                if let Some(raw) = raw {
                    pf.mask = Some(crate::derived::build_pf_mask(&raw, pf.h_div, pf.v_div));
                    pf.pass = 0;
                    pf.fail = 0;
                }
            }
            Action::PfReset => {
                pf.pass = 0;
                pf.fail = 0;
            }
            Action::Menu(m) => menus.open = m,
            Action::Layout(path) => {
                let open = menus.open.map(menu_name);
                let json = dump_json(&layout, open);
                match std::fs::write(&path, json) {
                    Ok(()) => info!("script: wrote layout {path}"),
                    Err(e) => error!("script: cannot write {path}: {e}"),
                }
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

/// Stable menu names for the `layout` JSON.
fn menu_name(m: Menu) -> &'static str {
    match m {
        Menu::Channel(0) => "channel0",
        Menu::Channel(1) => "channel1",
        Menu::Channel(_) => "channel",
        Menu::Horizontal => "horizontal",
        Menu::Trigger => "trigger",
        Menu::Acquire => "acquire",
        Menu::Display => "display",
        Menu::Measure => "measure",
        Menu::Math => "math",
        Menu::Cursor => "cursor",
        Menu::Utility => "utility",
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
