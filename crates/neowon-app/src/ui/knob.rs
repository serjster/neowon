//! Rotary knob widget — the front-panel idiom for ladder/range controls:
//! drag vertically to turn, double-click to restore the default.
//!
//! Deliberately **not** scroll-driven. The dock these live in is a scrolling
//! rail, and a widget that reacts to the wheel changes its value whenever
//! the pointer happens to cross it while the rail is being scrolled — the
//! scroll reaches both. egui's own sliders and drag-values take the same
//! line.

use bevy_egui::egui::{self, Color32, Sense, Stroke, Vec2};

const SIZE: f32 = 40.0;
/// Total widget width — a knob is a fixed-size cell so a row of them can
/// never outgrow the dock (the labels used to claim the parent's remaining
/// width and push the panel over the plot).
pub const KNOB_W: f32 = 74.0;
/// Two label lines under the dial.
const LABEL_H: f32 = 24.0;
/// Vertical drag pixels for a full min->max sweep.
const DRAG_PIXELS: f32 = 160.0;
/// Pointer sweep: -135..+135 degrees from the top.
const SWEEP: f32 = 270.0f32.to_radians();

/// A rotary control over `range` (snapped to `ladder` when given). Returns
/// true when the value changed.
pub fn knob(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    range: (f64, f64),
    ladder: Option<&[f64]>,
    default: f64,
    fmt: impl Fn(f64) -> String,
) -> bool {
    let (lo, hi) = range;
    let before = *value;
    // One fixed cell: dial on top, two centred label lines under it.
    let (cell, _) = ui.allocate_exact_size(egui::vec2(KNOB_W, SIZE + LABEL_H), Sense::hover());
    let dial = egui::Rect::from_center_size(
        egui::pos2(cell.center().x, cell.top() + SIZE / 2.0),
        egui::vec2(SIZE, SIZE),
    );
    let resp = ui.interact(dial, ui.id().with(("knob", label)), Sense::click_and_drag());
    let rect = dial;

    if resp.double_clicked() {
        *value = default;
    }
    if resp.dragged() {
        let d = -resp.drag_delta().y / DRAG_PIXELS;
        let step = if let Some(l) = ladder {
            // One rung per ~1/N of the sweep so ladders feel detented.
            ((hi - lo) / (l.len().saturating_sub(1).max(1)) as f64) * 0.6
        } else {
            hi - lo
        };
        *value += d as f64 * step;
    }
    *value = value.clamp(lo, hi);
    if let Some(l) = ladder {
        *value = nearest_rung(l, *value);
    }
    let changed = (*value - before).abs() > 1e-12;

    // Draw: ring, detent ticks, pointer.
    let painter = ui.painter();
    let c = rect.center();
    let r = SIZE * 0.42;
    painter.circle_stroke(c, r, Stroke::new(1.5, Color32::from_gray(80)));
    let angle = |f: f32| -std::f32::consts::FRAC_PI_2 - SWEEP / 2.0 + f * SWEEP;
    for f in [0.0f32, 0.5, 1.0] {
        let a = angle(f);
        let dir = Vec2::new(a.cos(), a.sin());
        painter.line_segment(
            [c + dir * (r + 1.5), c + dir * (r + 4.5)],
            Stroke::new(1.0, Color32::from_gray(70)),
        );
    }
    painter.circle_filled(c, r * 0.78, Color32::from_rgb(34, 37, 45));
    let a = angle(((*value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32);
    let dir = Vec2::new(a.cos(), a.sin());
    painter.line_segment(
        [c + dir * r * 0.2, c + dir * r * 0.72],
        Stroke::new(2.5, Color32::from_rgb(235, 180, 30)),
    );
    painter.line_segment([c, c], Stroke::NONE);

    // Labels are painted (not laid out) so they stay inside the cell
    // whatever their length.
    painter.text(
        egui::pos2(cell.center().x, dial.bottom() + 2.0),
        egui::Align2::CENTER_TOP,
        fmt(*value),
        egui::FontId::monospace(10.0),
        Color32::from_gray(190),
    );
    painter.text(
        egui::pos2(cell.center().x, dial.bottom() + 13.0),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::proportional(9.0),
        Color32::GRAY,
    );

    resp.on_hover_text(format!(
        "{label}: {}\ndrag = turn · 2x-click = {default_text}",
        fmt(*value),
        default_text = fmt(default)
    ));

    changed
}

fn nearest_rung(ladder: &[f64], v: f64) -> f64 {
    let mut best = ladder[0];
    for &r in ladder {
        if (r - v).abs() < (best - v).abs() {
            best = r;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rung_helpers() {
        let l = [0.1, 0.2, 0.5, 1.0];
        assert_eq!(nearest_rung(&l, 0.24), 0.2);
        assert_eq!(nearest_rung(&l, 0.42), 0.5);
    }
}
