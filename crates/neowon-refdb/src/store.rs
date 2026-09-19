//! The on-disk reference store (D20): one snapshot per source in
//! `~/.neowon/refdb/` (override `NEOWON_REFDB`), replaced wholesale and
//! written atomically. The operator's catalog never reads this directory
//! and this module never touches the catalog.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::index::Index;
use crate::station::{Source, Station};

/// What a snapshot is, for the Sources tab. `origin` is the URL or the
/// imported file path; `fetched_at` is the app's clock, RFC 3339.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub source: Source,
    pub count: usize,
    pub fetched_at: String,
    pub origin: String,
    pub licence: String,
}

impl Meta {
    pub fn new(source: Source, count: usize, fetched_at: String, origin: String) -> Self {
        Self {
            source,
            count,
            fetched_at,
            origin,
            licence: source.licence().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn snapshot(&self, src: Source) -> PathBuf {
        self.dir.join(format!("{}.json", src.stem()))
    }

    fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }

    /// Put `stations` in place as the source's only snapshot. The stations
    /// and the metadata are written atomically; a crash between the two
    /// leaves the previous station file readable (a `.tmp` is never read).
    pub fn replace(&self, src: Source, stations: &[Station], meta: &Meta) -> Result<(), Error> {
        let mut metas = self.metas()?;
        metas.retain(|m| m.source != src);
        metas.push(meta.clone());
        metas.sort_by_key(|m| m.source);
        crate::write_atomic(&self.snapshot(src), &serde_json::to_vec(stations)?)?;
        crate::write_atomic(&self.meta_path(), &serde_json::to_vec(&metas)?)
    }

    pub fn load_all(&self) -> Result<(Index, Vec<Meta>), Error> {
        let mut stations = Vec::new();
        for src in Source::ALL {
            match std::fs::read(self.snapshot(src)) {
                Ok(bytes) => stations.extend(serde_json::from_slice::<Vec<Station>>(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok((Index::new(stations), self.metas()?))
    }

    pub fn metas(&self) -> Result<Vec<Meta>, Error> {
        match std::fs::read(self.meta_path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Drop a source's snapshot and metadata; already absent is success.
    pub fn clear(&self, src: Source) -> Result<(), Error> {
        match std::fs::remove_file(self.snapshot(src)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut metas = self.metas()?;
        metas.retain(|m| m.source != src);
        crate::write_atomic(&self.meta_path(), &serde_json::to_vec(&metas)?)
    }
}

/// `~/.neowon/refdb`, or `$NEOWON_REFDB`.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NEOWON_REFDB") {
        return Some(p.into());
    }
    crate::neowon_dir().map(|d| d.join("refdb"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::{Modulation, Schedule, Service};

    fn station(id: &str, freq: f64) -> Station {
        let mut s = Station::new(Source::Wikidata, id.into(), format!("S{id}"), freq);
        s.modulation = Modulation::Wfm;
        s.service = Service::Broadcast;
        s.bandwidth_hz = Some(180_000.0);
        s.lat = Some(38.72);
        s.lon = Some(-9.14);
        s.country = Some("PT".into());
        s.callsign = Some("CSX".into());
        s.power_kw = Some(12.5);
        s.notes = "field-exact ✓ ünïcode".into();
        s
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neowon-refdb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn replace_then_load_is_field_exact() {
        let dir = temp("store");
        let store = Store::open(&dir).unwrap();
        let wiki = vec![station("Q1", 100.3e6), station("Q2", 9.75e6)];
        let mut eibi = Station::new(Source::Eibi, "r1".into(), "RNZ".into(), 17.7e6);
        eibi.schedule = Some(Schedule {
            start_min: 0,
            stop_min: 1440,
            days: Schedule::DAILY,
        });
        let m1 = Meta::new(
            Source::Wikidata,
            2,
            "2026-09-19T10:00:00Z".into(),
            "https://query.wikidata.org".into(),
        );
        let m2 = Meta::new(
            Source::Eibi,
            1,
            "2026-09-19T10:01:00Z".into(),
            "sked-a26.csv".into(),
        );
        store.replace(Source::Wikidata, &wiki, &m1).unwrap();
        store.replace(Source::Eibi, &[eibi.clone()], &m2).unwrap();

        let (index, metas) = store.load_all().unwrap();
        let mut got: Vec<Station> = index.iter().cloned().collect();
        got.sort_by(|a, b| a.id.cmp(&b.id));
        let mut want = vec![eibi.clone(), wiki[0].clone(), wiki[1].clone()];
        want.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(got, want);
        assert_eq!(metas, vec![m1, m2]);
        // The station file is `<source>.json` in the store directory.
        assert!(dir.join("wikidata.json").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_crashed_tmp_never_hides_the_previous_snapshot() {
        let dir = temp("crashed");
        let store = Store::open(&dir).unwrap();
        let meta = Meta::new(Source::Fcc, 1, "t".into(), "fmq".into());
        store
            .replace(Source::Fcc, &[station("a", 90e6)], &meta)
            .unwrap();
        // A half-written snapshot from a killed process.
        std::fs::write(dir.join("fcc.json.tmp"), b"[{\"source\":\"fcc\"").unwrap();
        let (index, metas) = store.load_all().unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(metas[0].count, 1);
        // The next replace cleans up: the tmp is overwritten and renamed.
        store
            .replace(Source::Fcc, &[station("b", 91e6)], &meta)
            .unwrap();
        let (index, _) = store.load_all().unwrap();
        assert_eq!(index.iter().next().unwrap().id, "b");
        assert!(!dir.join("fcc.json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clear_removes_only_that_source() {
        let dir = temp("clear");
        let store = Store::open(&dir).unwrap();
        store
            .replace(
                Source::Fcc,
                &[station("a", 90e6)],
                &Meta::new(Source::Fcc, 1, "t".into(), "fmq".into()),
            )
            .unwrap();
        store.clear(Source::Fcc).unwrap();
        store.clear(Source::Fcc).unwrap(); // absent is fine
        store
            .replace(
                Source::Eibi,
                &[Station::new(Source::Eibi, "x".into(), "X".into(), 6e6)],
                &Meta::new(Source::Eibi, 1, "t".into(), "csv".into()),
            )
            .unwrap();
        let (index, metas) = store.load_all().unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].source, Source::Eibi);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_snapshot_is_an_error_not_a_guess() {
        let dir = temp("corrupt");
        let store = Store::open(&dir).unwrap();
        std::fs::write(dir.join("fcc.json"), b"not json").unwrap();
        assert!(store.load_all().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
