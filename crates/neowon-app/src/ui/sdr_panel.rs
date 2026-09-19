//! The front panel in the SDR workspace: the radio's keys where the scope's
//! sit (D12). Tune steps and Follow, span presets, the demodulator, run,
//! and the views. Every key injects the script action a script would use.

use bevy_egui::egui;

use super::frontpanel::{color_key, group, key};
use super::layout::{Layout, Roi};
use super::sdr_view::inject;
use super::widgets::{RUN_COLOR, STOP_COLOR};
use crate::refmap::{RefMap, RefMapAction};
use crate::script::{Action, Script};
use crate::sdr::{SdrAction, SdrState};

const SPANS: [(&str, f64); 4] = [("Full", 0.0), ("1M", 1e6), ("200k", 200e3), ("50k", 50e3)];

pub fn show(
    ctx: &egui::Context,
    l: &Layout,
    sdr: &SdrState,
    rm: &RefMap,
    script: &mut Script,
) -> egui::Rect {
    let rect = l.points(Roi::FrontPanel.rect(l));
    let resp = egui::Area::new("frontpanel".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                group(ui, "TUNE", |ui| {
                    for (label, step) in
                        [("−1M", -1e6), ("−100k", -1e5), ("+100k", 1e5), ("+1M", 1e6)]
                    {
                        if key(ui, label, false) {
                            inject(script, SdrAction::Step(step));
                        }
                    }
                    if key(ui, "Follow", sdr.follow) {
                        inject(script, SdrAction::Follow(!sdr.follow));
                    }
                });
                group(ui, "SPAN", |ui| {
                    for (label, span) in SPANS {
                        if key(ui, label, sdr.span_hz == span) {
                            inject(script, SdrAction::Span(span));
                            inject(script, SdrAction::Pan(0.0));
                        }
                    }
                });
                group(ui, "DEMOD", |ui| {
                    if key(ui, "Off", sdr.demod.is_none()) {
                        inject(script, SdrAction::Demod(None));
                    }
                    for m in neowon_dsp::DemodMode::ALL {
                        if key(ui, m.label(), sdr.demod == Some(m)) {
                            inject(script, SdrAction::Demod(Some(m)));
                        }
                    }
                    if key(ui, "Mute", sdr.mute) {
                        inject(script, SdrAction::Mute(!sdr.mute));
                    }
                });
                group(ui, "RUN", |ui| {
                    let running = sdr.config.running;
                    let (label, color) = if running {
                        ("Run/Stop", RUN_COLOR)
                    } else {
                        ("Stopped", STOP_COLOR)
                    };
                    if color_key(ui, label.into(), color, true) {
                        inject(script, SdrAction::Run(!running));
                    }
                });
                group(ui, "VIEW", |ui| {
                    if key(ui, "RF map", rm.window) {
                        script.inject(Action::RefMap(RefMapAction::Window(!rm.window)));
                    }
                    if key(ui, "Bands", rm.strip) {
                        script.inject(Action::RefMap(RefMapAction::Strip(!rm.strip)));
                    }
                    if key(ui, "Minimap", rm.mini) {
                        script.inject(Action::RefMap(RefMapAction::Mini(!rm.mini)));
                    }
                    if key(ui, "Detect", sdr.detect_on) {
                        inject(script, SdrAction::Detect(!sdr.detect_on));
                    }
                    if key(ui, "Analyse", sdr.analyse_on) {
                        inject(script, SdrAction::Analyse(!sdr.analyse_on));
                    }
                    if key(ui, "Catalog", false) {
                        script.inject(Action::Catalog(crate::catalog::CatalogAction::Window(true)));
                    }
                });
            });
        });
    l.pixels(resp.response.rect)
}
