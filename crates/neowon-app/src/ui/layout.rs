//! Fixed screen geometry of the scope-grade UI — the SDS2000X Plus screen
//! anatomy (docs/ui-ux-research.md §1, vendor manual chapters 7–9). One
//! source of truth: egui panels, Bevy plot placement, and the layout tests
//! all read these constants. Window space: screen pixels, top-left origin.
//! Bevy world space: window-center origin, +y up.

use bevy::math::Vec2;
use bevy_egui::egui::{Pos2, Rect, Vec2 as EVec2};

pub const WINDOW_W: f32 = 1520.0;
pub const WINDOW_H: f32 = 820.0;

/// Plot texture size — must match `gpu::PLOT_W/H`.
pub const PLOT_W: f32 = 1000.0;
pub const PLOT_H: f32 = 500.0;

pub const MENU_H: f32 = 36.0;
pub const FRONT_PANEL_H: f32 = 96.0;
pub const DIALOG_W: f32 = 320.0;
pub const DESC_H: f32 = 54.0;
const DESC_GAP: f32 = 4.0;

const MID_TOP: f32 = MENU_H;
const MID_BOTTOM: f32 = WINDOW_H - FRONT_PANEL_H;
const BLOCK_H: f32 = PLOT_H + DESC_GAP + DESC_H;

pub const PLOT_LEFT: f32 = (WINDOW_W - DIALOG_W - PLOT_W) / 2.0;
pub const PLOT_TOP: f32 = MID_TOP + (MID_BOTTOM - MID_TOP - BLOCK_H) / 2.0;
pub const DESC_TOP: f32 = PLOT_TOP + PLOT_H + DESC_GAP;
pub const DIALOG_LEFT: f32 = WINDOW_W - DIALOG_W;

/// Graticule: the reference divides the grid into 8 vertical x 10
/// horizontal divisions.
pub const H_DIVS: i32 = 10;
pub const V_DIVS: i32 = 8;
pub const DIV_X: f32 = PLOT_W / H_DIVS as f32;
pub const DIV_Y: f32 = PLOT_H / V_DIVS as f32;

/// Plot center in Bevy world space (sprite + gizmo placement).
pub const PLOT_CENTER: Vec2 = Vec2::new(
    PLOT_LEFT + PLOT_W / 2.0 - WINDOW_W / 2.0,
    WINDOW_H / 2.0 - (PLOT_TOP + PLOT_H / 2.0),
);

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

    pub fn rect(self) -> Rect {
        match self {
            Roi::MenuBar => Rect::from_min_max(Pos2::ZERO, Pos2::new(WINDOW_W, MENU_H)),
            Roi::Plot => {
                Rect::from_min_size(Pos2::new(PLOT_LEFT, PLOT_TOP), EVec2::new(PLOT_W, PLOT_H))
            }
            Roi::Descriptors => {
                Rect::from_min_size(Pos2::new(PLOT_LEFT, DESC_TOP), EVec2::new(PLOT_W, DESC_H))
            }
            Roi::Dialog => Rect::from_min_max(
                Pos2::new(DIALOG_LEFT, MID_TOP),
                Pos2::new(WINDOW_W, MID_BOTTOM),
            ),
            Roi::FrontPanel => {
                Rect::from_min_max(Pos2::new(0.0, MID_BOTTOM), Pos2::new(WINDOW_W, WINDOW_H))
            }
            Roi::TrigBadge => Rect::from_min_size(
                Pos2::new(PLOT_LEFT + PLOT_W - 64.0, PLOT_TOP + PLOT_H / 2.0 - 40.0),
                EVec2::new(56.0, 80.0),
            ),
            Roi::MeasOverlay => Rect::from_min_size(
                Pos2::new(PLOT_LEFT, PLOT_TOP + PLOT_H - 30.0),
                EVec2::new(PLOT_W, 30.0),
            ),
        }
    }
}

/// Serialize the named-ROI map plus dynamic UI state as JSON — the
/// `layout <path>` script action writes this for UI tests. Geometry is
/// compile-time fixed; only the open menu varies.
pub fn dump_json(open_menu: Option<&str>) -> String {
    let mut rois = String::new();
    for (i, r) in Roi::ALL.iter().enumerate() {
        let rect = r.rect();
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
        WINDOW_W, WINDOW_H, PLOT_CENTER.x, PLOT_CENTER.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_fit_and_relate() {
        let plot = Roi::Plot.rect();
        let desc = Roi::Descriptors.rect();
        let dialog = Roi::Dialog.rect();
        // Descriptors hug the plot bottom, same width.
        assert_eq!(desc.top() - plot.bottom(), DESC_GAP);
        assert_eq!(desc.width(), PLOT_W);
        assert_eq!(desc.left(), plot.left());
        // Dialog sits to the right of the plot.
        assert!(dialog.left() >= plot.right());
        assert_eq!(dialog.width(), DIALOG_W);
        // Chrome strips span the full width.
        assert_eq!(Roi::MenuBar.rect().width(), WINDOW_W);
        assert_eq!(Roi::FrontPanel.rect().width(), WINDOW_W);
        // Everything stays on screen and nothing overlaps the plot.
        for r in Roi::ALL {
            let r = r.rect();
            assert!(r.min.x >= 0.0 && r.min.y >= 0.0, "{r:?}");
            assert!(r.max.x <= WINDOW_W && r.max.y <= WINDOW_H, "{r:?}");
        }
        assert!(!plot.intersects(Roi::FrontPanel.rect()));
        assert!(!plot.intersects(Roi::MenuBar.rect()));
    }

    #[test]
    fn plot_center_matches_screen_geometry() {
        let plot = Roi::Plot.rect();
        let cx = plot.center().x - WINDOW_W / 2.0;
        let cy = WINDOW_H / 2.0 - plot.center().y;
        assert!((cx - PLOT_CENTER.x).abs() < 1e-3);
        assert!((cy - PLOT_CENTER.y).abs() < 1e-3);
    }

    #[test]
    fn graticule_tiles_the_plot() {
        assert_eq!(DIV_X * H_DIVS as f32, PLOT_W);
        assert_eq!(DIV_Y * V_DIVS as f32, PLOT_H);
    }
}
