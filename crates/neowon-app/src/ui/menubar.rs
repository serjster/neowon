//! Status bar: run state, timebase/rate, backend identity, frame counter.
//! Function menus live in the always-visible dock (menu.rs), so this strip
//! carries ambient status only.

use bevy_egui::egui;
use neowon_core::Sweep;

use crate::Link;
use crate::derived::fmt_si;

use super::layout::{Layout, Roi};
use super::menu::MenuState;
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

pub fn show(ctx: &egui::Context, l: &Layout, link: &mut Link, _menus: &mut MenuState, now: f64) {
    let rect = Roi::MenuBar.rect(l);
    egui::Area::new("menubar".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Run state badge (manual 8.5: Run = yellow, Stop = red).
                let (label, color) = run_state(link, now);
                let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 22.0), egui::Sense::hover());
                ui.painter()
                    .rect(r, 4.0, color, egui::Stroke::NONE, egui::StrokeKind::Middle);
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    egui::Color32::BLACK,
                );
                let record_len = link.caps.as_ref().map(|c| c.record_len).unwrap_or(5000);
                let per_div = record_len as f64 / link.config.sample_rate / 10.0;
                ui.label(
                    egui::RichText::new(format!(
                        "{}/div   {}",
                        fmt_si(per_div, "s"),
                        fmt_si(link.config.sample_rate, "S/s"),
                    ))
                    .monospace(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Fixed 8-char field: the counter widening a digit must
                    // not nudge the device label beside it.
                    let n = format!("#{}", link.frames_seen);
                    ui.label(egui::RichText::new(format!("{n:>8}")).monospace());
                    if let Some(caps) = &link.caps {
                        ui.label(
                            egui::RichText::new(format!("{} · {}", caps.name, caps.serial)).small(),
                        );
                    } else {
                        ui.label(egui::RichText::new(link.status.clone()).small());
                    }
                });
            });
        });
}
