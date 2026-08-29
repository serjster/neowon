//! The dialog box: which function is open, and the right-side panel that
//! hosts its controls (manual 7.6 — one dialog at a time, collapsible title).

use bevy::prelude::*;
use bevy_egui::egui;

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::Phosphor;

use super::layout::Roi;
use super::{
    dialog_acquire, dialog_channel, dialog_cursor, dialog_display, dialog_horizontal, dialog_math,
    dialog_measure, dialog_trigger, dialog_utility,
};

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
        }
    }
}

/// The single open dialog. `None` = collapsed (manual 7.6 title bar).
#[derive(Resource, Default)]
pub struct MenuState {
    pub open: Option<Menu>,
}

impl MenuState {
    /// Toggle: pressing the already-open function collapses the dialog.
    pub fn toggle(&mut self, m: Menu) {
        self.open = match self.open {
            Some(cur) if cur == m => None,
            _ => Some(m),
        };
    }
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    menus: &mut MenuState,
    link: &mut Link,
    phosphor: &mut Phosphor,
    math: &mut MathState,
    meas: &mut MeasureState,
    fft: &mut FftState,
    cur: &mut CursorState,
    pf: &mut PfState,
) {
    let Some(menu) = menus.open else { return };
    let rect = Roi::Dialog.rect();
    let mut frame = egui::Frame::new();
    frame.fill = egui::Color32::from_rgb(12, 14, 18);
    frame.stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(60));
    frame.inner_margin = 8.0.into();

    egui::Area::new("dialog".into())
        .anchor(egui::Align2::LEFT_TOP, rect.min.to_vec2())
        .fixed_pos(rect.min)
        .constrain(true)
        .show(ctx, |ui| {
            ui.set_max_width(rect.width() - 16.0);
            ui.set_max_height(rect.height() - 12.0);
            frame.show(ui, |ui| {
                // Title bar: touching it collapses the dialog (manual 7.6).
                ui.horizontal(|ui| {
                    let title = egui::RichText::new(menu.title()).strong();
                    if ui.add(egui::Button::new(title).frame(false)).clicked() {
                        menus.open = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕").clicked() {
                            menus.open = None;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match menu {
                    Menu::Channel(ch) => dialog_channel::show(ui, link, ch),
                    Menu::Horizontal => dialog_horizontal::show(ui, link),
                    Menu::Trigger => dialog_trigger::show(ui, link),
                    Menu::Acquire => dialog_acquire::show(ui, link),
                    Menu::Display => dialog_display::show(ui, link, phosphor),
                    Menu::Measure => dialog_measure::show(ui, meas),
                    Menu::Math => dialog_math::show(ui, math),
                    Menu::Cursor => dialog_cursor::show(ui, link, cur, meas),
                    Menu::Utility => dialog_utility::show(ui, link, math, pf, fft),
                });
            });
        });
}
