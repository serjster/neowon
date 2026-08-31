//! Measure dialog — full measurement table with running statistics.

use bevy_egui::egui;

use crate::derived::{Band, METRICS, MeasureState, SLOT_NAMES, SLOTS, fmt, fmt_opt_sticky};

pub fn show(ui: &mut egui::Ui, meas: &mut MeasureState) {
    ui.strong("Measurements");
    if ui
        .button("Open measurements window")
        .on_hover_text(
            "The full table with statistics and trends. Eighteen metrics do \
             not fit in the rail, so they get a window that can be sized and \
             placed.",
        )
        .clicked()
    {
        meas.window = true;
    }
    ui.horizontal(|ui| {
        ui.label("stats");
        for (slot, name) in SLOT_NAMES.iter().enumerate() {
            if meas.latest[slot].is_some()
                && ui
                    .selectable_label(meas.stats_slot == slot, *name)
                    .clicked()
            {
                meas.stats_slot = slot;
            }
        }
        if ui.button("Reset").clicked() {
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
                        // Monospace + sticky SI band + dash padded to the
                        // unit's width: neither the grid's columns nor a
                        // cell's internal layout can move frame to frame.
                        let text = match meas.bands.get_mut(slot) {
                            Some(bands) => fmt_opt_sticky(get(&m), *unit, &mut bands[i]),
                            None => fmt_opt_sticky(get(&m), *unit, &mut Band::default()),
                        };
                        let cell = ui.label(egui::RichText::new(text).monospace());
                        if !meas.stats.is_empty() {
                            let t = &meas.stats[slot][i];
                            if t.count > 0 {
                                cell.on_hover_text(
                                    egui::RichText::new(format!(
                                        "mean {}\nmin  {}\nmax  {}\nσ    {}\nn    {}",
                                        fmt(t.mean, *unit),
                                        fmt(t.min, *unit),
                                        fmt(t.max, *unit),
                                        fmt(t.std_dev(), *unit),
                                        t.count,
                                    ))
                                    .monospace(),
                                );
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    });
}
