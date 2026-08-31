//! The measurements window: the full table, with room for it.
//!
//! Eighteen metrics across three sources does not fit in a 320-pixel rail —
//! the dock section could only ever show a handful at a time. This is the
//! same treatment the spectrum and 3D views get: a floating window that can
//! be sized and put where the reader wants it.

use bevy_egui::egui;

use crate::derived::{Band, METRICS, MeasureState, SLOT_NAMES, SLOTS, Unit, fmt, fmt_opt_sticky};
use crate::ui::widgets::{CH1_COLOR, CH2_COLOR, MATH_COLOR};

fn slot_color(slot: usize) -> egui::Color32 {
    match slot {
        0 => CH1_COLOR,
        1 => CH2_COLOR,
        _ => MATH_COLOR,
    }
}

pub fn show(ctx: &egui::Context, meas: &mut MeasureState) {
    if !meas.window {
        return;
    }
    let mut open = meas.window;
    egui::Window::new("Measurements")
        .open(&mut open)
        .default_width(560.0)
        .default_height(460.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (slot, name) in SLOT_NAMES.iter().enumerate() {
                    let on = meas.latest[slot].is_some();
                    ui.add_enabled_ui(on, |ui| {
                        let mut show = meas.show_slot[slot];
                        if ui
                            .checkbox(&mut show, *name)
                            .on_hover_text(if on {
                                "Show this source's column"
                            } else {
                                "Not measured: the source is off"
                            })
                            .changed()
                        {
                            meas.show_slot[slot] = show;
                        }
                    });
                }
                ui.separator();
                ui.checkbox(&mut meas.show_stats, "Statistics")
                    .on_hover_text("mean, min, max and standard deviation since the last reset");
                if ui.button("Reset").clicked() {
                    meas.reset_stats();
                }
            });
            ui.separator();

            let shown: Vec<usize> = (0..SLOTS)
                .filter(|&s| meas.show_slot[s] && meas.latest[s].is_some())
                .collect();
            if shown.is_empty() {
                ui.label(
                    egui::RichText::new("no source is being measured")
                        .weak()
                        .small(),
                );
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("meas-window")
                    .striped(true)
                    .min_col_width(52.0)
                    .show(ui, |ui| {
                        ui.label("");
                        for &slot in &shown {
                            ui.label(
                                egui::RichText::new(SLOT_NAMES[slot])
                                    .strong()
                                    .color(slot_color(slot)),
                            );
                            if meas.show_stats {
                                for h in ["mean", "min", "max", "σ"] {
                                    ui.label(egui::RichText::new(h).weak().size(10.0));
                                }
                            }
                            ui.label(egui::RichText::new("trend").weak().size(10.0));
                        }
                        ui.end_row();

                        for (i, (name, get, unit)) in METRICS.iter().enumerate() {
                            ui.label(*name);
                            for &slot in &shown {
                                value_cell(ui, meas, slot, i, get, *unit);
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    meas.window = open;
}

#[allow(clippy::type_complexity)]
fn value_cell(
    ui: &mut egui::Ui,
    meas: &mut MeasureState,
    slot: usize,
    i: usize,
    get: &fn(&crate::derived::Measurements) -> Option<f64>,
    unit: Unit,
) {
    let m = meas.latest[slot];
    let v = m.as_ref().and_then(get);
    let text = match meas.bands.get_mut(slot) {
        Some(b) => fmt_opt_sticky(v, unit, &mut b[i]),
        None => fmt_opt_sticky(v, unit, &mut Band::default()),
    };
    ui.label(egui::RichText::new(text).monospace());

    if meas.show_stats {
        let t = meas.stats.get(slot).map(|s| s[i]);
        for pick in [0usize, 1, 2, 3] {
            let cell = t.filter(|t| t.count > 0).map(|t| match pick {
                0 => t.mean,
                1 => t.min,
                2 => t.max,
                _ => t.std_dev(),
            });
            ui.label(
                egui::RichText::new(cell.map_or_else(|| "—".into(), |x| fmt(x, unit)))
                    .monospace()
                    .size(10.0)
                    .weak(),
            );
        }
    }
    sparkline(ui, meas, slot, i);
}

/// A small trend line for one metric: enough to see drift or jitter that a
/// single number and a standard deviation both hide.
fn sparkline(ui: &mut egui::Ui, meas: &MeasureState, slot: usize, i: usize) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(72.0, 14.0), egui::Sense::hover());
    let Some(ring) = meas.history.get(slot).map(|h| &h[i]) else {
        return;
    };
    if ring.len() < 2 {
        return;
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in ring {
        if v.is_finite() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return;
    }
    // A flat trace is the common and desirable case; centre it rather than
    // amplifying float noise into a fake wiggle.
    let span = (hi - lo).max(f64::EPSILON.max((hi.abs() + lo.abs()) * 1e-9));
    let pts: Vec<egui::Pos2> = ring
        .iter()
        .enumerate()
        .map(|(k, &v)| {
            let x = rect.left() + rect.width() * k as f32 / (ring.len() - 1) as f32;
            let f = if hi > lo { (v - lo) / span } else { 0.5 };
            egui::pos2(x, rect.bottom() - rect.height() * f as f32)
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        pts,
        egui::Stroke::new(1.0, slot_color(slot).gamma_multiply(0.8)),
    ));
    resp.on_hover_text(format!(
        "last {} acquisitions\nrange {:.6} … {:.6}",
        ring.len(),
        lo,
        hi
    ));
}
