//! Rotary knob widget — the front-panel idiom for ladder/range controls and
//! the mouse-scroll substitute: drag vertically to turn, scroll to step one
//! rung, double-click to restore the default.

use bevy_egui::egui::{self, Color32, Sense, Stroke, Vec2};

const SIZE: f32 = 40.0;
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
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), Sense::click_and_drag());

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
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 1.0 {
            let dir = if scroll > 0.0 { 1 } else { -1 };
            *value = match ladder {
                Some(l) => ladder_step(l, *value, dir > 0),
                None => *value + dir as f64 * (hi - lo) / 20.0,
            };
        }
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

    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(fmt(*value))
                .monospace()
                .size(10.0)
                .color(Color32::from_gray(190)),
        );
        ui.label(egui::RichText::new(label).size(9.0).color(Color32::GRAY));
    });

    resp.on_hover_text(format!(
        "{label}: {}\ndrag = turn · scroll = step · 2x-click = {default_text}",
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

fn ladder_step(ladder: &[f64], current: f64, up: bool) -> f64 {
    let mut idx = 0;
    let mut best = f64::MAX;
    for (i, &v) in ladder.iter().enumerate() {
        let d = (v.ln() - current.max(1e-12).ln()).abs();
        if d < best {
            best = d;
            idx = i;
        }
    }
    let idx = if up {
        (idx + 1).min(ladder.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    ladder[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rung_helpers() {
        let l = [0.1, 0.2, 0.5, 1.0];
        assert_eq!(nearest_rung(&l, 0.24), 0.2);
        assert_eq!(nearest_rung(&l, 0.42), 0.5);
        assert_eq!(ladder_step(&l, 0.2, true), 0.5);
        assert_eq!(ladder_step(&l, 0.2, false), 0.1);
        assert_eq!(ladder_step(&l, 1.0, true), 1.0);
        assert_eq!(ladder_step(&l, 0.1, false), 0.1);
    }
}
