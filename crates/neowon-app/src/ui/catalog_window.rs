//! The Catalog window: browse, filter and manage catalogued
//! signals. Every control injects a `catalog …` script action, so a script
//! reaches everything the window does.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use neowon_catalog::Id;

use crate::catalog::{CatalogAction, CatalogState};
use crate::script::{Action, Script};
use crate::sdr::SdrAction;

/// Text the window's fields hold between frames.
#[derive(Default)]
pub struct Edits {
    filter: String,
    rename: String,
    alias: String,
    tag: String,
    path: String,
    cascade: bool,
    merge_into: Option<Id>,
}

/// Rows the signal list draws per frame.
const ROWS: usize = 500;

fn act(script: &mut Script, a: CatalogAction) {
    script.inject(Action::Catalog(a));
}

pub fn show(
    mut contexts: EguiContexts,
    st: Res<CatalogState>,
    mut script: ResMut<Script>,
    mut ed: Local<Edits>,
) {
    if !st.window {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut open = true;
    egui::Window::new("Catalog")
        .open(&mut open)
        .default_size([560.0, 460.0])
        .show(ctx, |ui| {
            let Some(cat) = &st.cat else {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("no catalog open at {}", st.path.display()),
                );
                return;
            };
            ui.horizontal(|ui| {
                ui.label("filter");
                if ui.text_edit_singleline(&mut ed.filter).changed() {
                    act(&mut script, CatalogAction::List(ed.filter.clone()));
                }
                if ui
                    .button("Add strongest")
                    .on_hover_text("file the strongest live detection")
                    .clicked()
                {
                    act(&mut script, CatalogAction::Add(None));
                }
                if ui
                    .button("Observe")
                    .on_hover_text("file live detections against catalogued signals")
                    .clicked()
                {
                    act(&mut script, CatalogAction::Observe);
                }
                if ui.button("Undo").clicked() {
                    act(&mut script, CatalogAction::Undo);
                }
            });
            ui.weak(format!(
                "{} · seq {} · {} entities · integrity {}",
                st.path.display(),
                cat.seq(),
                cat.state().entities.len(),
                match st.integrity_problems() {
                    0 => "ok".to_string(),
                    n => format!("{n} problems"),
                }
            ));
            ui.separator();
            // A page per frame, not every signal.
            let (listed, signals) = st.signals_page(ROWS);
            if listed > signals.len() {
                ui.weak(format!(
                    "{listed} signals, the first {} shown — narrow with the filter",
                    signals.len()
                ));
            }
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    egui::Grid::new("catalog-signals")
                        .striped(true)
                        .show(ui, |ui| {
                            for h in ["", "id", "name", "MHz", "bw kHz", "obs", "tags"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for s in &signals {
                                let mut pinned = s.pinned;
                                if ui
                                    .checkbox(&mut pinned, "")
                                    .on_hover_text("pinned: never purged")
                                    .changed()
                                {
                                    act(&mut script, CatalogAction::Pin(s.id, pinned));
                                }
                                ui.monospace(s.id.to_string());
                                let sel = st.selected == Some(s.id);
                                if ui.selectable_label(sel, &s.name).clicked() {
                                    act(&mut script, CatalogAction::Select(Some(s.id)));
                                    ed.rename = s.name.clone();
                                }
                                ui.monospace(format!("{:.4}", s.centre_hz / 1e6));
                                ui.monospace(format!("{:.1}", s.bandwidth_hz / 1e3));
                                ui.monospace(st.observations_of(s.id).to_string());
                                ui.label(s.tags.iter().cloned().collect::<Vec<_>>().join(", "));
                                ui.end_row();
                            }
                        });
                });
            if let Some(s) = st.selected.and_then(|id| cat.state().signal(id)) {
                ui.separator();
                selected(ui, &st, s, &signals, &mut ed, &mut script);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("file");
                ui.text_edit_singleline(&mut ed.path);
                let has = !ed.path.trim().is_empty();
                if ui.add_enabled(has, egui::Button::new("Export")).clicked() {
                    act(&mut script, CatalogAction::Export(ed.path.trim().into()));
                }
                if ui.add_enabled(has, egui::Button::new("Import")).clicked() {
                    act(&mut script, CatalogAction::Import(ed.path.trim().into()));
                }
            });
        });
    if !open {
        act(&mut script, CatalogAction::Window(false));
    }
}

/// Actions on the selected signal.
fn selected(
    ui: &mut egui::Ui,
    st: &CatalogState,
    s: &neowon_catalog::Signal,
    signals: &[&neowon_catalog::Signal],
    ed: &mut Edits,
    script: &mut Script,
) {
    ui.horizontal(|ui| {
        ui.strong(format!("{} {}", s.id, s.name));
        if ui.button("Tune").clicked() {
            script.inject(Action::Sdr(SdrAction::Tune(s.centre_hz.round())));
        }
        ui.weak(format!("{} observations", st.observations_of(s.id)));
    });
    if !s.aliases.is_empty() {
        let names: Vec<&str> = s.aliases.iter().map(|a| a.name.as_str()).collect();
        ui.weak(format!("also known as: {}", names.join(", ")));
    }
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut ed.rename);
        if ui.button("Rename").clicked() && !ed.rename.trim().is_empty() {
            act(script, CatalogAction::Rename(s.id, ed.rename.trim().into()));
        }
        ui.text_edit_singleline(&mut ed.alias);
        if ui.button("Alias").clicked() && !ed.alias.trim().is_empty() {
            act(script, CatalogAction::Alias(s.id, ed.alias.trim().into()));
            ed.alias.clear();
        }
    });
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut ed.tag);
        let tag = ed.tag.trim().to_string();
        if ui.button("Tag").clicked() && !tag.is_empty() {
            act(script, CatalogAction::Tag(s.id, tag.clone(), true));
        }
        if ui.button("Untag").clicked() && !tag.is_empty() {
            act(script, CatalogAction::Tag(s.id, tag, false));
        }
    });
    ui.horizontal(|ui| {
        let target = ed
            .merge_into
            .and_then(|id| signals.iter().find(|x| x.id == id));
        egui::ComboBox::from_id_salt("merge-into")
            .selected_text(target.map_or("merge into…".into(), |t| format!("{} {}", t.id, t.name)))
            .show_ui(ui, |ui| {
                for t in signals.iter().filter(|t| t.id != s.id) {
                    ui.selectable_value(
                        &mut ed.merge_into,
                        Some(t.id),
                        format!("{} {}", t.id, t.name),
                    );
                }
            });
        if ui
            .add_enabled(target.is_some(), egui::Button::new("Merge"))
            .clicked()
            && let Some(to) = ed.merge_into.take()
        {
            act(script, CatalogAction::Merge(s.id, to));
        }
        ui.separator();
        ui.checkbox(&mut ed.cascade, "cascade");
        if ui
            .add_enabled(!s.pinned, egui::Button::new("Delete"))
            .on_disabled_hover_text("unpin first")
            .clicked()
        {
            act(script, CatalogAction::Delete(s.id, ed.cascade));
        }
    });
}
