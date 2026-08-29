//! Cursor dialog — time/amplitude cursors and readouts.

use bevy_egui::egui;

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{MeasureState, fmt_si};

pub fn show(ui: &mut egui::Ui, link: &Link, cur: &mut CursorState, meas: &MeasureState) {
    ui.group(|ui| {
        ui.strong("Cursors");
        ui.checkbox(&mut cur.time_on, "Time cursors");
        ui.checkbox(&mut cur.amp_on, "Amplitude cursors");
        let record_s = 5000.0 / meas.sample_rate.max(1.0);
        if cur.time_on {
            let dt = ((cur.pos[1] - cur.pos[0]).abs() as f64) * record_s;
            ui.label(format!(
                "Δt = {}   1/Δt = {}",
                fmt_si(dt, "s"),
                fmt_si(1.0 / dt.max(1e-12), "Hz")
            ));
        }
        if cur.amp_on {
            let c = link.config.channels[0];
            let fs = c.volts_div * 10.0 * c.probe;
            let dv = ((cur.pos[3] - cur.pos[2]).abs() as f64) * fs;
            ui.label(format!("ΔV = {} (CH1 scale)", fmt_si(dv, "V")));
        }
        ui.label(
            egui::RichText::new("drag cursors on the plot")
                .weak()
                .small(),
        );
    });
}
