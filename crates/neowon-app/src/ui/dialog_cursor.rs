//! Cursor dialog — time/amplitude cursors and readouts.

use bevy_egui::egui;

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{MeasureState, fmt_si_sticky};

pub fn show(ui: &mut egui::Ui, link: &Link, cur: &mut CursorState, meas: &MeasureState) {
    ui.group(|ui| {
        ui.strong("Cursors");
        ui.checkbox(&mut cur.time_on, "Time cursors");
        ui.checkbox(&mut cur.amp_on, "Amplitude cursors");
        let record_s = 5000.0 / meas.sample_rate.max(1.0);
        if cur.time_on {
            let dt = ((cur.pos[1] - cur.pos[0]).abs() as f64) * record_s;
            let dt_s = fmt_si_sticky(dt, "s", &mut cur.bands[0]);
            let dt_hz = fmt_si_sticky(1.0 / dt.max(1e-12), "Hz", &mut cur.bands[1]);
            ui.label(egui::RichText::new(format!("Δt = {dt_s}   1/Δt = {dt_hz}")).monospace());
        }
        if cur.amp_on {
            let c = link.config.channels[0];
            let fs = c.volts_div * 10.0 * c.probe;
            let dv = ((cur.pos[3] - cur.pos[2]).abs() as f64) * fs;
            let dv = fmt_si_sticky(dv, "V", &mut cur.bands[2]);
            ui.label(egui::RichText::new(format!("ΔV = {dv} (CH1 scale)")).monospace());
        }
        ui.label(
            egui::RichText::new("drag cursors on the plot")
                .weak()
                .small(),
        );
    });
}
