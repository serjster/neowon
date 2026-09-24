//! What the Catalog window and `get catalog` read every frame, built once
//! per catalog change.
//!
//! The window lists the filtered signals, each with its observation count,
//! and the catalog's integrity. Computed from scratch that is a scan of
//! every entity (signals are interleaved with their observations), a
//! history per listed signal (together, every listed observation) and a
//! full integrity check — per frame, growing with the catalog's history.
//! A catalog changes only by a commit, and every commit advances its
//! sequence number, so `(seq, filter)` says exactly when the listing is
//! stale; between changes a frame reads the listing and scans nothing.

use std::collections::HashMap;
use std::sync::Mutex;

use neowon_catalog::{Catalog, Entity, Id};

#[derive(Default)]
pub struct Listing(Mutex<Built>);

#[derive(Default)]
struct Built {
    /// `(seq, filter)` the listing was built for.
    key: Option<(u64, String)>,
    /// Listed signals, ordered by (creation time, id).
    ids: Vec<Id>,
    observations: HashMap<Id, usize>,
    problems: usize,
    /// Rebuilds so far: the tests' measure of per-frame cost.
    builds: u64,
}

impl Listing {
    fn read<R>(&self, cat: &Catalog, filter: &str, f: impl FnOnce(&Built) -> R) -> R {
        let mut b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = b
            .key
            .as_ref()
            .is_some_and(|(seq, f)| *seq == cat.seq() && f == filter);
        if !fresh {
            let builds = b.builds + 1;
            *b = build(cat, filter);
            b.builds = builds;
        }
        f(&b)
    }

    /// The listed signals' ids, ordered by (creation time, id).
    pub fn ids(&self, cat: &Catalog, filter: &str) -> Vec<Id> {
        self.read(cat, filter, |b| b.ids.clone())
    }

    /// The first `limit` listed ids and how many are listed in all: what a
    /// frame draws. It hands out `limit` ids at most, however many signals
    /// the catalog holds.
    pub fn page(&self, cat: &Catalog, filter: &str, limit: usize) -> (usize, Vec<Id>) {
        self.read(cat, filter, |b| {
            (b.ids.len(), b.ids.iter().take(limit).copied().collect())
        })
    }

    /// A listed signal's observation count; `None` when it is not listed.
    pub fn observations(&self, cat: &Catalog, filter: &str, id: Id) -> Option<usize> {
        self.read(cat, filter, |b| b.observations.get(&id).copied())
    }

    pub fn problems(&self, cat: &Catalog, filter: &str) -> usize {
        self.read(cat, filter, |b| b.problems)
    }

    #[cfg(test)]
    fn builds(&self) -> u64 {
        self.0.lock().unwrap().builds
    }
}

fn build(cat: &Catalog, filter: &str) -> Built {
    let f = filter.to_lowercase();
    let st = cat.state();
    let mut signals: Vec<_> = st
        .entities
        .values()
        .filter_map(|e| match e {
            Entity::Signal(s) => Some(s),
            _ => None,
        })
        .filter(|s| {
            f.is_empty()
                || s.name.to_lowercase().contains(&f)
                || s.tags.iter().any(|t| t.to_lowercase().contains(&f))
                || s.aliases.iter().any(|a| a.name.to_lowercase().contains(&f))
        })
        .collect();
    signals.sort_by(|a, b| {
        a.provenance
            .timestamp
            .cmp(&b.provenance.timestamp)
            .then(a.id.cmp(&b.id))
    });
    Built {
        key: Some((cat.seq(), filter.to_string())),
        observations: signals
            .iter()
            .map(|s| (s.id, st.history(s.id).map_or(0, |h| h.len())))
            .collect(),
        ids: signals.iter().map(|s| s.id).collect(),
        problems: st.integrity().len(),
        builds: 0,
    }
}

#[cfg(test)]
mod tests {
    use neowon_catalog::{Entity, ObsRecord, Observation, Op, Provenance, Signal};

    use super::*;

    fn signal(id: Id, name: &str) -> Entity {
        Entity::Signal(Signal {
            id,
            name: name.into(),
            centre_hz: 100e6,
            bandwidth_hz: 200e3,
            modulation: None,
            source: None,
            tags: Default::default(),
            aliases: vec![],
            notes: String::new(),
            pinned: false,
            confidence: None,
            provenance: Provenance::user("2026-09-23T00:00:00Z"),
        })
    }

    fn observation(id: Id, signal: Id, t: f64) -> Entity {
        Entity::Observation(Observation {
            id,
            signal,
            transmission: None,
            obs: ObsRecord {
                centre_hz: 100e6,
                lo_hz: 99.9e6,
                hi_hz: 100.1e6,
                power_dbfs: -30.0,
                snr_db: 20.0,
                t_start: t,
                t_end: t + 0.1,
            },
            confidence: None,
            provenance: Provenance::user("2026-09-23T00:00:00Z"),
        })
    }

    #[test]
    fn frames_between_changes_rebuild_nothing_as_history_grows() {
        let dir = std::env::temp_dir().join(format!("neowon-app-listing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cat = Catalog::open(&dir).unwrap();
        cat.checkpoint_every = usize::MAX;
        let a = cat.next_id();
        cat.commit(Op::Insert {
            entity: signal(a, "A"),
        })
        .unwrap();
        let b = cat.next_id();
        cat.commit(Op::Insert {
            entity: signal(b, "B"),
        })
        .unwrap();
        let listing = Listing::default();
        let mut builds = 0;
        let mut filed = 0;
        for total in [10, 1_000, 5_000] {
            let ops: Vec<Op> = (filed..total)
                .map(|i| Op::Insert {
                    entity: observation(cat.next_id(), if i % 2 == 0 { a } else { b }, i as f64),
                })
                .collect();
            cat.commit_many(ops).unwrap();
            filed = total;
            // Sixty frames of what the window reads.
            for _ in 0..60 {
                assert_eq!(listing.ids(&cat, ""), vec![a, b]);
                assert_eq!(listing.observations(&cat, "", a), Some(total / 2));
                assert_eq!(listing.problems(&cat, ""), 0);
            }
            builds += 1;
            assert_eq!(
                listing.builds(),
                builds,
                "the listing rebuilt per frame, not per change ({total} observations)"
            );
        }
        // A frame's page stays the page as the signal count grows.
        for total in [600, 3_000] {
            let ops: Vec<Op> = (0..total - listing.ids(&cat, "").len())
                .map(|i| Op::Insert {
                    entity: signal(cat.next_id(), &format!("S{i}")),
                })
                .collect();
            cat.commit_many(ops).unwrap();
            for _ in 0..60 {
                let (n, page) = listing.page(&cat, "", 500);
                assert_eq!(n, total);
                assert_eq!(page.len(), 500, "a frame handed out {} ids", page.len());
            }
        }
        builds += 2;
        // A new filter is a new listing; the same one again is not.
        assert_eq!(listing.ids(&cat, "b"), vec![b]);
        assert_eq!(listing.ids(&cat, "b"), vec![b]);
        assert_eq!(listing.builds(), builds + 1);
        drop(cat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
