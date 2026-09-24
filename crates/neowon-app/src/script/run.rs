//! Script execution: `run_script` pops every due action off the queue and
//! applies it to the app state, the same mutations the UI performs.

use bevy::prelude::*;
use neowon_backend::Command;
use neowon_core::TriggerKind;

use super::{Action, Script, parse, shot};
use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::Phosphor;
use crate::ui::MenuState;
use crate::ui::layout::dump_json;

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
    ResMut<'w, crate::deep::DeepView>,
    ResMut<'w, crate::decode::DecodeState>,
    ResMut<'w, crate::ui::settings::Settings>,
    ResMut<'w, crate::sdr::SdrState>,
    ResMut<'w, crate::catalog::CatalogState>,
    ResMut<'w, crate::uitree::UiTree>,
    ResMut<'w, crate::refmap::RefMap>,
    ResMut<'w, shot::WindowShots>,
);

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
    let deep = &mut ext.8;
    let dec = &mut ext.9;
    let settings = &mut ext.10;
    let shots = &mut ext.15;
    let now = time.elapsed_secs_f64();
    while let Some((due, _)) = script.queue.front() {
        if *due > now {
            return;
        }
        let (_, action) = script.queue.pop_front().unwrap();
        debug!("script: {action:?}");
        match action {
            Action::Sdr(a) => crate::sdr::run(a, &mut ext.11, &mut link),
            Action::RefMap(a) => {
                crate::refmap::run(a, &mut ext.14, &mut ext.11, &mut ext.12, &mut link)
            }
            Action::Catalog(a) => crate::catalog::run(a, &mut ext.12, &ext.11, &mut link),
            Action::Stimulus(name) => {
                // The sim cannot synthesise a DAB ensemble (it may not depend
                // on `neowon-dsp`), so the app composes the `rf-dab` scene and
                // installs it before the backend is asked for the name.
                if name == "rf-dab" {
                    crate::sdr::dab_scene::install();
                }
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
                crate::view::hzoom_timeline(&mut link, &mut phosphor, deep, anchor, inward)
            }
            Action::Decode(p) => dec.protocol = p,
            Action::DecodeLine(line, ch) => {
                if line < dec.channels.len() {
                    dec.channels[line] = ch;
                }
            }
            Action::DecodeBaud(b) => dec.uart.baud = b,
            Action::DeepFollow(f) => deep.follow = f,
            Action::Deep(on) => crate::deep::set_on(deep, &mut phosphor, on),
            Action::DeepSpan(s) => {
                deep.span = s.max(1e-6);
                if !deep.on {
                    crate::deep::set_on(deep, &mut phosphor, true);
                }
            }
            Action::Timebase(s_div) => crate::view::set_timebase(&mut link, s_div),
            Action::ZoomWin(on) => crate::view::set_zoom(&mut phosphor, on),
            Action::HView(center, span) => {
                // Setting a window narrower than the record *is* zooming, so
                // the mode follows the window rather than having to be
                // switched on separately.
                phosphor.hview = crate::view::hview_clamp(center, span);
                phosphor.zoom_on = phosphor.hview.1 < 0.999;
            }
            Action::Pan(dir) => crate::view::pan(&mut link, &mut phosphor, dir),
            Action::Home => {
                // Home means the normal view: the timeline is a mode.
                crate::deep::set_on(deep, &mut phosphor, false);
                crate::view::home(&mut link, &mut phosphor)
            }
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
            Action::Scrollback(b) => rec.budget = b.max(1 << 20),
            Action::SettingsOpen(on) => settings.open = on,
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
                let raw: Option<Vec<f32>> = if pf.source_slot < 2 {
                    link.latest
                        .as_ref()
                        .and_then(|f| f.channels.iter().find(|c| c.ch == pf.source_slot))
                        .map(|c| c.data.clone())
                } else {
                    math.trace.as_ref().map(|c| c.data.clone())
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
            Action::Dock(open) => menus.set_open(open),
            Action::MeasWin(on) => meas.window = on,
            Action::UiTree(path) => {
                ext.13.on = true;
                ext.13.pending.push(path);
            }
            Action::WindowPos(x, y) => {
                if let Ok(mut window) = windows.single_mut() {
                    window.position = bevy::window::WindowPosition::At(IVec2::new(x, y));
                }
            }
            Action::Layout(path) => {
                let names: Vec<&str> = menus.open_list().iter().map(|m| m.name()).collect();
                let open = (!names.is_empty()).then(|| names.join(","));
                let json = dump_json(&layout, open.as_deref(), rects);
                match neowon_core::atomic_file::write(&path, json) {
                    Ok(()) => info!("script: wrote layout {path}"),
                    Err(e) => error!("script: cannot write {path}: {e}"),
                }
            }
            Action::Shot { path, roi } => {
                // The whole window, egui included: what the operator sees.
                shot::window(shots, &mut commands, &path, roi);
            }
            Action::ShotPlot { path, roi } => {
                // WYSIWYG plot: capture the effect output while one is active.
                let source = if fx.active.is_some() {
                    fx.output.clone()
                } else {
                    phosphor.display_image.clone()
                };
                shot::plot(&mut commands, source, path, roi);
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
                    autopeak, deep, rec.budget, &link, &phosphor, &math, &meas, &fft, &cur, &pf,
                    wf, viz3d, fx,
                );
                match neowon_core::atomic_file::write(&path, text) {
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
                if shot::pending() > 0 {
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
