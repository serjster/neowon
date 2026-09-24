//! Every control injects a `stations …` / `location …` / `refdb …` script
//! action, so a script reaches everything here.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use neowon_refdb::{LocationSource, Source};

use crate::refmap::{LocationSet, RefMap, RefMapAction, Scope, StationRows, to_locator};
use crate::script::{Action, Script};
use crate::sdr::SdrState;
use crate::uitree;

#[derive(Default)]
pub struct Edits {
    tab: Tab,
    find: String,
    loc: String,
    import_path: String,
    import_source: Option<Source>,
    confirm_ip: bool,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Tab {
    #[default]
    Stations,
    Sources,
    Location,
}

fn act(script: &mut Script, a: RefMapAction) {
    script.inject(Action::RefMap(a));
}

pub fn show(
    mut contexts: EguiContexts,
    rm: Res<RefMap>,
    sdr: Res<SdrState>,
    mut script: ResMut<Script>,
    mut ed: Local<Edits>,
    mut rows: Local<StationRows>,
) {
    if !rm.stations_window {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut open = true;
    egui::Window::new("Stations")
        .open(&mut open)
        .default_size([840.0, 520.0])
        .show(ctx, |ui| {
            uitree::name(ui, "Stations");
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (Tab::Stations, "Stations"),
                    (Tab::Sources, "Sources"),
                    (Tab::Location, "Location"),
                ] {
                    if ui.selectable_label(ed.tab == tab, name).clicked() {
                        ed.tab = tab;
                    }
                }
                if let Some(job) = &rm.job {
                    ui.separator();
                    ui.spinner();
                    ui.weak(&job.what);
                }
            });
            ui.separator();
            match ed.tab {
                Tab::Stations => stations_tab(ui, &rm, &sdr, &mut ed, &mut rows, &mut script),
                Tab::Sources => sources_tab(ui, &rm, &mut ed, &mut script),
                Tab::Location => location_tab(ui, &rm, &mut ed, &mut script),
            }
        });
    if !open {
        act(&mut script, RefMapAction::StationsWindow(false));
    }
}

fn stations_tab(
    ui: &mut egui::Ui,
    rm: &RefMap,
    sdr: &SdrState,
    ed: &mut Edits,
    cache: &mut StationRows,
    script: &mut Script,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("find");
        if ui
            .add(egui::TextEdit::singleline(&mut ed.find).desired_width(130.0))
            .changed()
        {
            act(script, RefMapAction::Find(ed.find.clone()));
        }
        source_combo(ui, "st-source", rm, script);
        service_combo(ui, "st-service", rm, script);
        mod_combo(ui, "st-mod", rm, script);
        let mut on_air = rm.on_air;
        if ui
            .checkbox(&mut on_air, "on air")
            .on_hover_text("hide scheduled rows that are off air now")
            .changed()
        {
            act(script, RefMapAction::OnAir(on_air));
        }
        egui::ComboBox::from_id_salt("st-scope")
            .selected_text(match rm.scope {
                Scope::All => "all",
                Scope::View => "in view",
                Scope::Near => "near me",
            })
            .show_ui(ui, |ui| {
                for (scope, name) in [
                    (Scope::All, "all"),
                    (Scope::View, "in view"),
                    (Scope::Near, "near me"),
                ] {
                    if ui.selectable_label(rm.scope == scope, name).clicked() {
                        act(script, RefMapAction::Scope(scope));
                    }
                }
            });
        egui::ComboBox::from_id_salt("st-sort")
            .selected_text(match rm.sort {
                neowon_refdb::SortBy::Frequency => "by freq",
                neowon_refdb::SortBy::Distance => "by distance",
            })
            .show_ui(ui, |ui| {
                for (sort, name) in [
                    (neowon_refdb::SortBy::Frequency, "by freq"),
                    (neowon_refdb::SortBy::Distance, "by distance"),
                ] {
                    if ui.selectable_label(rm.sort == sort, name).clicked() {
                        act(script, RefMapAction::Sort(sort));
                    }
                }
            });
        if ui.button("Clear filters").clicked() {
            act(script, RefMapAction::Find(String::new()));
            act(script, RefMapAction::FilterSource(None));
            act(script, RefMapAction::FilterService(None));
            act(script, RefMapAction::FilterModulation(None));
            act(script, RefMapAction::OnAir(false));
            act(script, RefMapAction::Scope(Scope::All));
            ed.find.clear();
        }
    });
    // Built once per change of the set or the filters, not per frame.
    let (total, rows) = cache.page(rm, sdr, 500);
    ui.weak(format!(
        "{total} rows · click a frequency to tune (and pick the demod) · + cat copies the row into the catalog",
    ));
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("stations-grid")
            .striped(true)
            .show(ui, |ui| {
                for h in ["MHz", "name", "mod", "service", "km", "source", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (s, km) in &rows {
                    let key = RefMap::key(s);
                    let selected = rm.selected.as_deref() == Some(key.as_str());
                    if ui
                        .selectable_label(selected, format!("{:.4}", s.freq_hz / 1e6))
                        .clicked()
                    {
                        act(script, RefMapAction::TuneStation(key.clone()));
                    }
                    if ui.selectable_label(selected, &s.name).clicked() {
                        act(script, RefMapAction::TuneStation(key.clone()));
                    }
                    ui.monospace(s.modulation.label());
                    ui.label(s.service.label());
                    ui.monospace(km.map_or("-".to_string(), |k| format!("{k:.0}")));
                    ui.label(s.source.label());
                    let cat = ui
                        .small_button("+ cat")
                        .on_hover_text("copy into the catalog (D20)");
                    if cat.clicked() {
                        act(script, RefMapAction::CatalogStation(key));
                    }
                    ui.end_row();
                }
            });
    });
}

fn source_combo(ui: &mut egui::Ui, id: &str, rm: &RefMap, script: &mut Script) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(rm.filter_source.map_or("any source", |s| s.label()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(rm.filter_source.is_none(), "any source")
                .clicked()
            {
                act(script, RefMapAction::FilterSource(None));
            }
            for s in Source::ALL {
                if ui
                    .selectable_label(rm.filter_source == Some(s), s.label())
                    .clicked()
                {
                    act(script, RefMapAction::FilterSource(Some(s)));
                }
            }
        });
}

fn service_combo(ui: &mut egui::Ui, id: &str, rm: &RefMap, script: &mut Script) {
    use neowon_refdb::Service;
    let all = [
        Service::Broadcast,
        Service::Aviation,
        Service::Marine,
        Service::Amateur,
        Service::Utility,
        Service::Other,
    ];
    egui::ComboBox::from_id_salt(id)
        .selected_text(rm.filter_service.map_or("any service", |s| s.label()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(rm.filter_service.is_none(), "any service")
                .clicked()
            {
                act(script, RefMapAction::FilterService(None));
            }
            for s in all {
                if ui
                    .selectable_label(rm.filter_service == Some(s), s.label())
                    .clicked()
                {
                    act(script, RefMapAction::FilterService(Some(s)));
                }
            }
        });
}

fn mod_combo(ui: &mut egui::Ui, id: &str, rm: &RefMap, script: &mut Script) {
    use neowon_refdb::Modulation as M;
    let all = [
        M::Am,
        M::Fm,
        M::Wfm,
        M::Nfm,
        M::Usb,
        M::Lsb,
        M::Cw,
        M::Dab,
        M::Dvbt,
        M::Digital,
        M::Unknown,
    ];
    egui::ComboBox::from_id_salt(id)
        .selected_text(rm.filter_modulation.map_or("any mod", |m| m.label()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(rm.filter_modulation.is_none(), "any mod")
                .clicked()
            {
                act(script, RefMapAction::FilterModulation(None));
            }
            for m in all {
                if ui
                    .selectable_label(rm.filter_modulation == Some(m), m.label())
                    .clicked()
                {
                    act(script, RefMapAction::FilterModulation(Some(m)));
                }
            }
        });
}

fn sources_tab(ui: &mut egui::Ui, rm: &RefMap, ed: &mut Edits, script: &mut Script) {
    egui::Grid::new("sources-grid")
        .striped(true)
        .show(ui, |ui| {
            for h in ["source", "rows", "fetched", "origin", "licence", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for src in Source::ALL {
                let meta = rm.metas.iter().find(|m| m.source == src);
                ui.label(src.label());
                ui.monospace(meta.map_or("-".to_string(), |m| m.count.to_string()));
                ui.monospace(meta.map_or("-".to_string(), |m| m.fetched_at.clone()));
                ui.label(meta.map_or("-".to_string(), |m| m.origin.clone()));
                ui.label(meta.map_or("-".to_string(), |m| m.licence.clone()));
                ui.horizontal(|ui| {
                    let needs = matches!(src, Source::Wikidata | Source::Fcc);
                    let blocked = needs && rm.location.is_none();
                    let mut fetch =
                        ui.add_enabled(!blocked && rm.job.is_none(), egui::Button::new("Fetch"));
                    if blocked {
                        fetch = fetch.on_disabled_hover_text(
                            "needs a location — set one in the Location tab first",
                        );
                    }
                    if fetch.clicked() {
                        act(script, RefMapAction::Fetch(src, None));
                    }
                    // A damaged source has no metadata shown but must
                    // still be clearable.
                    let has = meta.is_some() || rm.problems.iter().any(|p| p.source == Some(src));
                    if ui
                        .add_enabled(has && rm.job.is_none(), egui::Button::new("Clear"))
                        .clicked()
                    {
                        act(script, RefMapAction::Clear(src));
                    }
                });
                ui.end_row();
            }
        });
    for p in &rm.problems {
        ui.colored_label(ui.visuals().warn_fg_color, p.to_string());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("import");
        ui.text_edit_singleline(&mut ed.import_path)
            .on_hover_text("file; an OurAirports import takes a directory");
        egui::ComboBox::from_id_salt("import-source")
            .selected_text(ed.import_source.map_or("source…", |s| s.label()))
            .show_ui(ui, |ui| {
                for s in Source::ALL {
                    if ui
                        .selectable_label(ed.import_source == Some(s), s.label())
                        .clicked()
                    {
                        ed.import_source = Some(s);
                    }
                }
            });
        let ready = !ed.import_path.trim().is_empty() && ed.import_source.is_some();
        if ui
            .add_enabled(ready && rm.job.is_none(), egui::Button::new("Import"))
            .clicked()
        {
            act(
                script,
                RefMapAction::Import(ed.import_source.unwrap(), ed.import_path.trim().to_string()),
            );
        }
    });
    ui.weak(
        "FMLIST is import-only (D17); OurAirports needs the two CSVs in one directory. \
         No source is fetched at startup or in the background.",
    );
    ui.separator();
    if let Some(job) = &rm.job {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(&job.what);
        });
    }
    if !rm.status.is_empty() {
        ui.label(&rm.status);
    }
}

fn location_tab(ui: &mut egui::Ui, rm: &RefMap, ed: &mut Edits, script: &mut Script) {
    ui.horizontal(|ui| {
        ui.label("lat lon, or a locator");
        ui.add(egui::TextEdit::singleline(&mut ed.loc).desired_width(130.0));
        if ui.button("Set").clicked() && !ed.loc.trim().is_empty() {
            act(script, parse_location_field(ed.loc.trim()));
        }
        if ui
            .button("Locate me…")
            .on_hover_text("one request to ipapi.co, after the dialog")
            .clicked()
        {
            ed.confirm_ip = true;
        }
        if ui.button("Clear").clicked() {
            act(script, RefMapAction::Location(LocationSet::Clear));
        }
    });
    match &rm.location {
        Some(l) => {
            let source = match l.source {
                LocationSource::Manual => "manual",
                LocationSource::Locator => "locator",
                LocationSource::Ip => "ip",
            };
            ui.monospace(format!(
                "{:.4}, {:.4}  {}  ({source}, {})",
                l.lat,
                l.lon,
                to_locator(l.at(), 6),
                l.set_at,
            ));
            if let Some(cc) = &l.country_code {
                ui.weak(format!("country {cc}"));
            }
        }
        None => {
            ui.weak("no location: stations are shown unranked and unfiltered by distance");
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("near-me radius");
        let text = rm
            .radius_km
            .map_or("auto (per service)".to_string(), |r| format!("{r} km"));
        egui::ComboBox::from_id_salt("st-radius")
            .selected_text(text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(rm.radius_km.is_none(), "auto (per service)")
                    .clicked()
                {
                    act(script, RefMapAction::Radius(None));
                }
                for km in [25.0, 50.0, 100.0, 150.0, 300.0] {
                    if ui
                        .selectable_label(rm.radius_km == Some(km), format!("{km} km"))
                        .clicked()
                    {
                        act(script, RefMapAction::Radius(Some(km)));
                    }
                }
            });
    });
    ui.weak(
        "`Locate me` sends your public IP to ipapi.co and stores the approximate fix only. \
         Nothing looks you up unless you ask.",
    );
    if ed.confirm_ip {
        egui::Window::new("Look up my location?")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(
                    "This performs one HTTPS request to ipapi.co with your public IP\n\
                     address and returns an approximate city and country. Nothing\n\
                     else is sent; only the resulting coordinates are stored.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        ed.confirm_ip = false;
                    }
                    if ui.button("Look up").clicked() {
                        ed.confirm_ip = false;
                        act(script, RefMapAction::Location(LocationSet::Ip));
                    }
                });
            });
    }
}

/// The location field is either two numbers or a locator; the action
/// carries whichever it is, so the field reaches the same code as the
/// script verb.
fn parse_location_field(text: &str) -> RefMapAction {
    let mut it = text.split_whitespace();
    if let (Some(a), Some(b)) = (it.next(), it.next())
        && let (Ok(lat), Ok(lon)) = (a.parse::<f64>(), b.parse::<f64>())
    {
        return RefMapAction::Location(LocationSet::Coords(lat, lon));
    }
    RefMapAction::Location(LocationSet::Locator(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::parse_location_field;

    #[test]
    fn the_field_reads_as_coordinates_or_a_locator() {
        assert!(matches!(
            parse_location_field("38.72 -9.14"),
            super::RefMapAction::Location(super::LocationSet::Coords(..))
        ));
        assert!(matches!(
            parse_location_field("IN58"),
            super::RefMapAction::Location(super::LocationSet::Locator(_))
        ));
    }
}
