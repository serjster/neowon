//! SDR mode's screen: spectrum over waterfall where the scope grid sits,
//! controls and an IQ constellation over the dock rail (`sdr_dock`). Drawn only while
//! an SDR is connected; mode-aware chrome (front panel, menus) is 10.9.
//!
//! Every control injects a script action instead of mutating state, so a
//! script can reach everything the UI can (script-parity rule).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::Link;
use crate::script::{Action, Script};
use crate::sdr::{SdrAction, SdrState, WF_H, WF_W};
use crate::ui::layout::Layout;

pub(super) const BG: egui::Color32 = egui::Color32::from_rgb(10, 12, 16);
pub(super) const GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(70, 80, 95, 90);
pub(super) const TRACE: egui::Color32 = egui::Color32::from_rgb(120, 220, 255);
const TEXT: egui::Color32 = egui::Color32::from_rgb(170, 180, 195);

pub fn fmt_mhz(hz: f64) -> String {
    format!("{:.4} MHz", hz / 1e6)
}

pub(super) fn inject(script: &mut Script, a: SdrAction) {
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
                super::sdr_dock::controls(ui, &sdr, &link, &mut script);
                ui.add_space(8.0);
                super::sdr_dock::constellation(ui, &sdr);
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
    // Active tracks: their occupied band, shaded, with the track id.
    let x_of =
        |hz: f64| r.min.x + ((hz - sdr.config.centre_hz) / sdr.span() + 0.5) as f32 * r.width();
    for t in sdr.tracker.active() {
        let (x0, x1) = (x_of(t.last.lo_hz), x_of(t.last.hi_hz));
        let (x0, x1) = (x0.max(r.min.x), (x1.max(x0 + 2.0)).min(r.max.x));
        if x0 >= r.max.x || x1 <= r.min.x {
            continue;
        }
        let band =
            egui::Rect::from_min_max(egui::pos2(x0, r.min.y + 20.0), egui::pos2(x1, r.max.y));
        p.rect_filled(
            band,
            0.0,
            egui::Color32::from_rgba_unmultiplied(80, 255, 140, 28),
        );
        p.text(
            egui::pos2(x0, r.min.y + 22.0),
            egui::Align2::LEFT_TOP,
            format!("#{}", t.id),
            font.clone(),
            egui::Color32::from_rgb(120, 255, 160),
        );
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
