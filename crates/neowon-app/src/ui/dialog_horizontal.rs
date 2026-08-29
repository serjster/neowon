//! Horizontal dialog — timebase (sample rate) and trigger position.

use bevy_egui::egui;

use crate::Link;

use super::widgets::{FALLBACK_RATES, ladder_combo};
use crate::derived::fmt_si;

/// Samples per record before capabilities arrive (VDS1022/sim shape).
const FALLBACK_RECORD_LEN: usize = 5000;

pub fn show(ui: &mut egui::Ui, link: &mut Link) {
    let rate_ladder: Vec<f64> = link
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| FALLBACK_RATES.to_vec());

    ui.group(|ui| {
        ui.strong("Horizontal");
        let record_len = link
            .caps
            .as_ref()
            .map(|c| c.record_len)
            .unwrap_or(FALLBACK_RECORD_LEN);
        let record_s = record_len as f64 / link.config.sample_rate;
        ui.label(format!(
            "Main {} /div   ({} record)",
            fmt_si(record_s / 10.0, "s"),
            fmt_si(record_s, "s")
        ));
        let mut rate = link.config.sample_rate;
        if ladder_combo(ui, "rate", "Rate", &mut rate, &rate_ladder, |v| {
            fmt_si(v, "S/s")
        }) {
            link.config.sample_rate = rate;
            link.dirty = true;
        }
        let mut pos = link.config.position as f32;
        if ui
            .add(egui::Slider::new(&mut pos, 0.0..=1.0).text("Trig position"))
            .changed()
        {
            link.config.position = pos as f64;
            link.dirty = true;
        }
        if ui.button("Trigger position → 50%").clicked() {
            link.config.position = 0.5;
            link.dirty = true;
        }
    });
}
