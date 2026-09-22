//! The SDR dock: tuning and display controls, detected signals, and the
//! IQ constellation. Every control injects a script action.

use bevy_egui::egui;
use neowon_backend::SdrGain;

use super::sdr_view::{BG, GRID, TRACE, fmt_mhz, inject};
use crate::Link;
use crate::refmap::RefMap;
use crate::script::{Action, Script};
use crate::sdr::{FFT_SIZES, SdrAction, SdrState};
use crate::uitree;

const SPANS: [f64; 6] = [0.0, 1e6, 500e3, 200e3, 100e3, 50e3];

/// Mouse gestures on the canvas, shown on hover rather than as fine print.
const GESTURES: &str = "click: tune\n\
    drag a channel edge: width\n\
    drag: pan (vertical: reference level)\n\
    right-drag: move the hardware window\n\
    scroll: span + sample rate · shift+scroll: pan\n\
    ctrl+scroll: dB range\n\
    double-click: reset the view";

/// The whole dock: titled, collapsible sections like the scope's, labels
/// in the left column. Run/stop, tune steps and the views live on the
/// front panel; the device is named in the app bar.
pub fn show(ui: &mut egui::Ui, sdr: &SdrState, link: &Link, rm: &RefMap, script: &mut Script) {
    section(ui, "Tuning", true, |ui| tuning(ui, sdr, rm, script));
    section(ui, "Receiver", true, |ui| receiver(ui, sdr, script));
    section(ui, "Display", false, |ui| display(ui, sdr, script));
    section(ui, "Audio", true, |ui| audio(ui, sdr, script));
    // While the receiver runs the section is held open: turning DAB on is a
    // request to see the ensemble, and a collapsed section would hide the
    // table, the DLS line and the service list the switch exists for.
    section_state(
        ui,
        "DAB",
        sdr.dab.is_some(),
        sdr.dab.is_some().then_some(true),
        |ui| super::dab_dock::show(ui, sdr, rm, script),
    );
    section(ui, "Signals", true, |ui| signals(ui, sdr, script));
    section(ui, "Analysis", sdr.analyse_on, |ui| {
        lab(ui, sdr, script);
        constellation(ui, sdr);
    });
    if sdr.caps.as_ref().is_some_and(|c| c.tuner == "sim") {
        section(ui, "Simulator", false, |ui| {
            egui::ComboBox::from_label("Scene")
                .selected_text(link.stimulus.clone())
                .show_ui(ui, |ui| {
                    for name in neowon_sim::RfScene::PRESETS {
                        if ui.selectable_label(link.stimulus == name, name).clicked() {
                            script.inject(Action::Stimulus(name.into()));
                        }
                    }
                });
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    section_state(ui, title, open, None, body);
}

/// `section` with an optional forced open/closed state (`None` leaves the
/// header user-controlled).
fn section_state(
    ui: &mut egui::Ui,
    title: &str,
    default_open: bool,
    force_open: Option<bool>,
    body: impl FnOnce(&mut egui::Ui),
) {
    let r = egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("sdr-dock", title))
        .default_open(default_open)
        .open(force_open)
        .show(ui, body);
    uitree::node(
        ui.ctx(),
        ui.id().with(("sdr-section", title)),
        egui::accesskit::Role::Group,
        &format!("dock section {title}"),
        r.header_response
            .rect
            .union(r.body_response.map_or(r.header_response.rect, |b| b.rect)),
    );
}

/// A two-column grid: label left, control right.
fn grid(ui: &mut egui::Ui, id: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, body);
}

fn tuning(ui: &mut egui::Ui, sdr: &SdrState, rm: &RefMap, script: &mut Script) {
    let c = &sdr.config;
    ui.label("Tuned (MHz)").on_hover_text(GESTURES);
    let mut mhz = sdr.tuned_hz / 1e6;
    // The primary readout: larger than everything else in the dock.
    ui.style_mut().override_text_style = Some(egui::TextStyle::Heading);
    let tuned = ui
        .add(
            egui::DragValue::new(&mut mhz)
                .speed(0.001)
                .fixed_decimals(4)
                .custom_formatter(|v, _| format!("{v:.4}")),
        )
        .on_hover_text(
            "the channel you listen to; the hardware window moves when the \
             target is outside it, or always with Follow",
        );
    ui.style_mut().override_text_style = None;
    if tuned.changed() {
        inject(script, SdrAction::Tune((mhz * 1e6).round()));
    }
    if let Some(b) = rm.at(sdr.tuned_hz).first() {
        ui.label(egui::RichText::new(&b.name).color(crate::refmap::colour(&b.kind)))
            .on_hover_text(crate::refmap::fmt_range(b.lo_hz, b.hi_hz));
    }
    grid(ui, "sdr-tuning", |ui| {
        ui.label("Centre");
        ui.horizontal(|ui| {
            let mut cmhz = c.centre_hz / 1e6;
            if ui
                .add(
                    egui::DragValue::new(&mut cmhz)
                        .speed(0.01)
                        .fixed_decimals(4),
                )
                .on_hover_text("the hardware window's centre; right-drag the canvas to move it")
                .changed()
            {
                inject(script, SdrAction::Centre(cmhz * 1e6));
            }
            ui.weak("MHz");
        });
        ui.end_row();
        ui.label("Follow");
        let mut follow = sdr.follow;
        if ui
            .checkbox(&mut follow, "window on tuned")
            .on_hover_text("keep the hardware window centred on the tuned frequency")
            .changed()
        {
            inject(script, SdrAction::Follow(follow));
        }
        ui.end_row();
        ui.label("Width");
        ui.horizontal(|ui| {
            let mut auto = sdr.width_auto;
            if ui
                .checkbox(&mut auto, "auto")
                .on_hover_text("from the nearest detected signal's occupied bandwidth")
                .changed()
            {
                inject(
                    script,
                    SdrAction::Width(if auto { None } else { Some(sdr.width_hz) }),
                );
            }
            if sdr.width_auto {
                ui.weak(format!("{:.1} kHz", sdr.channel_width() / 1e3));
            } else {
                let mut khz = sdr.width_hz / 1e3;
                if ui
                    .add(
                        egui::DragValue::new(&mut khz)
                            .speed(0.1)
                            .range(0.001..=(c.sample_rate / 1e3))
                            .suffix(" kHz"),
                    )
                    .changed()
                {
                    inject(script, SdrAction::Width(Some((khz * 1e3).max(1.0))));
                }
            }
        });
        ui.end_row();
    });
}

fn receiver(ui: &mut egui::Ui, sdr: &SdrState, script: &mut Script) {
    let c = &sdr.config;
    let rates = sdr
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_default();
    grid(ui, "sdr-receiver", |ui| {
        ui.label("Rate");
        egui::ComboBox::from_id_salt("sdr-rate")
            .selected_text(format!("{:.3} MS/s", c.sample_rate / 1e6))
            .show_ui(ui, |ui| {
                for r in rates {
                    if ui
                        .selectable_label(r == c.sample_rate, format!("{:.3} MS/s", r / 1e6))
                        .clicked()
                    {
                        inject(script, SdrAction::Rate(r));
                    }
                }
            });
        ui.end_row();
        ui.label("Gain");
        ui.horizontal(|ui| {
            let auto = c.gain == SdrGain::Auto;
            let mut want_auto = auto;
            if ui.checkbox(&mut want_auto, "tuner AGC").changed() {
                inject(
                    script,
                    SdrAction::Gain(if want_auto { None } else { Some(29.7) }),
                );
            }
            if let (SdrGain::Manual(db), Some(caps)) = (c.gain, &sdr.caps) {
                let mut g = db;
                let max = caps.gains_db.last().copied().unwrap_or(50.0);
                if ui
                    .add(egui::Slider::new(&mut g, 0.0..=max).suffix(" dB"))
                    .changed()
                {
                    inject(script, SdrAction::Gain(Some(g)));
                }
            }
        });
        ui.end_row();
        ui.label("RTL AGC");
        let mut agc = c.agc;
        if ui.checkbox(&mut agc, "digital").changed() {
            inject(script, SdrAction::Agc(agc));
        }
        ui.end_row();
        ui.label("Correction");
        let mut ppm = c.ppm;
        if ui
            .add(
                egui::DragValue::new(&mut ppm)
                    .speed(0.1)
                    .range(-200.0..=200.0)
                    .suffix(" ppm"),
            )
            .changed()
        {
            inject(script, SdrAction::Ppm(ppm));
        }
        ui.end_row();
    });
}

fn display(ui: &mut egui::Ui, sdr: &SdrState, script: &mut Script) {
    grid(ui, "sdr-display", |ui| {
        ui.label("Span");
        egui::ComboBox::from_id_salt("sdr-span")
            .selected_text(format!("{:.0} kHz", sdr.span() / 1e3))
            .show_ui(ui, |ui| {
                for s in SPANS {
                    let text = if s == 0.0 {
                        "full".to_string()
                    } else {
                        format!("{:.0} kHz", s / 1e3)
                    };
                    if ui.selectable_label(sdr.span_hz == s, text).clicked() {
                        inject(script, SdrAction::Span(s));
                    }
                }
            });
        ui.end_row();
        ui.label("FFT");
        egui::ComboBox::from_id_salt("sdr-fft")
            .selected_text(sdr.fft_size.to_string())
            .show_ui(ui, |ui| {
                for n in FFT_SIZES {
                    if ui
                        .selectable_label(sdr.fft_size == n, n.to_string())
                        .clicked()
                    {
                        inject(script, SdrAction::Fft(n));
                    }
                }
            });
        ui.end_row();
        let (mut r, mut d) = (sdr.ref_db, sdr.range_db);
        ui.label("Reference");
        let a = ui
            .add(
                egui::DragValue::new(&mut r)
                    .speed(1.0)
                    .range(-150.0..=30.0)
                    .suffix(" dBFS"),
            )
            .changed();
        ui.end_row();
        ui.label("Range");
        let b = ui
            .add(
                egui::DragValue::new(&mut d)
                    .speed(1.0)
                    .range(10.0..=200.0)
                    .suffix(" dB"),
            )
            .changed();
        ui.end_row();
        if a || b {
            inject(
                script,
                SdrAction::Level {
                    ref_db: r,
                    range_db: d,
                },
            );
        }
        ui.label("Peak");
        ui.monospace(match sdr.peak() {
            Some((hz, db)) => format!("{}  {db:.1} dBFS", fmt_mhz(hz)),
            None => "-".into(),
        });
        ui.end_row();
        ui.label("Floor");
        ui.monospace(match &sdr.spectrum {
            Some(s) => format!("{:.1} dBFS  (bin {:.0} Hz)", s.median_db(), s.bin_hz),
            None => "-".into(),
        });
        ui.end_row();
    });
}

/// The audio section: demodulator, volume, mute, squelch and the one-line
/// state. Every silent state is named (`off`, `no device`, `muted`,
/// `squelched`), because they all sound the same.
fn audio(ui: &mut egui::Ui, sdr: &SdrState, script: &mut Script) {
    ui.horizontal(|ui| {
        let label = sdr.demod.map_or("Off", |m| m.label());
        egui::ComboBox::from_id_salt("sdr-demod")
            .selected_text(label)
            .show_ui(ui, |ui| {
                if ui.selectable_label(sdr.demod.is_none(), "Off").clicked() {
                    inject(script, SdrAction::Demod(None));
                }
                for m in neowon_dsp::DemodMode::ALL {
                    if ui
                        .selectable_label(sdr.demod == Some(m), m.label())
                        .clicked()
                    {
                        inject(script, SdrAction::Demod(Some(m)));
                    }
                }
            });
        let mut mute = sdr.mute;
        if ui.checkbox(&mut mute, "Mute").changed() {
            inject(script, SdrAction::Mute(mute));
        }
    });
    ui.horizontal(|ui| {
        let mut v = sdr.volume;
        if ui
            .add(egui::Slider::new(&mut v, 0.0..=1.0).text("vol"))
            .changed()
        {
            inject(script, SdrAction::Volume(v));
        }
    });
    ui.horizontal(|ui| {
        // -120 dBFS is "off" for the gate.
        let mut db = sdr.squelch_db.max(-120.0);
        ui.label("squelch dB");
        if ui
            .add(egui::DragValue::new(&mut db).speed(0.5).range(-120.0..=0.0))
            .changed()
        {
            inject(
                script,
                SdrAction::Squelch(if db <= -120.0 { None } else { Some(db) }),
            );
        }
    });
    let ch = if sdr.audio_channel_dbfs.is_finite() {
        format!("{:.0}", sdr.audio_channel_dbfs)
    } else {
        "-".into()
    };
    grid(ui, "sdr-audio-state", |ui| {
        ui.label("State");
        ui.monospace(sdr.audio_state());
        ui.end_row();
        ui.label("Channel");
        ui.monospace(format!("{ch:>5} dBFS"));
        ui.end_row();
        ui.label("Output");
        ui.monospace(format!("{:.2} rms", sdr.audio_rms));
        ui.end_row();
    });
}

/// The modulation lab: on/off, the assumed modulation, and its results
/// for the signal nearest the tuned frequency.
fn lab(ui: &mut egui::Ui, sdr: &SdrState, script: &mut Script) {
    ui.horizontal(|ui| {
        let mut on = sdr.analyse_on;
        if ui
            .checkbox(&mut on, "Analyse")
            .on_hover_text("modulation lab on the signal nearest the tuned frequency")
            .changed()
        {
            inject(script, SdrAction::Analyse(on));
        }
        let label = sdr.modulation.map_or("auto", |m| m.label());
        egui::ComboBox::from_id_salt("lab-modulation")
            .selected_text(label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(sdr.modulation.is_none(), "auto")
                    .clicked()
                {
                    inject(script, SdrAction::Modulation(None));
                }
                for m in neowon_core::Modulation::ALL {
                    if ui
                        .selectable_label(sdr.modulation == Some(m), m.label())
                        .clicked()
                    {
                        inject(script, SdrAction::Modulation(Some(m)));
                    }
                }
            });
    });
    // Always five lines, so the constellation and signal list below never
    // jump as results come and go.
    let results = match &sdr.analysis {
        Some(a) => format!(
            "#{} {}{}  {:.1} ksym/s\nEVM {:.2} %  MER {:.1} dB\nC42 {:+.3}  |C40| {:.3}",
            a.track,
            a.modulation.label(),
            if a.auto { " (auto)" } else { "" },
            a.symbol_rate_hz / 1e3,
            a.evm_rms_pct,
            a.mer_db,
            a.cumulants.c42,
            a.cumulants.c40.norm()
        ),
        None if sdr.analyse_on => "no signal near the tuned frequency\n\n".into(),
        None => "\n\n".into(),
    };
    let class = match &sdr.classification {
        Some(c) => format!(
            "class {}  {:.2} ({})\n  next {} (margin {:.2})",
            if c.unknown {
                "unknown"
            } else {
                c.class.label()
            },
            c.confidence,
            c.trust.label(),
            c.runner_up.label(),
            c.margin
        ),
        None => "\n".into(),
    };
    ui.monospace(format!("{results}\n{class}"));
}

/// Detection controls and the active tracks, strongest first. The list
/// holds a fixed height whatever its length (so nothing around it jumps);
/// drag the handle under it to resize it (`sdr list <px>`).
fn signals(ui: &mut egui::Ui, sdr: &SdrState, script: &mut Script) {
    ui.horizontal(|ui| {
        let mut on = sdr.detect_on;
        if ui.checkbox(&mut on, "Detect").changed() {
            inject(script, SdrAction::Detect(on));
        }
        let mut th = sdr.threshold_db;
        ui.label("threshold dB");
        if ui
            .add(egui::DragValue::new(&mut th).speed(0.5).range(3.0..=60.0))
            .changed()
        {
            inject(script, SdrAction::Threshold(th));
        }
    });
    let mut tracks: Vec<_> = sdr.tracker.active().collect();
    tracks.sort_by(|a, b| b.last.power_dbfs.total_cmp(&a.last.power_dbfs));
    egui::ScrollArea::vertical()
        .id_salt("sdr-signals")
        .min_scrolled_height(sdr.list_px)
        .max_height(sdr.list_px)
        .auto_shrink([false, false])
        .show(ui, |ui| track_rows(ui, sdr, &tracks, script));
    // Resize handle.
    let (r, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::drag());
    let color = if resp.hovered() || resp.dragged() {
        egui::Color32::from_gray(160)
    } else {
        egui::Color32::from_gray(80)
    };
    ui.painter().line_segment(
        [
            egui::pos2(r.center().x - 20.0, r.center().y),
            egui::pos2(r.center().x + 20.0, r.center().y),
        ],
        (2.0, color),
    );
    let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);
    if resp.dragged() && resp.drag_delta().y != 0.0 {
        let px = (sdr.list_px + resp.drag_delta().y).clamp(40.0, 2000.0);
        inject(script, SdrAction::List(px.round()));
    }
}

fn track_rows(
    ui: &mut egui::Ui,
    sdr: &SdrState,
    tracks: &[&neowon_dsp::Track],
    script: &mut Script,
) {
    for t in tracks {
        let o = &t.last;
        let text = format!(
            "#{:<3} {:>10.4} MHz {:>7.1} kHz\n     {:>6.1} dBFS  SNR {:>5.1} dB",
            t.id,
            o.centre_hz / 1e6,
            o.bandwidth_hz() / 1e3,
            o.power_dbfs,
            o.snr_db
        );
        if ui
            .add(
                egui::Label::new(egui::RichText::new(text).monospace()).sense(egui::Sense::click()),
            )
            .on_hover_text("click to tune")
            .clicked()
        {
            inject(script, SdrAction::Tune((o.centre_hz / 1e3).round() * 1e3));
        }
    }
    if tracks.is_empty() && sdr.detect_on {
        ui.weak("no active signals");
    }
}

/// I/Q scatter of the latest frame's decimated samples. Scaled to the
/// samples' peak (real signals sit tens of dB below full scale, so a fixed
/// full-scale box shows a dot); the zoom factor is printed, and 1× means
/// the box edge is full scale.
fn constellation(ui: &mut egui::Ui, sdr: &SdrState) {
    if let Some(a) = &sdr.analysis {
        return recovered(ui, a);
    }
    let peak = sdr
        .iq
        .iter()
        .flat_map(|p| [p[0].abs(), p[1].abs()])
        .fold(0.0f32, f32::max);
    let zoom = if peak > 0.0 {
        (0.9 / peak).max(1.0)
    } else {
        1.0
    };
    ui.label(format!("IQ  ×{zoom:.0}"));
    let side = ui.available_width().min(360.0);
    let (r, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, BG);
    p.line_segment(
        [
            egui::pos2(r.center().x, r.min.y),
            egui::pos2(r.center().x, r.max.y),
        ],
        (1.0, GRID),
    );
    p.line_segment(
        [
            egui::pos2(r.min.x, r.center().y),
            egui::pos2(r.max.x, r.center().y),
        ],
        (1.0, GRID),
    );
    p.circle_stroke(r.center(), side / 2.0, (1.0, GRID));
    let half = side / 2.0 * zoom;
    for [i, q] in &sdr.iq {
        let pos = r.center() + egui::vec2(i * half, -q * half);
        p.rect_filled(
            egui::Rect::from_center_size(pos, egui::vec2(1.5, 1.5)),
            0.0,
            TRACE,
        );
    }
}

/// The lab's recovered decision points (unit energy) over the ideal
/// constellation.
fn recovered(ui: &mut egui::Ui, a: &crate::sdr::analysis::Analysis) {
    ui.label(format!("recovered {}", a.modulation.label()));
    let side = ui.available_width().min(360.0);
    let (r, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, BG);
    // ±1.6 of unit-energy constellation fills the box (64QAM's corners
    // sit at ±1.08).
    let half = side / 2.0 / 1.6;
    for [i, q] in &a.symbols {
        let pos = r.center() + egui::vec2(i * half, -q * half);
        p.rect_filled(
            egui::Rect::from_center_size(pos, egui::vec2(1.5, 1.5)),
            0.0,
            TRACE,
        );
    }
    for (i, q) in a.modulation.points() {
        let c = r.center() + egui::vec2(i as f32 * half, -(q as f32) * half);
        p.circle_stroke(c, 3.0, (1.0, egui::Color32::YELLOW));
    }
}
