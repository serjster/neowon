//! Reference integrity: who refers to an entity, and what is wrong with the
//! catalog's references. `apply` checks the one entity it changes
//! (`check_refs`), never the whole catalog.

use crate::Error;
use crate::model::{Entity, Id};
use crate::state::State;

impl State {
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

    /// Everything wrong with the catalog's references; empty when sound.
    pub fn integrity(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .entities
            .values()
            .flat_map(|e| self.problems_of(e))
            .collect();
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

    /// The integrity problems of `e`'s own references, against the live
    /// entities — one entity's check, not the whole catalog's.
    pub(crate) fn problems_of(&self, e: &Entity) -> Vec<String> {
        let live = |id: Id, want: &str| match self.entities.get(&id) {
            Some(e) if e.kind() == want => None,
            Some(e) => Some(format!("{id} is a {}, not a {want}", e.kind())),
            None => Some(format!("{id} ({want}) does not exist")),
        };
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
        problems
            .into_iter()
            .flatten()
            .map(|p| format!("{}: {p}", e.id()))
            .collect()
    }

    /// Refuse `e` if its own references do not hold.
    pub(crate) fn check_refs(&self, e: &Entity) -> Result<(), Error> {
        let bad = self.problems_of(e);
        if bad.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(bad.join("; ")))
        }
    }
}
