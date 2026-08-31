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

pub fn show(
    ui: &mut egui::Ui,
    link: &mut Link,
    phosphor: &mut Phosphor,
    deep: &mut crate::deep::DeepView,
) {
    timeline_group(ui, link, phosphor, deep);
    show_inner(ui, link, phosphor)
}

/// Timeline (deep) view: span more time than one record holds, at the
/// acquisition's own sample rate, by drawing the recorded history on a real
/// time axis.
fn timeline_group(
    ui: &mut egui::Ui,
    link: &Link,
    phosphor: &mut Phosphor,
    deep: &mut crate::deep::DeepView,
) {
    ui.group(|ui| {
        ui.strong("Timeline");
        let record_s = view::record_len(link) as f64 / link.config.sample_rate.max(1e-12);
        let mut on = deep.on;
        if ui
            .checkbox(&mut on, "Span history, not one record")
            .changed()
        {
            crate::deep::set_on(deep, phosphor, on);
        }
        if !deep.on {
            ui.label(
                egui::RichText::new(format!(
                    "off — the display shows one {} record",
                    fmt_si(record_s, "s")
                ))
                .weak()
                .small(),
            );
            return;
        }
        ui.label(
            egui::RichText::new(format!(
                "{} /div  ({} window)",
                fmt_si(deep.seconds_per_div(), "s"),
                fmt_si(deep.span, "s"),
            ))
            .monospace()
            .size(10.0),
        );
        ui.horizontal(|ui| {
            if button(ui, Icon::ZoomOut, "Longer window", 24.0).clicked() {
                crate::deep::span_step(deep, true);
            }
            if button(ui, Icon::ZoomIn, "Shorter window", 24.0).clicked() {
                crate::deep::span_step(deep, false);
            }
            let anchored = deep.anchor.is_some();
            if ui
                .add_enabled(anchored, egui::Button::new("Jump to now"))
                .on_hover_text(
                    "Return the window to the newest data and keep it there \
                     as acquisition continues. Dragging the plot scrolls back \
                     through history and leaves it parked.",
                )
                .clicked()
            {
                deep.anchor = None;
            }
        });
        // What the instrument actually saw, and what it missed.
        ui.label(
            egui::RichText::new(format!(
                "≈{:.0}% not acquired · {} breaks · {} records",
                deep.lost() * 100.0,
                deep.gap_count,
                deep.records,
            ))
            .monospace()
            .size(10.0)
            .color(if deep.lost() > 0.5 {
                egui::Color32::from_rgb(220, 120, 90)
            } else {
                egui::Color32::GRAY
            }),
        );
        let mut collapse = deep.collapse;
        if ui
            .checkbox(&mut collapse, "Close up the gaps")
            .on_hover_text(
                "Lay the acquired stretches end to end and mark each join with \
                 a single line, so the signal gets the full width. The x axis \
                 is then not time — a measurement spanning a join is short by \
                 however long the instrument was not acquiring — so time \
                 readouts are hidden while this is on.",
            )
            .changed()
        {
            deep.collapse = collapse;
        }
        ui.label(
            egui::RichText::new(if deep.anchor.is_some() {
                "parked in history — drag the plot to scroll"
            } else {
                "following the newest data"
            })
            .weak()
            .small(),
        );
    });
}

fn show_inner(ui: &mut egui::Ui, link: &mut Link, phosphor: &mut Phosphor) {
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
