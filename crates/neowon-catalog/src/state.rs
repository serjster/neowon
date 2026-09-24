//! The catalog's in-memory state: entities, merge redirects, tombstones,
//! and the (signal, time, id) index behind history. `apply` is the only
//! mutator; the WAL and snapshots are this state's durable form.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::history::ObsIndex;
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

/// The catalog's state. Its fields are public to read; every change goes
/// through [`State::apply`], which keeps the derived history index in step
/// with `entities`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Stored")]
pub struct State {
    /// Next id to hand out; only moves forward.
    pub next_id: u64,
    pub entities: BTreeMap<Id, Entity>,
    pub redirects: BTreeMap<Id, Redirect>,
    pub tombstones: BTreeMap<Id, Tombstone>,
    /// Derived from `entities`; not part of the stored form.
    #[serde(skip)]
    pub(crate) history: ObsIndex,
}

/// `State` as snapshots store it — the same four fields, unchanged — from
/// which the history index is rebuilt on load.
#[derive(Deserialize)]
struct Stored {
    next_id: u64,
    entities: BTreeMap<Id, Entity>,
    redirects: BTreeMap<Id, Redirect>,
    tombstones: BTreeMap<Id, Tombstone>,
}

impl From<Stored> for State {
    fn from(s: Stored) -> Self {
        Self {
            history: ObsIndex::build(&s.entities),
            next_id: s.next_id,
            entities: s.entities,
            redirects: s.redirects,
            tombstones: s.tombstones,
        }
    }
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

    /// A signal's observations under any id that now redirects to it,
    /// ordered by (time, id).
    pub fn history(&self, id: Id) -> Result<Vec<&Observation>, Error> {
        Ok(self.history_visiting(id)?.0)
    }

    /// `history`, plus how many entities it looked at to answer — the
    /// structural cost the history tests hold flat as the catalog grows.
    pub(crate) fn history_visiting(&self, id: Id) -> Result<(Vec<&Observation>, usize), Error> {
        let canon = self.resolve(id)?;
        let mut visited = 0;
        let v = self
            .history
            .of(canon)
            .filter_map(|oid| {
                visited += 1;
                match self.entities.get(&oid) {
                    Some(Entity::Observation(o)) => Some(o),
                    _ => None,
                }
            })
            .collect();
        Ok((v, visited))
    }

    /// The op that takes `op` back, computed before `op` is applied:
    /// single-entity edits are undone by replacing the entity with its
    /// prior value, which restores it exactly. `None` when no op restores
    /// the prior state exactly: the caller must then clear its undo
    /// history, not skip the entry.
    pub fn inverse(&self, op: &Op) -> Option<Op> {
        if !op.undoable() {
            return None;
        }
        let prior = |id: &Id| {
            Some(Op::Replace {
                entity: self.get(*id)?.clone(),
            })
        };
        match op {
            // A pinned entity refuses the delete that would take it back.
            Op::Insert { entity } | Op::Restore { entity } if entity.pinned() => None,
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
            // Deleting a merge target drops the redirects into it (`retire`),
            // and a restore brings back only the entity.
            Op::Delete {
                id, cascade: false, ..
            } if self.redirects.values().any(|r| r.to == *id) => None,
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
                // References must hold at insert, as they must after.
                self.check_refs(entity)?;
                self.next_id = self.next_id.max(id.0);
                self.put(entity.clone());
            }
            Op::Restore { entity } => {
                let id = entity.id();
                if !self.tombstones.contains_key(&id) || self.redirects.contains_key(&id) {
                    return Err(Error::Invalid(format!("{id} was not deleted")));
                }
                self.check_refs(entity)?;
                self.put(entity.clone());
                self.tombstones.remove(&id);
            }
            Op::Replace { entity } => {
                let id = entity.id();
                let kind = self.get(id).ok_or(Error::NotFound(id))?.kind();
                if kind != entity.kind() || !entity.finite() {
                    return Err(Error::Invalid(format!(
                        "{id}: replacement must be a finite {kind}"
                    )));
                }
                self.check_refs(entity)?;
                self.put(entity.clone());
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
                self.check_refs(&edited)?;
                self.put(edited);
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
        let moved: Vec<Id> = self
            .entities
            .values()
            .filter(|e| match e {
                Entity::Observation(o) => o.signal == from,
                Entity::Transmission(t) => t.signal == from,
                _ => false,
            })
            .map(Entity::id)
            .collect();
        for id in moved {
            let Some(mut e) = self.take(id) else { continue };
            match &mut e {
                Entity::Observation(o) => o.signal = to,
                Entity::Transmission(t) => t.signal = to,
                _ => {}
            }
            self.put(e);
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

    /// Put `e` in the entity map (replacing its id's entry), keeping the
    /// history index in step. The only way an entity goes in.
    fn put(&mut self, e: Entity) {
        if let Some(old) = self.entities.get(&e.id()) {
            self.history.entity_out(old);
        }
        self.history.entity_in(&e);
        self.entities.insert(e.id(), e);
    }

    /// Take `id` out of the entity map, keeping the index in step. The only
    /// way an entity comes out.
    fn take(&mut self, id: Id) -> Option<Entity> {
        let old = self.entities.remove(&id)?;
        self.history.entity_out(&old);
        Some(old)
    }

    /// Remove an entity, leaving a tombstone. Redirects into a retired
    /// signal go with it: the ids merged into it are tombstoned already,
    /// and a redirect to nothing would break every lookup through it.
    fn retire(&mut self, id: Id, reason: &str, at: &str) {
        self.redirects.retain(|_, r| r.to != id);
        if let Some(e) = self.take(id) {
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

    /// In-place access for the edits that change only names, aliases, tags
    /// or the pin — none of which moves an observation in the history index
    /// (each refuses an observation). Anything else goes through `put`.
    fn entity_mut(&mut self, id: Id) -> Result<&mut Entity, Error> {
        self.entities.get_mut(&id).ok_or(Error::NotFound(id))
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
