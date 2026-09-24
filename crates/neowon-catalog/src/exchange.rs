//! Export and import: the live entities as one JSON document. Import
//! gives every entity a fresh id (remapping references between them) and
//! import provenance naming the original id, so importing the same file
//! twice never collides.
//!
//! An import never invents, cross-wires or drops data: a
//! document whose `format` is not an export is refused before anything is
//! read into the catalog, a reference that does not resolve inside the
//! document refuses the whole import by name ([`problems`]), and the
//! `input_ref` an entity carried survives beside the import's own mark.

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
        format: export_format(),
        schema: SCHEMA,
        entities: cat.state().entities.values().cloned().collect(),
    }
}

/// The `format` every export carries and every import requires.
pub fn export_format() -> String {
    format!("{FORMAT}-export")
}

/// The document's own reference integrity: every id repeated, and every
/// reference that does not name an entity of the right kind *in the
/// document*. Empty when the document stands on its own. An import binds
/// references only inside the document, so anything listed here would
/// otherwise be bound to whatever local entity happens to hold that id.
pub fn problems(doc: &Export) -> Vec<String> {
    let mut kinds: BTreeMap<Id, &'static str> = BTreeMap::new();
    let mut out = Vec::new();
    for e in &doc.entities {
        if let Some(k) = kinds.insert(e.id(), e.kind()) {
            out.push(format!(
                "{} appears twice in the document ({k}, {})",
                e.id(),
                e.kind()
            ));
        }
    }
    for e in &doc.entities {
        let mut want = |field: &str, id: Id, kind: &str| match kinds.get(&id) {
            Some(k) if *k == kind => {}
            Some(k) => out.push(format!(
                "{} {}: {field} {id} is a {k} in the document, not a {kind}",
                e.kind(),
                e.id()
            )),
            None => out.push(format!(
                "{} {}: {field} {id} is not in the document",
                e.kind(),
                e.id()
            )),
        };
        match e {
            Entity::Signal(x) => x
                .source
                .into_iter()
                .for_each(|s| want("source", s, "source")),
            Entity::Emitter(x) => x
                .source
                .into_iter()
                .for_each(|s| want("source", s, "source")),
            Entity::Transmission(x) => want("signal", x.signal, "signal"),
            Entity::Observation(x) => {
                want("signal", x.signal, "signal");
                if let Some(t) = x.transmission {
                    want("transmission", t, "transmission");
                }
            }
            Entity::Source(_) | Entity::BandPlan(_) | Entity::Survey(_) => {}
        }
    }
    out
}

/// Import `doc`, returning the new ids in document order.
pub fn import(cat: &mut Catalog, doc: &Export, at: &str) -> Result<Vec<Id>, Error> {
    if doc.format != export_format() {
        return Err(Error::Invalid(format!(
            "format {:?} is not a catalog export ({:?})",
            doc.format,
            export_format()
        )));
    }
    if doc.schema > SCHEMA {
        return Err(Error::TooNew(doc.schema));
    }
    // Refused before an id is allocated: a refused import leaves the
    // catalog exactly as it was.
    let bad = problems(doc);
    if !bad.is_empty() {
        return Err(Error::Invalid(format!(
            "import refused: {}",
            bad.join("; ")
        )));
    }
    let map: BTreeMap<Id, Id> = doc
        .entities
        .iter()
        .map(|e| (e.id(), cat.next_id()))
        .collect();
    // Every reference is in the document (checked above), so every id has
    // a new one; nothing falls back to a local id.
    let remap = |id: Id| map[&id];
    // Referenced kinds first, so every insert finds its targets.
    let rank = |e: &Entity| match e {
        Entity::Source(_) | Entity::BandPlan(_) | Entity::Survey(_) => 0,
        Entity::Signal(_) | Entity::Emitter(_) => 1,
        Entity::Transmission(_) => 2,
        Entity::Observation(_) => 3,
    };
    let mut todo: Vec<&Entity> = doc.entities.iter().collect();
    todo.sort_by_key(|e| rank(e));
    // One fsync for the import, not one per entity.
    let mut ops = Vec::with_capacity(todo.len());
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
        // The document's id, then what the entity was derived from.
        p.input_ref = Some(match p.input_ref.take() {
            Some(from) => format!("import:{old} {from}"),
            None => format!("import:{old}"),
        });
        p.timestamp = at.to_string();
        ops.push(Op::Insert { entity: e });
    }
    cat.commit_many(ops)?;
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

/// Export to a JSON file; returns how many entities were written. The file
/// appears whole or not at all: a killed export leaves no truncated
/// document for a later import to trip on.
pub fn export_file(cat: &Catalog, path: &std::path::Path) -> Result<usize, Error> {
    let doc = export(cat);
    neowon_core::atomic_file::write(path, serde_json::to_vec_pretty(&doc)?)?;
    Ok(doc.entities.len())
}

/// Import a file written by `export_file`.
pub fn import_file(cat: &mut Catalog, path: &std::path::Path, at: &str) -> Result<Vec<Id>, Error> {
    let doc: Export = serde_json::from_slice(&std::fs::read(path)?)?;
    import(cat, &doc, at)
}
