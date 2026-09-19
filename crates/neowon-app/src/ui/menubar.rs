//! The application bar: drop-down menus on the left, ambient status on the
//! right. Instrument *function* menus live in the always-visible dock
//! (menu.rs) — these are the app-level ones: files, which views are showing,
//! and settings that are not part of an instrument setup.

use bevy_egui::egui;
use neowon_core::Sweep;

use crate::Link;
use crate::derived::fmt_si;

use super::layout::{Layout, Roi};
use super::menu::MenuState;
use super::widgets::{RUN_COLOR, STOP_COLOR, WAIT_COLOR};

/// Run/stop/wait classification for the badge.
pub fn run_state(link: &Link, now: f64) -> (&'static str, egui::Color32) {
    if !link.config.running {
        return ("STOP", STOP_COLOR);
    }
    let starved = matches!(link.config.trigger.sweep, Sweep::Normal | Sweep::Single)
        && now - link.last_frame_at > 0.5;
    if starved {
        ("WAIT", WAIT_COLOR)
    } else {
        ("RUN", RUN_COLOR)
    }
}

pub fn show(
    ctx: &egui::Context,
    l: &Layout,
    link: &mut Link,
    now: f64,
    deep: &crate::deep::DeepView,
    bar: &mut BarState<'_>,
) -> egui::Rect {
    let rect = l.points(Roi::MenuBar.rect(l));
    let resp = egui::Area::new("menubar".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                menus(ui, bar);
                ui.separator();
                if bar.sdr.active {
                    return sdr_status(ui, bar.sdr, bar.refmap, &link.status);
                }
                // Run state badge (manual 8.5: Run = yellow, Stop = red).
                let (label, color) = run_state(link, now);
                badge(ui, label, color);
                let record_len = link.caps.as_ref().map(|c| c.record_len()).unwrap_or(5000);
                let per_div = record_len as f64 / link.config.sample_rate / 10.0;
                // While the timeline is on, the on-screen time/div is the
                // window's, not the record's — show both rather than let the
                // chrome claim a time base the display is not using.
                let text = if deep.on {
                    format!(
                        "{}/div view   {}/div acq   {}",
                        fmt_si(deep.seconds_per_div(), "s"),
                        fmt_si(per_div, "s"),
                        fmt_si(link.config.sample_rate, "S/s"),
                    )
                } else {
                    format!(
                        "{}/div   {}",
                        fmt_si(per_div, "s"),
                        fmt_si(link.config.sample_rate, "S/s"),
                    )
                };
                ui.label(egui::RichText::new(text).monospace());
                if deep.on {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(112.0, 20.0), egui::Sense::hover());
                    ui.painter().rect(
                        r,
                        4.0,
                        egui::Color32::from_rgb(40, 44, 54),
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 140)),
                        egui::StrokeKind::Middle,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("TIMELINE {:.0}%", deep.lost() * 100.0),
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_rgb(80, 200, 140),
                    );
                    resp.on_hover_text(
                        "Showing the acquisition timeline at full sample rate. \
                         The percentage is how much of the window the instrument \
                         was not acquiring in; those columns are marked in red.",
                    );
                }
                // Slow time bases run the instrument in roll mode, where the
                // record fills progressively and the trigger is not used —
                // scopes always say so on screen.
                if crate::view::is_roll(link.config.sample_rate) {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(52.0, 20.0), egui::Sense::hover());
                    ui.painter().rect(
                        r,
                        4.0,
                        egui::Color32::from_rgb(40, 44, 54),
                        egui::Stroke::new(1.0, WAIT_COLOR),
                        egui::StrokeKind::Middle,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "ROLL",
                        egui::FontId::proportional(11.0),
                        WAIT_COLOR,
                    );
                    resp.on_hover_text(
                        "Roll mode: at this time base the instrument streams \
                         the record progressively and the trigger is not used.",
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Fixed 8-char field: the counter widening a digit must
                    // not nudge the device label beside it.
                    let n = format!("#{}", link.frames_seen);
                    ui.label(egui::RichText::new(format!("{n:>8}")).monospace())
                        .on_hover_text(
                            "Acquisitions since the app started. It should climb \
                             steadily; a stalled counter means the trigger is \
                             starving or the instrument stopped.",
                        );
                    if let Some(caps) = &link.caps {
                        ui.weak(format!("{} · {}", caps.name, caps.serial));
                    } else {
                        ui.weak(link.status.clone());
                    }
                });
            });
        });
    l.pixels(resp.response.rect)
}

fn badge(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 22.0), egui::Sense::hover());
    ui.painter()
        .rect(r, 4.0, color, egui::Stroke::NONE, egui::StrokeKind::Middle);
    ui.painter().text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        egui::Color32::BLACK,
    );
}

/// The bar's readouts in SDR mode: run state, tuning, the band it is in,
/// and the instrument.
fn sdr_status(
    ui: &mut egui::Ui,
    sdr: &crate::sdr::SdrState,
    refmap: &crate::refmap::RefMap,
    status: &str,
) {
    let c = &sdr.config;
    if c.running {
        badge(ui, "RUN", RUN_COLOR);
    } else {
        badge(ui, "STOP", STOP_COLOR);
    }
    let gain = match c.gain {
        neowon_backend::SdrGain::Auto => "AGC".to_string(),
        neowon_backend::SdrGain::Manual(db) => format!("{db:.1} dB"),
    };
    ui.label(
        egui::RichText::new(format!(
            "Tuned {:.4} MHz   {}   {gain}",
            sdr.tuned_hz / 1e6,
            fmt_si(c.sample_rate, "S/s")
        ))
        .monospace(),
    );
    // The band the tuned frequency is in (the most specific name), like
    // SDR++'s band overlay, but where it is always in view.
    let bands = refmap.at(sdr.tuned_hz);
    match bands.first() {
        Some(b) => {
            let all: Vec<String> = bands
                .iter()
                .map(|b| {
                    format!(
                        "{} · {} · {}",
                        b.name,
                        crate::refmap::fmt_range(b.lo_hz, b.hi_hz),
                        b.kind
                    )
                })
                .collect();
            ui.label(egui::RichText::new(&b.name).color(crate::refmap::colour(&b.kind)))
                .on_hover_text(format!(
                    "{}\n(band plan: {})",
                    all.join("\n"),
                    refmap.stem()
                ));
        }
        None => {
            ui.weak("no allocation")
                .on_hover_text(format!("band plan: {}", refmap.stem()));
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let n = format!("#{}", sdr.frames_seen);
        ui.label(egui::RichText::new(format!("{n:>8}")).monospace())
            .on_hover_text("IQ frames since the SDR connected.");
        match &sdr.caps {
            Some(c) => ui.weak(format!("{} · {}", c.name, c.tuner)),
            None => ui.weak(status),
        };
    });
}

/// What the menus need to reach. Bundled so the bar keeps one parameter.
pub struct BarState<'a> {
    pub settings: &'a mut crate::ui::settings::Settings,
    pub script: &'a mut crate::script::Script,
    pub menus: &'a mut MenuState,
    pub fft: &'a mut crate::derived::FftState,
    pub wf: &'a mut crate::viz::waterfall::WaterfallState,
    pub viz: &'a mut crate::viz::three_d::Viz3dState,
    pub sdr: &'a crate::sdr::SdrState,
    pub refmap: &'a crate::refmap::RefMap,
}

/// The workspace switch: SCOPE | SDR, first in the bar, the active one
/// lit. Ctrl/⌘+1 and +2 do the same. One device claim at a time, so a
/// switch releases one instrument and claims the other (`instrument`).
fn mode_toggle(ui: &mut egui::Ui, bar: &mut BarState<'_>) {
    use crate::script::Action;
    let active = bar.sdr.active;
    let key = |ui: &mut egui::Ui, k| ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k));
    let mut want = None;
    if key(ui, egui::Key::Num1) {
        want = Some(false);
    }
    if key(ui, egui::Key::Num2) {
        want = Some(true);
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (label, sdr, keys, what) in [
            ("SCOPE", false, "⌘/Ctrl+1", "oscilloscope"),
            ("SDR", true, "⌘/Ctrl+2", "software-defined radio"),
        ] {
            let on = active == sdr;
            let text = egui::RichText::new(label).strong().color(if on {
                egui::Color32::BLACK
            } else {
                egui::Color32::from_gray(200)
            });
            let b = egui::Button::new(text)
                .fill(if on {
                    egui::Color32::from_rgb(120, 200, 255)
                } else {
                    egui::Color32::from_rgb(34, 37, 44)
                })
                .min_size(egui::vec2(62.0, 22.0));
            if ui
                .add(b)
                .on_hover_text(format!("{what} workspace ({keys})"))
                .clicked()
            {
                want = Some(sdr);
            }
        }
    });
    if let Some(sdr) = want.filter(|&s| s != active) {
        bar.script
            .inject(Action::Sdr(crate::sdr::SdrAction::Instrument(sdr)));
    }
}

/// The drop-downs. Every item routes through a script action where one
/// exists, so the menu and a script take the same path.
fn menus(ui: &mut egui::Ui, bar: &mut BarState<'_>) {
    use crate::script::Action;
    // The menu buttons go straight into the bar's existing row: an
    // `egui::MenuBar` claims the full available width, which pushes the
    // status readouts onto a second row that the fixed-height bar clips.
    {
        mode_toggle(ui, bar);
        ui.separator();
        ui.menu_button("File", |ui| {
            let dir = crate::record::export_dir();
            if ui.button("Save setup…").clicked() {
                let p = dir.join("setup.nws");
                bar.script
                    .inject(Action::SessionSave(p.display().to_string()));
                ui.close();
            }
            if ui.button("Load setup").clicked() {
                let p = dir.join("setup.nws");
                bar.script
                    .inject(Action::SessionLoad(p.display().to_string()));
                ui.close();
            }
            ui.separator();
            if ui.button("Save capture (.nwc)").clicked() {
                let p = dir.join(format!("{}.nwc", crate::record::default_stem()));
                bar.script.inject(Action::CapSave(p.display().to_string()));
                ui.close();
            }
            if ui.button("Export WAV").clicked() {
                let p = dir.join(format!("{}.wav", crate::record::default_stem()));
                bar.script
                    .inject(Action::Export("wav".into(), p.display().to_string()));
                ui.close();
            }
            if ui.button("Export CSV").clicked() {
                let p = dir.join(format!("{}.csv", crate::record::default_stem()));
                bar.script
                    .inject(Action::Export("csv".into(), p.display().to_string()));
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            if bar.sdr.active {
                return sdr_view_menu(ui, bar);
            }
            ui.checkbox(&mut bar.fft.enabled, "Spectrum");
            ui.checkbox(&mut bar.wf.on, "Waterfall");
            let mut viz_on = bar.viz.mode != crate::viz::three_d::Viz3d::Off;
            if ui.checkbox(&mut viz_on, "3D viewport").changed() {
                bar.viz.mode = if viz_on {
                    crate::viz::three_d::Viz3d::Terrain
                } else {
                    crate::viz::three_d::Viz3d::Off
                };
            }
            ui.separator();
            for (label, m) in [
                ("Measurements", crate::ui::Menu::Measure),
                ("Cursors", crate::ui::Menu::Cursor),
                ("Decode", crate::ui::Menu::Decode),
                ("Record / Export", crate::ui::Menu::Record),
            ] {
                let mut on = bar.menus.is_open(m);
                if ui.checkbox(&mut on, label).changed() {
                    bar.menus.toggle(m);
                }
            }
        });
        if ui.button("Settings").clicked() {
            bar.settings.open = !bar.settings.open;
        }
    }
}

/// The View menu in the SDR workspace: the RF reference views and the
/// windows that belong to the radio.
fn sdr_view_menu(ui: &mut egui::Ui, bar: &mut BarState<'_>) {
    use crate::refmap::RefMapAction as R;
    use crate::script::Action;
    let rm = bar.refmap;
    for (label, on, act) in [
        ("RF map", rm.window, R::Window as fn(bool) -> R),
        ("Band strip", rm.strip, R::Strip),
        ("Minimap", rm.mini, R::Mini),
    ] {
        let mut v = on;
        if ui.checkbox(&mut v, label).changed() {
            bar.script.inject(Action::RefMap(act(v)));
        }
    }
    ui.menu_button(format!("Band plan: {}", rm.stem()), |ui| {
        for (stem, plan) in &rm.plans {
            if ui
                .radio(
                    rm.stem() == stem,
                    format!("{stem}  ({})", plan.country_name),
                )
                .clicked()
            {
                bar.script.inject(Action::RefMap(R::Plan(stem.clone())));
                ui.close();
            }
        }
    });
    ui.separator();
    if ui.button("Catalog").clicked() {
        bar.script
            .inject(Action::Catalog(crate::catalog::CatalogAction::Window(true)));
        ui.close();
    }
}
