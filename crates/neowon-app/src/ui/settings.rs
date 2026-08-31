//! Application settings — the things that are about the *app* rather than
//! about the instrument, so they do not belong in the instrument dock and
//! are not part of a saved instrument setup.

use bevy_egui::egui;

use crate::derived::fmt_si;
use crate::record::Recorder;
use crate::script::{Action, Script};
use crate::ui::UiScale;
use crate::ui::layout::UI_SCALE_RANGE;

/// Is the settings window open?
#[derive(bevy::prelude::Resource, Default)]
pub struct Settings {
    pub open: bool,
}

/// Scrollback budgets offered, in bytes.
const BUDGETS: [(&str, usize); 6] = [
    ("256 MB", 256 << 20),
    ("512 MB", 512 << 20),
    ("1 GB", 1 << 30),
    ("2 GB", 2 << 30),
    ("4 GB", 4usize << 30),
    ("8 GB", 8usize << 30),
];

pub fn window(
    ctx: &egui::Context,
    st: &mut Settings,
    rec: &mut Recorder,
    scale: &UiScale,
    script: &mut Script,
) {
    if !st.open {
        return;
    }
    let mut open = st.open;
    egui::Window::new("Settings")
        .open(&mut open)
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.group(|ui| {
                ui.strong("Display");
                let mut s = scale.0;
                if ui
                    .add(
                        egui::Slider::new(&mut s, UI_SCALE_RANGE.0..=UI_SCALE_RANGE.1)
                            .step_by(0.25)
                            .text("UI scale"),
                    )
                    .changed()
                {
                    script.inject(Action::UiScaleSet(s));
                }
                ui.label(
                    egui::RichText::new(
                        "for hi-DPI screens the OS does not scale; chosen from \
                         the monitor at startup",
                    )
                    .weak()
                    .small(),
                );
            });

            ui.group(|ui| {
                ui.strong("Scrollback");
                ui.label(
                    egui::RichText::new(
                        "How much captured history to keep. This is what the \
                         timeline view can reach back through.",
                    )
                    .weak()
                    .small(),
                );
                ui.horizontal_wrapped(|ui| {
                    for (label, bytes) in BUDGETS {
                        if ui.selectable_label(rec.budget == bytes, label).clicked() {
                            script.inject(Action::Scrollback(bytes));
                        }
                    }
                });
                // What it is actually holding, in the terms that matter:
                // seconds of history, not a frame count.
                let used = rec.bytes() as f64 / rec.budget.max(1) as f64;
                ui.add(
                    egui::ProgressBar::new(used as f32).text(
                        egui::RichText::new(format!(
                            "{} of {} · {} of history · {} records",
                            fmt_si(rec.bytes() as f64, "B"),
                            fmt_si(rec.budget as f64, "B"),
                            fmt_si(rec.span_seconds(), "s"),
                            rec.frames.len(),
                        ))
                        .monospace()
                        .size(10.0),
                    ),
                );
                if ui.button("Clear scrollback").clicked() {
                    script.inject(Action::RecordClear);
                }
            });
        });
    st.open = open;
}
