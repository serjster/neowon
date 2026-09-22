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
use crate::sdr::{SdrAction, SdrState, WF_H, WF_W, zoom};
use crate::ui::layout::Layout;
use crate::ui::sdr_bands;
use crate::uitree;
use bevy_egui::egui::accesskit::Role;

pub(super) const BG: egui::Color32 = egui::Color32::from_rgb(10, 12, 16);
pub(super) const GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(70, 80, 95, 90);
pub(super) const TRACE: egui::Color32 = egui::Color32::from_rgb(120, 220, 255);
const TEXT: egui::Color32 = egui::Color32::from_rgb(170, 180, 195);
/// The tuned cursor and its channel band.
pub(super) const CURSOR: egui::Color32 = egui::Color32::from_rgb(255, 80, 80);

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
    refmap: Res<crate::refmap::RefMap>,
    mut tex: Local<Option<egui::TextureHandle>>,
    mut uploaded: Local<u64>,
    mut width_drag: Local<bool>,
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
    // Linear filtering, deliberately: the texture is 1024 columns drawn at
    // several times that width, and nearest-neighbour upscaling of the fine
    // OFDM carrier texture makes stripe widths vary enough that the eye reads
    // a slant into a static pattern (measured 0.00 px/row of real shear).
    let tex_opts = egui::TextureOptions::LINEAR;
    match tex.as_mut() {
        None => *tex = Some(ctx.load_texture("sdr-waterfall", image(), tex_opts)),
        Some(t) if *uploaded != sdr.wf_rows => t.set(image(), tex_opts),
        _ => {}
    }
    *uploaded = sdr.wf_rows;
    let tex_id = tex.as_ref().expect("texture").id();

    let view = layout.points(layout.plot.union(layout.descriptors));
    egui::Area::new(egui::Id::new("sdr-view"))
        .order(egui::Order::Background)
        .fixed_pos(view.min)
        .show(&ctx, |ui| {
            let (full, _) = ui.allocate_exact_size(view.size(), egui::Sense::hover());
            ui.painter().rect_filled(full, 0.0, BG);
            uitree::name(ui, "SDR canvas");
            // Top to bottom: minimap, spectrum, band strip, waterfall. The
            // spectrum, strip and waterfall share one frequency axis.
            let mut top = full.min.y;
            let mini = refmap.mini.then(|| {
                let r = egui::Rect::from_min_max(
                    full.min,
                    egui::pos2(full.max.x, full.min.y + sdr_bands::MINI_H),
                );
                top = r.max.y + 4.0;
                r
            });
            let rect = egui::Rect::from_min_max(egui::pos2(full.min.x, top), full.max);
            let resp = ui.interact(
                rect,
                ui.id().with("sdr-canvas"),
                egui::Sense::click_and_drag(),
            );
            let split = rect.min.y + rect.height() * 0.45;
            let spec = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, split));
            let strip = refmap.strip.then(|| {
                egui::Rect::from_min_max(
                    egui::pos2(rect.min.x, split + 1.0),
                    egui::pos2(rect.max.x, split + 1.0 + sdr_bands::STRIP_H),
                )
            });
            let wf_top = strip.map_or(split + 2.0, |s| s.max.y + 1.0);
            let wf = egui::Rect::from_min_max(egui::pos2(rect.min.x, wf_top), rect.max);
            draw_spectrum(ui.painter(), spec, &sdr);
            ui.painter().image(
                tex_id,
                wf,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            uitree::node(
                ui.ctx(),
                ui.id().with("spectrum"),
                Role::Image,
                "spectrum",
                spec,
            );
            uitree::node(
                ui.ctx(),
                ui.id().with("waterfall"),
                Role::Image,
                "waterfall",
                wf,
            );
            draw_channel(ui.painter(), rect, wf, &sdr);
            let stations = super::station_overlay::draw(ui.painter(), spec, &refmap, &sdr);
            pointer(
                ui,
                &resp,
                rect,
                spec,
                &sdr,
                &stations,
                &mut width_drag,
                &mut script,
            );
            if let Some(r) = strip {
                sdr_bands::strip(ui, r, &refmap, &sdr, &mut script);
            }
            if let Some(r) = mini {
                sdr_bands::minimap(ui, r, &refmap, &sdr, &mut script);
            }
        });

    let dock = layout.points(layout.dialog);
    egui::Area::new(egui::Id::new("sdr-dock"))
        .order(egui::Order::Background)
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
            uitree::name(&inner, "SDR dock");
            egui::ScrollArea::vertical().show(&mut inner, |ui| {
                super::sdr_dock::show(ui, &sdr, &link, &refmap, &mut script);
            });
        });
    super::bandmap_window::show(&ctx, &refmap, &sdr, &mut script);
}

/// Frequency at x, Hz.
fn freq_at(sdr: &SdrState, r: egui::Rect, x: f32) -> f64 {
    let t = ((x - r.min.x) / r.width()) as f64;
    sdr.view_centre() + (t - 0.5) * sdr.span()
}

/// x of frequency `hz`.
pub(super) fn x_at(sdr: &SdrState, r: egui::Rect, hz: f64) -> f32 {
    r.min.x + ((hz - sdr.view_centre()) / sdr.span() + 0.5) as f32 * r.width()
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
        // The top line carries the unit instead of a bare number.
        let text = if i == 0 {
            format!("{db:.0} dBFS")
        } else {
            format!("{db:.0}")
        };
        p.text(
            egui::pos2(r.min.x + 4.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            text,
            font.clone(),
            TEXT,
        );
    }
    // Frequency ticks on round 1-2-5 steps, 6–12 across the span.
    let span = sdr.span();
    let step = tick_step(span / 8.0);
    let lo = sdr.view_centre() - span / 2.0;
    let decimals = (-(step / 1e6).log10()).ceil().max(0.0) as usize;
    let mut f = (lo / step).ceil() * step;
    while f <= lo + span {
        let x = x_at(sdr, r, f);
        p.line_segment(
            [egui::pos2(x, r.min.y), egui::pos2(x, r.max.y)],
            (1.0, GRID),
        );
        if x < r.max.x - 60.0 {
            p.text(
                egui::pos2(x + 3.0, r.max.y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.*}", decimals, f / 1e6),
                font.clone(),
                TEXT,
            );
        }
        f += step;
    }
    p.text(
        egui::pos2(r.max.x - 4.0, r.max.y - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "MHz",
        font.clone(),
        TEXT,
    );
    // Active tracks: their occupied band, shaded, with the track id.
    let x_of = |hz: f64| x_at(sdr, r, hz);
    // The hardware window's centre, when it differs from the tuned
    // frequency (or the view is panned off it).
    let xc = x_of(sdr.config.centre_hz);
    let hard_differs = (sdr.config.centre_hz - sdr.tuned_hz).abs() > 1.0 || sdr.pan_hz != 0.0;
    if hard_differs && (r.min.x..=r.max.x).contains(&xc) {
        p.line_segment(
            [egui::pos2(xc, r.min.y), egui::pos2(xc, r.max.y)],
            (
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 200, 80, 120),
            ),
        );
    }
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
        let x = x_of(hz);
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
            "span {:.1} kHz   width {:.1} kHz",
            sdr.span() / 1e3,
            sdr.channel_width() / 1e3
        ),
        egui::FontId::monospace(13.0),
        egui::Color32::WHITE,
    );
}

/// The 1-2-5 × 10^n step nearest above `raw` Hz.
fn tick_step(raw: f64) -> f64 {
    let mag = 10f64.powf(raw.max(1.0).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag)
}

/// Text on a dark plate, so a signal behind it cannot eat its digits.
/// Flips to the left of `at` when it would run off `bounds`.
fn plated(
    p: &egui::Painter,
    at: egui::Pos2,
    bounds: egui::Rect,
    text: String,
    color: egui::Color32,
) {
    let galley = p.layout_no_wrap(text, egui::FontId::monospace(11.0), color);
    let size = galley.size() + egui::vec2(8.0, 4.0);
    let x = if at.x + 5.0 + size.x > bounds.max.x {
        at.x - 5.0 - size.x
    } else {
        at.x + 5.0
    };
    let plate = egui::Rect::from_min_size(egui::pos2(x, at.y), size);
    p.rect_filled(
        plate,
        3.0,
        egui::Color32::from_rgba_unmultiplied(10, 12, 16, 220),
    );
    p.galley(plate.min + egui::vec2(4.0, 2.0), galley, color);
}

/// The tuned cursor and its channel: a shaded band spanning the channel
/// width with solid filter edges, a red bar carrying the frequency, and an
/// edge arrow when the tuned frequency is outside the view.
fn draw_channel(p: &egui::Painter, rect: egui::Rect, wf: egui::Rect, sdr: &SdrState) {
    let tuned = sdr.tuned_hz;
    let width = sdr.channel_width();
    let x = x_at(sdr, rect, tuned);
    if x < rect.min.x || x > rect.max.x {
        // Off screen: an edge arrow with the frequency.
        let right = tuned > sdr.view_centre();
        let (ax, tri) = if right {
            (rect.max.x - 4.0, -1.0)
        } else {
            (rect.min.x + 4.0, 1.0)
        };
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(ax, rect.min.y + 4.0),
                egui::pos2(ax + 10.0 * tri, rect.min.y + 4.0),
                egui::pos2(ax + 5.0 * tri, rect.min.y + 12.0),
            ],
            CURSOR,
            egui::Stroke::NONE,
        ));
        p.text(
            egui::pos2(ax + 4.0 * tri, rect.min.y + 14.0),
            if right {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::LEFT_TOP
            },
            format!("tuned {}", fmt_mhz(tuned)),
            egui::FontId::monospace(11.0),
            CURSOR,
        );
        return;
    }
    let half = (width / sdr.span() * rect.width() as f64 / 2.0) as f32;
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(x - half, rect.min.y),
            egui::pos2(x + half, rect.max.y),
        )
        .intersect(rect),
        0.0,
        egui::Color32::from_rgba_unmultiplied(255, 80, 80, 28),
    );
    for e in [x - half, x + half] {
        if (rect.min.x..=rect.max.x).contains(&e) {
            p.line_segment(
                [egui::pos2(e, rect.min.y), egui::pos2(e, rect.max.y)],
                (1.0, CURSOR),
            );
        }
    }
    p.line_segment(
        [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
        (1.5, CURSOR),
    );
    // The frequency, at the top of the waterfall so it clears the header.
    plated(
        p,
        egui::pos2(x, wf.min.y + 4.0),
        rect,
        format!("Tuned {}", fmt_mhz(tuned)),
        CURSOR,
    );
}

/// The spectrum and waterfall under the mouse, the way the scope's
/// Spectrum window works: plain scroll zooms the span at the pointer and
/// steps the sample rate to the rung that carries it (the zoom→rate choice
/// lives in `sdr::zoom`), shift+scroll pans horizontally (past the IQ band
/// the hardware window moves), ctrl+scroll zooms the dB range, a 2-D
/// wheel's x axis pans, left-drag pans the view (vertically it moves the
/// reference level), right-drag moves the hardware window, double-click
/// resets the view. A left-drag on a channel filter edge sets the Width;
/// elsewhere it pans. A left-click tunes to the frequency under the pointer
/// and leaves the window alone. Every gesture injects script actions
/// (`sdr tune|width|centre|span|pan|level|rate`).
#[allow(clippy::too_many_arguments)]
fn pointer(
    ui: &egui::Ui,
    resp: &egui::Response,
    rect: egui::Rect,
    spec: egui::Rect,
    sdr: &SdrState,
    stations: &[(egui::Rect, String)],
    width_drag: &mut bool,
    script: &mut Script,
) {
    let rate = sdr.config.sample_rate;
    let half_w = sdr.channel_width() / 2.0;
    // The two filter edges are grabbable handles.
    let edge_hit = |x: f32| -> bool {
        let (e0, e1) = (
            x_at(sdr, rect, sdr.tuned_hz - half_w),
            x_at(sdr, rect, sdr.tuned_hz + half_w),
        );
        (x - e0).abs() <= 6.0 || (x - e1).abs() <= 6.0
    };
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.double_clicked() {
        inject(script, SdrAction::Span(0.0));
        inject(script, SdrAction::Pan(0.0));
        inject(
            script,
            SdrAction::Level {
                ref_db: 0.0,
                range_db: 100.0,
            },
        );
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // A station under the pointer wins: tune to it and pick its
        // demodulator (D21), rather than the bare frequency.
        if let Some((_, key)) = stations.iter().find(|(r, _)| r.contains(pos)) {
            script.inject(Action::RefMap(crate::refmap::RefMapAction::TuneStation(
                key.clone(),
            )));
        } else {
            let hz = (freq_at(sdr, rect, pos.x) / 1e3).round() * 1e3;
            inject(script, SdrAction::Tune(hz));
        }
    }
    // A drag that started on a filter edge resizes the width; shift keeps
    // the pan, so the band is still reachable when it fills the screen.
    if resp.drag_started_by(egui::PointerButton::Primary) {
        *width_drag = !shift && resp.interact_pointer_pos().is_some_and(|p| edge_hit(p.x));
    }
    if resp.drag_stopped() {
        *width_drag = false;
    }
    if resp.dragged_by(egui::PointerButton::Secondary) {
        // Right-drag moves the hardware window: the band follows the
        // pointer, and the tuned frequency rides inside it.
        let d = resp.drag_delta();
        let hz = sdr.config.centre_hz - (d.x / rect.width()) as f64 * sdr.span();
        inject(script, SdrAction::Centre(hz));
    } else if resp.dragged_by(egui::PointerButton::Primary) {
        if *width_drag {
            if let Some(pos) = resp.interact_pointer_pos() {
                let w = (2.0 * (freq_at(sdr, rect, pos.x) - sdr.tuned_hz).abs()).clamp(200.0, rate);
                inject(script, SdrAction::Width(Some(w)));
            }
        } else {
            let d = resp.drag_delta();
            // Content follows the pointer, but the window stays put: the pan
            // only slides the view inside the IQ band.
            let want = sdr.pan_hz - (d.x / rect.width()) as f64 * sdr.span();
            let room = (rate - sdr.span()).max(0.0) / 2.0;
            let pan = want.clamp(-room, room);
            if pan != sdr.pan_hz {
                inject(script, SdrAction::Pan(pan));
            }
            if d.y != 0.0 {
                let ddb = (d.y / spec.height()) as f64 * sdr.range_db;
                inject(
                    script,
                    SdrAction::Level {
                        ref_db: (sdr.ref_db + ddb).clamp(-150.0, 30.0),
                        range_db: sdr.range_db,
                    },
                );
            }
        }
    }
    let Some(pos) = resp.hover_pos() else { return };
    if edge_hit(pos.x) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let hz = freq_at(sdr, rect, pos.x);
    ui.painter().line_segment(
        [egui::pos2(pos.x, rect.min.y), egui::pos2(pos.x, rect.max.y)],
        (1.0, egui::Color32::from_white_alpha(60)),
    );
    let label = if *width_drag {
        let w = 2.0 * (hz - sdr.tuned_hz).abs();
        format!("Width {:.1} kHz", w / 1e3)
    } else if spec.contains(pos) {
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
    let (scroll, ctrl) = ui.input(|i| {
        (
            i.smooth_scroll_delta,
            i.modifiers.ctrl || i.modifiers.command,
        )
    });
    if shift {
        // Pan horizontally: the content follows the scroll, so wheel down
        // goes to higher frequencies and a swipe right to lower ones. A
        // 2-D wheel's x axis pans with the y one.
        let lines = ((scroll.y - scroll.x) / zoom::WHEEL_LINE_POINTS as f32) as f64;
        if lines.abs() > 1e-3 {
            for a in zoom::pan_actions(sdr, lines) {
                inject(script, a);
            }
        }
    } else if ctrl {
        let zdb = -scroll.y / 240.0;
        if zdb.abs() > 1e-3 && spec.contains(pos) {
            // Zoom the dB range around the level under the pointer.
            let at =
                sdr.ref_db - (1.0 - ((spec.max.y - pos.y) / spec.height()) as f64) * sdr.range_db;
            let range = (sdr.range_db * 2f64.powf(zdb as f64)).clamp(10.0, 200.0);
            let k = range / sdr.range_db;
            inject(
                script,
                SdrAction::Level {
                    ref_db: (at + (sdr.ref_db - at) * k).clamp(-150.0, 30.0),
                    range_db: range,
                },
            );
        }
    } else {
        let zf = -scroll.y / 240.0;
        if zf.abs() > 1e-3 {
            let t = ((pos.x - rect.min.x) / rect.width()) as f64 - 0.5;
            for a in zoom::zoom_actions(sdr, t, zf as f64) {
                inject(script, a);
            }
        }
        // A 2-D wheel's x axis is a horizontal pan without a modifier too.
        let lines = (-scroll.x / zoom::WHEEL_LINE_POINTS as f32) as f64;
        if lines.abs() > 1e-3 {
            for a in zoom::pan_actions(sdr, lines) {
                inject(script, a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tick_step;

    #[test]
    fn ticks_land_on_round_steps() {
        assert_eq!(tick_step(2.048e6 / 8.0), 500e3);
        assert_eq!(tick_step(200e3 / 8.0), 50e3);
        assert_eq!(tick_step(30e3), 50e3);
        assert_eq!(tick_step(100e3), 100e3);
    }
}
