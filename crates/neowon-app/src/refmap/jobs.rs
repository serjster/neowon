//! Background work for the reference screen: fetching a source, importing
//! a file, and the one-shot IP lookup. One job at a time; each runs on its
//! own thread and reports back through a channel the `tick` system drains.
//! Network only ever happens here, and only after an explicit operator
//! action (D18/D19) — never at startup, never in tests.

use std::time::{SystemTime, UNIX_EPOCH};

use neowon_refdb::fetch::{BaseUrls, FetchTarget, fetch, locate_ip};
use neowon_refdb::sources::{eibi, fcc, fmlist, ourairports, wikidata};
use neowon_refdb::{Error, Location, Meta, Report, Source, Station, Store};

use super::RefMap;

/// A running background job: what it is, and its one-shot result.
pub struct Job {
    pub what: String,
    pub rx: crossbeam_channel::Receiver<Result<JobDone, Error>>,
}

pub enum JobDone {
    Fetched(Source, Report),
    Imported(Source, Report),
    Located(Location),
}

/// `refdb fetch <source> [radius_km]`: the worker downloads, parses,
/// writes the snapshot and reports. Requires a location for the radius
/// sources; the radius defaults to the operator's `stations radius`, then
/// to 150 km.
pub fn start_fetch(rm: &mut RefMap, src: Source, radius: Option<f64>) -> Result<(), String> {
    if rm.job.is_some() {
        return Err("a reference job is already running".into());
    }
    let store = rm
        .store
        .clone()
        .ok_or_else(|| format!("no refdb directory: {}", store_hint()))?;
    let center = rm.location.as_ref().map(Location::at);
    if center.is_none() && needs_location(src) {
        return Err(format!(
            "{} needs a location — set one with `location <lat> <lon>`",
            src.label()
        ));
    }
    let target = FetchTarget {
        center,
        radius_km: radius.or(rm.radius_km).unwrap_or(150.0),
        date: today_utc(),
    };
    let base = BaseUrls::default();
    let origin = origin_of(src, &base, target.date);
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let result = fetch(src, &target, &base).and_then(|(stations, report)| {
            save(&store, src, &stations, &origin)?;
            Ok(JobDone::Fetched(src, report))
        });
        let _ = tx.send(result);
    });
    rm.status = format!("fetching {}…", src.label());
    rm.job = Some(Job {
        what: format!("fetch {}", src.label()),
        rx,
    });
    Ok(())
}

/// `refdb import <source> <path>`: no network, but a 10 MB CSV parses off
/// the frame thread all the same. OurAirports takes a directory holding
/// `airport-frequencies.csv` and `airports.csv`.
pub fn start_import(rm: &mut RefMap, src: Source, path: &str) -> Result<(), String> {
    if rm.job.is_some() {
        return Err("a reference job is already running".into());
    }
    let store = rm
        .store
        .clone()
        .ok_or_else(|| format!("no refdb directory: {}", store_hint()))?;
    let path = path.to_string();
    let label = path.clone();
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let result = import_file(src, &path).and_then(|(stations, report)| {
            save(&store, src, &stations, &path)?;
            Ok(JobDone::Imported(src, report))
        });
        let _ = tx.send(result);
    });
    rm.status = format!("importing {} from {label}…", src.label());
    rm.job = Some(Job {
        what: format!("import {}", src.label()),
        rx,
    });
    Ok(())
}

/// `location ip` — the operator's explicit consent is the call itself
/// (D18); the UI shows a dialog first, the script line *is* the consent.
pub fn start_locate(rm: &mut RefMap) -> Result<(), String> {
    if rm.job.is_some() {
        return Err("a reference job is already running".into());
    }
    let base = BaseUrls::default().ipapi;
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send(locate_ip(&base).map(JobDone::Located));
    });
    rm.status = "looking up the location…".into();
    rm.job = Some(Job {
        what: "location ip".into(),
        rx,
    });
    Ok(())
}

fn needs_location(src: Source) -> bool {
    matches!(src, Source::Wikidata | Source::Fcc)
}

fn import_file(src: Source, path: &str) -> Result<(Vec<Station>, Report), Error> {
    let dir = std::path::Path::new(path);
    let read = |name: &str| std::fs::read(dir.join(name));
    match src {
        Source::Wikidata => Ok(wikidata::parse(&std::fs::read(path)?)),
        Source::Eibi => Ok(eibi::parse(&std::fs::read(path)?)),
        Source::OurAirports => {
            let frequencies = read("airport-frequencies.csv")
                .map_err(|e| Error::Invalid(format!("{path}/airport-frequencies.csv: {e}")))?;
            let airports = read("airports.csv")
                .map_err(|e| Error::Invalid(format!("{path}/airports.csv: {e}")))?;
            Ok(ourairports::parse(&frequencies, &airports))
        }
        Source::Fcc => Ok(fcc::parse(&std::fs::read(path)?)),
        Source::Fmlist => Ok(fmlist::parse(&std::fs::read(path)?)),
    }
}

fn save(store: &Store, src: Source, stations: &[Station], origin: &str) -> Result<(), Error> {
    let meta = Meta::new(
        src,
        stations.len(),
        neowon_catalog::now_rfc3339(),
        origin.to_string(),
    );
    store.replace(src, stations, &meta)
}

/// Where the bytes came from, for the Sources tab.
pub fn origin_of(src: Source, base: &BaseUrls, date: (i32, u8, u8)) -> String {
    match src {
        Source::Wikidata => base.wikidata.clone(),
        Source::Eibi => format!("{}{}", base.eibi, eibi::season_file(date)),
        Source::OurAirports => format!(
            "{}, {}",
            base.ourairports_frequencies, base.ourairports_airports
        ),
        Source::Fcc => format!("{}, {}", base.fcc_fm, base.fcc_am),
        Source::Fmlist => "operator export".into(),
    }
}

fn store_hint() -> String {
    match neowon_refdb::store::default_dir() {
        Some(d) => format!("set NEOWON_REFDB (looked at {})", d.display()),
        None => "set HOME or NEOWON_REFDB".into(),
    }
}

/// Today in UTC, for the EiBi season file.
pub fn today_utc() -> (i32, u8, u8) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_from_days((secs / 86_400) as i64)
}

/// Howard Hinnant's civil-from-days.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    ((if m <= 2 { y + 1 } else { y }) as i32, m as u8, d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_known_instants() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn the_fmlist_import_is_always_available_offline() {
        // No store needed to prove the dispatch: a missing directory is an
        // error that names the path, not a panic.
        let err = import_file(Source::OurAirports, "/nonexistent-neowon").unwrap_err();
        assert!(err.to_string().contains("airport-frequencies.csv"), "{err}");
    }
}
