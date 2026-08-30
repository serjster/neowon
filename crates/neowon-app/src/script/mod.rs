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
//! timebase <s/div>                    # primary horizontal control
//! zoom <h|v> <in|out>                 # h = horizontal (see hzoom), v = V/div
//! hzoom <in|out>                      # time base, or zoom window when on
//! zoomwin <on|off>                    # zoom (delayed sweep) window
//! hview <centre> <span>               # zoom window, fractions of record
//! pan <left|right|up|down>            # window (h) / offset (v), one step
//! home                                # default zoom + centre position
//! acq <sample|peak|avg4|avg16|avg64>
//! autopeak <on|off>                    # auto peak detect at slow time bases
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
//! record <0|1> / recordclear
//! export <wav|csv|raw> <path>           # write the recording
//! capsave <path.nwc> / capload <path>   # capture files (.nwc, vendor .cap)
//! history <idx|prev|next|live>          # scrub the recorded ring
//! refsave <ch> / ref <on|off> / refclear  # ghost reference traces
//! sessionsave <path> / sessionload <path> # setup files (are scripts)
//! trigpos <fraction>                    # horizontal trigger position
//! waterfall <on|off>                    # realtime spectrogram window
//! viz <off|terrain|tunnel|phase|xytime> # 3D signal viewport
//! palette <phosphor|thermal|green>
//! window <W>x<H>                        # resize (layout tests)
//! uiscale <factor>                      # egui zoom factor (hi-DPI screens)
//! layout <path.json>                    # named-ROI map + open menu
//! shot <path> [x y w h]                 # plot region; .png or .ppm
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

/// Later-phase resources bundled into one system param (Bevy caps
/// systems at 16 parameters).
type ExtraState<'w> = (
    ResMut<'w, crate::record::History>,
    ResMut<'w, crate::refs::RefState>,
    ResMut<'w, crate::viz::waterfall::WaterfallState>,
    ResMut<'w, crate::viz::three_d::Viz3dState>,
    ResMut<'w, crate::effects::Effects>,
    Res<'w, crate::ui::layout::UiRects>,
    ResMut<'w, crate::ui::UiScale>,
    ResMut<'w, crate::autopeak::AutoPeak>,
);

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
    Zoom {
        horiz: bool,
        inward: bool,
    },
    HZoom {
        inward: bool,
    },
    HView(f64, f64),
    /// Time base in seconds per division (the primary horizontal control).
    Timebase(f64),
    /// Zoom (delayed-sweep) window on/off.
    ZoomWin(bool),
    Pan(crate::view::Pan),
    Home,
    Acq(AcqMode),
    /// Automatic peak detect at slow time bases on/off.
    AutoPeak(bool),
    Mode(TraceMode),
    Persist(Persistence),
    Gain(f32),
    Crt(bool),
    Select(usize),
    Guides(bool),
    Markers(bool),
    Record(bool),
    RecordClear,
    Export(String, String),
    PaletteSet(crate::gpu::Palette),
    WindowSize(f32, f32),
    UiScaleSet(f32),
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
    TrigPos(f64),
    HistoryIdx(usize),
    HistoryStep(i64),
    HistoryLive,
    CapSave(String),
    CapLoad(String),
    RefSave(usize),
    RefShow(bool),
    RefClear,
    Waterfall(bool),
    Viz(crate::viz::three_d::Viz3d),
    Effect(Option<String>),
    EffectReload,
    SessionSave(String),
    SessionLoad(String),
    Quit,
}

#[derive(Resource, Default)]
pub struct Script {
    /// (due time in seconds since startup, action)
    queue: VecDeque<(f64, Action)>,
}

impl Script {
    /// UI-injected action: due immediately, applied on the next
    /// `run_script` pass — buttons and scripts share one code path.
    pub fn inject(&mut self, action: Action) {
        self.queue.push_back((0.0, action));
    }

    /// Control-socket injection with an explicit due time (supports
    /// `wait` inside a remotely submitted script fragment).
    pub fn inject_at(&mut self, due: f64, action: Action) {
        self.queue.push_back((due, action));
    }
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

mod grammar;

pub(crate) use grammar::parse;

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
    mut rec: ResMut<crate::record::Recorder>,
    mut shaders: ResMut<Assets<Shader>>,
    mut ext: ExtraState,
) {
    let (hist, refs, wf, viz3d, fx) = (&mut ext.0, &mut ext.1, &mut ext.2, &mut ext.3, &mut ext.4);
    let rects = &ext.5;
    let ui_scale = &mut ext.6;
    let autopeak = &mut ext.7;
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
            Action::Zoom { horiz, inward } => {
                if horiz {
                    let anchor = phosphor.hview.0;
                    crate::view::hzoom(&mut link, &mut phosphor, anchor, inward);
                } else {
                    let sel = link.selected.min(1);
                    crate::view::zoom_channel(&mut link, sel, inward);
                }
            }
            Action::HZoom { inward } => {
                let anchor = phosphor.hview.0;
                crate::view::hzoom(&mut link, &mut phosphor, anchor, inward)
            }
            Action::Timebase(s_div) => crate::view::set_timebase(&mut link, s_div),
            Action::ZoomWin(on) => crate::view::set_zoom(&mut phosphor, on),
            Action::HView(center, span) => {
                phosphor.hview = crate::view::hview_clamp(center, span);
            }
            Action::Pan(dir) => crate::view::pan(&mut link, &mut phosphor, dir),
            Action::Home => crate::view::home(&mut link, &mut phosphor),
            Action::Acq(a) => {
                // The user's choice, not the auto-peak rule's: sessions
                // persist this and the rule restores it on release.
                autopeak.set_user(a);
                link.config.acq = a;
                link.dirty = true;
            }
            Action::AutoPeak(on) => {
                autopeak.on = on;
                if !on && autopeak.engaged {
                    autopeak.engaged = false;
                    link.config.acq = autopeak.user_acq;
                    link.dirty = true;
                }
            }
            Action::Mode(m) => phosphor.mode = m,
            Action::Persist(p) => phosphor.persistence = p,
            Action::Gain(g) => phosphor.gain = g,
            Action::Crt(on) => phosphor.crt = on,
            Action::Select(ch) => link.selected = ch.min(1),
            Action::Guides(on) => meas.guides = on,
            Action::Markers(on) => cur.markers = on,
            Action::Record(on) => rec.on = on,
            Action::RecordClear => rec.clear(),
            Action::Export(kind, path) => {
                let path = std::path::PathBuf::from(&path);
                let result = match kind.as_str() {
                    "wav" => rec
                        .export_wav(&path)
                        .map(|_| vec![path.display().to_string()]),
                    "csv" => rec
                        .export_csv(&path)
                        .map(|_| vec![path.display().to_string()]),
                    "raw" => rec.export_raw(&path),
                    _ => Err(std::io::Error::other("bad export kind")),
                };
                match result {
                    Ok(files) => info!("script: exported {}", files.join(", ")),
                    Err(e) => error!("script: export failed: {e}"),
                }
            }
            Action::PaletteSet(p) => phosphor.palette = p,
            Action::WindowSize(w, h) => {
                if let Ok(mut window) = windows.single_mut() {
                    window.resolution.set(w, h);
                }
            }
            Action::UiScaleSet(s) => {
                ui_scale.0 = s.clamp(
                    crate::ui::layout::UI_SCALE_RANGE.0,
                    crate::ui::layout::UI_SCALE_RANGE.1,
                );
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
            Action::Menu(m) => menus.set_exclusive(m),
            Action::Layout(path) => {
                let names: Vec<&str> = menus.open_list().iter().map(|m| menu_name(*m)).collect();
                let open = (!names.is_empty()).then(|| names.join(","));
                let json = dump_json(&layout, open.as_deref(), rects);
                match std::fs::write(&path, json) {
                    Ok(()) => info!("script: wrote layout {path}"),
                    Err(e) => error!("script: cannot write {path}: {e}"),
                }
            }
            Action::Shot { path, roi } => {
                PENDING_SHOTS.fetch_add(1, Ordering::SeqCst);
                // WYSIWYG: capture the effect output while one is active.
                let shot_source = if fx.active.is_some() {
                    fx.output.clone()
                } else {
                    phosphor.display_image.clone()
                };
                commands
                    .spawn(bevy::render::gpu_readback::Readback::texture(shot_source))
                    .observe(
                        move |event: On<bevy::render::gpu_readback::ReadbackComplete>,
                              mut cmd: Commands| {
                            write_shot(&event.data, &path, roi);
                            PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
                            cmd.entity(event.entity).despawn();
                        },
                    );
            }
            Action::TrigPos(p) => {
                link.config.position = p.clamp(0.0, 1.0);
                link.dirty = true;
            }
            Action::HistoryIdx(i) => hist.show(&mut link, &rec, i),
            Action::HistoryStep(d) => {
                let n = rec.frames.len();
                if n > 0 {
                    let at = hist.active.unwrap_or(n - 1) as i64;
                    hist.show(&mut link, &rec, (at + d).clamp(0, n as i64 - 1) as usize);
                }
            }
            Action::HistoryLive => hist.live(&mut link),
            Action::CapSave(path) => match rec.save_nwc(std::path::Path::new(&path)) {
                Ok(()) => {
                    info!("script: saved {} frames to {path}", rec.frames.len());
                    rec.last_export = Some(path);
                }
                Err(e) => error!("script: capsave failed: {e}"),
            },
            Action::CapLoad(path) => {
                // A bare filename resolves against the export directory.
                let p = std::path::PathBuf::from(&path);
                let p = if p.is_relative() && !p.exists() {
                    crate::record::export_dir().join(p)
                } else {
                    p
                };
                match rec.load_capture(&p) {
                    Ok(n) => {
                        info!("script: loaded {n} frames from {path}");
                        hist.show(&mut link, &rec, 0);
                    }
                    Err(e) => error!("script: capload failed: {e}"),
                }
            }
            Action::RefSave(ch) => {
                if let Some(frame) = link.latest.clone() {
                    refs.capture(&frame, ch);
                }
            }
            Action::RefShow(on) => refs.show = on,
            Action::RefClear => refs.clear(),
            Action::Waterfall(on) => {
                wf.on = on;
                if on {
                    // The waterfall consumes the per-record spectrum.
                    fft.enabled = true;
                }
            }
            Action::Viz(mode) => {
                viz3d.mode = mode;
                if mode == crate::viz::three_d::Viz3d::Terrain {
                    fft.enabled = true;
                }
            }
            Action::Effect(name) => {
                crate::effects::activate(fx, &mut shaders, name.as_deref());
            }
            Action::EffectReload => {
                crate::effects::scan(fx);
                let current = fx.active.clone();
                crate::effects::activate(fx, &mut shaders, current.as_deref());
            }
            Action::SessionSave(path) => {
                let text = crate::session::emit(
                    autopeak, &link, &phosphor, &math, &meas, &fft, &cur, &pf, wf, viz3d, fx,
                );
                match std::fs::write(&path, text) {
                    Ok(()) => info!("script: saved session to {path}"),
                    Err(e) => error!("script: sessionsave failed: {e}"),
                }
            }
            Action::SessionLoad(path) => match std::fs::read_to_string(&path) {
                Ok(text) => match parse(&text) {
                    Ok(actions) => {
                        info!("script: session {path}: {} actions", actions.len());
                        // Splice at the FRONT so the session applies before
                        // anything already queued (a later queue entry must
                        // observe the loaded state, not race it).
                        for (dt, a) in actions.into_iter().rev() {
                            script.queue.push_front((now + dt, a));
                        }
                    }
                    Err(e) => error!("script: session parse error: {e}"),
                },
                Err(e) => error!("script: sessionload failed: {e}"),
            },
            Action::Quit => {
                if PENDING_SHOTS.load(Ordering::SeqCst) > 0 {
                    // Re-arm shortly; shots still in flight.
                    script.queue.push_front((now + 0.05, Action::Quit));
                    return;
                }
                info!("script: done");
                // Graceful shutdown: `process::exit` races the render thread
                // through the driver's atexit teardown (SIGSEGV in
                // libnvidia-glcore during swapchain present). A world
                // command keeps `run_script` under the 16-param system cap.
                commands.queue(|world: &mut World| {
                    world.write_message(AppExit::Success);
                });
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
        Menu::Record => "record",
    }
}

/// Write a (possibly cropped) region of the plot texture — PNG when the
/// path ends `.png`, binary PPM otherwise. Readback rows are 256-byte
/// aligned; the stride strips that.
fn write_shot(rgba: &[u8], path: &str, roi: Option<(u32, u32, u32, u32)>) {
    let stride = rgba.len() / PLOT_H as usize;
    let (x0, y0, w, h) = roi.unwrap_or((0, 0, PLOT_W, PLOT_H));
    let (x0, y0) = (x0.min(PLOT_W - 1), y0.min(PLOT_H - 1));
    let w = w.min(PLOT_W - x0);
    let h = h.min(PLOT_H - y0);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in y0..y0 + h {
        let base = row as usize * stride + x0 as usize * 4;
        for px in rgba[base..base + w as usize * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
    }
    let result = if path.ends_with(".png") {
        write_png(path, w, h, &rgb)
    } else {
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend_from_slice(&rgb);
        std::fs::write(path, &ppm)
    };
    match result {
        Ok(()) => info!("script: wrote {path} ({w}x{h})"),
        Err(e) => error!("script: cannot write {path}: {e}"),
    }
}

fn write_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(rgb)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}
