//! Utility dialog — MULTI port, pass/fail engine, spectrum (FFT) controls.

use bevy_egui::egui;
use neowon_backend::{Command, MultiMode};
use neowon_dsp::Window;

use crate::Link;
use crate::derived::{FftState, MathState, PfState, SLOT_NAMES, build_pf_mask};
use crate::script::{Action, Script};

pub fn show(
    ui: &mut egui::Ui,
    link: &mut Link,
    math: &MathState,
    pf: &mut PfState,
    fft: &mut FftState,
    script: &mut Script,
    scale: &crate::ui::UiScale,
) {
    ui.group(|ui| {
        ui.strong("Display scale");
        let mut s = scale.0;
        if ui
            .add(
                egui::Slider::new(
                    &mut s,
                    crate::ui::layout::UI_SCALE_RANGE.0..=crate::ui::layout::UI_SCALE_RANGE.1,
                )
                .step_by(0.25)
                .text("UI scale"),
            )
            .changed()
        {
            script.inject(Action::UiScaleSet(s));
        }
        ui.label(
            egui::RichText::new("for hi-DPI screens the OS does not scale")
                .weak()
                .small(),
        );
    });
    ui.group(|ui| {
        ui.strong("Session");
        ui.horizontal(|ui| {
            let path = crate::record::export_dir().join("setup.nws");
            if ui.button("Save setup").clicked() {
                script.inject(Action::SessionSave(path.display().to_string()));
            }
            let exists = path.exists();
            if ui
                .add_enabled(exists, egui::Button::new("Load setup"))
                .clicked()
            {
                script.inject(Action::SessionLoad(path.display().to_string()));
            }
        });
        ui.label(
            egui::RichText::new("a session file is a neowon script (setup.nws)")
                .weak()
                .small(),
        );
    });
    ui.group(|ui| {
        ui.strong("MULTI port");
        ui.horizontal(|ui| {
            for (m, label) in [
                (MultiMode::TriggerOut, "Trigger out"),
                (MultiMode::PassFailOut, "Pass-fail out"),
                (MultiMode::TriggerIn, "Trigger in"),
            ] {
                if ui.selectable_label(link.multi == m, label).clicked() {
                    link.multi = m;
                    let _ = link.sup.commands.send(Command::Multi(m));
                }
            }
        });
    });

    ui.group(|ui| {
        ui.strong("Pass/Fail");
        ui.horizontal(|ui| {
            ui.checkbox(&mut pf.enabled, "Enabled");
            ui.label("Src");
            for (slot, name) in SLOT_NAMES.iter().enumerate() {
                if ui.selectable_label(pf.source_slot == slot, *name).clicked() {
                    pf.source_slot = slot;
                    pf.mask = None; // reference came from another trace
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("H tol");
            ui.add(
                egui::DragValue::new(&mut pf.h_div)
                    .speed(0.05)
                    .range(0.0..=20.0)
                    .suffix(" div"),
            );
            ui.label("V tol");
            ui.add(
                egui::DragValue::new(&mut pf.v_div)
                    .speed(0.05)
                    .range(0.0..=10.0)
                    .suffix(" div"),
            );
        });
        ui.horizontal(|ui| {
            let can_capture = if pf.source_slot < 2 {
                link.latest
                    .as_ref()
                    .and_then(|f| f.channels.iter().find(|c| c.ch == pf.source_slot))
                    .is_some()
            } else {
                math.trace.is_some()
            };
            if ui
                .add_enabled(can_capture, egui::Button::new("Capture reference"))
                .clicked()
            {
                let raw: Option<Vec<i8>> = if pf.source_slot < 2 {
                    link.latest
                        .as_ref()
                        .and_then(|f| f.channels.iter().find(|c| c.ch == pf.source_slot))
                        .map(|c| c.raw.clone())
                } else {
                    math.trace.as_ref().map(|c| c.raw.clone())
                };
                if let Some(raw) = raw {
                    pf.mask = Some(build_pf_mask(&raw, pf.h_div, pf.v_div));
                    pf.pass = 0;
                    pf.fail = 0;
                }
            }
            if ui.button("Reset counts").clicked() {
                pf.pass = 0;
                pf.fail = 0;
            }
        });
        ui.checkbox(&mut pf.stop_on_fail, "Stop on fail");
        ui.checkbox(&mut pf.output_multi, "Output result to MULTI");
        let total = pf.pass + pf.fail;
        ui.label(format!(
            "pass {}   fail {}   total {}",
            pf.pass, pf.fail, total
        ));
        if pf.mask.is_none() {
            ui.label(egui::RichText::new("no reference captured").weak().small());
        }
    });

    ui.group(|ui| {
        ui.strong("Spectrum (FFT)");
        ui.checkbox(&mut fft.enabled, "Enabled");
        ui.horizontal(|ui| {
            ui.label("Source");
            for (slot, name) in SLOT_NAMES.iter().enumerate() {
                if ui.selectable_label(fft.source == slot, *name).clicked() {
                    fft.source = slot;
                }
            }
            egui::ComboBox::from_id_salt("fftwnd")
                .selected_text(fft.window.label())
                .show_ui(ui, |ui| {
                    for w in Window::ALL {
                        if ui.selectable_label(fft.window == w, w.label()).clicked() {
                            fft.window = w;
                        }
                    }
                });
        });
    });
}
