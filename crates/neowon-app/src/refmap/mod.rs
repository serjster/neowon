//! RF reference in the app: the band plans loaded at startup,
//! the station store (`~/.neowon/refdb`), the operator's location, and what
//! the SDR screen shows of them — the band strip under the spectrum, the
//! whole-range minimap, the RF map window, the station overlay and the
//! stations window. Script verbs live in `actions`; `get bands` /
//! `get station*` / `get refdb` / `get location` are `readout`.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy_egui::egui;
use neowon_refdb::{Band, BandPlan, Index, Location, Meta, NamedPlan, Source, Station, Store};

use crate::Link;
use crate::control::escape;
use crate::sdr::SdrState;

mod actions;
mod jobs;
mod readout;
mod rows;

pub use actions::{LocationSet, RefMapAction, parse, run};
use jobs::{Job, JobDone};
pub use readout::{location_json, refdb_json, stations_query};
pub use rows::StationRows;

pub use neowon_refdb::geo::{haversine_km, to_locator};

#[derive(Resource)]
pub struct RefMap {
    pub plans: Vec<NamedPlan>,
    /// Index into `plans`.
    pub active: usize,
    pub strip: bool,
    /// The whole-range minimap across the top of the SDR canvas.
    pub mini: bool,
    pub window: bool,

    pub stations_window: bool,
    pub overlay: bool,
    /// Radius for "near me", km; `None` = per-service defaults.
    pub radius_km: Option<f64>,
    pub location: Option<Location>,
    pub store: Option<Store>,
    pub index: Index,
    pub metas: Vec<Meta>,
    /// What the last load could not use, by source.
    pub problems: Vec<neowon_refdb::Problem>,
    pub status: String,
    pub job: Option<Job>,
    pub find: String,
    pub filter_source: Option<Source>,
    pub filter_service: Option<neowon_refdb::Service>,
    pub filter_modulation: Option<neowon_refdb::Modulation>,
    pub on_air: bool,
    pub scope: Scope,
    pub sort: neowon_refdb::SortBy,
    pub selected: Option<String>,
    /// UTC weekday (0 = Monday) and minute, for schedules and EiBi.
    pub utc: (u8, u16),
}

impl RefMap {
    /// Shipped plans (`assets/bandplans`, next to the binary when not
    /// found from the working directory) and the operator's
    /// (`~/.neowon/bandplans`); the worldwide plan is active. The station
    /// store and the location load from disk; nothing touches the network.
    pub fn load() -> Self {
        let user = std::env::var_os("HOME")
            .map(|h| std::path::Path::new(&h).join(".neowon/bandplans"))
            .unwrap_or_default();
        Self::load_from(
            &user,
            neowon_refdb::store::default_dir(),
            neowon_refdb::geo::location_path(),
        )
    }

    /// The shipped plans alone: no user plans, no station store, no
    /// location. A unit test's refmap, so it neither reads nor creates
    /// anything under the operator's `~`.
    #[cfg(test)]
    pub fn shipped_only() -> Self {
        Self::load_from(std::path::Path::new(""), None, None)
    }

    /// [`load`](Self::load) from explicit per-user paths; `None` leaves the
    /// station store or the location unset.
    fn load_from(
        user: &std::path::Path,
        refdb: Option<std::path::PathBuf>,
        location: Option<std::path::PathBuf>,
    ) -> Self {
        let (plans, errors) = neowon_refdb::load_plans(&shipped_dir(), user);
        for (path, e) in errors {
            warn!("bandplan: {} not loaded: {e}", path.display());
        }
        for (stem, p) in &plans {
            if !p.rejected.is_empty() {
                debug!("bandplan {stem}: dropped {:?}", p.rejected);
            }
        }
        info!("bandplan: {} plans", plans.len());
        let active = plans.iter().position(|(s, _)| s == "general").unwrap_or(0);

        let mut rm = Self {
            plans,
            active,
            strip: true,
            mini: true,
            window: false,
            stations_window: false,
            overlay: true,
            radius_km: None,
            location: None,
            store: None,
            index: Index::default(),
            metas: Vec::new(),
            problems: Vec::new(),
            status: String::new(),
            job: None,
            find: String::new(),
            filter_source: None,
            filter_service: None,
            filter_modulation: None,
            on_air: false,
            scope: Scope::All,
            sort: neowon_refdb::SortBy::Frequency,
            selected: None,
            utc: utc_now(),
        };
        match refdb {
            Some(dir) => match Store::open(&dir) {
                Ok(store) => {
                    rm.store = Some(store);
                    rm.reload();
                }
                Err(e) => rm.status = format!("refdb {}: {e}", dir.display()),
            },
            None => rm.status = "no refdb directory (set HOME or NEOWON_REFDB)".into(),
        }
        rm.location = location.and_then(|p| Location::load(&p).ok().flatten());
        rm
    }

    /// Re-read every snapshot from the store; cheap enough per job finish.
    /// A source that cannot be read is named in the status line and
    /// `get refdb`; the others load regardless.
    pub fn reload(&mut self) {
        let Some(store) = &self.store else { return };
        let loaded = store.load();
        info!(
            "refdb: {} stations from {} sources",
            loaded.index.len(),
            loaded.metas.len()
        );
        for p in &loaded.problems {
            warn!("refdb: {p}");
        }
        if !loaded.problems.is_empty() {
            let named: Vec<String> = loaded.problems.iter().map(ToString::to_string).collect();
            self.status = format!("refdb: {}", named.join("; "));
        }
        self.index = loaded.index;
        self.metas = loaded.metas;
        self.problems = loaded.problems;
    }

    pub fn plan(&self) -> Option<&BandPlan> {
        self.plans.get(self.active).map(|(_, p)| p)
    }

    pub fn stem(&self) -> &str {
        self.plans.get(self.active).map_or("", |(s, _)| s.as_str())
    }

    /// Bands containing `hz`, narrowest first (the most specific name
    /// leads: "2m Ham Band" before a wider allocation around it).
    pub fn at(&self, hz: f64) -> Vec<&Band> {
        let mut v: Vec<&Band> = self.plan().map(|p| p.at(hz).collect()).unwrap_or_default();
        v.sort_by(|a, b| (a.hi_hz - a.lo_hz).total_cmp(&(b.hi_hz - b.lo_hz)));
        v
    }

    pub fn key(s: &Station) -> String {
        format!("{}:{}", s.source.stem(), s.id)
    }

    pub fn station(&self, key: &str) -> Option<&Station> {
        let (stem, id) = key.split_once(':')?;
        let src = Source::from_stem(stem)?;
        self.index.iter().find(|s| s.source == src && s.id == id)
    }

    /// The stations the spectrum overlay shows: in view, radius-filtered
    /// (per service, or the operator's override), scheduled rows only
    /// while on air, nearest first.
    pub fn overlay_stations(&self, sdr: &SdrState) -> Vec<(&Station, Option<f64>)> {
        let at = self.location.as_ref().map(Location::at);
        let half = sdr.span() / 2.0;
        let (lo, hi) = (sdr.view_centre() - half, sdr.view_centre() + half);
        let mut v: Vec<(&Station, Option<f64>)> = self
            .index
            .within(lo, hi)
            .iter()
            .filter(|s| self.on_air_now(s))
            .filter_map(|s| {
                let km = at.and_then(|a| s.at().map(|p| haversine_km(a, p)));
                self.within_radius(s, km).then_some((s, km))
            })
            .collect();
        v.sort_by(|(a, ka), (b, kb)| {
            let d = |k: &Option<f64>| k.unwrap_or(f64::INFINITY);
            d(ka)
                .total_cmp(&d(kb))
                .then_with(|| a.freq_hz.total_cmp(&b.freq_hz))
        });
        v
    }

    /// Is a scheduled station on air at the app's clock? Unscheduled rows
    /// always pass; EiBi shortwave rows are the ones this hides.
    pub fn on_air_now(&self, s: &Station) -> bool {
        s.schedule
            .as_ref()
            .is_none_or(|sch| sch.on_air(self.utc.0, self.utc.1))
    }

    /// Per-service radius defaults: broadcast FM/TV 150 km,
    /// aviation 100 km, shortwave and everything else unconstrained. An
    /// operator radius overrides all of them. No coordinates and no
    /// location is never hidden.
    pub fn within_radius(&self, s: &Station, km: Option<f64>) -> bool {
        let Some(p) = s.at() else { return true };
        let from = self.location.as_ref().map(Location::at);
        let Some(km) = km.or_else(|| from.map(|a| haversine_km(a, p))) else {
            return true;
        };
        let limit = self.radius_km.unwrap_or(match s.service {
            neowon_refdb::Service::Aviation => 100.0,
            neowon_refdb::Service::Broadcast => 150.0,
            _ => return true,
        });
        km <= limit
    }

    pub fn set_location(&mut self, loc: Location) {
        if let Some(path) = neowon_refdb::geo::location_path()
            && let Err(e) = loc.save(&path)
        {
            self.status = format!("location: {e}");
            return;
        }
        self.status = format!(
            "location: {}, {} ({})",
            loc.lat,
            loc.lon,
            match loc.source {
                neowon_refdb::LocationSource::Manual => "manual",
                neowon_refdb::LocationSource::Locator => "locator",
                neowon_refdb::LocationSource::Ip => "ip",
            }
        );
        self.location = Some(loc);
    }
}

/// The scope of a station query: `view` (the IQ window), `near` (the
/// location radius) or `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    View,
    Near,
    All,
}

impl Scope {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "view" => Scope::View,
            "near" => Scope::Near,
            "all" => Scope::All,
            _ => return None,
        })
    }
}

pub fn tick(mut rm: ResMut<RefMap>, mut link: ResMut<Link>) {
    rm.utc = utc_now();
    let Some(rx) = rm.job.as_ref().map(|j| j.rx.clone()) else {
        return;
    };
    match rx.try_recv() {
        Ok(Ok(done)) => {
            rm.job = None;
            match done {
                JobDone::Fetched(src, report) | JobDone::Imported(src, report) => {
                    rm.reload();
                    rm.status = format!("{}: {}", src.label(), report.summary());
                }
                JobDone::Located(mut loc) => {
                    loc.set_at = neowon_catalog::now_rfc3339();
                    rm.set_location(loc);
                }
            }
            link.status = rm.status.clone();
        }
        Ok(Err(e)) => {
            rm.job = None;
            rm.status = e.to_string();
            link.status = format!("error: {e}");
            error!("script: refdb job: {e}");
        }
        Err(crossbeam_channel::TryRecvError::Empty) => {}
        Err(crossbeam_channel::TryRecvError::Disconnected) => {
            rm.job = None;
            rm.status = "refdb job failed: worker gone".into();
        }
    }
}

/// UTC weekday (0 = Monday) and minutes since midnight, from the system
/// clock — schedules are UTC and the operator reads them as such.
pub fn utc_now() -> (u8, u16) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let minute = ((secs % 86_400) / 60) as u16;
    // 1970-01-01 was a Thursday; 0 = Monday.
    let weekday = ((days + 3).rem_euclid(7)) as u8;
    (weekday, minute)
}

fn shipped_dir() -> std::path::PathBuf {
    let relative = std::path::PathBuf::from("assets/bandplans");
    if relative.is_dir() {
        return relative;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside = dir.join("assets/bandplans");
        if beside.is_dir() {
            return beside;
        }
    }
    // A development build run from elsewhere: the source tree.
    let tree = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/bandplans");
    if tree.is_dir() {
        return tree;
    }
    relative
}

/// Band colour by kind. SDR++'s defaults for the five kinds it colours
/// (amateur red, aviation green, broadcast blue, marine cyan, military
/// yellow), extended to the other kinds its plans use; unknown kinds grey.
pub fn colour(kind: &str) -> egui::Color32 {
    let k = kind.to_ascii_lowercase();
    let (r, g, b) = if k.starts_with("amateur") {
        (225, 70, 70)
    } else if k.starts_with("aviation") || k == "aircraft" {
        (70, 195, 95)
    } else if k.starts_with("broadcast") {
        (80, 120, 235)
    } else if k.starts_with("marine") {
        (45, 195, 205)
    } else if k.starts_with("military") {
        (220, 200, 50)
    } else if k.starts_with("satellite") {
        (165, 100, 225)
    } else if k.contains("mobile") || k.contains("lte") || k == "cellular" || k == "pmr" {
        (225, 100, 175)
    } else if k.starts_with("utility") || k == "fixed" || k == "comms" || k == "railway" {
        (230, 145, 55)
    } else if k == "navigation" || k == "radiolocation" || k == "astronomy" {
        (60, 170, 150)
    } else if k == "ism" {
        (150, 170, 90)
    } else {
        (125, 130, 140)
    };
    egui::Color32::from_rgb(r, g, b)
}

/// Stack overlapping bands into lanes (greedy, widest first, so an
/// allocation sits in lane 0 and the bands inside it above).
pub fn lanes<'a>(bands: impl Iterator<Item = &'a Band>) -> Vec<(usize, &'a Band)> {
    let mut v: Vec<&Band> = bands.collect();
    v.sort_by(|a, b| (b.hi_hz - b.lo_hz).total_cmp(&(a.hi_hz - a.lo_hz)));
    let mut ends: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut out = Vec::with_capacity(v.len());
    for b in v {
        let lane = ends
            .iter()
            .position(|l| l.iter().all(|&(lo, hi)| b.hi_hz <= lo || b.lo_hz >= hi))
            .unwrap_or_else(|| {
                ends.push(Vec::new());
                ends.len() - 1
            });
        ends[lane].push((b.lo_hz, b.hi_hz));
        out.push((lane, b));
    }
    out
}

/// "87.5–108 MHz", in the unit that reads best.
pub fn fmt_range(lo: f64, hi: f64) -> String {
    let (div, unit) = if hi >= 1e9 {
        (1e9, "GHz")
    } else if hi >= 1e6 {
        (1e6, "MHz")
    } else {
        (1e3, "kHz")
    };
    let f = |x: f64| {
        let s = format!("{:.4}", x / div);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    format!("{}–{} {unit}", f(lo), f(hi))
}

/// `get bands`: the plan, the bands at the tuned frequency (narrowest
/// first) and those in view. Station/refdb readouts live in `readout`.
pub fn bands_json(rm: &RefMap, sdr: &SdrState) -> String {
    let band = |b: &Band| {
        format!(
            r#"{{"name":"{}","kind":"{}","lo_hz":{},"hi_hz":{}}}"#,
            escape(&b.name),
            escape(&b.kind),
            b.lo_hz,
            b.hi_hz
        )
    };
    let at: Vec<String> = rm.at(sdr.tuned_hz).into_iter().map(band).collect();
    let half = sdr.span() / 2.0;
    let view = (sdr.view_centre() - half, sdr.view_centre() + half);
    let in_view: Vec<String> = rm
        .plan()
        .map(|p| p.within(view.0, view.1).map(band).collect())
        .unwrap_or_default();
    let plans: Vec<String> = rm.plans.iter().map(|(s, _)| format!("\"{s}\"")).collect();
    let on = |b: bool| if b { "true" } else { "false" };
    format!(
        r#"{{"ok":true,"plan":"{}","plans":[{}],"strip":{},"mini":{},"window":{},"tuned_hz":{},"at_tuned":[{}],"in_view":[{}]}}"#,
        escape(rm.stem()),
        plans.join(","),
        on(rm.strip),
        on(rm.mini),
        on(rm.window),
        sdr.tuned_hz,
        at.join(","),
        in_view.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(name: &str, lo: f64, hi: f64) -> Band {
        Band {
            name: name.into(),
            kind: "amateur".into(),
            lo_hz: lo,
            hi_hz: hi,
        }
    }

    #[test]
    fn nested_bands_stack_into_lanes() {
        let bands = [
            band("inner", 140.0, 160.0),
            band("wide", 100.0, 200.0),
            band("beside", 300.0, 400.0),
            band("inner2", 170.0, 180.0),
        ];
        let l = lanes(bands.iter());
        let lane = |n: &str| l.iter().find(|(_, b)| b.name == n).unwrap().0;
        assert_eq!((lane("wide"), lane("beside")), (0, 0));
        assert_eq!((lane("inner"), lane("inner2")), (1, 1));
    }

    #[test]
    fn ranges_read_in_the_right_unit() {
        assert_eq!(fmt_range(87.5e6, 108e6), "87.5–108 MHz");
        assert_eq!(fmt_range(148.5e3, 283.5e3), "148.5–283.5 kHz");
        assert_eq!(fmt_range(1.24e9, 1.3e9), "1.24–1.3 GHz");
    }

    #[test]
    fn every_shipped_kind_gets_a_colour_or_grey() {
        let rm = RefMap::shipped_only();
        assert!(rm.plans.len() >= 21, "{}", rm.plans.len());
        assert_eq!(rm.stem(), "general");
        assert_eq!(colour("broadcast"), egui::Color32::from_rgb(80, 120, 235));
        assert_eq!(colour("LTE.FDD.uplink"), colour("cellular"));
        assert_eq!(colour("who-knows"), egui::Color32::from_rgb(125, 130, 140));
    }
}
