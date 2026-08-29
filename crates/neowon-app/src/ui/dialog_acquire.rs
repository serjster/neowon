//! Acquire dialog — acquisition mode.

use bevy_egui::egui;
use neowon_core::AcqMode;

use crate::Link;

pub fn show(ui: &mut egui::Ui, link: &mut Link) {
    ui.group(|ui| {
        ui.strong("Acquire");
        let modes = [
            ("Sample", AcqMode::Sample),
            ("Peak", AcqMode::Peak),
            ("Avg 4", AcqMode::Average(4)),
            ("Avg 16", AcqMode::Average(16)),
            ("Avg 64", AcqMode::Average(64)),
        ];
        ui.horizontal_wrapped(|ui| {
            for (label, m) in modes {
                if ui.selectable_label(link.config.acq == m, label).clicked() {
                    link.config.acq = m;
                    link.dirty = true;
                }
            }
        });
    });
}
