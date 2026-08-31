//! Script grammar: the parse table that turns one text line into an
//! `Action`. Split out of `script/mod.rs` (the runtime) so the file that
//! grows with every new control stays inside the repo's size budget.

use std::collections::VecDeque;

use neowon_backend::MultiMode;
use neowon_core::{AcqMode, Coupling, PulseCondition, Slope, Sweep, VideoSync};
use neowon_dsp::{MathOp, Window};

use super::Action;
use crate::gpu::{Persistence, TraceMode};
use crate::ui::Menu;

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

pub(crate) fn parse(text: &str) -> Result<VecDeque<(f64, Action)>, String> {
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
            "zoom" => Action::Zoom {
                horiz: match rest()? {
                    "h" => true,
                    "v" => false,
                    _ => return Err(err("bad zoom axis")),
                },
                inward: match rest()? {
                    "in" => true,
                    "out" => false,
                    _ => return Err(err("bad zoom direction")),
                },
            },
            "hzoom" => Action::HZoom {
                inward: match rest()? {
                    "in" => true,
                    "out" => false,
                    _ => return Err(err("bad zoom direction")),
                },
            },
            "hview" => Action::HView(
                rest()?.parse().map_err(|_| err("bad centre"))?,
                rest()?.parse().map_err(|_| err("bad span"))?,
            ),
            "timebase" => Action::Timebase(rest()?.parse().map_err(|_| err("bad s/div"))?),
            "zoomwin" => Action::ZoomWin(rest()? == "on"),
            "deep" => Action::Deep(rest()? == "on"),
            "deepspan" => Action::DeepSpan(rest()?.parse().map_err(|_| err("bad seconds"))?),
            "decode" => Action::Decode(
                crate::decode::Protocol::parse(rest()?).ok_or_else(|| err("bad protocol"))?,
            ),
            "decodeline" => Action::DecodeLine(
                rest()?.parse().map_err(|_| err("bad line"))?,
                rest()?.parse().map_err(|_| err("bad channel"))?,
            ),
            "decodebaud" => Action::DecodeBaud(rest()?.parse().map_err(|_| err("bad baud"))?),
            "pan" => Action::Pan(match rest()? {
                "left" => crate::view::Pan::Left,
                "right" => crate::view::Pan::Right,
                "up" => crate::view::Pan::Up,
                "down" => crate::view::Pan::Down,
                _ => return Err(err("bad pan direction")),
            }),
            "home" => Action::Home,
            "autopeak" => Action::AutoPeak(rest()? == "on"),
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
            "record" => Action::Record(rest()? == "1"),
            "recordclear" => Action::RecordClear,
            "export" => Action::Export(rest()?.to_string(), rest()?.to_string()),
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
                "record" => Some(Menu::Record),
                "decode" => Some(Menu::Decode),
                _ => return Err(err("bad menu")),
            }),
            "uiscale" => Action::UiScaleSet(rest()?.parse().map_err(|_| err("bad scale"))?),
            "scrollback" => Action::Scrollback(rest()?.parse().map_err(|_| err("bad bytes"))?),
            "settings" => Action::SettingsOpen(rest()? == "on"),
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
            "trigpos" => Action::TrigPos(rest()?.parse().map_err(|_| err("bad fraction"))?),
            "history" => match rest()? {
                "live" => Action::HistoryLive,
                "prev" => Action::HistoryStep(-1),
                "next" => Action::HistoryStep(1),
                n => Action::HistoryIdx(n.parse().map_err(|_| err("bad frame index"))?),
            },
            "capsave" => Action::CapSave(rest()?.to_string()),
            "capload" => Action::CapLoad(rest()?.to_string()),
            "refsave" => Action::RefSave(rest()?.parse().map_err(|_| err("bad ch"))?),
            "waterfall" => Action::Waterfall(rest()? == "on"),
            "effect" => Action::Effect(match rest()? {
                "off" => None,
                name => Some(name.to_string()),
            }),
            "effectreload" => Action::EffectReload,
            "viz" => Action::Viz(
                crate::viz::three_d::Viz3d::parse(rest()?).ok_or_else(|| err("bad viz mode"))?,
            ),
            "ref" => Action::RefShow(rest()? == "on"),
            "refclear" => Action::RefClear,
            "sessionsave" => Action::SessionSave(rest()?.to_string()),
            "sessionload" => Action::SessionLoad(rest()?.to_string()),
            "quit" => Action::Quit,
            _ => return Err(err("unknown action")),
        };
        out.push_back((t, action));
    }
    Ok(out)
}
