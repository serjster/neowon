//! egui control panel, measurement table, and spectrum window.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use neowon_backend::{Command, MultiMode};
use neowon_core::{AcqMode, Coupling, PulseCondition, Slope, Sweep, TriggerKind, VideoSync};
use neowon_dsp::{MathOp, Window};

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{
    FftState, METRICS, MathState, MeasureState, PfState, SLOT_NAMES, SLOTS, build_pf_mask, fmt,
    fmt_si,
};
use crate::gpu::{Persistence, Phosphor, TraceMode};

const FALLBACK_VDIV: [f64; 10] = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
const FALLBACK_RATES: [f64; 6] = [2.5e3, 25e3, 250e3, 2.5e6, 25e6, 100e6];
const PROBES: [f64; 7] = [1.0, 10.0, 20.0, 50.0, 100.0, 500.0, 1000.0];

fn ladder_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    current: &mut f64,
    ladder: &[f64],
    fmt_val: impl Fn(f64) -> String,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(fmt_val(*current))
            .show_ui(ui, |ui| {
                for &v in ladder {
                    if ui
                        .selectable_label((*current - v).abs() < v * 1e-6, fmt_val(v))
                        .clicked()
                    {
                        *current = v;
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn multi_label(m: MultiMode) -> &'static str {
    match m {
        MultiMode::TriggerOut => "Trigger out",
        MultiMode::PassFailOut => "Pass-fail out",
        MultiMode::TriggerIn => "Trigger in",
    }
}

fn condition_combo(ui: &mut egui::Ui, id: &str, condition: &mut PulseCondition) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("When");
        egui::ComboBox::from_id_salt(id)
            .selected_text(condition.label())
            .show_ui(ui, |ui| {
                for c in PulseCondition::ALL {
                    if ui.selectable_label(*condition == c, c.label()).clicked() {
                        *condition = c;
                        changed = true;
                    }
                }
            });
    });
    changed
}

#[allow(clippy::too_many_arguments)]
pub fn panel(
    mut contexts: EguiContexts,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut math: ResMut<MathState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
    mut cur: ResMut<CursorState>,
    mut pf: ResMut<PfState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let mut root = egui::Ui::new(
        ctx.clone(),
        "ui-root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    let vdiv_ladder: Vec<f64> = link
        .caps
        .as_ref()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| FALLBACK_VDIV.to_vec());
    let rate_ladder: Vec<f64> = link
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| FALLBACK_RATES.to_vec());

    egui::Panel::left("controls")
        .default_size(285.0)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(link.status.clone()).small());
                ui.horizontal(|ui| {
                    let running = link.config.running;
                    if ui
                        .button(if running { "⏸ Stop" } else { "▶ Run" })
                        .clicked()
                    {
                        link.config.running = !running;
                        link.dirty = true;
                    }
                    if ui.button("Auto-set").clicked() {
                        let _ = link.sup.commands.send(Command::AutoSet);
                    }
                    if ui.button("Force trig").clicked() {
                        let _ = link.sup.commands.send(Command::ForceTrigger);
                    }
                });
                ui.separator();

                ui.collapsing("Horizontal", |ui| {
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
                });

                for ch in 0..2 {
                    ui.collapsing(format!("Channel {}", ch + 1), |ui| {
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

                ui.collapsing("Trigger", |ui| {
                    let mut t = link.config.trigger;
                    let mut dirty = false;
                    ui.horizontal(|ui| {
                        ui.label("Source");
                        for src in 0..2 {
                            if ui
                                .selectable_label(t.source == src, format!("CH{}", src + 1))
                                .clicked()
                            {
                                t.source = src;
                                dirty = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Type");
                        egui::ComboBox::from_id_salt("trigkind")
                            .selected_text(t.kind.label())
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        matches!(t.kind, TriggerKind::Edge { .. }),
                                        "Edge",
                                    )
                                    .clicked()
                                {
                                    let slope = match t.kind {
                                        TriggerKind::Edge { slope } => slope,
                                        _ => Slope::Rising,
                                    };
                                    t.kind = TriggerKind::Edge { slope };
                                    dirty = true;
                                }
                                if ui
                                    .selectable_label(
                                        matches!(t.kind, TriggerKind::Pulse { .. }),
                                        "Pulse",
                                    )
                                    .clicked()
                                {
                                    t.kind = TriggerKind::Pulse {
                                        condition: PulseCondition::PositiveGreater,
                                        width: 1e-6,
                                    };
                                    dirty = true;
                                }
                                if ui
                                    .selectable_label(
                                        matches!(t.kind, TriggerKind::Slope { .. }),
                                        "Slope",
                                    )
                                    .clicked()
                                {
                                    t.kind = TriggerKind::Slope {
                                        condition: PulseCondition::PositiveGreater,
                                        width: 1e-6,
                                        upper: t.level + 0.1,
                                        lower: t.level - 0.1,
                                    };
                                    dirty = true;
                                }
                                if ui
                                    .selectable_label(
                                        matches!(t.kind, TriggerKind::Video { .. }),
                                        "Video",
                                    )
                                    .clicked()
                                {
                                    t.kind = TriggerKind::Video {
                                        sync: VideoSync::Line,
                                        line: 1,
                                    };
                                    dirty = true;
                                }
                            });
                    });
                    match &mut t.kind {
                        TriggerKind::Edge { slope } => {
                            ui.horizontal(|ui| {
                                ui.label("Slope");
                                for (label, s) in
                                    [("Rising ⬈", Slope::Rising), ("Falling ⬊", Slope::Falling)]
                                {
                                    if ui.selectable_label(*slope == s, label).clicked() {
                                        *slope = s;
                                        dirty = true;
                                    }
                                }
                            });
                        }
                        TriggerKind::Pulse { condition, width } => {
                            dirty |= condition_combo(ui, "pulsecond", condition);
                            let mut w_us = *width * 1e6;
                            ui.horizontal(|ui| {
                                ui.label("Width");
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut w_us)
                                            .speed(0.1)
                                            .range(0.01..=655_360.0)
                                            .suffix(" µs"),
                                    )
                                    .changed()
                                {
                                    *width = w_us * 1e-6;
                                    dirty = true;
                                }
                            });
                        }
                        TriggerKind::Slope {
                            condition,
                            width,
                            upper,
                            lower,
                        } => {
                            dirty |= condition_combo(ui, "slopecond", condition);
                            let mut w_us = *width * 1e6;
                            ui.horizontal(|ui| {
                                ui.label("Width");
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut w_us)
                                            .speed(0.1)
                                            .range(0.01..=655_360.0)
                                            .suffix(" µs"),
                                    )
                                    .changed()
                                {
                                    *width = w_us * 1e-6;
                                    dirty = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Upper");
                                if ui
                                    .add(egui::DragValue::new(upper).speed(0.01).suffix(" V"))
                                    .changed()
                                {
                                    dirty = true;
                                }
                                ui.label("Lower");
                                if ui
                                    .add(egui::DragValue::new(lower).speed(0.01).suffix(" V"))
                                    .changed()
                                {
                                    dirty = true;
                                }
                            });
                        }
                        TriggerKind::Video { sync, line } => {
                            ui.horizontal(|ui| {
                                ui.label("Sync");
                                egui::ComboBox::from_id_salt("vidsync")
                                    .selected_text(sync.label())
                                    .show_ui(ui, |ui| {
                                        for s in VideoSync::ALL {
                                            if ui.selectable_label(*sync == s, s.label()).clicked()
                                            {
                                                *sync = s;
                                                dirty = true;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Line #");
                                if ui
                                    .add(egui::DragValue::new(line).range(0..=65535))
                                    .changed()
                                {
                                    dirty = true;
                                }
                            });
                            ui.label(
                                egui::RichText::new(
                                    "Video trigger: packing unverified on hardware",
                                )
                                .weak()
                                .small(),
                            );
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("Sweep");
                        for (label, s) in [
                            ("Auto", Sweep::Auto),
                            ("Normal", Sweep::Normal),
                            ("Single", Sweep::Single),
                        ] {
                            if ui.selectable_label(t.sweep == s, label).clicked() {
                                t.sweep = s;
                                if s == Sweep::Single {
                                    link.config.running = true;
                                }
                                dirty = true;
                            }
                        }
                    });
                    if matches!(t.kind, TriggerKind::Edge { .. } | TriggerKind::Pulse { .. }) {
                        let mut level = t.level;
                        ui.horizontal(|ui| {
                            ui.label("Level");
                            if ui
                                .add(egui::DragValue::new(&mut level).speed(0.01).suffix(" V"))
                                .changed()
                            {
                                t.level = level;
                                dirty = true;
                            }
                        });
                    }
                    let mut holdoff_us = t.holdoff * 1e6;
                    ui.horizontal(|ui| {
                        ui.label("Holdoff");
                        if ui
                            .add(
                                egui::DragValue::new(&mut holdoff_us)
                                    .speed(0.1)
                                    .range(0.1..=10_000_000.0)
                                    .suffix(" µs"),
                            )
                            .changed()
                        {
                            t.holdoff = holdoff_us * 1e-6;
                            dirty = true;
                        }
                    });
                    if dirty {
                        link.config.trigger = t;
                        link.dirty = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label("MULTI");
                        egui::ComboBox::from_id_salt("multiport")
                            .selected_text(multi_label(link.multi))
                            .show_ui(ui, |ui| {
                                for (m, label) in [
                                    (MultiMode::TriggerOut, "Trigger out"),
                                    (MultiMode::PassFailOut, "Pass-fail out"),
                                    (MultiMode::TriggerIn, "Trigger in"),
                                ] {
                                    if ui.selectable_label(link.multi == m, label).clicked() {
                                        link.multi = m;
                                        let _ = link.sup.commands.send(Command::Multi(m));
                                    }
                                }
                            });
                    });
                });

                ui.collapsing("Pass/Fail", |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut pf.enabled, "Enabled");
                        ui.label("Src");
                        for (slot, name) in SLOT_NAMES.iter().enumerate() {
                            if ui.selectable_label(pf.source_slot == slot, *name).clicked() {
                                pf.source_slot = slot;
                                pf.mask = None; // reference came from another trace
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("H tol");
                        ui.add(
                            egui::DragValue::new(&mut pf.h_div)
                                .speed(0.05)
                                .range(0.0..=20.0)
                                .suffix(" div"),
                        );
                        ui.label("V tol");
                        ui.add(
                            egui::DragValue::new(&mut pf.v_div)
                                .speed(0.05)
                                .range(0.0..=10.0)
                                .suffix(" div"),
                        );
                    });
                    ui.horizontal(|ui| {
                        let can_capture = if pf.source_slot < 2 {
                            link.latest
                                .as_ref()
                                .and_then(|f| f.channels.iter().find(|c| c.ch == pf.source_slot))
                                .is_some()
                        } else {
                            math.trace.is_some()
                        };
                        if ui
                            .add_enabled(can_capture, egui::Button::new("Capture reference"))
                            .clicked()
                        {
                            let raw: Option<Vec<i8>> = if pf.source_slot < 2 {
                                link.latest
                                    .as_ref()
                                    .and_then(|f| {
                                        f.channels.iter().find(|c| c.ch == pf.source_slot)
                                    })
                                    .map(|c| c.raw.clone())
                            } else {
                                math.trace.as_ref().map(|c| c.raw.clone())
                            };
                            if let Some(raw) = raw {
                                pf.mask = Some(build_pf_mask(&raw, pf.h_div, pf.v_div));
                                pf.pass = 0;
                                pf.fail = 0;
                            }
                        }
                        if ui.button("Reset counts").clicked() {
                            pf.pass = 0;
                            pf.fail = 0;
                        }
                    });
                    ui.checkbox(&mut pf.stop_on_fail, "Stop on fail");
                    ui.checkbox(&mut pf.output_multi, "Output result to MULTI");
                    let total = pf.pass + pf.fail;
                    ui.label(format!(
                        "pass {}   fail {}   total {}",
                        pf.pass, pf.fail, total
                    ));
                    if pf.mask.is_none() {
                        ui.label(egui::RichText::new("no reference captured").weak().small());
                    }
                });

                ui.collapsing("Acquire", |ui| {
                    let modes = [
                        ("Sample", AcqMode::Sample),
                        ("Peak", AcqMode::Peak),
                        ("Avg 4", AcqMode::Average(4)),
                        ("Avg 16", AcqMode::Average(16)),
                        ("Avg 64", AcqMode::Average(64)),
                    ];
                    ui.horizontal_wrapped(|ui| {
                        for (label, m) in modes {
                            if ui.selectable_label(link.config.acq == m, label).clicked() {
                                link.config.acq = m;
                                link.dirty = true;
                            }
                        }
                    });
                });

                ui.collapsing("Math (M = f(CH1, CH2))", |ui| {
                    ui.checkbox(&mut math.enabled, "Enabled");
                    egui::ComboBox::from_id_salt("mathop")
                        .selected_text(math.op.label())
                        .show_ui(ui, |ui| {
                            for op in MathOp::ALL {
                                if ui.selectable_label(math.op == op, op.label()).clicked() {
                                    math.op = op;
                                    math.rescale = true;
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "Scale: {} /div",
                            fmt_si(math.full_scale / 10.0, math.op.unit())
                        ));
                        if ui.button("Auto").clicked() {
                            math.rescale = true;
                        }
                    });
                });

                ui.collapsing("Display", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Persist");
                        for p in Persistence::LADDER {
                            if ui
                                .selectable_label(phosphor.persistence == p, p.label())
                                .clicked()
                            {
                                phosphor.persistence = p;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Trace");
                        for (label, m) in [
                            ("Vectors", TraceMode::Vectors),
                            ("Dots", TraceMode::Dots),
                            ("XY", TraceMode::Xy),
                        ] {
                            if ui.selectable_label(phosphor.mode == m, label).clicked() {
                                phosphor.mode = m;
                            }
                        }
                    });
                    ui.add(egui::Slider::new(&mut phosphor.gain, 0.05..=3.0).text("Intensity"));
                });

                ui.collapsing("Cursors", |ui| {
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
                });

                ui.checkbox(&mut fft.enabled, "Spectrum (FFT)");
            });
        });

    egui::Panel::bottom("measure")
        .resizable(true)
        .default_size(185.0)
        .show(&mut root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Measurements");
                ui.label("stats for");
                for (slot, name) in SLOT_NAMES.iter().enumerate() {
                    if meas.latest[slot].is_some()
                        && ui
                            .selectable_label(meas.stats_slot == slot, *name)
                            .clicked()
                    {
                        meas.stats_slot = slot;
                    }
                }
                if ui.button("Reset stats").clicked() {
                    meas.reset_stats();
                }
            });
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("meas-grid")
                    .striped(true)
                    .min_col_width(64.0)
                    .show(ui, |ui| {
                        ui.label("");
                        for (slot, name) in SLOT_NAMES.iter().enumerate() {
                            if meas.latest[slot].is_some() {
                                ui.label(egui::RichText::new(*name).strong());
                            }
                        }
                        let s = meas.stats_slot;
                        for h in ["mean", "min", "max", "σ"] {
                            ui.label(
                                egui::RichText::new(format!("{h} ({})", SLOT_NAMES[s])).weak(),
                            );
                        }
                        ui.end_row();
                        for (i, (name, get, unit)) in METRICS.iter().enumerate() {
                            ui.label(*name);
                            for slot in 0..SLOTS {
                                if let Some(m) = &meas.latest[slot] {
                                    ui.label(get(m).map_or("—".into(), |v| fmt(v, *unit)));
                                }
                            }
                            if !meas.stats.is_empty() {
                                let t = &meas.stats[meas.stats_slot][i];
                                if t.count > 0 {
                                    ui.label(fmt(t.mean, *unit));
                                    ui.label(fmt(t.min, *unit));
                                    ui.label(fmt(t.max, *unit));
                                    ui.label(fmt(t.std_dev(), *unit));
                                } else {
                                    for _ in 0..4 {
                                        ui.label("—");
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        });

    if fft.enabled {
        egui::Window::new("Spectrum")
            .default_width(680.0)
            .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Source");
                    for (slot, name) in SLOT_NAMES.iter().enumerate() {
                        if ui.selectable_label(fft.source == slot, *name).clicked() {
                            fft.source = slot;
                        }
                    }
                    egui::ComboBox::from_id_salt("fftwnd")
                        .selected_text(fft.window.label())
                        .show_ui(ui, |ui| {
                            for w in Window::ALL {
                                if ui.selectable_label(fft.window == w, w.label()).clicked() {
                                    fft.window = w;
                                }
                            }
                        });
                });
                let (resp, painter) = ui.allocate_painter(
                    egui::vec2(ui.available_width(), 230.0),
                    egui::Sense::hover(),
                );
                let rect = resp.rect;
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(8, 10, 14));
                let (db_lo, db_hi) = (-100.0f32, 20.0f32);
                for db in (-100..=0).step_by(20) {
                    let y = rect.bottom() - (db as f32 - db_lo) / (db_hi - db_lo) * rect.height();
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        egui::Stroke::new(0.5, egui::Color32::from_gray(50)),
                    );
                    painter.text(
                        egui::pos2(rect.left() + 3.0, y - 2.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{db} dBV"),
                        egui::FontId::proportional(9.0),
                        egui::Color32::from_gray(110),
                    );
                }
                if let Some(s) = &fft.spectrum {
                    let n = s.amplitude.len();
                    let pts: Vec<egui::Pos2> = (1..n)
                        .map(|i| {
                            let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                            let y = rect.bottom()
                                - (s.dbv(i) as f32 - db_lo) / (db_hi - db_lo) * rect.height();
                            egui::pos2(x, y.clamp(rect.top(), rect.bottom()))
                        })
                        .collect();
                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 216, 40)),
                    ));
                    let nyquist = s.bin_hz * n as f64;
                    ui.horizontal(|ui| {
                        ui.label(format!("span 0 — {}", fmt_si(nyquist, "Hz")));
                        if let Some((f, a)) = s.peak() {
                            ui.label(format!(
                                "   peak: {} at {} ({:.1} dBV)",
                                fmt_si(a, "V"),
                                fmt_si(f, "Hz"),
                                20.0 * a.max(1e-12).log10()
                            ));
                        }
                    });
                } else {
                    ui.label("no data");
                }
            });
    }
}
