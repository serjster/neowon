//! Virtual front panel (manual chapter 8): the hardware keys reproduced as
//! grouped buttons along the bottom strip. Groups: Vertical | Horizontal |
//! Trigger | Run | Common functions. Features we lack are omitted.

use bevy_egui::egui;
use neowon_backend::Command;
use neowon_core::Sweep;

use crate::Link;
use crate::derived::{MathState, MeasureState};
use crate::gpu::{Persistence, Phosphor};

use super::layout::{Layout, Roi};
use super::menu::{Menu, MenuState};
use super::widgets::{RUN_COLOR, STOP_COLOR, channel_color};

fn key(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let btn = egui::Button::new(egui::RichText::new(label).small())
        .fill(if active {
            egui::Color32::from_rgb(60, 64, 74)
        } else {
            egui::Color32::from_rgb(28, 30, 36)
        })
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(70)))
        .min_size(egui::vec2(58.0, 30.0));
    ui.add(btn).clicked()
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    l: &Layout,
    link: &mut Link,
    phosphor: &mut Phosphor,
    math: &mut MathState,
    meas: &mut MeasureState,
    menus: &mut MenuState,
) {
    let rect = Roi::FrontPanel.rect(l);
    egui::Area::new("frontpanel".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                // Vertical (manual 8.2): channel + math keys.
                group(ui, "VERTICAL", |ui| {
                    // CH keys toggle the channel on/off only — configuring
                    // happens in the dock or via the descriptor box.
                    for ch in 0..2 {
                        let enabled = link.config.channels[ch].enabled;
                        if color_key(ui, format!("CH{}", ch + 1), channel_color(ch), enabled) {
                            link.config.channels[ch].enabled = !enabled;
                            link.dirty = true;
                        }
                    }
                    if key(ui, "Math", math.enabled) {
                        math.enabled = !math.enabled;
                    }
                });

                // Horizontal (manual 8.3).
                group(ui, "HORIZONTAL", |ui| {
                    if key(ui, "H", matches!(menus.open, Some(Menu::Horizontal))) {
                        menus.toggle(Menu::Horizontal);
                    }
                    if key(ui, "Pos 50%", false) {
                        link.config.position = 0.5;
                        link.dirty = true;
                    }
                });

                // Trigger (manual 8.4): mode keys + level-to-50%.
                group(ui, "TRIGGER", |ui| {
                    for (label, s) in [
                        ("Auto", Sweep::Auto),
                        ("Normal", Sweep::Normal),
                        ("Single", Sweep::Single),
                    ] {
                        if key(ui, label, link.config.trigger.sweep == s) {
                            link.config.trigger.sweep = s;
                            if s == Sweep::Single {
                                link.config.running = true;
                            }
                            link.dirty = true;
                        }
                    }
                    if key(ui, "Lvl 50%", false) {
                        // Manual 8.4E: push to set level to 50% of waveform.
                        let src = link.config.trigger.source;
                        if let Some(m) = meas.latest.get(src).and_then(|m| m.as_ref()) {
                            link.config.trigger.level = (m.vtop + m.vbase) / 2.0;
                            link.dirty = true;
                        }
                    }
                });

                // Run control (manual 8.5/8.6).
                group(ui, "RUN", |ui| {
                    let running = link.config.running;
                    let color = if running { RUN_COLOR } else { STOP_COLOR };
                    let label = if running { "Run/Stop" } else { "Stopped" };
                    if color_key(ui, label.to_string(), color, true) {
                        link.config.running = !running;
                        link.dirty = true;
                    }
                    if key(ui, "Force", false) {
                        let _ = link.sup.commands.send(Command::ForceTrigger);
                    }
                    if key(ui, "AutoSetup", false) {
                        let _ = link.sup.commands.send(Command::AutoSet);
                    }
                });

                // Common functions (manual 8.10, minus features we lack).
                group(ui, "FUNCTION", |ui| {
                    for (label, m) in [
                        ("Measure", Menu::Measure),
                        ("Cursor", Menu::Cursor),
                        ("Acquire", Menu::Acquire),
                        ("Display", Menu::Display),
                        ("Utility", Menu::Utility),
                    ] {
                        if key(ui, label, matches!(menus.open, Some(x) if x == m)) {
                            menus.toggle(m);
                        }
                    }
                    // Display key second function (manual 8.10): persistence.
                    let persist_on = phosphor.persistence != Persistence::Off;
                    if key(ui, "Persist", persist_on) {
                        phosphor.persistence = if persist_on {
                            Persistence::Off
                        } else {
                            Persistence::Seconds(1.0)
                        };
                    }
                    if key(ui, "Clear", false) {
                        // Manual 8.10: clears persistence + statistics.
                        meas.reset_stats();
                    }
                });
            });
        });
}

fn group(ui: &mut egui::Ui, name: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(name)
                .size(9.0)
                .color(egui::Color32::GRAY),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            content(ui);
        });
    });
    ui.separator();
}

/// Channel-colored key; lit while `on`.
fn color_key(ui: &mut egui::Ui, label: String, color: egui::Color32, on: bool) -> bool {
    let fill = if on {
        color.gamma_multiply(0.35)
    } else {
        egui::Color32::from_rgb(28, 30, 36)
    };
    let stroke_color = if on {
        color
    } else {
        egui::Color32::from_gray(70)
    };
    let btn = egui::Button::new(egui::RichText::new(label).small().color(if on {
        color
    } else {
        egui::Color32::LIGHT_GRAY
    }))
    .fill(fill)
    .stroke(egui::Stroke::new(1.0, stroke_color))
    .min_size(egui::vec2(48.0, 30.0));
    ui.add(btn).clicked()
}
