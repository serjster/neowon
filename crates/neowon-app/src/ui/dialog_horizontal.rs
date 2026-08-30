//! Horizontal dialog — the bench-scope horizontal controls in the order a
//! front panel puts them: **time base** (s/div, the primary control),
//! **position** (trigger delay), and **zoom** (delayed sweep, a magnified
//! window into the record). See docs/tasks/phase78-lab-semantics-spec.md §1.

use bevy_egui::egui;

use crate::Link;
use crate::gpu::Phosphor;
use crate::view;

use super::icons::{Icon, button};
use super::knob::knob;
use super::widgets::ladder_combo;
use crate::derived::fmt_si;

pub fn show(ui: &mut egui::Ui, link: &mut Link, phosphor: &mut Phosphor) {
    let record_len = view::record_len(link);
    let tb_ladder = view::timebase_ladder(link);
    let record_s = record_len as f64 / link.config.sample_rate;

    ui.group(|ui| {
        ui.strong("Time base");
        // s/div is the control; the sample rate is what it costs. Slower
        // s/div walks down the rate ladder, which is how the instrument
        // reaches seconds per division.
        let mut s_div = view::timebase(link);
        if ladder_combo(ui, "sdiv", "s/div", &mut s_div, &tb_ladder, |v| {
            fmt_si(v, "s")
        }) {
            view::set_timebase(link, s_div);
        }
        ui.horizontal(|ui| {
            if button(ui, Icon::ZoomOut, "Slower time base (more s/div)", 24.0).clicked() {
                view::timebase_step(link, true);
            }
            if button(ui, Icon::ZoomIn, "Faster time base (fewer s/div)", 24.0).clicked() {
                view::timebase_step(link, false);
            }
            let mut tb = view::timebase(link);
            if knob(
                ui,
                "s/div",
                &mut tb,
                (tb_ladder[0], *tb_ladder.last().unwrap()),
                Some(&tb_ladder),
                view::s_per_div(view::startup_config().sample_rate, record_len),
                |v| fmt_si(v, "s"),
            ) {
                view::set_timebase(link, tb);
            }
        });
        ui.label(
            egui::RichText::new(format!(
                "{} · {} span · {record_len} pts",
                fmt_si(link.config.sample_rate, "S/s"),
                fmt_si(record_s, "s"),
            ))
            .monospace()
            .size(10.0),
        );
    });

    ui.group(|ui| {
        ui.strong("Position");
        // Trigger delay: where the trigger point sits in the record. Shown
        // in seconds from the record's centre, the way scopes label delay.
        let delay = (link.config.position - 0.5) * record_s;
        ui.label(
            egui::RichText::new(format!("delay {}", fmt_si(delay, "s")))
                .monospace()
                .size(10.0),
        );
        ui.horizontal(|ui| {
            let mut pos = link.config.position;
            if knob(ui, "Position", &mut pos, (0.0, 1.0), None, 0.5, |v| {
                format!("{:.0}%", v * 100.0)
            }) {
                link.config.position = pos;
                link.dirty = true;
            }
            if ui.button("Set to 50%").clicked() {
                link.config.position = 0.5;
                link.dirty = true;
            }
        });
    });

    ui.group(|ui| {
        ui.strong("Zoom");
        let mut on = view::zoom_active(phosphor);
        if ui
            .checkbox(&mut on, "Zoom window (delayed sweep)")
            .changed()
        {
            view::set_zoom(phosphor, on);
        }
        if !on {
            ui.label(
                egui::RichText::new("off — the display shows the whole record")
                    .weak()
                    .small(),
            );
            return;
        }
        let (center, span) = phosphor.hview;
        let zoom_s = record_s * span;
        ui.label(
            egui::RichText::new(format!(
                "{} /div  ({:.0}x)",
                fmt_si(zoom_s / 10.0, "s"),
                1.0 / span
            ))
            .monospace()
            .size(10.0),
        );
        ui.horizontal(|ui| {
            if button(ui, Icon::ZoomOut, "Wider zoom window", 24.0).clicked() {
                view::hview_zoom(phosphor, center, false);
            }
            if button(ui, Icon::ZoomIn, "Narrower zoom window", 24.0).clicked() {
                view::hview_zoom(phosphor, center, true);
            }
            if button(ui, Icon::Recenter, "Zoom window to the whole record", 24.0).clicked() {
                view::hview_home(phosphor);
            }
        });
        let mut span_f = span as f32;
        if ui
            .add(
                egui::Slider::new(&mut span_f, view::HVIEW_MIN_SPAN as f32..=1.0)
                    .logarithmic(true)
                    .text("Window"),
            )
            .changed()
        {
            phosphor.hview = view::hview_clamp(center, span_f as f64);
        }
        let mut centre_f = center as f32;
        if ui
            .add(egui::Slider::new(&mut centre_f, 0.0..=1.0).text("Centre"))
            .changed()
        {
            phosphor.hview = view::hview_clamp(centre_f as f64, span);
        }
    });
}
