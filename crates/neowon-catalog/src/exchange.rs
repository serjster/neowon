//! Export and import: the live entities as one JSON document. Import
//! gives every entity a fresh id (remapping references between them) and
//! import provenance naming the original id, so importing the same file
//! twice never collides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Entity, Id, ProvKind};
use crate::op::Op;
use crate::{Catalog, Error, FORMAT, SCHEMA};

#[derive(Debug, Serialize, Deserialize)]
pub struct Export {
    pub format: String,
    pub schema: u32,
    pub entities: Vec<Entity>,
}

pub fn export(cat: &Catalog) -> Export {
    Export {
        format: format!("{FORMAT}-export"),
        schema: SCHEMA,
        entities: cat.state().entities.values().cloned().collect(),
    }
}

/// Import `doc`, returning the new ids in document order.
pub fn import(cat: &mut Catalog, doc: &Export, at: &str) -> Result<Vec<Id>, Error> {
    if doc.schema > SCHEMA {
        return Err(Error::TooNew(doc.schema));
    }
    let map: BTreeMap<Id, Id> = doc
        .entities
        .iter()
        .map(|e| (e.id(), cat.next_id()))
        .collect();
    let remap = |id: Id| map.get(&id).copied().unwrap_or(id);
    // Referenced kinds first, so every insert finds its targets.
    let rank = |e: &Entity| match e {
        Entity::Source(_) | Entity::BandPlan(_) | Entity::Survey(_) => 0,
        Entity::Signal(_) | Entity::Emitter(_) => 1,
        Entity::Transmission(_) => 2,
        Entity::Observation(_) => 3,
    };
    let mut todo: Vec<&Entity> = doc.entities.iter().collect();
    todo.sort_by_key(|e| rank(e));
    for e in todo {
        let mut e = e.clone();
        let old = e.id();
        match &mut e {
            Entity::Signal(x) => {
                x.id = remap(x.id);
                x.source = x.source.map(remap);
            }
            Entity::Emitter(x) => {
                x.id = remap(x.id);
                x.source = x.source.map(remap);
            }
            Entity::Transmission(x) => {
                x.id = remap(x.id);
                x.signal = remap(x.signal);
            }
            Entity::Observation(x) => {
                x.id = remap(x.id);
                x.signal = remap(x.signal);
                x.transmission = x.transmission.map(remap);
            }
            Entity::Source(x) => x.id = remap(x.id),
            Entity::BandPlan(x) => x.id = remap(x.id),
            Entity::Survey(x) => x.id = remap(x.id),
        }
        let p = provenance_mut(&mut e);
        p.kind = ProvKind::Import;
        p.input_ref = Some(format!("import:{old}"));
        p.timestamp = at.to_string();
        cat.commit(Op::Insert { entity: e })?;
    }
    Ok(doc.entities.iter().map(|e| remap(e.id())).collect())
}

fn provenance_mut(e: &mut Entity) -> &mut crate::Provenance {
    match e {
        Entity::Signal(x) => &mut x.provenance,
        Entity::Transmission(x) => &mut x.provenance,
        Entity::Source(x) => &mut x.provenance,
        Entity::Emitter(x) => &mut x.provenance,
        Entity::BandPlan(x) => &mut x.provenance,
        Entity::Survey(x) => &mut x.provenance,
        Entity::Observation(x) => &mut x.provenance,
    }
}
