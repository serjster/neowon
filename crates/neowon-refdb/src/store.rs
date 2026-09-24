//! The on-disk reference store: one snapshot per source in
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
///
/// The snapshot and its metadata are two files, so a crash between the two
/// writes can leave metadata describing another fetch. `digest` binds
/// the metadata to the exact snapshot bytes it was written with; the loader
/// reports metadata that does not match its snapshot instead of showing it.
/// A `meta.json` from before the digest (`None`) is still read, and checked
/// by its count alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub source: Source,
    pub count: usize,
    pub fetched_at: String,
    pub origin: String,
    pub licence: String,
    /// FNV-1a 64 of the snapshot file, hex; set by [`Store::replace`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl Meta {
    pub fn new(source: Source, count: usize, fetched_at: String, origin: String) -> Self {
        Self {
            source,
            count,
            fetched_at,
            origin,
            licence: source.licence().to_string(),
            digest: None,
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:016x}", crate::sources::fnv1a_bytes(bytes))
}

/// Something in the store that could not be used, named by source (or by
/// `meta.json`, which belongs to none).
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub source: Option<Source>,
    pub what: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.source {
            Some(src) => write!(f, "{}: {}", src.label(), self.what),
            None => write!(f, "meta.json: {}", self.what),
        }
    }
}

/// Everything [`Store::load`] could use, and a named problem for
/// everything it could not.
#[derive(Debug, Default)]
pub struct Loaded {
    pub index: Index,
    /// Only metadata that matches the snapshot loaded beside it.
    pub metas: Vec<Meta>,
    pub problems: Vec<Problem>,
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
    /// and the metadata are each written atomically; the metadata carries
    /// the snapshot's digest and count, so a crash between the two writes
    /// is detected on load, not read as the new fetch's metadata.
    pub fn replace(&self, src: Source, stations: &[Station], meta: &Meta) -> Result<(), Error> {
        let bytes = serde_json::to_vec(stations)?;
        let meta = Meta {
            count: stations.len(),
            digest: Some(digest(&bytes)),
            ..meta.clone()
        };
        let mut metas = self.metas()?;
        metas.retain(|m| m.source != src);
        metas.push(meta);
        metas.sort_by_key(|m| m.source);
        crate::write_atomic(&self.snapshot(src), &bytes)?;
        crate::write_atomic(&self.meta_path(), &serde_json::to_vec(&metas)?)
    }

    /// Every source that loads, each on its own: a snapshot that cannot be
    /// read contributes nothing and is reported by name, and the other
    /// sources load regardless. Nothing from a failed source is used
    /// and nothing is dropped without a [`Problem`] saying so.
    pub fn load(&self) -> Loaded {
        let mut problems = Vec::new();
        let mut stations = Vec::new();
        // Per source: its snapshot's (count, digest), when it loaded.
        let mut loaded: Vec<(Source, usize, String)> = Vec::new();
        let mut failed: Vec<Source> = Vec::new();
        for src in Source::ALL {
            let bytes = match std::fs::read(self.snapshot(src)) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    problems.push(problem(Some(src), format!("snapshot unreadable: {e}")));
                    failed.push(src);
                    continue;
                }
            };
            match serde_json::from_slice::<Vec<Station>>(&bytes) {
                Ok(v) => {
                    loaded.push((src, v.len(), digest(&bytes)));
                    stations.extend(v);
                }
                Err(e) => {
                    problems.push(problem(Some(src), format!("snapshot corrupt: {e}")));
                    failed.push(src);
                }
            }
        }
        let stored = match self.metas() {
            Ok(m) => m,
            Err(e) => {
                problems.push(problem(None, e.to_string()));
                Vec::new()
            }
        };
        let mut metas = Vec::new();
        for m in stored {
            if failed.contains(&m.source) {
                continue; // the snapshot's own problem is already reported
            }
            let Some((_, count, dig)) = loaded.iter().find(|(s, ..)| *s == m.source) else {
                problems.push(problem(
                    Some(m.source),
                    "metadata without a snapshot (an interrupted fetch or clear)".into(),
                ));
                continue;
            };
            let same = m.count == *count && m.digest.as_ref().is_none_or(|d| d == dig);
            if same {
                metas.push(m);
            } else {
                problems.push(problem(
                    Some(m.source),
                    format!(
                        "metadata ({} rows, fetched {}) describes another snapshot ({count} rows); \
                         fetch or import the source again",
                        m.count, m.fetched_at
                    ),
                ));
            }
        }
        for (src, ..) in &loaded {
            let described = metas.iter().any(|m| m.source == *src);
            let flagged = problems.iter().any(|p| p.source == Some(*src));
            if !described && !flagged {
                problems.push(problem(
                    Some(*src),
                    "snapshot without metadata (an interrupted fetch)".into(),
                ));
            }
        }
        Loaded {
            index: Index::new(stations),
            metas,
            problems,
        }
    }

    /// [`load`](Self::load), all or nothing: any problem is an error that
    /// names its source. For callers that must not proceed on part of the
    /// store; the app uses `load` and shows the problems.
    pub fn load_all(&self) -> Result<(Index, Vec<Meta>), Error> {
        let loaded = self.load();
        match loaded.problems.first() {
            Some(p) => Err(Error::Invalid(format!("refdb {p}"))),
            None => Ok((loaded.index, loaded.metas)),
        }
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

fn problem(source: Option<Source>, what: String) -> Problem {
    Problem { source, what }
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
        // Every field the caller gave comes back exact; the store adds the
        // digest of the station file it wrote, and nothing else.
        let on_disk = |stem: &str| Some(digest(&std::fs::read(dir.join(stem)).unwrap()));
        let m1 = Meta {
            digest: on_disk("wikidata.json"),
            ..m1
        };
        let m2 = Meta {
            digest: on_disk("eibi.json"),
            ..m2
        };
        assert_eq!(metas, vec![m1, m2]);
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

    /// The pair-write's crash windows: a snapshot written without its
    /// metadata, metadata left over from another fetch (same count too),
    /// and metadata whose snapshot is gone. Each is an error from the
    /// strict loader, never metadata presented as describing the data.
    #[test]
    fn metadata_that_does_not_describe_its_snapshot_is_an_error() {
        let mut failures = Vec::new();
        type Damage = fn(&Path);
        let cases: [(&str, Damage); 4] = [
            ("snapshot of another fetch, another count", |d| {
                let v = vec![station("Q7", 1e6), station("Q8", 2e6), station("Q9", 3e6)];
                std::fs::write(d.join("wikidata.json"), serde_json::to_vec(&v).unwrap()).unwrap();
            }),
            ("snapshot of another fetch, same count", |d| {
                let v = vec![station("Q7", 1e6), station("Q8", 2e6)];
                std::fs::write(d.join("wikidata.json"), serde_json::to_vec(&v).unwrap()).unwrap();
            }),
            ("metadata without its snapshot", |d| {
                std::fs::remove_file(d.join("wikidata.json")).unwrap();
            }),
            ("snapshot without its metadata", |d| {
                std::fs::write(d.join("meta.json"), b"[]").unwrap();
            }),
        ];
        for (i, (name, damage)) in cases.iter().enumerate() {
            let dir = temp(&format!("pair-{i}"));
            let store = Store::open(&dir).unwrap();
            let meta = Meta::new(Source::Wikidata, 2, "t0".into(), "fetch A".into());
            store
                .replace(
                    Source::Wikidata,
                    &[station("Q1", 100.3e6), station("Q2", 9.75e6)],
                    &meta,
                )
                .unwrap();
            damage(&dir);
            match store.load_all() {
                Ok((index, metas)) => failures.push(format!(
                    "{name}: loaded {} stations with metadata {metas:?}",
                    index.len()
                )),
                Err(e) if !e.to_string().contains("Wikidata") => {
                    failures.push(format!("{name}: refused, but the source is not named: {e}"))
                }
                Err(_) => {}
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// What earlier builds wrote — a `meta.json` without a digest — still
    /// loads, as it did (existing stores stay readable).
    #[test]
    fn a_store_written_before_the_digest_still_loads() {
        let dir = temp("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let v = vec![station("Q1", 100.3e6), station("Q2", 9.75e6)];
        std::fs::write(dir.join("wikidata.json"), serde_json::to_vec(&v).unwrap()).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            br#"[{"source":"wikidata","count":2,"fetched_at":"2026-09-19T10:00:00Z","origin":"https://query.wikidata.org","licence":"CC0"}]"#,
        )
        .unwrap();
        let store = Store::open(&dir).unwrap();
        let (index, metas) = store.load_all().unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].origin, "https://query.wikidata.org");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_corrupt_source_leaves_the_others_loaded() {
        let dir = temp("isolate");
        let store = Store::open(&dir).unwrap();
        let meta = Meta::new(Source::Wikidata, 2, "t".into(), "q".into());
        let wiki = [station("Q1", 100.3e6), station("Q2", 9.75e6)];
        store.replace(Source::Wikidata, &wiki, &meta).unwrap();
        let meta = Meta::new(Source::Fcc, 1, "t".into(), "fmq".into());
        store
            .replace(Source::Fcc, &[station("a", 90e6)], &meta)
            .unwrap();
        std::fs::write(dir.join("fcc.json"), b"[{\"source\":\"fcc\"").unwrap();
        let loaded = store.load();
        let ids: Vec<&str> = loaded.index.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["Q2", "Q1"]);
        assert_eq!(loaded.metas.len(), 1);
        assert_eq!(loaded.metas[0].source, Source::Wikidata);
        // The FCC metadata is not shown for a snapshot that did not load,
        // and the snapshot's own problem is the one reported.
        assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
        assert_eq!(loaded.problems[0].source, Some(Source::Fcc));
        assert!(
            loaded.problems[0]
                .to_string()
                .starts_with("FCC: snapshot corrupt")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_snapshot_is_an_error_not_a_guess() {
        let dir = temp("corrupt");
        let store = Store::open(&dir).unwrap();
        std::fs::write(dir.join("fcc.json"), b"not json").unwrap();
        assert!(store.load_all().is_err());
        // Isolated, not guessed: the source is named, nothing of it
        // is used, and the other sources are unaffected.
        let err = store.load_all().unwrap_err().to_string();
        assert!(err.contains("FCC"), "{err}");
        let loaded = store.load();
        assert!(loaded.index.is_empty());
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].source, Some(Source::Fcc));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
