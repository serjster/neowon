//! Vector icons drawn with the egui painter — strokes, not font glyphs, so
//! nothing can render as tofu on a platform missing a codepoint. Every icon
//! is defined in unit space (0..1) and scaled to its rect.

use bevy_egui::egui::{self, Color32, Pos2, Rect, Response, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Home,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    ZoomIn,
    ZoomOut,
    /// Circular arrow (reset/re-centre).
    Recenter,
}

/// Stroke width relative to the icon box.
const REL_STROKE: f32 = 0.10;

fn pt(rect: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(
        rect.min.x + x * rect.width(),
        rect.min.y + y * rect.height(),
    )
}

/// Paint `icon` into `rect` with `color`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new((rect.width() * REL_STROKE).max(1.0), color);
    match icon {
        Icon::Home => {
            // Roof + body, open at the bottom.
            painter.line_segment([pt(rect, 0.15, 0.50), pt(rect, 0.50, 0.18)], stroke);
            painter.line_segment([pt(rect, 0.50, 0.18), pt(rect, 0.85, 0.50)], stroke);
            painter.line_segment([pt(rect, 0.24, 0.46), pt(rect, 0.24, 0.84)], stroke);
            painter.line_segment([pt(rect, 0.76, 0.46), pt(rect, 0.76, 0.84)], stroke);
            painter.line_segment([pt(rect, 0.24, 0.84), pt(rect, 0.76, 0.84)], stroke);
        }
        Icon::ArrowLeft => arrow(painter, rect, stroke, 1.0),
        Icon::ArrowRight => arrow(painter, rect, stroke, -1.0),
        Icon::ArrowUp => arrow_v(painter, rect, stroke, 1.0),
        Icon::ArrowDown => arrow_v(painter, rect, stroke, -1.0),
        Icon::ZoomIn | Icon::ZoomOut => {
            let r = rect.width().min(rect.height()) * 0.30;
            let c = pt(rect, 0.44, 0.44);
            painter.circle_stroke(c, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x + r * 0.72, c.y + r * 0.72),
                    pt(rect, 0.86, 0.86),
                ],
                stroke,
            );
            let s = r * 0.5;
            painter.line_segment([Pos2::new(c.x - s, c.y), Pos2::new(c.x + s, c.y)], stroke);
            if icon == Icon::ZoomIn {
                painter.line_segment([Pos2::new(c.x, c.y - s), Pos2::new(c.x, c.y + s)], stroke);
            }
        }
        Icon::Recenter => {
            let r = rect.width().min(rect.height()) * 0.32;
            let c = pt(rect, 0.5, 0.5);
            painter.circle_stroke(c, r, stroke);
            painter.circle_filled(c, r * 0.28, color);
        }
    }
}

fn arrow(painter: &egui::Painter, rect: Rect, stroke: Stroke, dir: f32) {
    // dir = 1 -> left, -1 -> right.
    let (tip, tail) = if dir > 0.0 {
        (pt(rect, 0.20, 0.5), pt(rect, 0.80, 0.5))
    } else {
        (pt(rect, 0.80, 0.5), pt(rect, 0.20, 0.5))
    };
    painter.line_segment([tail, tip], stroke);
    let head = Vec2::new(dir * -0.18, 0.0);
    painter.line_segment([tip, pt(rect, 0.20 + 0.18, 0.5 - 0.20)], stroke);
    painter.line_segment([tip, pt(rect, 0.20 + 0.18, 0.5 + 0.20)], stroke);
    let _ = head;
}

fn arrow_v(painter: &egui::Painter, rect: Rect, stroke: Stroke, dir: f32) {
    // dir = 1 -> up, -1 -> down.
    let (tip, tail) = if dir > 0.0 {
        (pt(rect, 0.5, 0.20), pt(rect, 0.5, 0.80))
    } else {
        (pt(rect, 0.5, 0.80), pt(rect, 0.5, 0.20))
    };
    painter.line_segment([tail, tip], stroke);
    painter.line_segment([tip, pt(rect, 0.5 - 0.20, 0.20 + 0.18)], stroke);
    painter.line_segment([tip, pt(rect, 0.5 + 0.20, 0.20 + 0.18)], stroke);
}

/// Square icon button with a tooltip; returns true on click.
pub fn button(ui: &mut egui::Ui, icon: Icon, tooltip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let color = if ui.is_enabled() {
        Color32::from_gray(200)
    } else {
        Color32::from_gray(90)
    };
    let fill = if resp.hovered() {
        Color32::from_rgb(48, 52, 62)
    } else {
        Color32::from_rgb(28, 30, 36)
    };
    ui.painter().rect(
        rect,
        4.0,
        fill,
        Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Middle,
    );
    let inset = rect.shrink(size * 0.22);
    paint(ui.painter(), inset, icon, color);
    resp.on_hover_text(tooltip)
}
