//! Menu bar (manual 7.2): drop-down menus reach every dialog; the right
//! side carries the ambient status (run state, backend, trigger status).

use bevy_egui::egui;
use neowon_core::Sweep;

use crate::Link;
use crate::derived::fmt_si;

use super::layout::Roi;
use super::menu::{Menu, MenuState};
use super::widgets::{RUN_COLOR, STOP_COLOR, WAIT_COLOR};

/// Run/stop/wait classification for the badge.
pub fn run_state(link: &Link, now: f64) -> (&'static str, egui::Color32) {
    if !link.config.running {
        return ("STOP", STOP_COLOR);
    }
    let starved = matches!(link.config.trigger.sweep, Sweep::Normal | Sweep::Single)
        && now - link.last_frame_at > 0.5;
    if starved {
        ("WAIT", WAIT_COLOR)
    } else {
        ("RUN", RUN_COLOR)
    }
}

pub fn show(ctx: &egui::Context, link: &mut Link, menus: &mut MenuState, now: f64) {
    let rect = Roi::MenuBar.rect();
    egui::Area::new("menubar".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                dropdown(ui, menus, "Horizontal", Menu::Horizontal);
                dropdown(ui, menus, "Trigger", Menu::Trigger);
                dropdown(ui, menus, "Acquire", Menu::Acquire);
                dropdown(ui, menus, "Measure", Menu::Measure);
                dropdown(ui, menus, "Math", Menu::Math);
                dropdown(ui, menus, "Cursor", Menu::Cursor);
                dropdown(ui, menus, "Display", Menu::Display);
                dropdown(ui, menus, "Utility", Menu::Utility);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("#{}", link.frames_seen)).monospace());
                    if let Some(caps) = &link.caps {
                        ui.label(
                            egui::RichText::new(format!("{} · {}", caps.name, caps.serial)).small(),
                        );
                    } else {
                        ui.label(egui::RichText::new(link.status.clone()).small());
                    }
                    ui.label(
                        egui::RichText::new(fmt_si(link.config.sample_rate, "S/s")).monospace(),
                    );
                    // Run state badge (manual 8.5: Run = yellow, Stop = red).
                    let (label, color) = run_state(link, now);
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(64.0, 22.0), egui::Sense::hover());
                    ui.painter()
                        .rect(r, 4.0, color, egui::Stroke::NONE, egui::StrokeKind::Middle);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(13.0),
                        egui::Color32::BLACK,
                    );
                });
            });
        });
}

fn dropdown(ui: &mut egui::Ui, menus: &mut MenuState, label: &str, menu: Menu) {
    ui.menu_button(label, |ui| {
        ui.set_min_width(160.0);
        let items: Vec<(&str, Menu)> = match menu {
            Menu::Horizontal => vec![
                ("Timebase / position", Menu::Horizontal),
                ("Channel 1", Menu::Channel(0)),
                ("Channel 2", Menu::Channel(1)),
            ],
            Menu::Trigger => vec![("Trigger setup", Menu::Trigger)],
            Menu::Acquire => vec![("Acquisition", Menu::Acquire)],
            Menu::Measure => vec![("Measurements", Menu::Measure)],
            Menu::Math => vec![("Math", Menu::Math)],
            Menu::Cursor => vec![("Cursors", Menu::Cursor)],
            Menu::Display => vec![("Display", Menu::Display)],
            Menu::Utility => vec![("Utility / Pass-Fail", Menu::Utility)],
            Menu::Channel(_) => vec![],
        };
        for (name, m) in items {
            if ui.button(name).clicked() {
                menus.open = Some(m);
                ui.close();
            }
        }
    });
}
