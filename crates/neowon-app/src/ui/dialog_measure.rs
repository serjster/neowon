//! Measure dialog — full measurement table with running statistics.

use bevy_egui::egui;

use crate::derived::{METRICS, MeasureState, SLOT_NAMES, SLOTS, fmt};

pub fn show(ui: &mut egui::Ui, meas: &mut MeasureState) {
    ui.horizontal(|ui| {
        ui.strong("Measurements");
        ui.label("stats for");
        for (slot, name) in SLOT_NAMES.iter().enumerate() {
            if meas.latest[slot].is_some()
                && ui
                    .selectable_label(meas.stats_slot == slot, *name)
                    .clicked()
            {
                meas.stats_slot = slot;
            }
        }
        if ui.button("Reset stats").clicked() {
            meas.reset_stats();
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("meas-grid")
            .striped(true)
            .min_col_width(56.0)
            .show(ui, |ui| {
                ui.label("");
                for (slot, name) in SLOT_NAMES.iter().enumerate() {
                    if meas.latest[slot].is_some() {
                        ui.label(egui::RichText::new(*name).strong());
                    }
                }
                let s = meas.stats_slot;
                for h in ["mean", "min", "max", "σ"] {
                    ui.label(egui::RichText::new(format!("{h} ({})", SLOT_NAMES[s])).weak());
                }
                ui.end_row();
                for (i, (name, get, unit)) in METRICS.iter().enumerate() {
                    ui.label(*name);
                    for slot in 0..SLOTS {
                        if let Some(m) = &meas.latest[slot] {
                            ui.label(get(m).map_or("—".into(), |v| fmt(v, *unit)));
                        }
                    }
                    if !meas.stats.is_empty() {
                        let t = &meas.stats[meas.stats_slot][i];
                        if t.count > 0 {
                            ui.label(fmt(t.mean, *unit));
                            ui.label(fmt(t.min, *unit));
                            ui.label(fmt(t.max, *unit));
                            ui.label(fmt(t.std_dev(), *unit));
                        } else {
                            for _ in 0..4 {
                                ui.label("—");
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    });
}
