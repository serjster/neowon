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
pub mod dialog_trigger;
pub mod dialog_utility;
pub mod frontpanel;
pub mod layout;
pub mod menu;
pub mod menubar;
pub mod widgets;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState, fmt_si};
use crate::gpu::Phosphor;

pub use menu::{Menu, MenuState};

#[allow(clippy::too_many_arguments)]
pub fn panel(
    mut contexts: EguiContexts,
    time: Res<Time>,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut math: ResMut<MathState>,
    mut meas: ResMut<MeasureState>,
    mut fft: ResMut<FftState>,
    mut cur: ResMut<CursorState>,
    mut pf: ResMut<PfState>,
    mut menus: ResMut<MenuState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let now = time.elapsed_secs_f64();

    menubar::show(&ctx, &mut link, &mut menus, now);
    descriptors::show(&ctx, &mut link, &phosphor, &meas, &mut menus);
    frontpanel::show(
        &ctx,
        &mut link,
        &mut phosphor,
        &mut math,
        &mut meas,
        &mut menus,
    );
    menu::show(
        &ctx,
        &mut menus,
        &mut link,
        &mut phosphor,
        &mut math,
        &mut meas,
        &mut fft,
        &mut cur,
        &mut pf,
    );

    if fft.enabled {
        spectrum_window(&ctx, &mut fft);
    }
}

/// Floating spectrum view (kept as a window: it needs a large plot area).
fn spectrum_window(ctx: &egui::Context, fft: &mut FftState) {
    egui::Window::new("Spectrum")
        .default_width(680.0)
        .show(ctx, |ui| {
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
