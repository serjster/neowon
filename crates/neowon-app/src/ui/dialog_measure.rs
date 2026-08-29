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
        ui.checkbox(&mut meas.guides, "Guides");
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
                ui.end_row();
                // Compact: values only; hover a value for its running
                // statistics (mean/min/max/sigma/n).
                for (i, (name, get, unit)) in METRICS.iter().enumerate() {
                    ui.label(*name);
                    for slot in 0..SLOTS {
                        if meas.latest[slot].is_none() {
                            continue;
                        }
                        let m = meas.latest[slot].unwrap();
                        let cell = ui.label(get(&m).map_or("—".into(), |v| fmt(v, *unit)));
                        if !meas.stats.is_empty() {
                            let t = &meas.stats[slot][i];
                            if t.count > 0 {
                                cell.on_hover_text(format!(
                                    "mean {}\nmin  {}\nmax  {}\nσ    {}\nn    {}",
                                    fmt(t.mean, *unit),
                                    fmt(t.min, *unit),
                                    fmt(t.max, *unit),
                                    fmt(t.std_dev(), *unit),
                                    t.count,
                                ));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    });
}
