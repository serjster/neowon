//! Catalog entities (D4/D7). Ids are opaque and immutable; every entity
//! carries provenance; confidence appears only on derived values.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An opaque, immutable identifier, never reused (the store's counter
/// only moves forward, and tombstones keep retired ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub u64);

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl std::str::FromStr for Id {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        s.trim_start_matches('#')
            .parse()
            .map(Id)
            .map_err(|_| format!("bad id {s:?}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProvKind {
    User,
    Decoder,
    Classifier,
    Db,
    Import,
    Merge,
    Fingerprint,
}

/// Where a record came from. `input_ref` names what it was derived from
/// (an id such as `#12`, a file, a capture); ids in it are resolved
/// through merge redirects on read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub kind: ProvKind,
    pub tool: String,
    pub tool_version: String,
    /// RFC 3339, UTC.
    pub timestamp: String,
    pub input_ref: Option<String>,
}

impl Provenance {
    pub fn user(timestamp: impl Into<String>) -> Self {
        Self {
            kind: ProvKind::User,
            tool: "neowon".into(),
            tool_version: env!("CARGO_PKG_VERSION").into(),
            timestamp: timestamp.into(),
            input_ref: None,
        }
    }
}

/// A former or alternative name, and when it was attached (a rename
/// keeps the old name as an alias edge).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alias {
    pub name: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub id: Id,
    pub name: String,
    pub centre_hz: f64,
    pub bandwidth_hz: f64,
    pub modulation: Option<String>,
    pub source: Option<Id>,
    pub tags: BTreeSet<String>,
    pub aliases: Vec<Alias>,
    pub notes: String,
    pub pinned: bool,
    /// Only for derived signals (classifier, fingerprint).
    pub confidence: Option<f64>,
    pub provenance: Provenance,
}

/// One on-air instance of a signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transmission {
    pub id: Id,
    pub signal: Id,
    pub t_start: f64,
    pub t_end: f64,
    pub provenance: Provenance,
}

/// Who or what transmits (a station, a service, a device class).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: Id,
    pub name: String,
    pub kind: String,
    pub tags: BTreeSet<String>,
    pub aliases: Vec<Alias>,
    pub notes: String,
    pub pinned: bool,
    pub provenance: Provenance,
}

/// A specific physical transmitter (10.7 fingerprinting attaches here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Emitter {
    pub id: Id,
    pub source: Option<Id>,
    pub name: String,
    pub aliases: Vec<Alias>,
    pub notes: String,
    pub pinned: bool,
    pub confidence: Option<f64>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandPlanEntry {
    pub id: Id,
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub service: String,
    /// ITU emission designator, when known (e.g. `16K0F3E`).
    pub designator: Option<String>,
    pub notes: String,
    pub pinned: bool,
    pub provenance: Provenance,
}

/// What a survey did to one band; an unscanned or truncated band leaves
/// its signals `unknown`, never `gone` (10.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandCoverage {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub scanned: bool,
    pub bins: usize,
    pub threshold_db: f64,
    pub truncated: bool,
    pub peak_cap: usize,
    pub selection_rule: String,
    pub retained_power_floor_dbfs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Survey {
    pub id: Id,
    pub name: String,
    pub started: String,
    pub coverage: Vec<BandCoverage>,
    pub pinned: bool,
    pub provenance: Provenance,
}

/// `neowon_core::SignalObservation`, as stored (core stays serde-free).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsRecord {
    pub centre_hz: f64,
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub power_dbfs: f64,
    pub snr_db: f64,
    pub t_start: f64,
    pub t_end: f64,
}

impl From<&neowon_core::SignalObservation> for ObsRecord {
    fn from(o: &neowon_core::SignalObservation) -> Self {
        Self {
            centre_hz: o.centre_hz,
            lo_hz: o.lo_hz,
            hi_hz: o.hi_hz,
            power_dbfs: o.power_dbfs,
            snr_db: o.snr_db,
            t_start: o.t_start,
            t_end: o.t_end,
        }
    }
}

impl From<&ObsRecord> for neowon_core::SignalObservation {
    fn from(o: &ObsRecord) -> Self {
        Self {
            centre_hz: o.centre_hz,
            lo_hz: o.lo_hz,
            hi_hz: o.hi_hz,
            power_dbfs: o.power_dbfs,
            snr_db: o.snr_db,
            t_start: o.t_start,
            t_end: o.t_end,
        }
    }
}

/// A `SignalObservation` filed against a signal (D7): identity plus
/// provenance around the record detection emitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: Id,
    pub signal: Id,
    pub transmission: Option<Id>,
    pub obs: ObsRecord,
    pub confidence: Option<f64>,
    pub provenance: Provenance,
}

/// Any entity, tagged in JSON by an `entity` key (not `kind`: a source
/// has a `kind` field of its own, and two `kind` keys do not round-trip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entity", rename_all = "snake_case")]
pub enum Entity {
    Signal(Signal),
    Transmission(Transmission),
    Source(Source),
    Emitter(Emitter),
    BandPlan(BandPlanEntry),
    Survey(Survey),
    Observation(Observation),
}

impl Entity {
    pub fn id(&self) -> Id {
        match self {
            Entity::Signal(e) => e.id,
            Entity::Transmission(e) => e.id,
            Entity::Source(e) => e.id,
            Entity::Emitter(e) => e.id,
            Entity::BandPlan(e) => e.id,
            Entity::Survey(e) => e.id,
            Entity::Observation(e) => e.id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Entity::Signal(_) => "signal",
            Entity::Transmission(_) => "transmission",
            Entity::Source(_) => "source",
            Entity::Emitter(_) => "emitter",
            Entity::BandPlan(_) => "band_plan",
            Entity::Survey(_) => "survey",
            Entity::Observation(_) => "observation",
        }
    }

    pub fn pinned(&self) -> bool {
        match self {
            Entity::Signal(e) => e.pinned,
            Entity::Source(e) => e.pinned,
            Entity::Emitter(e) => e.pinned,
            Entity::BandPlan(e) => e.pinned,
            Entity::Survey(e) => e.pinned,
            Entity::Transmission(_) | Entity::Observation(_) => false,
        }
    }

    pub fn provenance(&self) -> &Provenance {
        match self {
            Entity::Signal(e) => &e.provenance,
            Entity::Transmission(e) => &e.provenance,
            Entity::Source(e) => &e.provenance,
            Entity::Emitter(e) => &e.provenance,
            Entity::BandPlan(e) => &e.provenance,
            Entity::Survey(e) => &e.provenance,
            Entity::Observation(e) => &e.provenance,
        }
    }

    /// Every f64 field is finite (JSON cannot carry NaN or infinity, and a
    /// catalog value that is not a number is a bug upstream).
    pub fn finite(&self) -> bool {
        let all = |v: &[f64]| v.iter().all(|x| x.is_finite());
        let conf = |c: Option<f64>| c.is_none_or(f64::is_finite);
        match self {
            Entity::Signal(e) => all(&[e.centre_hz, e.bandwidth_hz]) && conf(e.confidence),
            Entity::Transmission(e) => all(&[e.t_start, e.t_end]),
            Entity::BandPlan(e) => all(&[e.lo_hz, e.hi_hz]),
            Entity::Survey(e) => e.coverage.iter().all(|c| {
                all(&[
                    c.lo_hz,
                    c.hi_hz,
                    c.threshold_db,
                    c.retained_power_floor_dbfs,
                ])
            }),
            Entity::Observation(e) => {
                let o = &e.obs;
                all(&[
                    o.centre_hz,
                    o.lo_hz,
                    o.hi_hz,
                    o.power_dbfs,
                    o.snr_db,
                    o.t_start,
                    o.t_end,
                ]) && conf(e.confidence)
            }
            Entity::Emitter(e) => conf(e.confidence),
            Entity::Source(_) => true,
        }
    }
}
