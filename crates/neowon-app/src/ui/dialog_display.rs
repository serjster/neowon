//! Display dialog — persistence, trace mode, intensity, XY, stimulus (sim).

use bevy_egui::egui;
use neowon_backend::Command;

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::FftState;
use crate::gpu::{Palette, Persistence, Phosphor, TraceMode};
use crate::refs::RefState;
use crate::viz::three_d::{Viz3d, Viz3dState};
use crate::viz::waterfall::WaterfallState;

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    link: &mut Link,
    phosphor: &mut Phosphor,
    cur: &mut CursorState,
    refs: &mut RefState,
    fft: &mut FftState,
    wf: &mut WaterfallState,
    viz: &mut Viz3dState,
    fx: &crate::effects::Effects,
    script: &mut crate::script::Script,
) {
    ui.group(|ui| {
        ui.strong("Display");
        ui.horizontal(|ui| {
            ui.label("Persist");
            for p in Persistence::LADDER {
                if ui
                    .selectable_label(phosphor.persistence == p, p.label())
                    .clicked()
                {
                    phosphor.persistence = p;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Trace");
            for (label, m) in [
                ("Vectors", TraceMode::Vectors),
                ("Dots", TraceMode::Dots),
                ("XY", TraceMode::Xy),
            ] {
                if ui.selectable_label(phosphor.mode == m, label).clicked() {
                    phosphor.mode = m;
                }
            }
        });
        ui.add(egui::Slider::new(&mut phosphor.gain, 0.05..=3.0).text("Intensity"));
        ui.checkbox(&mut phosphor.crt, "CRT screen (halo, scanlines)");
        ui.checkbox(&mut cur.markers, "On-graph handles (trigger, offsets)");
        ui.horizontal(|ui| {
            ui.label("Palette");
            for (label, p) in [
                ("Phosphor", Palette::Phosphor),
                ("Thermal", Palette::Thermal),
                ("Green", Palette::Green),
            ] {
                if ui.selectable_label(phosphor.palette == p, label).clicked() {
                    phosphor.palette = p;
                }
            }
        });
    });

    ui.group(|ui| {
        ui.strong("Visualizations");
        ui.horizontal(|ui| {
            if ui.checkbox(&mut wf.on, "Waterfall").changed() && wf.on {
                fft.enabled = true; // the waterfall consumes the spectrum
            }
            ui.label("3D");
            egui::ComboBox::from_id_salt("viz3d-mode")
                .selected_text(viz.mode.name())
                .show_ui(ui, |ui| {
                    for m in Viz3d::ALL {
                        if ui.selectable_label(viz.mode == m, m.name()).clicked() {
                            viz.mode = m;
                            if m == Viz3d::Terrain {
                                fft.enabled = true;
                            }
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Effect");
            let current = fx.active.as_deref().unwrap_or("off").to_string();
            egui::ComboBox::from_id_salt("user-effect")
                .selected_text(current.clone())
                .show_ui(ui, |ui| {
                    if ui.selectable_label(fx.active.is_none(), "off").clicked() {
                        script.inject(crate::script::Action::Effect(None));
                    }
                    for name in &fx.available {
                        if ui
                            .selectable_label(current == *name, name.clone())
                            .clicked()
                        {
                            script.inject(crate::script::Action::Effect(Some(name.clone())));
                        }
                    }
                });
            if ui.button("Reload").clicked() {
                script.inject(crate::script::Action::EffectReload);
            }
        });
    });

    ui.group(|ui| {
        ui.strong("Reference traces");
        ui.horizontal(|ui| {
            for ch in 0..2 {
                let has_ch = link
                    .latest
                    .as_ref()
                    .is_some_and(|f| f.channels.iter().any(|c| c.ch == ch));
                if ui
                    .add_enabled(has_ch, egui::Button::new(format!("Save CH{}", ch + 1)))
                    .clicked()
                    && let Some(frame) = link.latest.clone()
                {
                    refs.capture(&frame, ch);
                }
            }
            ui.checkbox(&mut refs.show, "Show");
            if ui.button("Clear").clicked() {
                refs.clear();
            }
        });
        if refs.traces.iter().all(Option::is_none) {
            ui.label(egui::RichText::new("no reference saved").weak().small());
        }
    });

    // Stimulus selection exists only on generating backends (the sim); on
    // hardware the control is omitted entirely (reference rule: missing
    // features are removed, not grayed out).
    let is_sim = link
        .caps
        .as_ref()
        .is_some_and(|c| c.serial.starts_with("sim"));
    if is_sim {
        ui.group(|ui| {
            ui.strong("Stimulus");
            egui::ComboBox::from_id_salt("stimulus")
                .selected_text(link.stimulus.clone())
                .show_ui(ui, |ui| {
                    for name in neowon_sim::Scenario::PRESETS {
                        if ui.selectable_label(link.stimulus == name, name).clicked() {
                            link.stimulus = name.into();
                            let _ = link.sup.commands.send(Command::Stimulus(name.into()));
                        }
                    }
                });
        });
    }
}
