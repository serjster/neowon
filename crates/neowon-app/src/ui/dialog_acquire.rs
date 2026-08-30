//! Acquire dialog — acquisition mode.

use bevy_egui::egui;
use neowon_core::AcqMode;

use crate::Link;
use crate::autopeak::AutoPeak;

pub fn show(ui: &mut egui::Ui, link: &mut Link, ap: &mut AutoPeak) {
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
                // The selection shows the user's choice; while auto peak is
                // engaged the badge below says what is actually running.
                if ui.selectable_label(ap.user_acq == m, label).clicked() {
                    ap.set_user(m);
                    link.config.acq = m;
                    link.dirty = true;
                }
            }
        });
        ui.checkbox(&mut ap.on, "Auto peak at slow time bases");
        if ap.engaged {
            ui.label(
                egui::RichText::new("PEAK (auto) — too few samples per cycle to sample plainly")
                    .small()
                    .color(egui::Color32::from_rgb(235, 180, 30)),
            );
        }
        ui.label(
            egui::RichText::new(
                "peak records are min/max pairs: amplitudes are valid, timings are not",
            )
            .weak()
            .small(),
        );
    });
}
