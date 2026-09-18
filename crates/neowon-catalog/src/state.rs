//! The catalog's in-memory state: entities, merge redirects, tombstones,
//! and the (signal, time, id) index behind history. `apply` is the only
//! mutator; the WAL and snapshots are this state's durable form.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::model::{Alias, Entity, Id, Observation, Signal};
use crate::op::Op;

/// Longest redirect chain `resolve` follows (merges compress chains, so
/// in a healthy catalog every chain is one hop).
pub const MAX_REDIRECT_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redirect {
    pub to: Id,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tombstone {
    pub kind: String,
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Next id to hand out; only moves forward.
    pub next_id: u64,
    pub entities: BTreeMap<Id, Entity>,
    pub redirects: BTreeMap<Id, Redirect>,
    pub tombstones: BTreeMap<Id, Tombstone>,
}

/// Fields `Op::Edit` may not touch.
const FROZEN: [&str; 6] = ["id", "entity", "provenance", "pinned", "tags", "aliases"];

impl State {
    pub fn alloc_id(&mut self) -> Id {
        self.next_id += 1;
        Id(self.next_id)
    }

    pub fn get(&self, id: Id) -> Option<&Entity> {
        self.entities.get(&id)
    }

    pub fn signal(&self, id: Id) -> Option<&Signal> {
        match self.entities.get(&id) {
            Some(Entity::Signal(s)) => Some(s),
            _ => None,
        }
    }

    /// Follow merge redirects to the live id.
    pub fn resolve(&self, mut id: Id) -> Result<Id, Error> {
        for _ in 0..=MAX_REDIRECT_DEPTH {
            match self.redirects.get(&id) {
                Some(r) => id = r.to,
                None => return Ok(id),
            }
        }
        Err(Error::RedirectLoop(id))
    }

    /// Every entity that names `id` (observations and transmissions of a
    /// signal, signals and emitters of a source).
    pub fn referrers(&self, id: Id) -> Vec<Id> {
        self.entities
            .values()
            .filter(|e| match e {
                Entity::Observation(o) => o.signal == id || o.transmission == Some(id),
                Entity::Transmission(t) => t.signal == id,
                Entity::Signal(s) => s.source == Some(id),
                Entity::Emitter(m) => m.source == Some(id),
                _ => false,
            })
            .map(Entity::id)
            .collect()
    }

    /// Everything that would dangle if `id` went: its referrers, theirs,
    /// and so on (a source's signals and those signals' observations).
    pub fn referrers_deep(&self, id: Id) -> Vec<Id> {
        let (mut out, mut todo) = (Vec::new(), vec![id]);
        while let Some(x) = todo.pop() {
            for r in self.referrers(x) {
                if !out.contains(&r) {
                    out.push(r);
                    todo.push(r);
                }
            }
        }
        out
    }

    /// A signal's observations under any id that now redirects to it,
    /// ordered by (time, id).
    pub fn history(&self, id: Id) -> Result<Vec<&Observation>, Error> {
        let canon = self.resolve(id)?;
        let mut v: Vec<&Observation> = self
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
        Ok(v)
    }

    /// Everything wrong with the catalog's references; empty when sound.
    pub fn integrity(&self) -> Vec<String> {
        let mut out = Vec::new();
        let live = |id: Id, want: &str| match self.entities.get(&id) {
            Some(e) if e.kind() == want => None,
            Some(e) => Some(format!("{id} is a {}, not a {want}", e.kind())),
            None => Some(format!("{id} ({want}) does not exist")),
        };
        for e in self.entities.values() {
            let problems = match e {
                Entity::Observation(o) => vec![
                    live(o.signal, "signal"),
                    o.transmission.and_then(|t| live(t, "transmission")),
                ],
                Entity::Transmission(t) => vec![live(t.signal, "signal")],
                Entity::Signal(s) => vec![s.source.and_then(|x| live(x, "source"))],
                Entity::Emitter(m) => vec![m.source.and_then(|x| live(x, "source"))],
                _ => vec![],
            };
            out.extend(
                problems
                    .into_iter()
                    .flatten()
                    .map(|p| format!("{}: {p}", e.id())),
            );
        }
        for (from, r) in &self.redirects {
            match self.resolve(*from) {
                Err(e) => out.push(e.to_string()),
                Ok(to) if self.signal(to).is_none() => {
                    out.push(format!("redirect {from} -> {} ends at no signal", r.to))
                }
                Ok(_) => {}
            }
            if self.entities.contains_key(from) {
                out.push(format!("{from} is both live and redirected"));
            }
        }
        out
    }

    /// The op that takes `op` back, computed before `op` is applied:
    /// single-entity edits are undone by replacing the entity with its
    /// prior value, which restores it exactly.
    pub fn inverse(&self, op: &Op) -> Option<Op> {
        let prior = |id: &Id| {
            Some(Op::Replace {
                entity: self.get(*id)?.clone(),
            })
        };
        match op {
            Op::Insert { entity } | Op::Restore { entity } => Some(Op::Delete {
                id: entity.id(),
                cascade: false,
                at: String::new(),
            }),
            Op::Replace { entity } => prior(&entity.id()),
            Op::Rename { id, .. }
            | Op::Alias { id, .. }
            | Op::Unalias { id, .. }
            | Op::Tag { id, .. }
            | Op::Pin { id, .. }
            | Op::Edit { id, .. } => prior(id),
            Op::Delete {
                id, cascade: false, ..
            } => Some(Op::Restore {
                entity: self.get(*id)?.clone(),
            }),
            Op::Delete { .. } | Op::Merge { .. } | Op::Purge { .. } => None,
        }
    }

    /// Apply one op: validated first, then applied whole.
    pub fn apply(&mut self, op: &Op) -> Result<(), Error> {
        match op {
            Op::Insert { entity } => {
                let id = entity.id();
                if self.entities.contains_key(&id)
                    || self.redirects.contains_key(&id)
                    || self.tombstones.contains_key(&id)
                    || id.0 == 0
                {
                    return Err(Error::Invalid(format!("{id} is not a fresh id")));
                }
                if !entity.finite() {
                    return Err(Error::Invalid(format!("{id} has a non-finite number")));
                }
                self.next_id = self.next_id.max(id.0);
                self.entities.insert(id, entity.clone());
                // References must hold at insert, as they must after.
                let bad = self.integrity_of(id);
                if !bad.is_empty() {
                    self.entities.remove(&id);
                    return Err(Error::Invalid(bad.join("; ")));
                }
            }
            Op::Restore { entity } => {
                let id = entity.id();
                if !self.tombstones.contains_key(&id) || self.redirects.contains_key(&id) {
                    return Err(Error::Invalid(format!("{id} was not deleted")));
                }
                self.entities.insert(id, entity.clone());
                let bad = self.integrity_of(id);
                if !bad.is_empty() {
                    self.entities.remove(&id);
                    return Err(Error::Invalid(bad.join("; ")));
                }
                self.tombstones.remove(&id);
            }
            Op::Replace { entity } => {
                let id = entity.id();
                let old = self.get(id).ok_or(Error::NotFound(id))?.clone();
                if old.kind() != entity.kind() || !entity.finite() {
                    return Err(Error::Invalid(format!(
                        "{id}: replacement must be a finite {}",
                        old.kind()
                    )));
                }
                self.entities.insert(id, entity.clone());
                let bad = self.integrity_of(id);
                if !bad.is_empty() {
                    self.entities.insert(id, old);
                    return Err(Error::Invalid(bad.join("; ")));
                }
            }
            Op::Rename { id, name, at } => {
                let e = self.entity_mut(*id)?;
                if aliases_of(e).is_none() {
                    return Err(unnamed(*id));
                }
                let old = name_of(e).ok_or_else(|| unnamed(*id))?.to_string();
                if old != *name {
                    set_name(e, name.clone());
                    let aliases = aliases_mut(e).ok_or_else(|| unnamed(*id))?;
                    aliases.retain(|a| a.name != *name);
                    aliases.push(Alias {
                        name: old,
                        at: at.clone(),
                    });
                }
            }
            Op::Alias { id, name, at } => {
                let aliases = aliases_mut(self.entity_mut(*id)?).ok_or_else(|| unnamed(*id))?;
                if !aliases.iter().any(|a| a.name == *name) {
                    aliases.push(Alias {
                        name: name.clone(),
                        at: at.clone(),
                    });
                }
            }
            Op::Unalias { id, name } => {
                let aliases = aliases_mut(self.entity_mut(*id)?).ok_or_else(|| unnamed(*id))?;
                aliases.retain(|a| a.name != *name);
            }
            Op::Tag { id, tag, on } => {
                let tags = match self.entity_mut(*id)? {
                    Entity::Signal(s) => &mut s.tags,
                    Entity::Source(s) => &mut s.tags,
                    e => return Err(Error::Invalid(format!("a {} has no tags", e.kind()))),
                };
                if *on {
                    tags.insert(tag.clone());
                } else {
                    tags.remove(tag);
                }
            }
            Op::Pin { id, on } => match self.entity_mut(*id)? {
                Entity::Signal(e) => e.pinned = *on,
                Entity::Source(e) => e.pinned = *on,
                Entity::Emitter(e) => e.pinned = *on,
                Entity::BandPlan(e) => e.pinned = *on,
                Entity::Survey(e) => e.pinned = *on,
                e => return Err(Error::Invalid(format!("a {} cannot be pinned", e.kind()))),
            },
            Op::Edit { id, field, value } => {
                if FROZEN.contains(&field.as_str()) {
                    return Err(Error::Invalid(format!("{field} is not editable")));
                }
                let e = self.get(*id).ok_or(Error::NotFound(*id))?.clone();
                let mut v = serde_json::to_value(&e).map_err(|x| Error::Invalid(x.to_string()))?;
                let slot = v
                    .get_mut(field)
                    .ok_or_else(|| Error::Invalid(format!("a {} has no {field}", e.kind())))?;
                *slot = value.clone();
                let edited: Entity =
                    serde_json::from_value(v).map_err(|x| Error::Invalid(x.to_string()))?;
                if !edited.finite() {
                    return Err(Error::Invalid(format!("{field} must be finite")));
                }
                self.entities.insert(*id, edited);
                let bad = self.integrity_of(*id);
                if !bad.is_empty() {
                    self.entities.insert(*id, e);
                    return Err(Error::Invalid(bad.join("; ")));
                }
            }
            Op::Merge { from, to, at } => self.merge(*from, *to, at)?,
            Op::Delete { id, cascade, at } => {
                let e = self.get(*id).ok_or(Error::NotFound(*id))?;
                if e.pinned() {
                    return Err(Error::Pinned(*id));
                }
                let refs = self.referrers_deep(*id);
                if !refs.is_empty() && !cascade {
                    return Err(Error::Referenced(*id, refs.len()));
                }
                for r in refs {
                    self.retire(r, "cascade", at);
                }
                self.retire(*id, "delete", at);
            }
            Op::Purge { ids, cascade, at } => {
                let doomed: Vec<Id> = ids
                    .iter()
                    .copied()
                    .filter(|id| self.signal(*id).is_some_and(|s| !s.pinned))
                    .collect();
                let refs: Vec<Id> = doomed
                    .iter()
                    .flat_map(|id| self.referrers_deep(*id))
                    .collect();
                if !refs.is_empty() && !cascade {
                    return Err(Error::Invalid(format!(
                        "{} observations/transmissions reference these signals; purge with cascade",
                        refs.len()
                    )));
                }
                for r in refs {
                    self.retire(r, "cascade", at);
                }
                for id in doomed {
                    self.retire(id, "purge", at);
                }
            }
        }
        Ok(())
    }

    fn merge(&mut self, from: Id, to: Id, at: &str) -> Result<(), Error> {
        if from == to {
            return Err(Error::Invalid("a signal cannot merge into itself".into()));
        }
        let src = self.signal(from).ok_or(Error::NotFound(from))?.clone();
        let to = self.resolve(to)?;
        if to == from {
            return Err(Error::RedirectLoop(from));
        }
        let dst = match self.entities.get_mut(&to) {
            Some(Entity::Signal(s)) => s,
            _ => return Err(Error::NotFound(to)),
        };
        for name in
            std::iter::once(src.name.clone()).chain(src.aliases.iter().map(|a| a.name.clone()))
        {
            if name != dst.name && !dst.aliases.iter().any(|a| a.name == name) {
                dst.aliases.push(Alias {
                    name,
                    at: at.to_string(),
                });
            }
        }
        dst.tags.extend(src.tags.iter().cloned());
        dst.pinned |= src.pinned;
        for e in self.entities.values_mut() {
            match e {
                Entity::Observation(o) if o.signal == from => o.signal = to,
                Entity::Transmission(t) if t.signal == from => t.signal = to,
                _ => {}
            }
        }
        // Compress: whatever pointed at `from` now points straight at `to`.
        for r in self.redirects.values_mut() {
            if r.to == from {
                r.to = to;
            }
        }
        self.redirects.insert(
            from,
            Redirect {
                to,
                at: at.to_string(),
            },
        );
        self.retire(from, "merge", at);
        Ok(())
    }

    /// Remove an entity, leaving a tombstone. Redirects into a retired
    /// signal go with it: the ids merged into it are tombstoned already,
    /// and a redirect to nothing would break every lookup through it.
    fn retire(&mut self, id: Id, reason: &str, at: &str) {
        self.redirects.retain(|_, r| r.to != id);
        if let Some(e) = self.entities.remove(&id) {
            self.tombstones.insert(
                id,
                Tombstone {
                    kind: e.kind().to_string(),
                    reason: reason.to_string(),
                    at: at.to_string(),
                },
            );
        }
    }

    fn entity_mut(&mut self, id: Id) -> Result<&mut Entity, Error> {
        self.entities.get_mut(&id).ok_or(Error::NotFound(id))
    }

    /// Integrity problems of one entity's own references.
    fn integrity_of(&self, id: Id) -> Vec<String> {
        let tag = format!("{id}:");
        self.integrity()
            .into_iter()
            .filter(|p| p.starts_with(&tag))
            .collect()
    }
}

fn unnamed(id: Id) -> Error {
    Error::Invalid(format!("{id} has no name"))
}

fn name_of(e: &Entity) -> Option<&str> {
    match e {
        Entity::Signal(x) => Some(&x.name),
        Entity::Source(x) => Some(&x.name),
        Entity::Emitter(x) => Some(&x.name),
        _ => None,
    }
}

fn set_name(e: &mut Entity, name: String) {
    match e {
        Entity::Signal(x) => x.name = name,
        Entity::Source(x) => x.name = name,
        Entity::Emitter(x) => x.name = name,
        _ => {}
    }
}

fn aliases_of(e: &Entity) -> Option<&Vec<Alias>> {
    match e {
        Entity::Signal(x) => Some(&x.aliases),
        Entity::Source(x) => Some(&x.aliases),
        Entity::Emitter(x) => Some(&x.aliases),
        _ => None,
    }
}

fn aliases_mut(e: &mut Entity) -> Option<&mut Vec<Alias>> {
    match e {
        Entity::Signal(x) => Some(&mut x.aliases),
        Entity::Source(x) => Some(&mut x.aliases),
        Entity::Emitter(x) => Some(&mut x.aliases),
        _ => None,
    }
}
