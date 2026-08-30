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

pub fn show(
    ctx: &egui::Context,
    l: &Layout,
    link: &mut Link,
    _menus: &mut MenuState,
    now: f64,
    deep: &crate::deep::DeepView,
) -> egui::Rect {
    let rect = l.points(Roi::MenuBar.rect(l));
    let resp = egui::Area::new("menubar".into())
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
                // While the timeline is on, the on-screen time/div is the
                // window's, not the record's — show both rather than let the
                // chrome claim a time base the display is not using.
                let text = if deep.on {
                    format!(
                        "{}/div view   {}/div acq   {}",
                        fmt_si(deep.seconds_per_div(), "s"),
                        fmt_si(per_div, "s"),
                        fmt_si(link.config.sample_rate, "S/s"),
                    )
                } else {
                    format!(
                        "{}/div   {}",
                        fmt_si(per_div, "s"),
                        fmt_si(link.config.sample_rate, "S/s"),
                    )
                };
                ui.label(egui::RichText::new(text).monospace());
                if deep.on {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(112.0, 20.0), egui::Sense::hover());
                    ui.painter().rect(
                        r,
                        4.0,
                        egui::Color32::from_rgb(40, 44, 54),
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 140)),
                        egui::StrokeKind::Middle,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("TIMELINE {:.0}%", deep.lost() * 100.0),
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_rgb(80, 200, 140),
                    );
                    resp.on_hover_text(
                        "Showing the acquisition timeline at full sample rate. \
                         The percentage is how much of the window the instrument \
                         was not acquiring in; those columns are marked in red.",
                    );
                }
                // Slow time bases run the instrument in roll mode, where the
                // record fills progressively and the trigger is not used —
                // scopes always say so on screen.
                if crate::view::is_roll(link.config.sample_rate) {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(52.0, 20.0), egui::Sense::hover());
                    ui.painter().rect(
                        r,
                        4.0,
                        egui::Color32::from_rgb(40, 44, 54),
                        egui::Stroke::new(1.0, WAIT_COLOR),
                        egui::StrokeKind::Middle,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "ROLL",
                        egui::FontId::proportional(11.0),
                        WAIT_COLOR,
                    );
                    resp.on_hover_text(
                        "Roll mode: at this time base the instrument streams \
                         the record progressively and the trigger is not used.",
                    );
                }
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
    l.pixels(resp.response.rect)
}
