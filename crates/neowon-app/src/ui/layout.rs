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

/// egui zoom factor — the app's answer to a high-DPI screen the OS does not
/// scale for us (a 4K panel at 1x makes 12 pt text 12 physical pixels tall).
/// Layout geometry stays in logical window pixels, which is what the Bevy
/// plot sprite, gizmos, and pointer hit-tests all use; only the egui regions
/// convert to points via `Layout::points`.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct UiScale(pub f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Scale range offered in the UI and accepted by the `uiscale` action.
pub const UI_SCALE_RANGE: (f32, f32) = (0.75, 3.0);

/// Pick a starting zoom factor for a monitor of `physical_height` pixels
/// that the OS reports at `os_scale` (macOS "Looks like 3840x2160" gives a
/// 4K panel an os_scale of 1.0 — the case that made the UI unreadable).
pub fn auto_scale(physical_height: u32, os_scale: f32) -> f32 {
    let effective = physical_height as f32 / os_scale.max(0.1);
    if effective >= 2000.0 {
        2.0
    } else if effective >= 1400.0 {
        1.5
    } else {
        1.0
    }
}

#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// egui points per logical pixel (the zoom factor in force).
    pub scale: f32,
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
        Self::compute(1520.0, 820.0, 1.0)
    }
}

impl Layout {
    /// Chrome sized in logical pixels for `scale`: at a 2x zoom factor the
    /// dock has to be twice as many pixels wide to hold the same content.
    pub fn compute(win_w: f32, win_h: f32, scale: f32) -> Self {
        let scale = scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1);
        let win_w = win_w.max(MIN_W * scale);
        let win_h = win_h.max(MIN_H * scale);
        let mid_top = MENU_H * scale;
        let mid_bottom = win_h - FRONT_PANEL_H * scale;

        // The plot stretches to fill the middle area (a scope grid's
        // divisions are just rectangles). The settings dock is always
        // visible on the right — the Photoshop/Blender model — so the plot
        // stops at its edge.
        let (margin, dialog_w, desc_h, desc_gap) = (
            MARGIN * scale,
            DIALOG_W * scale,
            DESC_H * scale,
            DESC_GAP * scale,
        );
        let plot = Rect::from_min_max(
            Pos2::new(margin, mid_top + margin),
            Pos2::new(
                win_w - dialog_w - margin,
                mid_bottom - desc_gap - desc_h - margin,
            ),
        );
        let descriptors = Rect::from_min_size(
            Pos2::new(plot.left(), plot.bottom() + desc_gap),
            EVec2::new(plot.width(), desc_h),
        );
        Self {
            scale,
            window: EVec2::new(win_w, win_h),
            menu_bar: Rect::from_min_max(Pos2::ZERO, Pos2::new(win_w, mid_top)),
            plot,
            descriptors,
            dialog: Rect::from_min_max(
                Pos2::new(win_w - dialog_w, mid_top),
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

    /// Logical-pixel rect -> egui points (egui's screen shrinks by the zoom
    /// factor, so every Area position and size has to divide through).
    pub fn points(&self, r: Rect) -> Rect {
        let s = self.scale;
        Rect::from_min_max(
            Pos2::new(r.min.x / s, r.min.y / s),
            Pos2::new(r.max.x / s, r.max.y / s),
        )
    }

    /// egui points -> logical pixels (inverse of `points`), used to report
    /// painted geometry in the same space as the ROI map.
    pub fn pixels(&self, r: Rect) -> Rect {
        let s = self.scale;
        Rect::from_min_max(
            Pos2::new(r.min.x * s, r.min.y * s),
            Pos2::new(r.max.x * s, r.max.y * s),
        )
    }
}

/// Where the UI regions *actually* painted this frame, as egui reports them
/// — the geometry the `Layout` rects only promise. UI tests assert these
/// against the promise (nothing may overlap the plot), which is how the dock
/// sliding over the waveform when a section expanded was caught.
#[derive(Resource, Debug, Clone, Default)]
pub struct UiRects {
    pub regions: Vec<(&'static str, Rect)>,
    /// Floating windows (spectrum, waterfall, 3D) — these are movable by
    /// the user, so they are reported but not constrained.
    pub floating: Vec<(String, Rect)>,
}

impl UiRects {
    pub fn begin(&mut self) {
        self.regions.clear();
        self.floating.clear();
    }

    pub fn put(&mut self, name: &'static str, rect: Rect) {
        self.regions.push((name, rect));
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

/// Serialize the named-ROI map, the rects the UI actually painted, and
/// dynamic UI state as JSON — the `layout <path>` script action writes this
/// for UI tests. Geometry as JSON beats pixel diffing here: the assertion
/// ("no panel overlaps the plot") is exact and survives restyling.
pub fn dump_json(l: &Layout, open_menu: Option<&str>, painted: &UiRects) -> String {
    let rect_json = |r: &Rect| {
        format!(
            "[{:.1}, {:.1}, {:.1}, {:.1}]",
            r.min.x,
            r.min.y,
            r.width(),
            r.height()
        )
    };
    let mut rois = String::new();
    for (i, r) in Roi::ALL.iter().enumerate() {
        if i > 0 {
            rois.push(',');
        }
        rois.push_str(&format!(
            "\n    \"{}\": {}",
            r.name(),
            rect_json(&r.rect(l))
        ));
    }
    let mut paint = String::new();
    for (i, (name, r)) in painted.regions.iter().enumerate() {
        if i > 0 {
            paint.push(',');
        }
        paint.push_str(&format!("\n    \"{name}\": {}", rect_json(r)));
    }
    let mut floating = String::new();
    for (i, (name, r)) in painted.floating.iter().enumerate() {
        if i > 0 {
            floating.push(',');
        }
        floating.push_str(&format!("\n    \"{name}\": {}", rect_json(r)));
    }
    let menu = match open_menu {
        Some(m) => format!("\"{m}\""),
        None => "null".into(),
    };
    format!(
        "{{\n  \"window\": [{:.1}, {:.1}],\n  \"plot_center\": [{:.1}, {:.1}],\n  \"menu\": {menu},\n  \"rois\": {{{rois}\n  }},\n  \"painted\": {{{paint}\n  }},\n  \"floating\": {{{floating}\n  }}\n}}\n",
        l.window.x, l.window.y, l.plot_center.x, l.plot_center.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window sizes x UI scales the layout must survive: the 1080p and 4K
    /// cases are the ones users actually run.
    const CASES: [(f32, f32, f32); 7] = [
        (1100.0, 700.0, 1.0),
        (1520.0, 820.0, 1.0),
        (1920.0, 1080.0, 1.0),
        (1920.0, 1080.0, 1.5),
        (2560.0, 1440.0, 1.5),
        (3840.0, 2160.0, 2.0),
        (2688.0, 1512.0, 2.0), // 70% of a 4K panel, the auto-fit default
    ];

    #[test]
    fn regions_fit_and_relate_at_all_sizes() {
        for (w, h, s) in CASES {
            let l = Layout::compute(w, h, s);
            let (w, h) = (l.window.x, l.window.y);
            let plot = l.plot;
            // Descriptors hug the plot bottom, same width.
            assert!((l.descriptors.top() - plot.bottom() - DESC_GAP * s).abs() < 1e-3);
            assert_eq!(l.descriptors.width(), plot.width());
            assert_eq!(l.descriptors.left(), plot.left());
            // Dock flush right, fixed width, beside the plot.
            assert!(l.dialog.left() >= plot.right());
            assert!((l.dialog.width() - DIALOG_W * s).abs() < 1e-3);
            assert_eq!(l.dialog.right(), w);
            // Chrome strips span the full width.
            assert_eq!(l.menu_bar.width(), w);
            assert_eq!(l.front_panel.width(), w);
            // The plot stays usefully large — in divisions, not pixels, so
            // the check means the same thing at every scale.
            assert!(plot.width() / s >= 580.0, "{w}x{h}@{s}: plot {plot:?}");
            assert!(plot.height() / s >= 320.0, "{w}x{h}@{s}: plot {plot:?}");
            // Everything on screen, nothing overlapping the plot.
            for r in Roi::ALL {
                let r = r.rect(&l);
                assert!(r.min.x >= 0.0 && r.min.y >= 0.0, "{r:?}");
                assert!(r.max.x <= w + 1e-3 && r.max.y <= h + 1e-3, "{r:?}");
            }
            assert!(!plot.intersects(l.front_panel));
            assert!(!plot.intersects(l.menu_bar));
            assert!(!plot.intersects(l.dialog));
        }
    }

    #[test]
    fn plot_center_matches_screen_geometry() {
        for (w, h, s) in CASES {
            let l = Layout::compute(w, h, s);
            let cx = l.plot.center().x - l.window.x / 2.0;
            let cy = l.window.y / 2.0 - l.plot.center().y;
            assert!((cx - l.plot_center.x).abs() < 1e-3);
            assert!((cy - l.plot_center.y).abs() < 1e-3);
        }
    }

    #[test]
    fn graticule_tiles_the_plot() {
        for (w, h, s) in CASES {
            let l = Layout::compute(w, h, s);
            assert!((l.div.x * H_DIVS as f32 - l.plot.width()).abs() < 1e-3);
            assert!((l.div.y * V_DIVS as f32 - l.plot.height()).abs() < 1e-3);
        }
    }

    #[test]
    fn points_and_pixels_round_trip() {
        let l = Layout::compute(3840.0, 2160.0, 2.0);
        let r = Roi::Dialog.rect(&l);
        let back = l.pixels(l.points(r));
        assert!((back.min.x - r.min.x).abs() < 1e-3);
        assert!((back.max.y - r.max.y).abs() < 1e-3);
        // egui sees half-size geometry at a 2x zoom factor.
        assert!((l.points(r).width() - r.width() / 2.0).abs() < 1e-3);
    }

    #[test]
    fn hidpi_screens_get_a_bigger_default_scale() {
        // macOS reporting a 4K panel as "Looks like 3840x2160" (os scale 1).
        assert_eq!(auto_scale(2160, 1.0), 2.0);
        // The same panel with the OS already doing 2x: no extra zoom.
        assert_eq!(auto_scale(2160, 2.0), 1.0);
        assert_eq!(auto_scale(1440, 1.0), 1.5);
        assert_eq!(auto_scale(1080, 1.0), 1.0);
    }

    #[test]
    fn tiny_windows_clamp_to_minimum() {
        let l = Layout::compute(400.0, 300.0, 1.0);
        assert_eq!(l.window.x, MIN_W);
        assert_eq!(l.window.y, MIN_H);
        // The floor scales with the UI: chrome needs the room.
        let l = Layout::compute(400.0, 300.0, 2.0);
        assert_eq!(l.window.x, MIN_W * 2.0);
        assert_eq!(l.window.y, MIN_H * 2.0);
    }
}
