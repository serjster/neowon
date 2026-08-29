//! Math dialog — operator, scale.

use bevy_egui::egui;
use neowon_dsp::MathOp;

use crate::derived::{MathState, fmt_si};

pub fn show(ui: &mut egui::Ui, math: &mut MathState) {
    ui.group(|ui| {
        ui.strong("Math (M = f(CH1, CH2))");
        ui.checkbox(&mut math.enabled, "Enabled");
        egui::ComboBox::from_id_salt("mathop")
            .selected_text(math.op.label())
            .show_ui(ui, |ui| {
                for op in MathOp::ALL {
                    if ui.selectable_label(math.op == op, op.label()).clicked() {
                        math.op = op;
                        math.rescale = true;
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label(format!(
                "Scale: {} /div",
                fmt_si(math.full_scale / 10.0, math.op.unit())
            ));
            if ui.button("Auto").clicked() {
                math.rescale = true;
            }
        });
    });
}
