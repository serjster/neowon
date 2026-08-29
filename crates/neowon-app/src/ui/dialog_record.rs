//! Record / Export section: capture the record stream to memory and write
//! it out as WAV (audio, CH1 = L / CH2 = R), CSV, or raw i8.

use bevy_egui::egui;

use crate::Link;
use crate::derived::fmt_si;
use crate::record::{Recorder, default_stem, export_dir};

pub fn show(ui: &mut egui::Ui, _link: &mut Link, rec: &mut Recorder) {
    ui.horizontal(|ui| {
        let label = if rec.on {
            "⏺ Recording…"
        } else {
            "⏺ Record"
        };
        let color = if rec.on {
            egui::Color32::from_rgb(220, 60, 50)
        } else {
            egui::Color32::from_rgb(28, 30, 36)
        };
        if ui.add(egui::Button::new(label).fill(color)).clicked() {
            rec.on = !rec.on;
        }
        if ui.button("Clear").clicked() {
            rec.clear();
        }
    });
    ui.label(
        egui::RichText::new(format!(
            "{} records · {} samples/ch · {}",
            rec.frames.len(),
            rec.samples_per_channel(),
            fmt_si(rec.seconds(), "s"),
        ))
        .monospace()
        .small(),
    );
    ui.separator();
    let stem = default_stem();
    let dir = export_dir();
    ui.horizontal(|ui| {
        let disabled = rec.frames.is_empty();
        let export = |name: &str| dir.join(format!("{stem}.{name}"));
        if ui
            .add_enabled(!disabled, egui::Button::new("Export WAV"))
            .clicked()
        {
            let path = export("wav");
            match rec.export_wav(&path) {
                Ok(()) => rec.last_export = Some(path.display().to_string()),
                Err(e) => rec.last_export = Some(format!("failed: {e}")),
            }
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("CSV"))
            .clicked()
        {
            let path = export("csv");
            match rec.export_csv(&path) {
                Ok(()) => rec.last_export = Some(path.display().to_string()),
                Err(e) => rec.last_export = Some(format!("failed: {e}")),
            }
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Raw i8"))
            .clicked()
        {
            match rec.export_raw(&export("raw")) {
                Ok(files) => rec.last_export = files.first().cloned(),
                Err(e) => rec.last_export = Some(format!("failed: {e}")),
            }
        }
    });
    if let Some(last) = &rec.last_export {
        ui.label(egui::RichText::new(last.clone()).small().weak());
    } else {
        ui.label(
            egui::RichText::new(format!("→ {}", dir.display()))
                .small()
                .weak(),
        );
    }
}
