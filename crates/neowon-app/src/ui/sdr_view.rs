//! SDR mode's screen: spectrum over waterfall where the scope grid sits,
//! controls and an IQ constellation over the dock rail. Drawn only while
//! an SDR is connected; mode-aware chrome (front panel, menus) is 10.9.
//!
//! Every control injects a script action instead of mutating state, so a
//! script can reach everything the UI can (script-parity rule).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use neowon_backend::SdrGain;

use crate::Link;
use crate::script::{Action, Script};
use crate::sdr::{FFT_SIZES, SdrAction, SdrState, WF_H, WF_W};
use crate::ui::layout::Layout;

const BG: egui::Color32 = egui::Color32::from_rgb(10, 12, 16);
const GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(70, 80, 95, 90);
const TRACE: egui::Color32 = egui::Color32::from_rgb(120, 220, 255);
const TEXT: egui::Color32 = egui::Color32::from_rgb(170, 180, 195);
const SPANS: [f64; 6] = [0.0, 1e6, 500e3, 200e3, 100e3, 50e3];

pub fn fmt_mhz(hz: f64) -> String {
    format!("{:.4} MHz", hz / 1e6)
}

fn inject(script: &mut Script, a: SdrAction) {
    script.inject(Action::Sdr(a));
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    mut contexts: EguiContexts,
    layout: Res<Layout>,
    sdr: Res<SdrState>,
    link: Res<Link>,
    mut script: ResMut<Script>,
    mut tex: Local<Option<egui::TextureHandle>>,
    mut uploaded: Local<u64>,
) {
    if !sdr.active {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();

    // Waterfall texture: re-upload only when a row was added.
    let image = || egui::ColorImage {
        size: [WF_W, WF_H],
        pixels: sdr
            .waterfall
            .iter()
            .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
            .collect(),
        source_size: egui::vec2(WF_W as f32, WF_H as f32),
    };
    match tex.as_mut() {
        None => *tex = Some(ctx.load_texture("sdr-waterfall", image(), Default::default())),
        Some(t) if *uploaded != sdr.wf_rows => t.set(image(), Default::default()),
        _ => {}
    }
    *uploaded = sdr.wf_rows;
    let tex_id = tex.as_ref().expect("texture").id();

    let view = layout.points(layout.plot.union(layout.descriptors));
    egui::Area::new(egui::Id::new("sdr-view"))
        .order(egui::Order::Foreground)
        .fixed_pos(view.min)
        .show(&ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(view.size(), egui::Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, BG);
            let split = rect.min.y + rect.height() * 0.45;
            let spec = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, split));
            let wf = egui::Rect::from_min_max(egui::pos2(rect.min.x, split + 2.0), rect.max);
            draw_spectrum(ui.painter(), spec, &sdr);
            ui.painter().image(
                tex_id,
                wf,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            pointer(ui, &resp, rect, spec, &sdr, &mut script);
        });

    let dock = layout.points(layout.dialog);
    egui::Area::new(egui::Id::new("sdr-dock"))
        .order(egui::Order::Foreground)
        .fixed_pos(dock.min)
        .show(&ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(dock.size(), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_rgb(22, 25, 31));
            let mut inner = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink(8.0))
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            inner.set_clip_rect(rect);
            egui::ScrollArea::vertical().show(&mut inner, |ui| {
                controls(ui, &sdr, &link, &mut script);
                ui.add_space(8.0);
                constellation(ui, &sdr);
            });
        });
}

/// Frequency at x, Hz.
fn freq_at(sdr: &SdrState, r: egui::Rect, x: f32) -> f64 {
    let t = ((x - r.min.x) / r.width()) as f64;
    sdr.config.centre_hz + (t - 0.5) * sdr.span()
}

fn draw_spectrum(p: &egui::Painter, r: egui::Rect, sdr: &SdrState) {
    let font = egui::FontId::monospace(11.0);
    // Level grid every 10 dB below the reference.
    let steps = (sdr.range_db / 10.0).floor() as usize;
    for i in 0..=steps {
        let db = sdr.ref_db - 10.0 * i as f64;
        let y = r.max.y - sdr.level(db) * r.height();
        p.line_segment(
            [egui::pos2(r.min.x, y), egui::pos2(r.max.x, y)],
            (1.0, GRID),
        );
        p.text(
            egui::pos2(r.min.x + 4.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            format!("{db:.0}"),
            font.clone(),
            TEXT,
        );
    }
    // Ten frequency divisions, labelled at the bottom edge.
    for i in 0..=10 {
        let x = r.min.x + r.width() * i as f32 / 10.0;
        p.line_segment(
            [egui::pos2(x, r.min.y), egui::pos2(x, r.max.y)],
            (1.0, GRID),
        );
        if i % 2 == 0 && i < 10 {
            let f = sdr.config.centre_hz + (i as f64 / 10.0 - 0.5) * sdr.span();
            p.text(
                egui::pos2(x + 3.0, r.max.y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.3}", f / 1e6),
                font.clone(),
                TEXT,
            );
        }
    }
    let n = sdr.columns.len();
    if n > 1 {
        let pts: Vec<egui::Pos2> = sdr
            .columns
            .iter()
            .enumerate()
            .map(|(c, &db)| {
                let x = r.min.x + r.width() * (c as f32 + 0.5) / n as f32;
                egui::pos2(x, r.max.y - sdr.level(db) * r.height())
            })
            .collect();
        p.add(egui::Shape::line(pts, (1.2, TRACE)));
    }
    if let Some((hz, db)) = sdr.peak() {
        let x = r.min.x + ((hz - sdr.config.centre_hz) / sdr.span() + 0.5) as f32 * r.width();
        let y = r.max.y - sdr.level(db) * r.height();
        p.circle_filled(egui::pos2(x, y), 3.0, egui::Color32::YELLOW);
        p.text(
            egui::pos2(r.max.x - 6.0, r.min.y + 4.0),
            egui::Align2::RIGHT_TOP,
            format!("peak {}  {db:6.1} dBFS", fmt_mhz(hz)),
            egui::FontId::monospace(13.0),
            egui::Color32::YELLOW,
        );
    }
    p.text(
        egui::pos2(r.center().x, r.min.y + 4.0),
        egui::Align2::CENTER_TOP,
        format!(
            "{}   span {:.0} kHz",
            fmt_mhz(sdr.config.centre_hz),
            sdr.span() / 1e3
        ),
        egui::FontId::monospace(13.0),
        egui::Color32::WHITE,
    );
}

/// Hover readout, click to tune, wheel to zoom the span.
fn pointer(
    ui: &egui::Ui,
    resp: &egui::Response,
    rect: egui::Rect,
    spec: egui::Rect,
    sdr: &SdrState,
    script: &mut Script,
) {
    let Some(pos) = resp.hover_pos() else { return };
    let hz = freq_at(sdr, rect, pos.x);
    ui.painter().line_segment(
        [egui::pos2(pos.x, rect.min.y), egui::pos2(pos.x, rect.max.y)],
        (1.0, egui::Color32::from_white_alpha(60)),
    );
    let label = if spec.contains(pos) {
        let db = sdr.ref_db - (1.0 - ((spec.max.y - pos.y) / spec.height()) as f64) * sdr.range_db;
        format!("{}  {db:.1} dBFS", fmt_mhz(hz))
    } else {
        fmt_mhz(hz)
    };
    ui.painter().text(
        pos + egui::vec2(8.0, -16.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        egui::FontId::monospace(12.0),
        egui::Color32::WHITE,
    );
    if resp.clicked() {
        inject(script, SdrAction::Tune((hz / 1e3).round() * 1e3));
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll.abs() > 0.5 {
        let rate = sdr.config.sample_rate;
        let factor = if scroll > 0.0 { 0.8 } else { 1.25 };
        let span = (sdr.span() * factor).clamp(rate / 64.0, rate);
        inject(
            script,
            SdrAction::Span(if span >= rate { 0.0 } else { span }),
        );
    }
}

fn controls(ui: &mut egui::Ui, sdr: &SdrState, link: &Link, script: &mut Script) {
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
}

/// I/Q scatter of the latest frame's decimated samples, full scale = the
/// box edge.
fn constellation(ui: &mut egui::Ui, sdr: &SdrState) {
    ui.label("IQ");
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
    let half = side / 2.0;
    for [i, q] in &sdr.iq {
        let pos = r.center() + egui::vec2(i * half, -q * half);
        p.rect_filled(
            egui::Rect::from_center_size(pos, egui::vec2(1.5, 1.5)),
            0.0,
            TRACE,
        );
    }
}
