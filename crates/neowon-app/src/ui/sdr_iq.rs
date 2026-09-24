//! The Analysis section's I/Q scatter: the latest frame's decimated samples,
//! or the modulation lab's recovered decision points over the ideal
//! constellation.

use bevy_egui::egui;

use super::sdr_view::{BG, GRID, TRACE};
use crate::sdr::SdrState;

/// I/Q scatter of the latest frame's decimated samples. Scaled to the
/// samples' peak (real signals sit tens of dB below full scale, so a fixed
/// full-scale box shows a dot); the zoom factor is printed, and 1× means
/// the box edge is full scale.
pub fn constellation(ui: &mut egui::Ui, sdr: &SdrState) {
    // Joined by identity, like every lab readout: only the current
    // target's recovered points are drawn.
    if let Some(a) = sdr.nearest_track().and_then(|t| sdr.analysis_of(t.id)) {
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
    ui.label(format!("#{} recovered {}", a.track, a.modulation.label()));
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
