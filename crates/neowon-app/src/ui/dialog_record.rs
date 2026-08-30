//! Record / Export / History section: capture the record stream to memory,
//! scrub back through it, and write it out — `.nwc` (lossless, reloadable),
//! WAV (audio, CH1 = L / CH2 = R), CSV, raw i8, or a PNG of the plot.

use bevy_egui::egui;

use crate::Link;
use crate::derived::fmt_si;
use crate::record::{History, Recorder, default_stem, export_dir};
use crate::script::{Action, Script};

pub fn show(
    ui: &mut egui::Ui,
    link: &mut Link,
    rec: &mut Recorder,
    hist: &mut History,
    script: &mut Script,
) {
    ui.horizontal(|ui| {
        // The scrollback ring is always capturing unless paused; the
        // History slider below scrubs it, like terminal scrollback.
        let label = if rec.on {
            "⏺ Capturing"
        } else {
            "⏸ Paused"
        };
        let color = if rec.on {
            egui::Color32::from_rgb(150, 45, 40)
        } else {
            egui::Color32::from_rgb(28, 30, 36)
        };
        if ui.add(egui::Button::new(label).fill(color)).clicked() {
            rec.on = !rec.on;
        }
        if ui.button("Clear").clicked() {
            rec.clear();
            hist.live(link);
        }
    });
    ui.label(
        // Counts right-aligned in fixed fields — they tick up while
        // recording and must not push the line wider digit by digit.
        egui::RichText::new(format!(
            "{:>6} records · {:>9} samples/ch · {}",
            rec.frames.len(),
            rec.samples_per_channel(),
            fmt_si(rec.seconds(), "s"),
        ))
        .monospace()
        .small(),
    );

    // History browser: scrub through the recorded ring.
    ui.separator();
    let n = rec.frames.len();
    ui.horizontal(|ui| {
        ui.label("History");
        let at = hist.active;
        let mut idx = at.unwrap_or(n.saturating_sub(1));
        let slider = ui.add_enabled(
            n > 1,
            egui::Slider::new(&mut idx, 0..=n.saturating_sub(1).max(1)).show_value(false),
        );
        if slider.changed() {
            hist.show(link, rec, idx);
        }
        if ui.add_enabled(n > 0, egui::Button::new("◀")).clicked() {
            hist.show(link, rec, at.unwrap_or(n - 1).saturating_sub(1));
        }
        if ui.add_enabled(n > 0, egui::Button::new("▶")).clicked() {
            hist.show(link, rec, at.map_or(n - 1, |i| i + 1));
        }
        if ui
            .add_enabled(at.is_some(), egui::Button::new("Live"))
            .clicked()
        {
            hist.live(link);
        }
    });
    ui.label(
        egui::RichText::new(match hist.active {
            Some(i) => format!("{:>6} / {n:<6}", i + 1),
            None => format!("  live / {n:<6}"),
        })
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
            .add_enabled(!disabled, egui::Button::new("Save .nwc"))
            .clicked()
        {
            let path = export("nwc");
            match rec.save_nwc(&path) {
                Ok(()) => rec.last_export = Some(path.display().to_string()),
                Err(e) => rec.last_export = Some(format!("failed: {e}")),
            }
        }
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
        if ui.button("PNG").clicked() {
            // Goes through the script queue: the shot needs a GPU readback.
            script.inject(Action::Shot {
                path: export("png").display().to_string(),
                roi: None,
            });
            rec.last_export = Some(export("png").display().to_string());
        }
    });

    // Load a saved capture (path box + button).
    ui.horizontal(|ui| {
        if ui.button("Load").clicked() {
            let path = rec.load_path.clone();
            script.inject(Action::CapLoad(path));
        }
        ui.add(
            egui::TextEdit::singleline(&mut rec.load_path)
                .hint_text("capture.nwc or vendor .cap")
                .desired_width(ui.available_width()),
        );
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
