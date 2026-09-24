//! Shared builders: one of every entity with every field populated, so a
//! round-trip that drops or rounds anything is caught.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use neowon_catalog::*;

pub const AT: &str = "2026-09-18T21:15:07Z";

/// A fresh, empty directory under the system temp dir.
pub fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("neowon-catalog-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

pub fn prov(kind: ProvKind) -> Provenance {
    Provenance {
        kind,
        tool: "test".into(),
        tool_version: "1.2.3".into(),
        timestamp: AT.into(),
        input_ref: Some("capture:unit".into()),
    }
}

pub fn tags(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn source(id: Id) -> Entity {
    Entity::Source(Source {
        id,
        name: "Radio One".into(),
        kind: "broadcast".into(),
        tags: tags(&["fm", "public"]),
        aliases: vec![Alias {
            name: "R1".into(),
            at: AT.into(),
        }],
        notes: "national".into(),
        pinned: false,
        provenance: prov(ProvKind::Db),
    })
}

pub fn signal(id: Id, name: &str, centre_hz: f64, source: Option<Id>) -> Entity {
    Entity::Signal(Signal {
        id,
        name: name.into(),
        centre_hz,
        // Awkward binary fractions: they must survive JSON exactly.
        bandwidth_hz: 115_123.456_789_012_3,
        modulation: Some("WFM".into()),
        source,
        tags: tags(&["broadcast"]),
        aliases: vec![],
        notes: "strong".into(),
        pinned: false,
        confidence: Some(0.873_125_000_000_001),
        provenance: prov(ProvKind::Classifier),
    })
}

pub fn transmission(id: Id, signal: Id) -> Entity {
    Entity::Transmission(Transmission {
        id,
        signal,
        t_start: 1.0 / 3.0,
        t_end: 2.0 / 3.0,
        provenance: prov(ProvKind::Decoder),
    })
}

pub fn observation(id: Id, signal: Id, t: f64) -> Entity {
    Entity::Observation(Observation {
        id,
        signal,
        transmission: None,
        obs: ObsRecord {
            centre_hz: 99.405_4e6 + t,
            lo_hz: 99.35e6,
            hi_hz: 99.46e6,
            power_dbfs: -25.2,
            snr_db: 24.5,
            t_start: t,
            t_end: t + 0.064,
        },
        confidence: None,
        provenance: prov(ProvKind::User),
    })
}

pub fn emitter(id: Id, source: Id) -> Entity {
    Entity::Emitter(Emitter {
        id,
        source: Some(source),
        name: "Tx site A".into(),
        aliases: vec![],
        notes: "".into(),
        pinned: true,
        confidence: Some(0.5),
        provenance: prov(ProvKind::Fingerprint),
    })
}

pub fn band_plan(id: Id) -> Entity {
    Entity::BandPlan(BandPlanEntry {
        id,
        lo_hz: 87.5e6,
        hi_hz: 108e6,
        service: "FM broadcast".into(),
        designator: Some("256KF8EHF".into()),
        notes: "ITU region 1".into(),
        pinned: false,
        provenance: prov(ProvKind::Import),
    })
}

pub fn survey(id: Id) -> Entity {
    Entity::Survey(Survey {
        id,
        name: "FM sweep".into(),
        started: AT.into(),
        coverage: vec![CoverageRecord {
            lo_hz: 88e6,
            hi_hz: 108e6,
            scanned: true,
            bins: 4096,
            threshold_db: 12.0,
            truncated: false,
            peak_cap: 64,
            selection_rule: "strongest".into(),
            retained_power_floor_dbfs: -80.5,
        }],
        pinned: false,
        provenance: prov(ProvKind::Merge),
    })
}

pub fn insert(cat: &mut Catalog, e: Entity) -> Id {
    let id = e.id();
    cat.commit(Op::Insert { entity: e }).expect("insert");
    id
}

/// Insert the entity `make` builds around a fresh id.
pub fn add(cat: &mut Catalog, make: impl FnOnce(Id) -> Entity) -> Id {
    let id = cat.next_id();
    insert(cat, make(id))
}
