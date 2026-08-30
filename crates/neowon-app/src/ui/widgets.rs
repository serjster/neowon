//! Shared UI widgets and visual constants (scope-grade restyle,
//! docs/ui-ux-research.md §3).

use bevy_egui::egui;
use neowon_core::PulseCondition;

/// Ladders used before the backend's capabilities arrive.
pub const FALLBACK_VDIV: [f64; 10] = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
pub const FALLBACK_RATES: [f64; 6] = [2.5e3, 25e3, 250e3, 2.5e6, 25e6, 100e6];
pub const PROBES: [f64; 7] = [1.0, 10.0, 20.0, 50.0, 100.0, 500.0, 1000.0];

/// Channel hues — shared with the GPU trace colors in `gpu.rs`.
pub const CH1_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 217, 26);
pub const CH2_COLOR: egui::Color32 = egui::Color32::from_rgb(51, 191, 255);
pub const MATH_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 89, 217);

/// Run-state badge colors (manual 8.5: Run = yellow, Stop = red).
pub const RUN_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 180, 30);
pub const STOP_COLOR: egui::Color32 = egui::Color32::from_rgb(220, 60, 50);
pub const WAIT_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 140, 40);

pub fn channel_color(ch: usize) -> egui::Color32 {
    match ch {
        0 => CH1_COLOR,
        1 => CH2_COLOR,
        _ => MATH_COLOR,
    }
}

/// Discrete ladder selector (V/div, s/div, probe): scopes step ladders,
/// they don't slide.
pub fn ladder_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    current: &mut f64,
    ladder: &[f64],
    fmt_val: impl Fn(f64) -> String,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(fmt_val(*current)).monospace())
            .show_ui(ui, |ui| {
                for &v in ladder {
                    if ui
                        .selectable_label(
                            (*current - v).abs() < v * 1e-6,
                            egui::RichText::new(fmt_val(v)).monospace(),
                        )
                        .clicked()
                    {
                        *current = v;
                        changed = true;
                    }
                }
            });
    });
    changed
}

pub fn condition_combo(ui: &mut egui::Ui, id: &str, condition: &mut PulseCondition) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("When");
        egui::ComboBox::from_id_salt(id)
            .selected_text(condition.label())
            .show_ui(ui, |ui| {
                for c in PulseCondition::ALL {
                    if ui.selectable_label(*condition == c, c.label()).clicked() {
                        *condition = c;
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// A chip drawn in the channel hue — descriptor-box building block
/// (manual 7.4: touching a descriptor box opens its dialog).
pub fn chip(
    ui: &mut egui::Ui,
    color: egui::Color32,
    text: &str,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    // A real egui Button sized by its galley — text can never overflow the
    // box, whatever the font or content.
    let alpha = if enabled { 255 } else { 90 };
    let border = egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), alpha);
    let fill = if selected {
        egui::Color32::from_rgb(28, 30, 38)
    } else {
        egui::Color32::from_rgb(16, 18, 24)
    };
    let text_color = if enabled { color } else { egui::Color32::GRAY };
    ui.add(
        egui::Button::new(egui::RichText::new(text).monospace().color(text_color))
            .fill(fill)
            .stroke(egui::Stroke::new(if selected { 2.5 } else { 1.5 }, border))
            .corner_radius(4.0)
            .min_size(egui::vec2(0.0, 40.0)),
    )
}
