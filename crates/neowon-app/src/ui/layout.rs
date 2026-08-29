//! Screen geometry of the scope-grade UI — the SDS2000X Plus screen anatomy
//! (docs/ui-ux-research.md §1, vendor manual chapters 7–9), computed at
//! runtime from the window size so the app resizes like an application, not
//! a bitmap. One source of truth: egui panels, Bevy plot placement, and the
//! layout tests all read the `Layout` resource. Window space: logical
//! pixels, top-left origin. Bevy world space: window-center origin, +y up.

use bevy::math::Vec2;
use bevy::prelude::Resource;
use bevy_egui::egui::{Pos2, Rect, Vec2 as EVec2};

pub const MENU_H: f32 = 36.0;
pub const FRONT_PANEL_H: f32 = 96.0;
pub const DIALOG_W: f32 = 320.0;
pub const DESC_H: f32 = 54.0;
const DESC_GAP: f32 = 4.0;
const MARGIN: f32 = 8.0;

/// Graticule: the reference divides the grid into 8 vertical x 10
/// horizontal divisions.
pub const H_DIVS: i32 = 10;
pub const V_DIVS: i32 = 8;

/// Smallest window the layout stays usable at (also the Window resize
/// constraint).
pub const MIN_W: f32 = 1100.0;
pub const MIN_H: f32 = 700.0;

#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub window: EVec2,
    pub menu_bar: Rect,
    pub plot: Rect,
    pub descriptors: Rect,
    pub dialog: Rect,
    pub front_panel: Rect,
    /// Plot center in Bevy world space (sprite + gizmo placement).
    pub plot_center: Vec2,
    /// One graticule division, in screen pixels.
    pub div: Vec2,
}

impl Default for Layout {
    fn default() -> Self {
        Self::compute(1520.0, 820.0)
    }
}

impl Layout {
    pub fn compute(win_w: f32, win_h: f32) -> Self {
        let win_w = win_w.max(MIN_W);
        let win_h = win_h.max(MIN_H);
        let mid_top = MENU_H;
        let mid_bottom = win_h - FRONT_PANEL_H;

        // The plot stretches to fill the middle area (a scope grid's
        // divisions are just rectangles). The settings dock is always
        // visible on the right — the Photoshop/Blender model — so the plot
        // stops at its edge.
        let plot = Rect::from_min_max(
            Pos2::new(MARGIN, mid_top + MARGIN),
            Pos2::new(
                win_w - DIALOG_W - MARGIN,
                mid_bottom - DESC_GAP - DESC_H - MARGIN,
            ),
        );
        let descriptors = Rect::from_min_size(
            Pos2::new(plot.left(), plot.bottom() + DESC_GAP),
            EVec2::new(plot.width(), DESC_H),
        );
        Self {
            window: EVec2::new(win_w, win_h),
            menu_bar: Rect::from_min_max(Pos2::ZERO, Pos2::new(win_w, MENU_H)),
            plot,
            descriptors,
            dialog: Rect::from_min_max(
                Pos2::new(win_w - DIALOG_W, mid_top),
                Pos2::new(win_w, mid_bottom),
            ),
            front_panel: Rect::from_min_max(Pos2::new(0.0, mid_bottom), Pos2::new(win_w, win_h)),
            plot_center: Vec2::new(plot.center().x - win_w / 2.0, win_h / 2.0 - plot.center().y),
            div: Vec2::new(plot.width() / H_DIVS as f32, plot.height() / V_DIVS as f32),
        }
    }

    /// Screen y of a value expressed as a fraction of the full 10-division
    /// encoding (visible window = ±4 of 8 shown divisions), in world space.
    pub fn frac_to_world_y(&self, frac: f32) -> f32 {
        self.plot_center.y + frac * 10.0 * self.div.y
    }
}

/// Named screen regions — the stable ROI map for UI tests
/// (docs/ui-ux-research.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roi {
    MenuBar,
    Plot,
    Descriptors,
    Dialog,
    FrontPanel,
    /// Overlaid on the plot's right edge (trigger level indicator).
    TrigBadge,
    /// Overlaid along the plot's bottom (measurement readouts).
    MeasOverlay,
}

impl Roi {
    pub const ALL: [Roi; 7] = [
        Roi::MenuBar,
        Roi::Plot,
        Roi::Descriptors,
        Roi::Dialog,
        Roi::FrontPanel,
        Roi::TrigBadge,
        Roi::MeasOverlay,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Roi::MenuBar => "menu_bar",
            Roi::Plot => "plot",
            Roi::Descriptors => "descriptors",
            Roi::Dialog => "dialog",
            Roi::FrontPanel => "front_panel",
            Roi::TrigBadge => "trig_badge",
            Roi::MeasOverlay => "meas_overlay",
        }
    }

    pub fn rect(self, l: &Layout) -> Rect {
        match self {
            Roi::MenuBar => l.menu_bar,
            Roi::Plot => l.plot,
            Roi::Descriptors => l.descriptors,
            Roi::Dialog => l.dialog,
            Roi::FrontPanel => l.front_panel,
            Roi::TrigBadge => Rect::from_min_size(
                Pos2::new(l.plot.right() - 64.0, l.plot.center().y - 40.0),
                EVec2::new(56.0, 80.0),
            ),
            Roi::MeasOverlay => Rect::from_min_size(
                Pos2::new(l.plot.left(), l.plot.bottom() - 30.0),
                EVec2::new(l.plot.width(), 30.0),
            ),
        }
    }
}

/// Serialize the named-ROI map plus dynamic UI state as JSON — the
/// `layout <path>` script action writes this for UI tests.
pub fn dump_json(l: &Layout, open_menu: Option<&str>) -> String {
    let mut rois = String::new();
    for (i, r) in Roi::ALL.iter().enumerate() {
        let rect = r.rect(l);
        if i > 0 {
            rois.push(',');
        }
        rois.push_str(&format!(
            "\n    \"{}\": [{:.1}, {:.1}, {:.1}, {:.1}]",
            r.name(),
            rect.min.x,
            rect.min.y,
            rect.width(),
            rect.height(),
        ));
    }
    let menu = match open_menu {
        Some(m) => format!("\"{m}\""),
        None => "null".into(),
    };
    format!(
        "{{\n  \"window\": [{:.1}, {:.1}],\n  \"plot_center\": [{:.1}, {:.1}],\n  \"menu\": {menu},\n  \"rois\": {{{rois}\n  }}\n}}\n",
        l.window.x, l.window.y, l.plot_center.x, l.plot_center.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: [(f32, f32); 3] = [(1100.0, 700.0), (1520.0, 820.0), (1920.0, 1080.0)];

    #[test]
    fn regions_fit_and_relate_at_all_sizes() {
        for (w, h) in SIZES {
            let l = Layout::compute(w, h);
            let plot = l.plot;
            // Descriptors hug the plot bottom, same width.
            assert_eq!(l.descriptors.top() - plot.bottom(), DESC_GAP);
            assert_eq!(l.descriptors.width(), plot.width());
            assert_eq!(l.descriptors.left(), plot.left());
            // Dock flush right, fixed width, beside the plot.
            assert!(l.dialog.left() >= plot.right());
            assert_eq!(l.dialog.width(), DIALOG_W);
            assert_eq!(l.dialog.right(), w);
            // Chrome strips span the full width.
            assert_eq!(l.menu_bar.width(), w);
            assert_eq!(l.front_panel.width(), w);
            // The plot stays usefully large.
            assert!(plot.width() >= 580.0, "{w}x{h}: plot {plot:?}");
            assert!(plot.height() >= 320.0, "{w}x{h}: plot {plot:?}");
            // Everything on screen, nothing overlapping the plot.
            for r in Roi::ALL {
                let r = r.rect(&l);
                assert!(r.min.x >= 0.0 && r.min.y >= 0.0, "{r:?}");
                assert!(r.max.x <= w && r.max.y <= h, "{r:?}");
            }
            assert!(!plot.intersects(l.front_panel));
            assert!(!plot.intersects(l.menu_bar));
        }
    }

    #[test]
    fn plot_center_matches_screen_geometry() {
        for (w, h) in SIZES {
            let l = Layout::compute(w, h);
            let cx = l.plot.center().x - w / 2.0;
            let cy = h / 2.0 - l.plot.center().y;
            assert!((cx - l.plot_center.x).abs() < 1e-3);
            assert!((cy - l.plot_center.y).abs() < 1e-3);
        }
    }

    #[test]
    fn graticule_tiles_the_plot() {
        for (w, h) in SIZES {
            let l = Layout::compute(w, h);
            assert!((l.div.x * H_DIVS as f32 - l.plot.width()).abs() < 1e-3);
            assert!((l.div.y * V_DIVS as f32 - l.plot.height()).abs() < 1e-3);
        }
    }

    #[test]
    fn tiny_windows_clamp_to_minimum() {
        let l = Layout::compute(400.0, 300.0);
        assert_eq!(l.window.x, MIN_W);
        assert_eq!(l.window.y, MIN_H);
    }
}
