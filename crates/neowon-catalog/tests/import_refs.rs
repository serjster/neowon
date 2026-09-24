//! An import never invents, cross-wires or drops data.
//!
//! Every reference in an imported document either resolves to an entity
//! of the right kind *in that document*, or the whole import is refused
//! and names it. A document that is not an export (`format`) is refused
//! before anything is read into the catalog. What an entity was derived
//! from (`input_ref`) survives the import beside the import's own mark.

mod common;
use common::*;
use neowon_catalog::exchange::{self, Export};
use neowon_catalog::*;

/// A complete document: source, signal, transmission, observation (of the
/// signal, in the transmission) and an emitter of the source.
fn document() -> Export {
    // One directory per call: the tests run in parallel.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = scratch(&format!("import-refs-doc-{n}"));
    let mut cat = Catalog::open(&dir).unwrap();
    let so = add(&mut cat, source);
    let si = add(&mut cat, |id| signal(id, "REMOTE", 99.1e6, Some(so)));
    let tr = add(&mut cat, |id| transmission(id, si));
    add(&mut cat, |id| {
        let mut o = observation(id, si, 3.0);
        if let Entity::Observation(x) = &mut o {
            x.transmission = Some(tr);
        }
        o
    });
    add(&mut cat, |id| emitter(id, so));
    let doc = exchange::export(&cat);
    drop(cat);
    std::fs::remove_dir_all(&dir).unwrap();
    doc
}

/// A local catalog whose ids overlap the document's: `#1` is a local
/// source, `#2` a local signal "LOCAL", `#3` a local transmission — the
/// entities a reference missing from the document would otherwise bind to.
fn local(name: &str) -> (std::path::PathBuf, Catalog) {
    let dir = scratch(name);
    let mut cat = Catalog::open(&dir).unwrap();
    let so = add(&mut cat, source);
    let si = add(&mut cat, |id| signal(id, "LOCAL", 100e6, Some(so)));
    add(&mut cat, |id| transmission(id, si));
    (dir, cat)
}

fn without(doc: &Export, kind: &str) -> Export {
    Export {
        format: doc.format.clone(),
        schema: doc.schema,
        entities: doc
            .entities
            .iter()
            .filter(|e| e.kind() != kind)
            .cloned()
            .collect(),
    }
}

#[test]
fn a_reference_the_document_lacks_is_refused_not_bound_locally() {
    let doc = document();
    // Each row: the kind removed from the document, and which references
    // then point outside it.
    let cases = [
        ("signal", "observation's and transmission's signal"),
        ("source", "signal's and emitter's source"),
        ("transmission", "observation's transmission"),
    ];
    let mut failures = Vec::new();
    for (i, (kind, what)) in cases.iter().enumerate() {
        let (dir, mut cat) = local(&format!("import-refs-{i}"));
        let before = cat.state().clone();
        let seq = cat.seq();
        let bad = without(&doc, kind);
        assert!(
            !exchange::problems(&bad).is_empty(),
            "{kind}: the document's own integrity check must flag the {what}"
        );
        match exchange::import(&mut cat, &bad, AT) {
            Ok(ids) => {
                let bound: Vec<String> = cat
                    .state()
                    .entities
                    .values()
                    .filter(|e| ids.contains(&e.id()))
                    .map(|e| format!("{e:?}"))
                    .collect();
                failures.push(format!(
                    "no {kind} in the document: import accepted it and bound the {what} to local \
                     entities (integrity {:?}): {}",
                    cat.state().integrity(),
                    bound.join(" | ")
                ));
            }
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains("not in the document") {
                    failures.push(format!("no {kind}: refused, but not by name: {msg}"));
                }
                if cat.state() != &before || cat.seq() != seq {
                    failures.push(format!("no {kind}: refused, but the catalog changed"));
                }
            }
        }
        drop(cat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_whole_document_imports_and_passes_its_own_check() {
    let doc = document();
    assert_eq!(exchange::problems(&doc), Vec::<String>::new());
    let (dir, mut cat) = local("import-refs-whole");
    let ids = exchange::import(&mut cat, &doc, AT).unwrap();
    assert_eq!(ids.len(), 5);
    assert!(cat.state().integrity().is_empty());
    // The imported observation names the imported signal, never the local one.
    let st = cat.state();
    for e in st.entities.values().filter(|e| ids.contains(&e.id())) {
        if let Entity::Observation(o) = e {
            assert!(ids.contains(&o.signal), "observation bound to {}", o.signal);
            assert_eq!(st.signal(o.signal).unwrap().name, "REMOTE");
        }
    }
    drop(cat);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_reference_to_the_wrong_kind_or_a_repeated_id_is_refused() {
    let doc = document();
    let src_id = doc
        .entities
        .iter()
        .find(|e| e.kind() == "source")
        .unwrap()
        .id();
    // An observation whose signal is the document's source.
    let mut wrong = Export {
        format: doc.format.clone(),
        schema: doc.schema,
        entities: doc.entities.clone(),
    };
    for e in &mut wrong.entities {
        if let Entity::Observation(o) = e {
            o.signal = src_id;
        }
    }
    // Two entities under one id: which one a reference means is a guess.
    let mut twice = Export {
        format: doc.format.clone(),
        schema: doc.schema,
        entities: doc.entities.clone(),
    };
    let first = twice.entities[0].clone();
    twice.entities.push(first);
    for (name, bad) in [("wrong kind", wrong), ("repeated id", twice)] {
        assert!(!exchange::problems(&bad).is_empty(), "{name}: not flagged");
        let (dir, mut cat) = local(&format!("import-refs-{}", name.replace(' ', "-")));
        let before = cat.state().clone();
        assert!(
            exchange::import(&mut cat, &bad, AT).is_err(),
            "{name}: imported"
        );
        assert_eq!(cat.state(), &before, "{name}: the catalog changed");
        drop(cat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_document_that_is_not_an_export_is_refused_up_front() {
    let doc = document();
    let mut failures = Vec::new();
    for format in [
        "neowon-catalog",
        "sdrpp-bandplan",
        "",
        "neowon-catalog-export2",
    ] {
        let (dir, mut cat) = local(&format!("import-refs-format-{}", format.len()));
        let before = cat.state().clone();
        let bad = Export {
            format: format.into(),
            schema: doc.schema,
            entities: doc.entities.clone(),
        };
        match exchange::import(&mut cat, &bad, AT) {
            Ok(ids) => failures.push(format!(
                "format {format:?}: imported {} entities",
                ids.len()
            )),
            Err(e) if !e.to_string().contains("format") => failures.push(format!(
                "format {format:?}: refused, not by its format: {e}"
            )),
            Err(_) => assert_eq!(cat.state(), &before),
        }
        drop(cat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_import_keeps_what_each_entity_was_derived_from() {
    let doc = document();
    let (dir, mut cat) = local("import-refs-provenance");
    let ids = exchange::import(&mut cat, &doc, AT).unwrap();
    for (old, new) in doc.entities.iter().zip(&ids) {
        let p = cat.state().get(*new).unwrap().provenance().clone();
        assert_eq!(p.kind, ProvKind::Import);
        let r = p.input_ref.unwrap_or_default();
        assert!(
            r.starts_with(&format!("import:{}", old.id())),
            "{new}: {r:?} does not name the document's id"
        );
        assert!(
            r.contains("capture:unit"),
            "{new}: {r:?} dropped the original input_ref \"capture:unit\""
        );
    }
    drop(cat);
    std::fs::remove_dir_all(&dir).unwrap();
}
