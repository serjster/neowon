//! Descriptor boxes under the grid (manual 7.4/7.5) and the measurement
//! readout overlay along the plot's bottom edge.

use bevy_egui::egui;
use neowon_core::{Coupling, Slope, Sweep, TriggerKind};

use crate::Link;
use crate::derived::{MeasureState, SLOT_NAMES, fmt, fmt_si};
use crate::gpu::{Phosphor, TraceMode};

use super::layout::Roi;
use super::menu::{Menu, MenuState};
use super::widgets::{CH1_COLOR, CH2_COLOR, MATH_COLOR, channel_color, chip};

pub fn show(
    ctx: &egui::Context,
    link: &mut Link,
    phosphor: &Phosphor,
    meas: &MeasureState,
    menus: &mut MenuState,
) {
    let rect = Roi::Descriptors.rect();
    egui::Area::new("descriptors".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                // Channel descriptor boxes (manual 7.4).
                for ch in 0..2 {
                    let c = link.config.channels[ch];
                    let coup = match c.coupling {
                        Coupling::Dc => "DC",
                        Coupling::Ac => "AC",
                        Coupling::Gnd => "GND",
                    };
                    let mut text = format!(
                        "C{} {} {} ×{}",
                        ch + 1,
                        fmt_si(c.volts_div, "V"),
                        coup,
                        c.probe,
                    );
                    if c.offset.abs() > 1e-9 {
                        text.push_str(&format!(
                            " off {}",
                            fmt_si(c.offset * c.volts_div * 10.0 * c.probe, "V")
                        ));
                    }
                    if chip(ui, channel_color(ch), &text, c.enabled, 170.0).clicked() {
                        menus.open = Some(Menu::Channel(ch));
                    }
                }

                // Math descriptor (F1) while the math trace is on.
                if phosphor.mode != TraceMode::Xy
                    && meas.latest[2].is_some()
                    && chip(ui, MATH_COLOR, "F Math", true, 90.0).clicked()
                {
                    menus.open = Some(Menu::Math);
                }

                ui.add_space(8.0);

                // Timebase descriptor box (manual 7.5).
                let record_len = link.caps.as_ref().map(|c| c.record_len).unwrap_or(5000);
                let record_s = record_len as f64 / link.config.sample_rate;
                let tb = format!(
                    "Main {}/div  {}  {} pts",
                    fmt_si(record_s / 10.0, "s"),
                    fmt_si(link.config.sample_rate, "S/s"),
                    record_len,
                );
                if chip(ui, egui::Color32::from_gray(150), &tb, true, 270.0).clicked() {
                    menus.open = Some(Menu::Horizontal);
                }

                // Trigger descriptor box (manual 7.5).
                let t = link.config.trigger;
                let slope_glyph = match t.kind {
                    TriggerKind::Edge { slope } => match slope {
                        Slope::Rising => "↗",
                        Slope::Falling => "↘",
                    },
                    _ => "◇",
                };
                let sweep = match t.sweep {
                    Sweep::Auto => "Auto",
                    Sweep::Normal => "Normal",
                    Sweep::Single => "Single",
                };
                let tg = format!(
                    "C{} {} {} {} {}",
                    t.source + 1,
                    t.kind.label(),
                    slope_glyph,
                    fmt(t.level, crate::derived::Unit::Volt),
                    sweep,
                );
                if chip(ui, egui::Color32::from_rgb(255, 128, 64), &tg, true, 210.0).clicked() {
                    menus.open = Some(Menu::Trigger);
                }
            });
        });

    measurement_overlay(ctx, meas);
}

/// Latest measurements per slot, source-colored, along the plot bottom.
fn measurement_overlay(ctx: &egui::Context, meas: &MeasureState) {
    let rect = Roi::MeasOverlay.rect();
    egui::Area::new("meas-overlay".into())
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_height(rect.height());
            ui.horizontal_centered(|ui| {
                for (slot, name) in SLOT_NAMES.iter().enumerate() {
                    let Some(m) = &meas.latest[slot] else {
                        continue;
                    };
                    let color = match slot {
                        0 => CH1_COLOR,
                        1 => CH2_COLOR,
                        _ => MATH_COLOR,
                    };
                    let freq = m.freq.map(|v| fmt(v, crate::derived::Unit::Hertz));
                    let text = format!(
                        "{} {} {}",
                        name,
                        freq.unwrap_or_else(|| "—".into()),
                        fmt(m.vpp, crate::derived::Unit::Volt),
                    );
                    let (r, _) = ui.allocate_exact_size(
                        egui::vec2(text.len() as f32 * 7.0 + 12.0, 22.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect(
                        r,
                        3.0,
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 160),
                        egui::Stroke::new(1.0, color),
                        egui::StrokeKind::Middle,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::monospace(11.0),
                        color,
                    );
                }
            });
        });
}
