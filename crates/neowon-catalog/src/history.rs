//! The ordered `(signal, time, id)` index behind `State::history`. History is
//! read once per listed signal per UI frame, so it
//! must cost what that signal's history holds, not what the catalog does:
//! a range over this set, never a scan of every entity.
//!
//! The index is derived, never stored: snapshots stay the entity map they
//! always were, and the index is rebuilt from it on load.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Entity, Id, Observation};

/// `f64` → `u64` whose unsigned order is `f64::total_cmp`'s.
fn time_key(t: f64) -> u64 {
    let b = t.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObsIndex {
    /// `(signal, time_key(t_start), observation)`.
    set: BTreeSet<(Id, u64, Id)>,
}

impl ObsIndex {
    pub fn build(entities: &BTreeMap<Id, Entity>) -> Self {
        let mut x = Self::default();
        for e in entities.values() {
            x.entity_in(e);
        }
        x
    }

    /// Account for `e` having been added to the entity map.
    pub fn entity_in(&mut self, e: &Entity) {
        if let Entity::Observation(o) = e {
            self.set.insert(key(o));
        }
    }

    /// Account for `e` having been removed from the entity map.
    pub fn entity_out(&mut self, e: &Entity) {
        if let Entity::Observation(o) = e {
            self.set.remove(&key(o));
        }
    }

    /// The observations filed against `signal`, ordered by (time, id).
    pub fn of(&self, signal: Id) -> impl Iterator<Item = Id> + '_ {
        self.set
            .range((signal, 0, Id(0))..=(signal, u64::MAX, Id(u64::MAX)))
            .map(|&(_, _, id)| id)
    }
}

fn key(o: &Observation) -> (Id, u64, Id) {
    (o.signal, time_key(o.obs.t_start), o.id)
}

#[cfg(test)]
mod tests {
    use super::{ObsIndex, time_key};
    use crate::model::{Entity, Id, ObsRecord, Observation, Provenance, Signal};
    use crate::op::Op;
    use crate::state::State;

    const AT: &str = "2026-09-23T00:00:00Z";

    fn signal(id: u64) -> Entity {
        Entity::Signal(Signal {
            id: Id(id),
            name: format!("S{id}"),
            centre_hz: 100e6,
            bandwidth_hz: 200e3,
            modulation: None,
            source: None,
            tags: Default::default(),
            aliases: vec![],
            notes: String::new(),
            pinned: false,
            confidence: None,
            provenance: Provenance::user(AT),
        })
    }

    fn rec(t: f64) -> ObsRecord {
        ObsRecord {
            centre_hz: 100e6,
            lo_hz: 99.9e6,
            hi_hz: 100.1e6,
            power_dbfs: -30.0,
            snr_db: 20.0,
            t_start: t,
            t_end: t + 0.1,
        }
    }

    fn observation(id: u64, signal: u64, t: f64) -> Entity {
        Entity::Observation(Observation {
            id: Id(id),
            signal: Id(signal),
            transmission: None,
            obs: rec(t),
            confidence: None,
            provenance: Provenance::user(AT),
        })
    }

    fn insert(st: &mut State, e: Entity) {
        st.apply(&Op::Insert { entity: e }).unwrap();
    }

    /// History ordered by (time, id), by a full scan.
    fn oracle(st: &State, id: Id) -> Vec<Id> {
        let canon = st.resolve(id).unwrap();
        let mut v: Vec<&Observation> = st
            .entities
            .values()
            .filter_map(|e| match e {
                Entity::Observation(o) if o.signal == canon => Some(o),
                _ => None,
            })
            .collect();
        v.sort_by(|a, b| {
            a.obs
                .t_start
                .total_cmp(&b.obs.t_start)
                .then(a.id.cmp(&b.id))
        });
        v.iter().map(|o| o.id).collect()
    }

    #[test]
    fn history_visits_the_signals_own_rows_as_the_catalog_grows() {
        // One signal with three observations; the rest of the catalog's
        // history grows 10 000-fold around it. What `history` looks at must
        // stay three (a scan cost 0.25 ms a call at 5 000 observations,
        // once per listed signal per frame).
        let mut st = State::default();
        insert(&mut st, signal(1));
        insert(&mut st, signal(2));
        for (id, t) in [(3, 2.0), (4, 1.0), (5, 2.0)] {
            insert(&mut st, observation(id, 1, t));
        }
        let mut next = 6;
        let mut seed = 0x5EED_u64;
        for total in [0, 10, 1_000, 10_000] {
            while next - 6 < total {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                insert(&mut st, observation(next, 2, (seed >> 40) as f64));
                next += 1;
            }
            let (h, visited) = st.history_visiting(Id(1)).unwrap();
            println!("other observations {total:>6}: history(#1) visited {visited}");
            assert_eq!(
                visited, 3,
                "history cost grew with the catalog ({total} others)"
            );
            let ids: Vec<Id> = h.iter().map(|o| o.id).collect();
            assert_eq!(ids, vec![Id(4), Id(3), Id(5)]);
            assert_eq!(ids, oracle(&st, Id(1)));
            let (h2, visited2) = st.history_visiting(Id(2)).unwrap();
            assert_eq!((h2.len(), visited2), (total as usize, total as usize));
        }
    }

    #[test]
    fn the_index_matches_a_rebuild_after_every_op() {
        // A seeded walk over every op that moves an observation: insert,
        // edit its signal or time, replace, merge its signal away, delete,
        // cascade, restore. After each, the maintained index equals one
        // built from scratch, history equals the scan oracle, and the state
        // survives its stored form (which does not carry the index).
        let mut st = State::default();
        for s in 1..=4 {
            insert(&mut st, signal(s));
        }
        let mut next = 5_u64;
        let mut seed = 0x1022_u64;
        let mut roll = |n: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let mut deleted: Vec<Entity> = Vec::new();
        let mut applied = std::collections::BTreeMap::<String, u32>::new();
        for step in 0..600 {
            let live_sig: Vec<Id> = st
                .entities
                .values()
                .filter(|e| e.kind() == "signal")
                .map(Entity::id)
                .collect();
            let obs: Vec<Id> = st
                .entities
                .values()
                .filter(|e| e.kind() == "observation")
                .map(Entity::id)
                .collect();
            let pick_sig = |r: u64| live_sig[(r % live_sig.len() as u64) as usize];
            let op = match roll(9) {
                _ if obs.is_empty() || live_sig.len() < 2 => None,
                0 | 1 => None,
                2 => Some(Op::Edit {
                    id: obs[roll(obs.len() as u64) as usize],
                    field: "signal".into(),
                    value: serde_json::json!(pick_sig(roll(64)).0),
                }),
                3 => Some(Op::Edit {
                    id: obs[roll(obs.len() as u64) as usize],
                    field: "obs".into(),
                    value: serde_json::to_value(rec(roll(8) as f64)).unwrap(),
                }),
                4 => {
                    let id = obs[roll(obs.len() as u64) as usize];
                    let Some(Entity::Observation(mut o)) = st.get(id).cloned() else {
                        unreachable!()
                    };
                    o.obs.t_start = -(roll(8) as f64);
                    Some(Op::Replace {
                        entity: Entity::Observation(o),
                    })
                }
                5 if live_sig.len() > 2 => Some(Op::Merge {
                    from: pick_sig(roll(64)),
                    to: pick_sig(roll(64)),
                    at: AT.into(),
                }),
                6 => {
                    let id = obs[roll(obs.len() as u64) as usize];
                    deleted.push(st.get(id).unwrap().clone());
                    Some(Op::Delete {
                        id,
                        cascade: false,
                        at: AT.into(),
                    })
                }
                7 if live_sig.len() > 2 => Some(Op::Delete {
                    id: pick_sig(roll(64)),
                    cascade: true,
                    at: AT.into(),
                }),
                8 if !deleted.is_empty() => Some(Op::Restore {
                    entity: deleted.swap_remove(roll(deleted.len() as u64) as usize),
                }),
                _ => None,
            };
            let op = op.unwrap_or_else(|| {
                if live_sig.len() < 3 {
                    next += 1;
                    return Op::Insert {
                        entity: signal(next),
                    };
                }
                next += 1;
                Op::Insert {
                    entity: observation(next, pick_sig(roll(64)).0, roll(8) as f64),
                }
            });
            // Refusals (a merge into itself, a restore whose signal is
            // gone) must leave the index untouched too.
            let kind = format!("{op:?}");
            let kind = kind.split([' ', '{']).next().unwrap_or("").to_string();
            *applied.entry(kind).or_insert(0) += u32::from(st.apply(&op).is_ok());
            assert_eq!(
                st.history,
                ObsIndex::build(&st.entities),
                "step {step}: {op:?}"
            );
            for s in &live_sig {
                if st.signal(*s).is_some() {
                    let got: Vec<Id> = st.history(*s).unwrap().iter().map(|o| o.id).collect();
                    assert_eq!(got, oracle(&st, *s), "step {step}");
                }
            }
        }
        println!("applied: {applied:?}");
        for kind in ["Insert", "Edit", "Replace", "Merge", "Delete", "Restore"] {
            assert!(
                applied.get(kind).is_some_and(|n| *n > 0),
                "{kind} never applied"
            );
        }
        let stored: State = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(stored, st, "the stored form rebuilds the same index");
        assert!(
            !serde_json::to_string(&st).unwrap().contains("history"),
            "the index is not stored"
        );
    }

    #[test]
    fn time_key_orders_as_total_cmp() {
        let v = [
            f64::NEG_INFINITY,
            -1e300,
            -1.5,
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            1.0,
            1e300,
            f64::INFINITY,
            f64::NAN,
        ];
        for a in v {
            for b in v {
                assert_eq!(time_key(a).cmp(&time_key(b)), a.total_cmp(&b), "{a} vs {b}");
            }
        }
    }
}
