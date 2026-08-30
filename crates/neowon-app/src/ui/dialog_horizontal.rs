//! Horizontal dialog — timebase (sample rate), trigger position, and the
//! zoom window into the acquired record.

use bevy_egui::egui;

use crate::Link;
use crate::gpu::Phosphor;

use super::icons::{Icon, button};
use super::knob::knob;
use super::widgets::{FALLBACK_RATES, ladder_combo};
use crate::derived::fmt_si;

/// Samples per record before capabilities arrive (VDS1022/sim shape).
const FALLBACK_RECORD_LEN: usize = 5000;

pub fn show(ui: &mut egui::Ui, link: &mut Link, phosphor: &mut Phosphor) {
    let rate_ladder: Vec<f64> = link
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| FALLBACK_RATES.to_vec());
    let record_len = link
        .caps
        .as_ref()
        .map(|c| c.record_len)
        .unwrap_or(FALLBACK_RECORD_LEN);
    let record_s = record_len as f64 / link.config.sample_rate;

    ui.group(|ui| {
        ui.strong("Horizontal");
        ui.label(format!(
            "Main {} /div   ({} record)",
            fmt_si(record_s / 10.0, "s"),
            fmt_si(record_s, "s")
        ));
        // Rate stays a discrete ladder — the hardware timebase is the rate.
        let mut rate = link.config.sample_rate;
        if ladder_combo(ui, "rate", "Rate", &mut rate, &rate_ladder, |v| {
            fmt_si(v, "S/s")
        }) {
            link.config.sample_rate = rate;
            link.dirty = true;
        }
        ui.horizontal(|ui| {
            let mut pos = link.config.position;
            if knob(ui, "Trig position", &mut pos, (0.0, 1.0), None, 0.5, |v| {
                format!("{:.0}%", v * 100.0)
            }) {
                link.config.position = pos;
                link.dirty = true;
            }
        });
        if ui.button("Trigger position → 50%").clicked() {
            link.config.position = 0.5;
            link.dirty = true;
        }
    });

    ui.group(|ui| {
        ui.strong("Zoom window");
        let (center, span) = phosphor.hview;
        let zoom_s = record_s * span;
        ui.label(format!(
            "{} /div visible  ({:.0}× zoom)",
            fmt_si(zoom_s / 10.0, "s"),
            1.0 / span
        ));
        ui.horizontal(|ui| {
            if button(ui, Icon::ZoomOut, "Zoom out (wider window)", 24.0).clicked() {
                crate::view::hview_zoom(phosphor, center, false);
            }
            if button(ui, Icon::ZoomIn, "Zoom in (narrower window)", 24.0).clicked() {
                crate::view::hview_zoom(phosphor, center, true);
            }
            if button(ui, Icon::Recenter, "Reset window to the full record", 24.0).clicked() {
                crate::view::hview_home(phosphor);
            }
        });
        let mut span_f = span as f32;
        if ui
            .add_enabled(
                true,
                egui::Slider::new(&mut span_f, 0.01..=1.0)
                    .logarithmic(true)
                    .text("Window"),
            )
            .changed()
        {
            phosphor.hview = crate::view::hview_clamp(center, span_f as f64);
        }
        if span < 0.999 {
            let mut centre_f = center as f32;
            if ui
                .add(egui::Slider::new(&mut centre_f, 0.0..=1.0).text("Centre"))
                .changed()
            {
                phosphor.hview = crate::view::hview_clamp(centre_f as f64, span);
            }
        }
    });
}
