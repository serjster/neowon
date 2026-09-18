//! The SDR dock: tuning and display controls, detected signals, and the
//! IQ constellation. Every control injects a script action.

use bevy_egui::egui;
use neowon_backend::SdrGain;

use super::sdr_view::{BG, GRID, TRACE, fmt_mhz, inject};
use crate::Link;
use crate::script::{Action, Script};
use crate::sdr::{FFT_SIZES, SdrAction, SdrState};

const SPANS: [f64; 6] = [0.0, 1e6, 500e3, 200e3, 100e3, 50e3];

pub fn controls(ui: &mut egui::Ui, sdr: &SdrState, link: &Link, script: &mut Script) {
    let c = &sdr.config;
    ui.heading("SDR");
    if let Some(caps) = &sdr.caps {
        ui.label(format!("{} · {} · {}", caps.name, caps.tuner, caps.serial));
    }
    ui.separator();

    ui.label("Centre frequency (MHz)");
    let mut mhz = c.centre_hz / 1e6;
    if ui
        .add(egui::DragValue::new(&mut mhz).speed(0.01).max_decimals(6))
        .changed()
    {
        inject(script, SdrAction::Tune(mhz * 1e6));
    }
    ui.horizontal(|ui| {
        for (label, step) in [("−1M", -1e6), ("−100k", -1e5), ("+100k", 1e5), ("+1M", 1e6)] {
            if ui.small_button(label).clicked() {
                inject(script, SdrAction::Step(step));
            }
        }
    });

    let rates = sdr
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_default();
    egui::ComboBox::from_label("Rate")
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

    let auto = c.gain == SdrGain::Auto;
    let mut want_auto = auto;
    if ui.checkbox(&mut want_auto, "Tuner AGC").changed() {
        inject(
            script,
            SdrAction::Gain(if want_auto { None } else { Some(29.7) }),
        );
    }
    if let (SdrGain::Manual(db), Some(caps)) = (c.gain, &sdr.caps) {
        let mut g = db;
        let max = caps.gains_db.last().copied().unwrap_or(50.0);
        if ui
            .add(egui::Slider::new(&mut g, 0.0..=max).text("gain dB"))
            .changed()
        {
            inject(script, SdrAction::Gain(Some(g)));
        }
    }
    let mut agc = c.agc;
    if ui.checkbox(&mut agc, "RTL digital AGC").changed() {
        inject(script, SdrAction::Agc(agc));
    }
    ui.horizontal(|ui| {
        let mut ppm = c.ppm;
        ui.label("ppm");
        if ui
            .add(
                egui::DragValue::new(&mut ppm)
                    .speed(0.1)
                    .range(-200.0..=200.0),
            )
            .changed()
        {
            inject(script, SdrAction::Ppm(ppm));
        }
    });
    ui.separator();

    egui::ComboBox::from_label("Span")
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
    egui::ComboBox::from_label("FFT")
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
    ui.horizontal(|ui| {
        let (mut r, mut d) = (sdr.ref_db, sdr.range_db);
        ui.label("ref");
        let a = ui
            .add(egui::DragValue::new(&mut r).speed(1.0).range(-150.0..=30.0))
            .changed();
        ui.label("range");
        let b = ui
            .add(egui::DragValue::new(&mut d).speed(1.0).range(10.0..=200.0))
            .changed();
        if a || b {
            inject(
                script,
                SdrAction::Level {
                    ref_db: r,
                    range_db: d,
                },
            );
        }
    });

    if sdr.caps.as_ref().is_some_and(|c| c.tuner == "sim") {
        egui::ComboBox::from_label("Scene")
            .selected_text(link.stimulus.clone())
            .show_ui(ui, |ui| {
                for name in neowon_sim::RfScene::PRESETS {
                    if ui.selectable_label(link.stimulus == name, name).clicked() {
                        script.inject(Action::Stimulus(name.into()));
                    }
                }
            });
    }
    let label = if c.running { "Stop" } else { "Run" };
    if ui.button(label).clicked() {
        inject(script, SdrAction::Run(!c.running));
    }
    ui.separator();
    if let Some((hz, db)) = sdr.peak() {
        ui.monospace(format!("peak  {}\n      {db:.1} dBFS", fmt_mhz(hz)));
    }
    if let Some(s) = &sdr.spectrum {
        ui.monospace(format!(
            "floor {:.1} dBFS\nbin   {:.0} Hz",
            s.median_db(),
            s.bin_hz
        ));
    }
    ui.monospace(format!("frames {}", sdr.frames_seen));
    ui.separator();
    signals(ui, sdr, script);
    if ui.button("Catalog…").clicked() {
        script.inject(Action::Catalog(crate::catalog::CatalogAction::Window(true)));
    }
}

/// Detection controls and the active tracks, strongest first.
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
    for t in tracks.iter().take(8) {
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
pub fn constellation(ui: &mut egui::Ui, sdr: &SdrState) {
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
    let side = ui.available_width().min(220.0);
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
