//! RF reference script verbs:
//!
//! ```text
//! bandplan <name>                   # the active band plan (file stem)
//! bandmap strip <on|off>            # band strip under the spectrum
//! bandmap mini <on|off>             # whole-range minimap over the canvas
//! bandmap window <on|off>           # the RF map window
//! bandmap goto <band name…>         # tune to a band's centre, span to fit
//! location <lat> <lon> | <locator> | ip | clear
//! refdb fetch <source> [radius_km]  # Wikidata/EiBi/OurAirports/FCC
//! refdb import <source> <path>      # offline; OurAirports takes a dir
//! refdb clear <source>
//! stations window <on|off>          # the stations window
//! stations overlay <on|off>         # station ticks/labels on the spectrum
//! stations radius <km|auto>         # "near me" radius; auto = per service
//! stations find <text…|->           # search box; `-` clears
//! stations filter <source|service|mod> <value|->
//! stations onair <on|off>           # only scheduled stations on air now
//! stations scope <view|near|all>    # the window's range filter
//! stations sort <freq|distance>
//! stations tune <source:id>         # tune + the fitting demodulator
//! stations catalog <source:id>      # copy one row into the catalog
//! ```

use bevy::log::error;
use neowon_catalog::{Entity, Op, ProvKind, Provenance, Signal};
use neowon_refdb::{Location, LocationSource, Modulation, Service, SortBy, Source, geo};

use super::jobs::{start_fetch, start_import, start_locate};
use super::{RefMap, Scope};
use crate::Link;
use crate::catalog::CatalogState;
use crate::sdr::SdrAction;
use crate::sdr::SdrState;

#[derive(Debug, Clone, PartialEq)]
pub enum LocationSet {
    Coords(f64, f64),
    Locator(String),
    Ip,
    Clear,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RefMapAction {
    Plan(String),
    Strip(bool),
    Mini(bool),
    Window(bool),
    Goto(String),
    Location(LocationSet),
    Fetch(Source, Option<f64>),
    Import(Source, String),
    Clear(Source),
    StationsWindow(bool),
    Overlay(bool),
    Radius(Option<f64>),
    Find(String),
    FilterSource(Option<Source>),
    FilterService(Option<Service>),
    FilterModulation(Option<Modulation>),
    OnAir(bool),
    Scope(Scope),
    Sort(SortBy),
    TuneStation(String),
    CatalogStation(String),
}

impl std::fmt::Display for RefMapAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let on = |b: bool| if b { "on" } else { "off" };
        match self {
            RefMapAction::Plan(p) => write!(f, "bandplan {p}"),
            RefMapAction::Strip(b) => write!(f, "bandmap strip {}", on(*b)),
            RefMapAction::Mini(b) => write!(f, "bandmap mini {}", on(*b)),
            RefMapAction::Window(b) => write!(f, "bandmap window {}", on(*b)),
            RefMapAction::Goto(n) => write!(f, "bandmap goto {n}"),
            RefMapAction::Location(LocationSet::Coords(lat, lon)) => {
                write!(f, "location {lat} {lon}")
            }
            RefMapAction::Location(LocationSet::Locator(s)) => write!(f, "location {s}"),
            RefMapAction::Location(LocationSet::Ip) => write!(f, "location ip"),
            RefMapAction::Location(LocationSet::Clear) => write!(f, "location clear"),
            RefMapAction::Fetch(s, None) => write!(f, "refdb fetch {}", s.stem()),
            RefMapAction::Fetch(s, Some(r)) => write!(f, "refdb fetch {} {r}", s.stem()),
            RefMapAction::Import(s, p) => write!(f, "refdb import {} {p}", s.stem()),
            RefMapAction::Clear(s) => write!(f, "refdb clear {}", s.stem()),
            RefMapAction::StationsWindow(b) => write!(f, "stations window {}", on(*b)),
            RefMapAction::Overlay(b) => write!(f, "stations overlay {}", on(*b)),
            RefMapAction::Radius(None) => write!(f, "stations radius auto"),
            RefMapAction::Radius(Some(r)) => write!(f, "stations radius {r}"),
            RefMapAction::Find(t) => {
                write!(f, "stations find {}", if t.is_empty() { "-" } else { t })
            }
            RefMapAction::FilterSource(None) => write!(f, "stations filter source -"),
            RefMapAction::FilterSource(Some(s)) => write!(f, "stations filter source {}", s.stem()),
            RefMapAction::FilterService(None) => write!(f, "stations filter service -"),
            RefMapAction::FilterService(Some(s)) => {
                write!(f, "stations filter service {}", s.label())
            }
            RefMapAction::FilterModulation(None) => write!(f, "stations filter mod -"),
            RefMapAction::FilterModulation(Some(m)) => {
                write!(f, "stations filter mod {}", m.label())
            }
            RefMapAction::OnAir(b) => write!(f, "stations onair {}", on(*b)),
            RefMapAction::Scope(s) => write!(
                f,
                "stations scope {}",
                match s {
                    Scope::View => "view",
                    Scope::Near => "near",
                    Scope::All => "all",
                }
            ),
            RefMapAction::Sort(s) => write!(
                f,
                "stations sort {}",
                match s {
                    SortBy::Frequency => "freq",
                    SortBy::Distance => "distance",
                }
            ),
            RefMapAction::TuneStation(k) => write!(f, "stations tune {k}"),
            RefMapAction::CatalogStation(k) => write!(f, "stations catalog {k}"),
        }
    }
}

pub fn parse<'a>(
    verb: &str,
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<RefMapAction, String> {
    let on_off = |s: &str| match s {
        "on" | "1" => Ok(true),
        "off" | "0" => Ok(false),
        _ => Err(format!("expected on|off, got {s:?}")),
    };
    let source = |s: &str| {
        Source::from_stem(&s.to_ascii_lowercase())
            .ok_or_else(|| format!("unknown source {s:?} (wikidata|eibi|ourairports|fcc|fmlist)"))
    };
    let rest = |next: &mut dyn FnMut() -> Result<&'a str, String>| -> Result<String, String> {
        let mut words = vec![next()?];
        while let Ok(w) = next() {
            words.push(w);
        }
        Ok(words.join(" "))
    };
    match verb {
        "bandplan" => Ok(RefMapAction::Plan(next()?.to_string())),
        "bandmap" => Ok(match next()? {
            "strip" => RefMapAction::Strip(on_off(next()?)?),
            "mini" => RefMapAction::Mini(on_off(next()?)?),
            "window" => RefMapAction::Window(on_off(next()?)?),
            "goto" => RefMapAction::Goto(rest(next)?),
            v => return Err(format!("unknown bandmap verb {v:?}")),
        }),
        "location" => {
            let first = next()?;
            if first.eq_ignore_ascii_case("ip") {
                return Ok(RefMapAction::Location(LocationSet::Ip));
            }
            if first.eq_ignore_ascii_case("clear") {
                return Ok(RefMapAction::Location(LocationSet::Clear));
            }
            // Two numbers are a fix; anything else is a Maidenhead locator.
            match (first.parse::<f64>(), next()) {
                (Ok(lat), Ok(lon)) if lon.parse::<f64>().is_ok() => Ok(RefMapAction::Location(
                    LocationSet::Coords(lat, lon.parse().unwrap()),
                )),
                _ => Ok(RefMapAction::Location(LocationSet::Locator(
                    first.to_string(),
                ))),
            }
        }
        "refdb" => Ok(match next()? {
            "fetch" => {
                let src = source(next()?)?;
                let radius = match next() {
                    Ok(r) => Some(r.parse::<f64>().map_err(|_| format!("bad radius {r:?}"))?),
                    Err(_) => None,
                };
                RefMapAction::Fetch(src, radius)
            }
            "import" => RefMapAction::Import(source(next()?)?, rest(next)?),
            "clear" => RefMapAction::Clear(source(next()?)?),
            v => return Err(format!("unknown refdb verb {v:?} (fetch|import|clear)")),
        }),
        "stations" => Ok(match next()? {
            "window" => RefMapAction::StationsWindow(on_off(next()?)?),
            "overlay" => RefMapAction::Overlay(on_off(next()?)?),
            "radius" => match next()? {
                "auto" => RefMapAction::Radius(None),
                km => RefMapAction::Radius(Some(
                    km.parse().map_err(|_| format!("bad radius {km:?}"))?,
                )),
            },
            "find" => {
                let text = rest(next)?;
                RefMapAction::Find(if text == "-" { String::new() } else { text })
            }
            "filter" => {
                let field = next()?;
                let value = next()?;
                let clear = value == "-";
                match field {
                    "source" => {
                        RefMapAction::FilterSource(if clear { None } else { Some(source(value)?) })
                    }
                    "service" => RefMapAction::FilterService(if clear {
                        None
                    } else {
                        Some(super::readout::parse_service(value)?)
                    }),
                    "mod" | "modulation" => RefMapAction::FilterModulation(if clear {
                        None
                    } else {
                        Some(super::readout::parse_modulation(value)?)
                    }),
                    v => return Err(format!("unknown station filter {v:?}")),
                }
            }
            "onair" => RefMapAction::OnAir(on_off(next()?)?),
            "scope" => RefMapAction::Scope(
                Scope::parse(next()?).ok_or_else(|| "expected view|near|all".to_string())?,
            ),
            "sort" => RefMapAction::Sort(super::readout::parse_sort(next()?)?),
            "tune" => RefMapAction::TuneStation(next()?.to_string()),
            "catalog" => RefMapAction::CatalogStation(next()?.to_string()),
            v => return Err(format!("unknown stations verb {v:?}")),
        }),
        v => Err(format!("unknown reference verb {v:?}")),
    }
}

pub fn run(
    a: RefMapAction,
    rm: &mut RefMap,
    sdr: &mut SdrState,
    cat: &mut CatalogState,
    link: &mut Link,
) {
    if let Err(e) = apply(a, rm, sdr, cat, link) {
        error!("script: {e}");
        link.status = format!("error: {e}");
    }
}

fn apply(
    a: RefMapAction,
    rm: &mut RefMap,
    sdr: &mut SdrState,
    cat: &mut CatalogState,
    link: &mut Link,
) -> Result<(), String> {
    match a {
        RefMapAction::Plan(name) => {
            rm.active = rm
                .plans
                .iter()
                .position(|(s, _)| s.eq_ignore_ascii_case(&name))
                .ok_or_else(|| {
                    let names: Vec<&str> = rm.plans.iter().map(|(s, _)| s.as_str()).collect();
                    format!("no band plan {name:?} (have {})", names.join(", "))
                })?;
        }
        RefMapAction::Strip(on) => rm.strip = on,
        RefMapAction::Mini(on) => rm.mini = on,
        RefMapAction::Window(on) => rm.window = on,
        RefMapAction::Goto(name) => {
            let plan = rm.plan().ok_or("no band plan loaded")?;
            // Several bands can share a name ("Shortwave Broadcast"): the
            // one nearest the tuned frequency.
            let tuned = sdr.tuned_hz;
            let band = plan
                .bands
                .iter()
                .filter(|b| b.name.eq_ignore_ascii_case(&name))
                .min_by(|a, b| {
                    let d = |x: &neowon_refdb::Band| ((x.lo_hz + x.hi_hz) / 2.0 - tuned).abs();
                    d(a).total_cmp(&d(b))
                })
                .ok_or_else(|| format!("no band {name:?} in {}", rm.stem()))?;
            goto(sdr, link.sdr_caps(), band.lo_hz, band.hi_hz)?;
        }
        RefMapAction::Location(set) => match set {
            LocationSet::Coords(lat, lon) => {
                if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                    return Err(format!("coordinates {lat}, {lon} are not on Earth"));
                }
                rm.set_location(Location {
                    lat,
                    lon,
                    country_code: None,
                    source: LocationSource::Manual,
                    set_at: neowon_catalog::now_rfc3339(),
                });
            }
            LocationSet::Locator(s) => {
                let p = geo::from_locator(&s).map_err(|e| e.to_string())?;
                rm.set_location(Location {
                    lat: p.lat,
                    lon: p.lon,
                    country_code: None,
                    source: LocationSource::Locator,
                    set_at: neowon_catalog::now_rfc3339(),
                });
            }
            LocationSet::Ip => start_locate(rm)?,
            LocationSet::Clear => {
                if let Some(path) = geo::location_path()
                    && let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(format!("location {}: {e}", path.display()));
                }
                rm.location = None;
                rm.status = "location cleared".into();
                link.status = rm.status.clone();
            }
        },
        RefMapAction::Fetch(src, radius) => {
            start_fetch(rm, src, radius)?;
            link.status = rm.status.clone();
        }
        RefMapAction::Import(src, path) => {
            start_import(rm, src, &path)?;
            link.status = rm.status.clone();
        }
        RefMapAction::Clear(src) => {
            let store = rm.store.clone().ok_or("no refdb directory")?;
            store.clear(src).map_err(|e| e.to_string())?;
            rm.reload();
            rm.status = format!("{}: cleared", src.label());
            link.status = rm.status.clone();
        }
        RefMapAction::StationsWindow(on) => rm.stations_window = on,
        RefMapAction::Overlay(on) => rm.overlay = on,
        RefMapAction::Radius(r) => rm.radius_km = r,
        RefMapAction::Find(t) => rm.find = t,
        RefMapAction::FilterSource(s) => rm.filter_source = s,
        RefMapAction::FilterService(s) => rm.filter_service = s,
        RefMapAction::FilterModulation(m) => rm.filter_modulation = m,
        RefMapAction::OnAir(on) => rm.on_air = on,
        RefMapAction::Scope(s) => rm.scope = s,
        RefMapAction::Sort(s) => rm.sort = s,
        RefMapAction::TuneStation(key) => {
            let s = rm
                .station(&key)
                .ok_or_else(|| format!("no station {key:?}"))?
                .clone();
            rm.selected = Some(key.clone());
            crate::sdr::run(SdrAction::Tune(s.freq_hz.round()), sdr, link);
            let mut lines = format!("sdr tune {}", s.freq_hz.round());
            if let Some(verb) = s.modulation.demod() {
                let mode = neowon_dsp::DemodMode::parse(verb).unwrap();
                crate::sdr::run(SdrAction::Demod(Some(mode)), sdr, link);
                lines.push_str(&format!(" · sdr demod {verb}"));
            }
            link.status = lines;
        }
        RefMapAction::CatalogStation(key) => {
            let s = rm
                .station(&key)
                .ok_or_else(|| format!("no station {key:?}"))?
                .clone();
            catalog(rm, cat, &s)?;
            rm.selected = Some(key);
            link.status = format!("catalog: {} added", s.name);
        }
    }
    Ok(())
}

/// A copy, not a link. `ProvKind::Db` is the catalog's "came from a
/// database" provenance; `input_ref` names the refdb row.
fn catalog(
    rm: &mut RefMap,
    cat: &mut CatalogState,
    s: &neowon_refdb::Station,
) -> Result<(), String> {
    let c = cat
        .cat
        .as_mut()
        .ok_or_else(|| format!("no catalog open at {}", cat.path.display()))?;
    let mut p = Provenance::user(neowon_catalog::now_rfc3339());
    p.kind = ProvKind::Db;
    p.tool = "neowon-refdb".into();
    p.input_ref = Some(format!("refdb:{}:{}", s.source.stem(), s.id));
    let id = c.next_id();
    // The provenance line names the refdb row; `Signal.source` is a
    // catalog-internal entity id, so the source lives here and in
    // `input_ref` instead.
    let mut notes = format!("[{} {}]", s.source.label(), s.id);
    if !s.notes.is_empty() {
        notes.push(' ');
        notes.push_str(&s.notes);
    }
    if let Some(country) = &s.country {
        notes.push_str(&format!(" · {country}"));
    }
    c.commit(Op::Insert {
        entity: Entity::Signal(Signal {
            id,
            name: s.name.clone(),
            centre_hz: s.freq_hz,
            bandwidth_hz: s.bandwidth_hz.unwrap_or(0.0),
            modulation: Some(s.modulation.label().to_string()),
            source: None,
            tags: Default::default(),
            aliases: Vec::new(),
            notes,
            pinned: false,
            confidence: None,
            provenance: p,
        }),
    })
    .map(|_| ())
    .map_err(|e| e.to_string())?;
    rm.status = format!("catalog: {} ({})", s.name, cat.path.display());
    Ok(())
}

/// Tune to the middle of `lo..hi` and fit the span to it when it fits the
/// IQ band (a wider band shows the full span around its centre).
pub fn goto(
    sdr: &mut SdrState,
    caps: Option<&neowon_backend::SdrCaps>,
    lo: f64,
    hi: f64,
) -> Result<(), String> {
    let centre = ((lo + hi) / 2.0 / 1e3).round() * 1e3;
    if let Some(c) = caps
        && !(c.freq_range_hz.0..=c.freq_range_hz.1).contains(&centre)
    {
        return Err(format!(
            "{} is outside the tuner's {}",
            super::fmt_range(lo, hi),
            super::fmt_range(c.freq_range_hz.0, c.freq_range_hz.1)
        ));
    }
    let before = sdr.config.centre_hz;
    sdr.set_tuned(centre);
    let rate = sdr.config.sample_rate;
    let want = (hi - lo) * 1.1;
    if want < rate {
        sdr.span_hz = want;
        sdr.pan_hz = centre - sdr.config.centre_hz;
    } else {
        sdr.span_hz = 0.0;
        sdr.pan_hz = 0.0;
    }
    sdr.clamp_pan();
    sdr.dirty |= sdr.config.centre_hz != before;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Result<RefMapAction, String> {
        let mut w = line.split_whitespace();
        let verb = w.next().unwrap();
        parse(verb, &mut || w.next().ok_or_else(|| "eol".to_string()))
    }

    /// One number per variant. An exhaustive match, so a new variant does
    /// not compile until it is placed here, and the count below then fails
    /// until it has a round-trip sample.
    fn variant(a: &RefMapAction) -> usize {
        match a {
            RefMapAction::Plan(_) => 0,
            RefMapAction::Strip(_) => 1,
            RefMapAction::Mini(_) => 2,
            RefMapAction::Window(_) => 3,
            RefMapAction::Goto(_) => 4,
            RefMapAction::Location(_) => 5,
            RefMapAction::Fetch(..) => 6,
            RefMapAction::Import(..) => 7,
            RefMapAction::Clear(_) => 8,
            RefMapAction::StationsWindow(_) => 9,
            RefMapAction::Overlay(_) => 10,
            RefMapAction::Radius(_) => 11,
            RefMapAction::Find(_) => 12,
            RefMapAction::FilterSource(_) => 13,
            RefMapAction::FilterService(_) => 14,
            RefMapAction::FilterModulation(_) => 15,
            RefMapAction::OnAir(_) => 16,
            RefMapAction::Scope(_) => 17,
            RefMapAction::Sort(_) => 18,
            RefMapAction::TuneStation(_) => 19,
            RefMapAction::CatalogStation(_) => 20,
        }
    }

    #[test]
    fn every_action_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for a in [
            RefMapAction::Plan("usa".into()),
            RefMapAction::Strip(false),
            RefMapAction::Mini(true),
            RefMapAction::Window(true),
            RefMapAction::Goto("2m Ham Band".into()),
            RefMapAction::Location(LocationSet::Coords(38.72, -9.14)),
            RefMapAction::Location(LocationSet::Locator("IN58".into())),
            RefMapAction::Location(LocationSet::Ip),
            RefMapAction::Location(LocationSet::Clear),
            RefMapAction::Fetch(Source::Wikidata, None),
            RefMapAction::Fetch(Source::Fcc, Some(100.0)),
            RefMapAction::Import(Source::Eibi, "/tmp/sked.csv".into()),
            RefMapAction::Clear(Source::Fmlist),
            RefMapAction::StationsWindow(true),
            RefMapAction::Overlay(false),
            RefMapAction::Radius(None),
            RefMapAction::Radius(Some(75.0)),
            RefMapAction::Find("antena".into()),
            RefMapAction::Find(String::new()),
            RefMapAction::FilterSource(Some(Source::Eibi)),
            RefMapAction::FilterSource(None),
            RefMapAction::FilterService(Some(Service::Aviation)),
            RefMapAction::FilterService(None),
            RefMapAction::FilterModulation(Some(Modulation::Wfm)),
            RefMapAction::FilterModulation(None),
            RefMapAction::OnAir(true),
            RefMapAction::Scope(Scope::Near),
            RefMapAction::Sort(SortBy::Distance),
            RefMapAction::TuneStation("wikidata:Q1".into()),
            RefMapAction::CatalogStation("eibi:abc".into()),
        ] {
            seen.insert(variant(&a));
            assert_eq!(p(&a.to_string()).unwrap(), a, "{a}");
        }
        assert_eq!(seen.len(), 21, "a variant has no round-trip sample");
        assert!(p("bandmap sideways on").is_err());
        assert!(p("refdb fetch nosuch").is_err());
        assert!(p("stations filter colour red").is_err());
        assert!(p("location 91 0").is_ok()); // range checked at apply time
    }

    #[test]
    fn goto_fits_a_narrow_band_and_recentres_the_hardware() {
        let mut sdr = SdrState::default();
        sdr.config.centre_hz = 100e6;
        sdr.config.sample_rate = 2.048e6;
        goto(&mut sdr, None, 144e6, 146e6).unwrap();
        assert_eq!(sdr.tuned_hz, 145e6);
        assert_eq!(sdr.config.centre_hz, 145e6);
        assert_eq!(sdr.span_hz, 0.0); // 2.2 MHz does not fit 2.048 MS/s
        goto(&mut sdr, None, 144.8e6, 145.2e6).unwrap();
        assert!((sdr.span_hz - 440e3).abs() < 1e-6);
        assert_eq!(sdr.view_centre(), 145e6);
    }
}
