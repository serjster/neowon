//! Background work for the reference screen: fetching a source, importing
//! a file, and the one-shot IP lookup. One job at a time; each runs on its
//! own thread and reports back through a channel the `tick` system drains.
//! Network only ever happens here, and only after an explicit operator
//! action — never at startup, never in tests.

use std::time::{SystemTime, UNIX_EPOCH};

use neowon_refdb::fetch::{BaseUrls, FetchTarget, fetch, locate_ip};
use neowon_refdb::sources::{eibi, fcc, fmlist, ourairports, wikidata};
use neowon_refdb::{Error, Location, Meta, Report, Source, Station, Store};

use super::RefMap;

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
            save(&store, src, &stations, &report, &origin)?;
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
        let result = import_into(&store, src, &path).map(|report| JobDone::Imported(src, report));
        let _ = tx.send(result);
    });
    rm.status = format!("importing {} from {label}…", src.label());
    rm.job = Some(Job {
        what: format!("import {}", src.label()),
        rx,
    });
    Ok(())
}

/// `location ip` — the operator's explicit consent is the call itself: the
/// UI shows a dialog first, the script line *is* the consent.
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

fn import_into(store: &Store, src: Source, path: &str) -> Result<Report, Error> {
    let (stations, report) = import_file(src, path)?;
    save(store, src, &stations, &report, path)?;
    Ok(report)
}

fn save(
    store: &Store,
    src: Source,
    stations: &[Station],
    report: &Report,
    origin: &str,
) -> Result<(), Error> {
    if report.unusable() {
        return Err(Error::Invalid(format!(
            "{} refused, snapshot kept: {origin} is not a {} document ({})",
            src.label(),
            src.label(),
            report.summary()
        )));
    }
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

    /// A document that is not the source's format must not replace the
    /// snapshot it was meant to update (the refdb side of the import
    /// invariant): every row rejected and none kept is a refusal, not an
    /// empty source.
    #[test]
    fn a_document_of_the_wrong_kind_is_refused_and_the_snapshot_kept() {
        let dir =
            std::env::temp_dir().join(format!("neowon-app-refdb-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../neowon-refdb/tests/fixtures");
        let good = fixtures.join("wikidata.json");
        let report = import_into(&store, Source::Wikidata, good.to_str().unwrap()).unwrap();
        assert!(report.kept > 0);
        let before = store.load_all().unwrap().0.len();
        let mut failures = Vec::new();
        // An EiBi CSV and an FCC listing offered as Wikidata; an FCC
        // listing offered as EiBi (whose snapshot is empty: nothing to lose,
        // but the metadata must not claim a fetch that produced nothing).
        for (src, file) in [
            (Source::Wikidata, "eibi.csv"),
            (Source::Wikidata, "fcc.txt"),
            (Source::Eibi, "wikidata.json"),
        ] {
            let path = fixtures.join(file);
            match import_into(&store, src, path.to_str().unwrap()) {
                Ok(r) => failures.push(format!(
                    "{file} as {}: accepted ({}), store now {} stations",
                    src.label(),
                    r.summary(),
                    store.load().index.len()
                )),
                Err(e) if !e.to_string().contains(src.label()) => failures.push(format!(
                    "{file} as {}: refused, source not named: {e}",
                    src.label()
                )),
                Err(_) => {}
            }
        }
        let (index, metas) = store.load_all().unwrap();
        if index.len() != before {
            failures.push(format!(
                "the Wikidata snapshot went from {before} to {} stations",
                index.len()
            ));
        }
        if metas.iter().any(|m| m.source == Source::Eibi) {
            failures.push("EiBi metadata claims a fetch that produced nothing".into());
        }
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // The app's view of a damaged reference store: one bad source is
    // reported by name and cannot take the others down with it.

    fn store_scratch(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("neowon-app-refdb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn stations(src: Source, n: usize) -> Vec<Station> {
        (0..n)
            .map(|i| Station::new(src, format!("s{i}"), format!("S{i}"), 90e6 + i as f64 * 1e5))
            .collect()
    }

    #[test]
    fn a_corrupt_source_is_named_and_the_others_still_load() {
        let dir = store_scratch("isolate");
        let store = Store::open(&dir).unwrap();
        for (src, n) in [(Source::Wikidata, 3), (Source::Eibi, 2)] {
            let meta = Meta::new(src, n, "2026-09-23T00:00:00Z".into(), "fixture".into());
            store.replace(src, &stations(src, n), &meta).unwrap();
        }
        std::fs::write(dir.join("fcc.json"), b"[{\"source\":\"fcc\"").unwrap();

        let rm = RefMap::load_from(std::path::Path::new(""), Some(dir.clone()), None);
        let loaded: Vec<Source> = rm.metas.iter().map(|m| m.source).collect();
        assert_eq!(
            rm.index.len(),
            5,
            "the good sources did not load (status {:?})",
            rm.status
        );
        assert_eq!(loaded, [Source::Wikidata, Source::Eibi]);
        assert!(
            rm.status.contains("FCC"),
            "the bad source is not named: {:?}",
            rm.status
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_fmlist_import_is_always_available_offline() {
        // No store needed to prove the dispatch: a missing directory is an
        // error that names the path, not a panic.
        let err = import_file(Source::OurAirports, "/nonexistent-neowon").unwrap_err();
        assert!(err.to_string().contains("airport-frequencies.csv"), "{err}");
    }
}
