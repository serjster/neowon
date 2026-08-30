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
    dialog_acquire, dialog_channel, dialog_cursor, dialog_display, dialog_horizontal, dialog_math,
    dialog_measure, dialog_record, dialog_trigger, dialog_utility,
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
        }
    }
}

/// Which dock sections are expanded. Sections are independent — leave as
/// many open as you like; the rail scrolls.
#[derive(Resource)]
pub struct MenuState {
    open: Vec<Menu>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: vec![Menu::Trigger],
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
        }
    }

    /// Expand a section (keeping others as they are).
    pub fn open(&mut self, m: Menu) {
        if !self.is_open(m) {
            self.open.push(m);
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
) {
    let rect = Roi::Dialog.rect(l);
    let mut frame = egui::Frame::new();
    frame.fill = egui::Color32::from_rgb(12, 14, 18);
    frame.stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(50));
    frame.inner_margin = 6.0.into();

    const SECTIONS: [Menu; 9] = [
        Menu::Trigger,
        Menu::Horizontal,
        Menu::Acquire,
        Menu::Channel(0),
        Menu::Channel(1),
        Menu::Math,
        Menu::Measure,
        Menu::Cursor,
        Menu::Display,
    ];

    egui::Area::new("dock".into())
        .fixed_pos(rect.min)
        .constrain(true)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width());
            ui.set_min_width(rect.width());
            ui.set_max_height(rect.height());
            frame.show(ui, |ui| {
                ui.set_min_height(rect.height() - 12.0);
                egui::ScrollArea::vertical()
                    .max_height(rect.height() - 16.0)
                    .show(ui, |ui| {
                        ui.set_min_width(rect.width() - 24.0);
                        for m in SECTIONS {
                            section(ui, menus, m, |ui, menus| match m {
                                Menu::Channel(ch) => dialog_channel::show(ui, link, ch),
                                Menu::Horizontal => dialog_horizontal::show(ui, link),
                                Menu::Trigger => dialog_trigger::show(ui, link),
                                Menu::Acquire => dialog_acquire::show(ui, link),
                                Menu::Display => dialog_display::show(
                                    ui, link, phosphor, cur, refs, fft, wf, viz, fx, script,
                                ),
                                Menu::Measure => dialog_measure::show(ui, meas),
                                Menu::Math => dialog_math::show(ui, math),
                                Menu::Cursor => dialog_cursor::show(ui, link, cur, meas),
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
    let arrow = if open { "▼" } else { "▶" };
    let header = egui::Button::new(
        egui::RichText::new(format!("{arrow} {}", m.title()))
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
    if ui.add(header).clicked() {
        menus.toggle(m);
    }
    if menus.is_open(m) {
        ui.add_space(4.0);
        body(ui, menus);
        ui.add_space(6.0);
    }
}
