//! Control-socket readouts for the reference screen: `get location`,
//! `get refdb`, `get stations [view|near|all] [n]`.

use neowon_refdb::{LocationSource, Modulation, Service, SortBy};

use super::{RefMap, Scope, to_locator};
use crate::control::escape;
use crate::sdr::SdrState;

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn opt(v: Option<f64>) -> String {
    v.map_or("null".into(), num)
}

/// `get location`: the fix, the locator, and where it came from.
pub fn location_json(rm: &RefMap) -> String {
    let Some(l) = &rm.location else {
        return r#"{"ok":true,"set":false}"#.into();
    };
    let source = match l.source {
        LocationSource::Manual => "manual",
        LocationSource::Locator => "locator",
        LocationSource::Ip => "ip",
    };
    format!(
        concat!(
            r#"{{"ok":true,"set":true,"lat":{},"lon":{},"country":"{}","source":"{}","#,
            r#""locator":"{}","set_at":"{}"}}"#
        ),
        num(l.lat),
        num(l.lon),
        escape(l.country_code.as_deref().unwrap_or("")),
        source,
        to_locator(l.at(), 6),
        escape(&l.set_at),
    )
}

/// `get stations [view|near|all] [n]`: the filtered rows, sorted as the
/// window sorts them.
pub fn stations_json(rm: &RefMap, sdr: &SdrState, scope: Scope, limit: usize) -> String {
    let rows = rm.query(scope, sdr);
    let total = rows.len();
    let items: Vec<String> = rows
        .iter()
        .take(limit)
        .map(|(s, km)| {
            format!(
                concat!(
                    r#"{{"key":"{}","id":"{}","source":"{}","name":"{}","freq_hz":{},"#,
                    r#""mod":"{}","service":"{}","km":{},"country":"{}","lat":{},"lon":{},"#,
                    r#""power_kw":{},"on_air":{},"callsign":"{}"}}"#
                ),
                escape(&RefMap::key(s)),
                escape(&s.id),
                s.source.stem(),
                escape(&s.name),
                num(s.freq_hz),
                s.modulation.label(),
                s.service.label(),
                opt(*km),
                escape(s.country.as_deref().unwrap_or("")),
                opt(s.lat),
                opt(s.lon),
                opt(s.power_kw),
                rm.on_air_now(s),
                escape(s.callsign.as_deref().unwrap_or("")),
            )
        })
        .collect();
    let scope = match scope {
        Scope::View => "view",
        Scope::Near => "near",
        Scope::All => "all",
    };
    format!(
        r#"{{"ok":true,"scope":"{scope}","total":{total},"stations":[{}]}}"#,
        items.join(",")
    )
}

/// `get stations [view|near|all] [n]` as the control socket spells it;
/// a bad scope comes back as the standard JSON error object.
pub fn stations_query(rm: &RefMap, sdr: &SdrState, args: &str) -> String {
    let mut scope = rm.scope;
    let mut limit = 50usize;
    let mut it = args.split_whitespace();
    if let Some(w) = it.next() {
        match Scope::parse(w) {
            Some(s) => scope = s,
            None => {
                return format!(
                    r#"{{"ok":false,"error":"unknown scope {} (view|near|all)"}}"#,
                    escape(w)
                );
            }
        }
    }
    if let Some(n) = it.next().and_then(|n| n.parse().ok()) {
        limit = n;
    }
    stations_json(rm, sdr, scope, limit)
}

/// `get refdb`: the snapshots and their provenance, plus the running job
/// and the last status line.
pub fn refdb_json(rm: &RefMap) -> String {
    let metas: Vec<String> = rm
        .metas
        .iter()
        .map(|m| {
            format!(
                concat!(
                    r#"{{"source":"{}","count":{},"fetched_at":"{}","origin":"{}","#,
                    r#""licence":"{}"}}"#
                ),
                m.source.stem(),
                m.count,
                escape(&m.fetched_at),
                escape(&m.origin),
                escape(&m.licence),
            )
        })
        .collect();
    let job = rm
        .job
        .as_ref()
        .map_or("null".to_string(), |j| format!("\"{}\"", escape(&j.what)));
    let dir = rm
        .store
        .as_ref()
        .map_or(String::new(), |s| s.dir().display().to_string());
    format!(
        concat!(
            r#"{{"ok":true,"dir":"{}","sources":[{}],"stations":{},"job":{},"#,
            r#""filters":{{"find":"{}","source":"{}","service":"{}","mod":"{}","on_air":{},"scope":"{}"}},"#,
            r#""status":"{}"}}"#
        ),
        escape(&dir),
        metas.join(","),
        rm.index.len(),
        job,
        escape(&rm.find),
        rm.filter_source.map_or("", |s| s.stem()),
        rm.filter_service.map_or("", |s| s.label()),
        rm.filter_modulation.map_or("", |m| m.label()),
        rm.on_air,
        match rm.scope {
            Scope::View => "view",
            Scope::Near => "near",
            Scope::All => "all",
        },
        escape(&rm.status),
    )
}

/// The station filters and sort a script or MCP caller can set, shared by
/// the verbs.
pub fn parse_modulation(word: &str) -> Result<Modulation, String> {
    Modulation::parse(word).ok_or_else(|| format!("unknown modulation {word:?}"))
}

pub fn parse_service(word: &str) -> Result<Service, String> {
    Service::parse(word).ok_or_else(|| format!("unknown service {word:?}"))
}

pub fn parse_sort(word: &str) -> Result<SortBy, String> {
    match word {
        "freq" | "frequency" => Ok(SortBy::Frequency),
        "distance" | "near" => Ok(SortBy::Distance),
        _ => Err(format!("unknown sort {word:?} (freq|distance)")),
    }
}
