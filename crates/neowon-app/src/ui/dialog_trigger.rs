//! Trigger dialog — source, type, sweep, level, holdoff.

use bevy_egui::egui;
use neowon_core::{PulseCondition, Slope, Sweep, TriggerKind, VideoSync};

use crate::Link;

use super::widgets::condition_combo;

pub fn show(ui: &mut egui::Ui, link: &mut Link) {
    ui.group(|ui| {
        ui.strong("Trigger");
        let mut t = link.config.trigger;
        let mut dirty = false;
        ui.horizontal(|ui| {
            ui.label("Source");
            for src in 0..2 {
                if ui
                    .selectable_label(t.source == src, format!("CH{}", src + 1))
                    .clicked()
                {
                    t.source = src;
                    dirty = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Type");
            egui::ComboBox::from_id_salt("trigkind")
                .selected_text(t.kind.label())
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(matches!(t.kind, TriggerKind::Edge { .. }), "Edge")
                        .clicked()
                    {
                        let slope = match t.kind {
                            TriggerKind::Edge { slope } => slope,
                            _ => Slope::Rising,
                        };
                        t.kind = TriggerKind::Edge { slope };
                        dirty = true;
                    }
                    if ui
                        .selectable_label(matches!(t.kind, TriggerKind::Pulse { .. }), "Pulse")
                        .clicked()
                    {
                        t.kind = TriggerKind::Pulse {
                            condition: PulseCondition::PositiveGreater,
                            width: 1e-6,
                        };
                        dirty = true;
                    }
                    if ui
                        .selectable_label(matches!(t.kind, TriggerKind::Slope { .. }), "Slope")
                        .clicked()
                    {
                        t.kind = TriggerKind::Slope {
                            condition: PulseCondition::PositiveGreater,
                            width: 1e-6,
                            upper: t.level + 0.1,
                            lower: t.level - 0.1,
                        };
                        dirty = true;
                    }
                    if ui
                        .selectable_label(matches!(t.kind, TriggerKind::Video { .. }), "Video")
                        .clicked()
                    {
                        t.kind = TriggerKind::Video {
                            sync: VideoSync::Line,
                            line: 1,
                        };
                        dirty = true;
                    }
                });
        });
        match &mut t.kind {
            TriggerKind::Edge { slope } => {
                ui.horizontal(|ui| {
                    ui.label("Slope");
                    for (label, s) in [("Rising ⬈", Slope::Rising), ("Falling ⬊", Slope::Falling)]
                    {
                        if ui.selectable_label(*slope == s, label).clicked() {
                            *slope = s;
                            dirty = true;
                        }
                    }
                });
            }
            TriggerKind::Pulse { condition, width } => {
                dirty |= condition_combo(ui, "pulsecond", condition);
                let mut w_us = *width * 1e6;
                ui.horizontal(|ui| {
                    ui.label("Width");
                    if ui
                        .add(
                            egui::DragValue::new(&mut w_us)
                                .speed(0.1)
                                .range(0.01..=655_360.0)
                                .suffix(" µs"),
                        )
                        .changed()
                    {
                        *width = w_us * 1e-6;
                        dirty = true;
                    }
                });
            }
            TriggerKind::Slope {
                condition,
                width,
                upper,
                lower,
            } => {
                dirty |= condition_combo(ui, "slopecond", condition);
                let mut w_us = *width * 1e6;
                ui.horizontal(|ui| {
                    ui.label("Width");
                    if ui
                        .add(
                            egui::DragValue::new(&mut w_us)
                                .speed(0.1)
                                .range(0.01..=655_360.0)
                                .suffix(" µs"),
                        )
                        .changed()
                    {
                        *width = w_us * 1e-6;
                        dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Upper");
                    if ui
                        .add(egui::DragValue::new(upper).speed(0.01).suffix(" V"))
                        .changed()
                    {
                        dirty = true;
                    }
                    ui.label("Lower");
                    if ui
                        .add(egui::DragValue::new(lower).speed(0.01).suffix(" V"))
                        .changed()
                    {
                        dirty = true;
                    }
                });
            }
            TriggerKind::Video { sync, line } => {
                ui.horizontal(|ui| {
                    ui.label("Sync");
                    egui::ComboBox::from_id_salt("vidsync")
                        .selected_text(sync.label())
                        .show_ui(ui, |ui| {
                            for s in VideoSync::ALL {
                                if ui.selectable_label(*sync == s, s.label()).clicked() {
                                    *sync = s;
                                    dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Line #");
                    if ui
                        .add(egui::DragValue::new(line).range(0..=65535))
                        .changed()
                    {
                        dirty = true;
                    }
                });
                ui.label(
                    egui::RichText::new("Video trigger: packing unverified on hardware")
                        .weak()
                        .small(),
                );
            }
        }
        ui.horizontal(|ui| {
            ui.label("Sweep");
            for (label, s) in [
                ("Auto", Sweep::Auto),
                ("Normal", Sweep::Normal),
                ("Single", Sweep::Single),
            ] {
                if ui.selectable_label(t.sweep == s, label).clicked() {
                    t.sweep = s;
                    if s == Sweep::Single {
                        link.config.running = true;
                    }
                    dirty = true;
                }
            }
        });
        if matches!(t.kind, TriggerKind::Edge { .. } | TriggerKind::Pulse { .. }) {
            let mut level = t.level;
            ui.horizontal(|ui| {
                ui.label("Level");
                if ui
                    .add(egui::DragValue::new(&mut level).speed(0.01).suffix(" V"))
                    .changed()
                {
                    t.level = level;
                    dirty = true;
                }
            });
        }
        let mut holdoff_us = t.holdoff * 1e6;
        ui.horizontal(|ui| {
            ui.label("Holdoff");
            if ui
                .add(
                    egui::DragValue::new(&mut holdoff_us)
                        .speed(0.1)
                        .range(0.1..=10_000_000.0)
                        .suffix(" µs"),
                )
                .changed()
            {
                t.holdoff = holdoff_us * 1e-6;
                dirty = true;
            }
        });
        if dirty {
            link.config.trigger = t;
            link.dirty = true;
        }
    });
}
