//! Scope-grade UI: the SDS2000X Plus screen anatomy (docs/ui-ux-research.md
//! §1). Composition lives here; each region is its own module. The old
//! monolithic collapsible panel is gone.

pub mod descriptors;
pub mod dialog_acquire;
pub mod dialog_channel;
pub mod dialog_cursor;
pub mod dialog_display;
pub mod dialog_horizontal;
pub mod dialog_math;
pub mod dialog_measure;
pub mod dialog_record;
pub mod dialog_trigger;
pub mod dialog_utility;
pub mod frontpanel;
pub mod layout;
pub mod menu;
pub mod menubar;
pub mod touch;
pub mod widgets;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState, Unit, fmt_opt_sticky, fmt_si};
use crate::gpu::Phosphor;
use crate::ui::layout::Layout;

pub use menu::{Menu, MenuState};

#[allow(clippy::too_many_arguments)]
pub fn panel(
    mut contexts: EguiContexts,
    time: Res<Time>,
    layout: Res<Layout>,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut math: ResMut<MathState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
    mut cur: ResMut<CursorState>,
    mut pf: ResMut<PfState>,
    mut menus: ResMut<MenuState>,
    mut rec: ResMut<crate::record::Recorder>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let now = time.elapsed_secs_f64();

    menubar::show(&ctx, &layout, &mut link, &mut menus, now);
    descriptors::show(&ctx, &layout, &mut link, &phosphor, &mut meas, &mut menus);
    frontpanel::show(
        &ctx,
        &layout,
        &mut link,
        &mut phosphor,
        &mut math,
        &mut meas,
        &mut menus,
    );
    menu::show(
        &ctx,
        &layout,
        &mut menus,
        &mut link,
        &mut phosphor,
        &mut math,
        &mut meas,
        &mut fft,
        &mut cur,
        &mut pf,
        &mut rec,
    );

    if fft.enabled {
        spectrum_window(&ctx, &mut fft);
    }
}

/// Floating spectrum view with zoom and pan: scroll = frequency zoom at the
/// pointer, shift+scroll (or a 2-D wheel's x axis) = dB zoom, drag = pan,
/// double-click = reset.
fn spectrum_window(ctx: &egui::Context, fft: &mut FftState) {
    egui::Window::new("Spectrum")
        .default_width(680.0)
        .show(ctx, |ui| {
            let (resp, painter) = ui.allocate_painter(
                egui::vec2(ui.available_width(), 230.0),
                egui::Sense::click_and_drag(),
            );
            let rect = resp.rect;
            painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(8, 10, 14));

            // View interactions.
            if resp.double_clicked() {
                fft.view = (0.0, 1.0);
                fft.db = (-100.0, 20.0);
            }
            if resp.hovered() {
                let (scroll, shift, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta,
                        i.modifiers.shift,
                        i.pointer.hover_pos(),
                    )
                });
                let zx = -scroll.y / 240.0;
                let zy = scroll.x / 240.0;
                if zx.abs() > 1e-3 && !shift {
                    // Zoom frequency around the pointer.
                    let anchor = pointer
                        .map(|p| ((p.x - rect.left()) / rect.width()) as f64)
                        .unwrap_or(0.5)
                        .clamp(0.0, 1.0);
                    let (f0, f1) = fft.view;
                    let a = f0 + anchor * (f1 - f0);
                    let k = (2f64).powf(zx as f64);
                    fft.view = ((a - (a - f0) * k).max(0.0), (a + (f1 - a) * k).min(1.0));
                    if fft.view.1 - fft.view.0 < 1e-3 {
                        let m = (fft.view.0 + fft.view.1) / 2.0;
                        fft.view = ((m - 5e-4).max(0.0), (m + 5e-4).min(1.0));
                    }
                }
                let zdb = if shift { -scroll.y / 240.0 } else { zy };
                if zdb.abs() > 1e-3 {
                    // Zoom the dB span around its middle.
                    let (lo, hi) = fft.db;
                    let mid = (lo + hi) / 2.0;
                    let half = ((hi - lo) / 2.0 * (2f32).powf(zdb)).clamp(5.0, 90.0);
                    fft.db = (mid - half, mid + half);
                }
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                let (f0, f1) = fft.view;
                let span = f1 - f0;
                let df = -(d.x / rect.width()) as f64 * span;
                let df = df.clamp(-f0, 1.0 - f1);
                fft.view = (f0 + df, f1 + df);
                let ddb = d.y / rect.height() * (fft.db.1 - fft.db.0);
                fft.db = (fft.db.0 + ddb, fft.db.1 + ddb);
            }

            let (db_lo, db_hi) = fft.db;
            let grid_step = if db_hi - db_lo > 60.0 { 20 } else { 10 };
            let mut db = (db_lo / grid_step as f32).ceil() as i32 * grid_step;
            while (db as f32) < db_hi {
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
                db += grid_step;
            }
            if let Some(s) = &fft.spectrum {
                let n = s.amplitude.len();
                let (f0, f1) = fft.view;
                let (b0, b1) = (
                    ((n as f64 * f0) as usize).clamp(1, n - 2),
                    ((n as f64 * f1).ceil() as usize).clamp(2, n),
                );
                let pts: Vec<egui::Pos2> = (b0..b1)
                    .map(|i| {
                        let fx = (i as f64 / n as f64 - f0) / (f1 - f0);
                        let x = rect.left() + fx as f32 * rect.width();
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
                let bands = &mut fft.peak_bands;
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} — {}",
                            fmt_si(f0 * nyquist, "Hz"),
                            fmt_si(f1 * nyquist, "Hz"),
                        ))
                        .monospace(),
                    );
                    // The peak slot is always drawn, dash-padded when no
                    // peak is found, with sticky SI bands so the readout
                    // never reflows or flaps at a band boundary.
                    let peak = s.peak();
                    let db = peak.map_or_else(
                        || "     —".into(),
                        |(_, a)| format!("{:>6.1}", 20.0 * a.max(1e-12).log10()),
                    );
                    let amp = fmt_opt_sticky(peak.map(|(_, a)| a), Unit::Volt, &mut bands.0);
                    let hz = fmt_opt_sticky(peak.map(|(f, _)| f), Unit::Hertz, &mut bands.1);
                    ui.label(
                        egui::RichText::new(format!("   peak: {amp} at {hz} ({db} dBV)"))
                            .monospace(),
                    );
                    ui.label(
                        egui::RichText::new(
                            "scroll: zoom · shift+scroll: dB · drag: pan · 2x-click: reset",
                        )
                        .small()
                        .weak(),
                    );
                });
            } else {
                ui.label("no data");
            }
        });
}
