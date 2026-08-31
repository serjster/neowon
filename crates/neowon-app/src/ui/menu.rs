//! The settings dock: an always-visible right-side rail (the
//! Photoshop/Blender model). Sections expand independently and stay open
//! (the rail scrolls on overflow); the `menu` script action opens a
//! section exclusively so tests stay deterministic.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::Phosphor;

use super::layout::{Layout, Roi};
use super::{
    dialog_acquire, dialog_channel, dialog_cursor, dialog_decode, dialog_display,
    dialog_horizontal, dialog_math, dialog_measure, dialog_record, dialog_trigger, dialog_utility,
};

use crate::record::Recorder;

/// Which function's dialog box is open. Exactly one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Channel(usize),
    Horizontal,
    Trigger,
    Acquire,
    Display,
    Measure,
    Math,
    Cursor,
    Utility,
    Record,
    Decode,
}

impl Menu {
    pub fn title(self) -> &'static str {
        match self {
            Menu::Channel(ch) => match ch {
                0 => "Channel 1",
                1 => "Channel 2",
                _ => "Channel",
            },
            Menu::Horizontal => "Horizontal",
            Menu::Trigger => "Trigger",
            Menu::Acquire => "Acquire",
            Menu::Display => "Display",
            Menu::Measure => "Measure",
            Menu::Math => "Math",
            Menu::Cursor => "Cursors",
            Menu::Utility => "Utility",
            Menu::Record => "Record / Export",
            Menu::Decode => "Decode",
        }
    }
}

/// Which dock sections are expanded. Sections are independent — leave as
/// many open as you like; the rail scrolls.
#[derive(Resource)]
pub struct MenuState {
    open: Vec<Menu>,
    /// Section to scroll into view on the next frame. Opening a section from
    /// the front panel or the View menu is useless if it lands off-screen in
    /// the scrolled rail.
    focus: Option<Menu>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: vec![Menu::Trigger],
            focus: None,
        }
    }
}

impl MenuState {
    pub fn is_open(&self, m: Menu) -> bool {
        self.open.contains(&m)
    }

    pub fn toggle(&mut self, m: Menu) {
        if let Some(i) = self.open.iter().position(|&x| x == m) {
            self.open.remove(i);
        } else {
            self.open.push(m);
            self.focus = Some(m);
        }
    }

    /// Ask for this section to be scrolled into view.
    pub fn reveal(&mut self, m: Menu) {
        self.open(m);
        self.focus = Some(m);
    }

    /// Expand a section (keeping others as they are).
    pub fn open(&mut self, m: Menu) {
        if !self.is_open(m) {
            self.open.push(m);
        }
    }

    fn take_focus(&mut self, m: Menu) -> bool {
        if self.focus == Some(m) {
            self.focus = None;
            true
        } else {
            false
        }
    }

    /// Script semantics: exactly this section open (None = all collapsed),
    /// so `layout` dumps stay deterministic.
    pub fn set_exclusive(&mut self, m: Option<Menu>) {
        self.open = m.into_iter().collect();
    }

    /// Open sections, in opening order (for the layout dump).
    pub fn open_list(&self) -> &[Menu] {
        &self.open
    }
}

/// Draws the dock and returns the rect it actually painted into — the
/// caller records it so tests can assert the rail never reaches the plot.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    l: &Layout,
    menus: &mut MenuState,
    link: &mut Link,
    phosphor: &mut Phosphor,
    math: &mut MathState,
    meas: &mut MeasureState,
    fft: &mut FftState,
    cur: &mut CursorState,
    pf: &mut PfState,
    rec: &mut Recorder,
    hist: &mut crate::record::History,
    refs: &mut crate::refs::RefState,
    script: &mut crate::script::Script,
    wf: &mut crate::viz::waterfall::WaterfallState,
    viz: &mut crate::viz::three_d::Viz3dState,
    fx: &crate::effects::Effects,
    ap: &mut crate::autopeak::AutoPeak,
    deep: &mut crate::deep::DeepView,
    dec: &mut crate::decode::DecodeState,
) -> egui::Rect {
    let rect = l.points(Roi::Dialog.rect(l));
    let mut frame = egui::Frame::new();
    frame.fill = egui::Color32::from_rgb(12, 14, 18);
    frame.stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(50));
    frame.inner_margin = 6.0.into();

    const SECTIONS: [Menu; 10] = [
        Menu::Trigger,
        Menu::Horizontal,
        Menu::Acquire,
        Menu::Channel(0),
        Menu::Channel(1),
        Menu::Math,
        Menu::Measure,
        Menu::Cursor,
        Menu::Decode,
        Menu::Display,
    ];

    // `constrain` is off and the clip rect is pinned to the dock: an Area
    // that egui is allowed to constrain slides *left* over the plot as soon
    // as its content is wider than the rail, which is how expanding a
    // channel section came to cover the trigger marker. Content is clipped
    // to the rail, so overflow can never reach the waveform.
    let resp = egui::Area::new("dock".into())
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);
            ui.set_max_width(rect.width());
            ui.set_min_width(rect.width());
            ui.set_max_height(rect.height());
            frame.show(ui, |ui| {
                ui.set_min_height(rect.height() - 16.0);
                // Scrolls in both axes: a section too wide for the rail
                // scrolls (manual 7.6 — "scrollbar when overflowing")
                // instead of spilling over the grid.
                egui::ScrollArea::both()
                    .max_height(rect.height() - 16.0)
                    .max_width(rect.width() - 12.0)
                    .show(ui, |ui| {
                        ui.set_max_width(rect.width() - 24.0);
                        ui.set_min_width(rect.width() - 24.0);
                        view_toolbar(ui, link, phosphor);
                        for m in SECTIONS {
                            section(ui, menus, m, |ui, menus| match m {
                                Menu::Channel(ch) => dialog_channel::show(ui, link, ch),
                                Menu::Horizontal => {
                                    dialog_horizontal::show(ui, link, phosphor, deep)
                                }
                                Menu::Trigger => dialog_trigger::show(ui, link),
                                Menu::Acquire => dialog_acquire::show(ui, link, ap),
                                Menu::Display => dialog_display::show(
                                    ui, link, phosphor, cur, refs, fft, wf, viz, fx, script,
                                ),
                                Menu::Measure => dialog_measure::show(ui, meas),
                                Menu::Math => dialog_math::show(ui, math),
                                Menu::Cursor => dialog_cursor::show(ui, link, cur, meas, deep),
                                Menu::Decode => dialog_decode::show(ui, dec),
                                Menu::Utility | Menu::Record => {
                                    let _ = menus;
                                }
                            });
                        }
                        section(ui, menus, Menu::Record, |ui, _| {
                            dialog_record::show(ui, link, rec, hist, script)
                        });
                        section(ui, menus, Menu::Utility, |ui, _| {
                            dialog_utility::show(ui, link, math, pf, fft, script)
                        });
                    });
            });
        });
    l.pixels(resp.response.rect)
}

/// Always-visible zoom/pan/home strip at the top of the dock — the same
/// operations the plot gestures (`ui/touch.rs`) and the `zoom`/`pan`/`home`
/// script actions drive.
fn view_toolbar(ui: &mut egui::Ui, link: &mut crate::Link, phosphor: &mut crate::gpu::Phosphor) {
    use crate::ui::icons::{Icon, button};
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let sel = link.selected.min(1);
        if button(
            ui,
            Icon::ZoomOut,
            "Vertical zoom out — coarser V/div (selected channel)",
            26.0,
        )
        .clicked()
        {
            crate::view::zoom_channel(link, sel, false);
        }
        if button(
            ui,
            Icon::ZoomIn,
            "Vertical zoom in — finer V/div (selected channel)",
            26.0,
        )
        .clicked()
        {
            crate::view::zoom_channel(link, sel, true);
        }
        let zoomed = crate::view::zoom_active(phosphor);
        let (out_tip, in_tip) = if zoomed {
            (
                "Horizontal zoom out — wider zoom window",
                "Horizontal zoom in — narrower zoom window",
            )
        } else {
            (
                "Horizontal zoom out — slower time base (more s/div)",
                "Horizontal zoom in — faster time base (fewer s/div)",
            )
        };
        if button(ui, Icon::ZoomOut, out_tip, 26.0).clicked() {
            crate::view::hzoom(link, phosphor, phosphor.hview.0, false);
        }
        if button(ui, Icon::ZoomIn, in_tip, 26.0).clicked() {
            crate::view::hzoom(link, phosphor, phosphor.hview.0, true);
        }
        if button(
            ui,
            Icon::Home,
            "Home — default zoom and centre position (key: H)",
            26.0,
        )
        .clicked()
        {
            crate::view::home(link, phosphor);
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (icon, dir, tip) in [
            (
                Icon::ArrowLeft,
                crate::view::Pan::Left,
                "Pan left (record window)",
            ),
            (
                Icon::ArrowRight,
                crate::view::Pan::Right,
                "Pan right (record window)",
            ),
            (
                Icon::ArrowUp,
                crate::view::Pan::Up,
                "Pan up (selected channel offset)",
            ),
            (
                Icon::ArrowDown,
                crate::view::Pan::Down,
                "Pan down (selected channel offset)",
            ),
        ] {
            if button(ui, icon, tip, 26.0).clicked() {
                crate::view::pan(link, phosphor, dir);
            }
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("VIEW — drag plot to pan, scroll to zoom")
                .size(9.0)
                .color(egui::Color32::GRAY),
        );
    });
    ui.add_space(4.0);
}

/// Accordion section: a full-width header that expands its body (collapsing
/// any other open section).
fn section(
    ui: &mut egui::Ui,
    menus: &mut MenuState,
    m: Menu,
    body: impl FnOnce(&mut egui::Ui, &mut MenuState),
) {
    let open = menus.is_open(m);
    // The caret is painted, not a glyph: egui's bundled fonts are subset per
    // platform and the triangles rendered as tofu on some of them, the same
    // failure the toolbar icons were replaced for.
    let header = egui::Button::new(
        egui::RichText::new(format!("    {}", m.title()))
            .strong()
            .size(12.0),
    )
    .fill(if open {
        egui::Color32::from_rgb(30, 34, 44)
    } else {
        egui::Color32::from_rgb(20, 22, 28)
    })
    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(55)))
    .min_size(egui::vec2(ui.available_width(), 24.0));
    let resp = ui.add(header);
    caret(ui, resp.rect, open);
    if resp.clicked() {
        menus.toggle(m);
    }
    // Opened from somewhere else (a front-panel key, the View menu): bring
    // it into view, or the click looks like it did nothing.
    if menus.take_focus(m) {
        ui.scroll_to_rect(resp.rect, Some(egui::Align::TOP));
    }
    if menus.is_open(m) {
        ui.add_space(4.0);
        body(ui, menus);
        ui.add_space(6.0);
    }
}

/// Disclosure triangle, painted so it cannot fall back to tofu.
fn caret(ui: &egui::Ui, rect: egui::Rect, open: bool) {
    let c = egui::pos2(rect.left() + 12.0, rect.center().y);
    let r = 4.0;
    let pts = if open {
        vec![
            egui::pos2(c.x - r, c.y - r * 0.6),
            egui::pos2(c.x + r, c.y - r * 0.6),
            egui::pos2(c.x, c.y + r * 0.8),
        ]
    } else {
        vec![
            egui::pos2(c.x - r * 0.6, c.y - r),
            egui::pos2(c.x - r * 0.6, c.y + r),
            egui::pos2(c.x + r * 0.8, c.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        pts,
        egui::Color32::from_gray(190),
        egui::Stroke::NONE,
    ));
}
