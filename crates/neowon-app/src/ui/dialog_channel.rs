//! Channel (vertical) dialog — per-channel scale, coupling, probe, offset.

use bevy_egui::egui;
use neowon_core::Coupling;

use crate::Link;

use super::widgets::{FALLBACK_VDIV, PROBES, ladder_combo};
use crate::derived::fmt_si;

pub fn show(ui: &mut egui::Ui, link: &mut Link, ch: usize) {
    let vdiv_ladder: Vec<f64> = link
        .caps
        .as_ref()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| FALLBACK_VDIV.to_vec());

    ui.group(|ui| {
        ui.strong(format!("Channel {}", ch + 1));
        let mut c = link.config.channels[ch];
        let mut dirty = false;
        dirty |= ui.checkbox(&mut c.enabled, "Enabled").changed();
        dirty |= ladder_combo(
            ui,
            &format!("vdiv{ch}"),
            "V/div",
            &mut c.volts_div,
            &vdiv_ladder,
            |v| fmt_si(v, "V"),
        );
        ui.horizontal(|ui| {
            ui.label("Coupling");
            for (label, v) in [
                ("DC", Coupling::Dc),
                ("AC", Coupling::Ac),
                ("GND", Coupling::Gnd),
            ] {
                if ui.selectable_label(c.coupling == v, label).clicked() {
                    c.coupling = v;
                    dirty = true;
                }
            }
        });
        let mut probe = c.probe;
        if ladder_combo(
            ui,
            &format!("probe{ch}"),
            "Probe",
            &mut probe,
            &PROBES,
            |v| format!("×{v}"),
        ) {
            c.probe = probe;
            dirty = true;
        }
        let mut off = c.offset as f32;
        if ui
            .add(egui::Slider::new(&mut off, -0.5..=0.5).text("Offset"))
            .changed()
        {
            c.offset = off as f64;
            dirty = true;
        }
        if dirty {
            link.config.channels[ch] = c;
            link.dirty = true;
        }
    });
}
