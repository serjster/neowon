//! The signal catalog in the app: the open `neowon_catalog`,
//! its script actions, and its control-socket readouts. The catalog lives
//! at `NEOWON_CATALOG` or `~/.neowon/catalog` and is the single writer's:
//! a second app on the same directory is refused, not raced.
//!
//! `catalog add` with no arguments files the channel at the tuned
//! frequency: the active detection covering it (its measured centre,
//! occupied bandwidth and modulation) when there is one, else the tuned
//! frequency itself with the manual channel width and no confidence.
//! `catalog observe` files every active track against the catalogued
//! signal whose band it overlaps.

use bevy::log::{error, info};
use bevy::prelude::*;
use neowon_catalog::{
    Catalog, Entity, Id, ObsRecord, Observation, Op, ProvKind, Provenance, Signal,
};

use crate::Link;
use crate::sdr::SdrState;

mod grammar;
#[cfg(test)]
mod join_tests;
mod listing;
mod readout;
pub use grammar::{CatalogAction, parse};
pub use readout::{catalog_json, history_json};

#[derive(Resource)]
pub struct CatalogState {
    pub cat: Option<Catalog>,
    pub path: std::path::PathBuf,
    /// Case-insensitive substring the list shows (`catalog list <text>`).
    pub filter: String,
    pub window: bool,
    pub selected: Option<Id>,
    /// What the window reads each frame, rebuilt only when the catalog or
    /// the filter changes.
    listing: listing::Listing,
}

impl CatalogState {
    pub fn open_from_env() -> Self {
        let path = std::env::var_os("NEOWON_CATALOG")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".neowon/catalog"))
            })
            .unwrap_or_else(|| "neowon-catalog".into());
        let cat = match Catalog::open(&path) {
            Ok(c) => {
                info!(
                    "catalog: {} ({} entities)",
                    path.display(),
                    c.state().entities.len()
                );
                Some(c)
            }
            Err(e) => {
                error!("catalog: cannot open {}: {e}", path.display());
                None
            }
        };
        Self {
            cat,
            path,
            filter: String::new(),
            window: false,
            selected: None,
            listing: Default::default(),
        }
    }

    /// Live signals matching the filter, ordered by (creation time, id).
    pub fn signals(&self) -> Vec<&Signal> {
        let Some(cat) = &self.cat else {
            return Vec::new();
        };
        self.listing
            .ids(cat, &self.filter)
            .into_iter()
            .filter_map(|id| cat.state().signal(id))
            .collect()
    }

    /// The first `limit` listed signals and how many are listed: a frame's
    /// worth, bounded whatever the catalog holds.
    pub fn signals_page(&self, limit: usize) -> (usize, Vec<&Signal>) {
        let Some(cat) = &self.cat else {
            return (0, Vec::new());
        };
        let (total, ids) = self.listing.page(cat, &self.filter, limit);
        let page = ids
            .into_iter()
            .filter_map(|id| cat.state().signal(id))
            .collect();
        (total, page)
    }

    pub fn observations_of(&self, id: Id) -> usize {
        let Some(cat) = &self.cat else { return 0 };
        self.listing
            .observations(cat, &self.filter, id)
            .unwrap_or_else(|| cat.state().history(id).map_or(0, |h| h.len()))
    }

    /// The catalog's integrity problem count (0 when sound or none open).
    pub fn integrity_problems(&self) -> usize {
        self.cat
            .as_ref()
            .map_or(0, |cat| self.listing.problems(cat, &self.filter))
    }
}

fn user(at: &str) -> Provenance {
    Provenance::user(at)
}

/// A signal from the channel at the tuned frequency: the active detection
/// covering it (measured centre, occupied bandwidth and modulation) when
/// there is one, else the operator's tuned frequency with the manual
/// channel width. The tuned frequency chooses the signal; the hardware
/// window's centre is never the recorded value.
fn from_tuned(cat: &mut Catalog, sdr: &SdrState, at: &str) -> Result<Id, String> {
    let hit = sdr
        .nearest_track()
        .filter(|t| {
            let half = (t.last.bandwidth_hz() / 2.0).max(1.0);
            (sdr.tuned_hz - t.last.centre_hz).abs() <= half
        })
        .map(|t| (t.last.centre_hz, t.last.bandwidth_hz(), t.id));
    let (centre_hz, bandwidth_hz, modulation, p) = match hit {
        Some((centre_hz, bandwidth_hz, id)) => {
            let mut p = Provenance::user(at);
            p.kind = ProvKind::Decoder;
            p.tool = "neowon detect".into();
            p.input_ref = Some(format!("track:{id}"));
            // The classifier's verdict only when it judged this very track.
            let modulation = sdr.modulation.map(|m| m.label().to_string()).or_else(|| {
                sdr.classification_of(id)
                    .filter(|c| !c.verdict.unknown)
                    .map(|c| c.verdict.class.label().to_string())
            });
            (centre_hz, bandwidth_hz, modulation, p)
        }
        None => {
            let mut p = Provenance::user(at);
            p.tool = "neowon sdr channel".into();
            (sdr.tuned_hz, sdr.channel_width(), None, p)
        }
    };
    let id = cat.next_id();
    let name = format!("{:.4} MHz", centre_hz / 1e6);
    commit(
        cat,
        Op::Insert {
            entity: Entity::Signal(Signal {
                id,
                name,
                centre_hz,
                bandwidth_hz,
                modulation,
                source: None,
                tags: Default::default(),
                aliases: Vec::new(),
                notes: String::new(),
                pinned: false,
                confidence: None,
                provenance: p,
            }),
        },
    )?;
    Ok(id)
}

fn commit(cat: &mut Catalog, op: Op) -> Result<(), String> {
    cat.commit(op).map(|_| ()).map_err(|e| e.to_string())
}

fn observe(cat: &mut Catalog, sdr: &SdrState, at: &str) -> Result<usize, String> {
    let mut filed = 0;
    let tracks: Vec<_> = sdr.tracker.active().cloned().collect();
    for t in tracks {
        let o = &t.last;
        // The catalogued signal whose band overlaps the track's, nearest
        // centre first.
        let hit = cat
            .state()
            .entities
            .values()
            .filter_map(|e| match e {
                Entity::Signal(s) => Some(s),
                _ => None,
            })
            .filter(|s| {
                let half = (s.bandwidth_hz / 2.0).max(1.0);
                o.lo_hz <= s.centre_hz + half && o.hi_hz >= s.centre_hz - half
            })
            .min_by(|a, b| {
                (a.centre_hz - o.centre_hz)
                    .abs()
                    .total_cmp(&(b.centre_hz - o.centre_hz).abs())
            })
            .map(|s| s.id);
        let Some(signal) = hit else { continue };
        let id = cat.next_id();
        let mut p = Provenance::user(at);
        p.kind = ProvKind::Decoder;
        p.tool = "neowon detect".into();
        p.input_ref = Some(format!("track:{}", t.id));
        commit(
            cat,
            Op::Insert {
                entity: Entity::Observation(Observation {
                    id,
                    signal,
                    transmission: None,
                    obs: ObsRecord::from(o),
                    confidence: None,
                    provenance: p,
                }),
            },
        )?;
        filed += 1;
    }
    Ok(filed)
}

/// Apply one action; errors go to the log and the status line.
pub fn run(a: CatalogAction, st: &mut CatalogState, sdr: &SdrState, link: &mut Link) {
    if let Err(e) = apply(a, st, sdr) {
        error!("script: catalog: {e}");
        link.status = format!("error: {e}");
    }
}

fn apply(a: CatalogAction, st: &mut CatalogState, sdr: &SdrState) -> Result<(), String> {
    match a {
        CatalogAction::List(f) => st.filter = f,
        CatalogAction::Window(on) => st.window = on,
        CatalogAction::Select(id) => st.selected = id,
        a => {
            let at = neowon_catalog::now_rfc3339();
            let cat = st
                .cat
                .as_mut()
                .ok_or_else(|| format!("no catalog open at {}", st.path.display()))?;
            match a {
                CatalogAction::Add(None) => {
                    let id = from_tuned(cat, sdr, &at)?;
                    st.selected = Some(id);
                }
                CatalogAction::Add(Some((hz, name))) => {
                    let id = cat.next_id();
                    let name = if name.is_empty() {
                        format!("{:.4} MHz", hz / 1e6)
                    } else {
                        name
                    };
                    commit(
                        cat,
                        Op::Insert {
                            entity: Entity::Signal(Signal {
                                id,
                                name,
                                centre_hz: hz,
                                bandwidth_hz: 0.0,
                                modulation: None,
                                source: None,
                                tags: Default::default(),
                                aliases: Vec::new(),
                                notes: String::new(),
                                pinned: false,
                                confidence: None,
                                provenance: user(&at),
                            }),
                        },
                    )?;
                    st.selected = Some(id);
                }
                CatalogAction::Survey(name) => {
                    let r = sdr.surveys.last().ok_or("no completed survey to file")?;
                    let id = cat.next_id();
                    let name = if name.is_empty() {
                        format!(
                            "{:.3}–{:.3} MHz",
                            r.plan.start_hz / 1e6,
                            r.plan.stop_hz / 1e6
                        )
                    } else {
                        name
                    };
                    commit(
                        cat,
                        Op::Insert {
                            entity: Entity::Survey(neowon_catalog::Survey {
                                id,
                                name,
                                started: at.clone(),
                                coverage: r.coverage.iter().map(Into::into).collect(),
                                pinned: false,
                                provenance: user(&at),
                            }),
                        },
                    )?;
                }
                CatalogAction::Observe => {
                    let n = observe(cat, sdr, &at)?;
                    info!("catalog: filed {n} observations");
                }
                CatalogAction::Rename(id, name) => commit(cat, Op::Rename { id, name, at })?,
                CatalogAction::Delete(id, cascade) => commit(cat, Op::Delete { id, cascade, at })?,
                CatalogAction::Purge(ids, cascade) => commit(cat, Op::Purge { ids, cascade, at })?,
                CatalogAction::Merge(from, to) => commit(cat, Op::Merge { from, to, at })?,
                CatalogAction::Tag(id, tag, on) => commit(cat, Op::Tag { id, tag, on })?,
                CatalogAction::Alias(id, name) => commit(cat, Op::Alias { id, name, at })?,
                CatalogAction::Edit(id, field, text) => {
                    commit(cat, neowon_catalog::edit_from_text(id, &field, &text))?
                }
                CatalogAction::Bulk(verb, ids, arg) => {
                    // One fsync for the batch, not one per id.
                    let mut ops = Vec::with_capacity(ids.len());
                    for id in ids {
                        let op = match verb.as_str() {
                            "tag" => Op::Tag {
                                id,
                                tag: arg.clone(),
                                on: true,
                            },
                            "untag" => Op::Tag {
                                id,
                                tag: arg.clone(),
                                on: false,
                            },
                            "pin" => Op::Pin { id, on: true },
                            "unpin" => Op::Pin { id, on: false },
                            "delete" => Op::Delete {
                                id,
                                cascade: arg == "cascade",
                                at: at.clone(),
                            },
                            v => return Err(format!("bulk {v}: use tag|untag|pin|unpin|delete")),
                        };
                        ops.push(op);
                    }
                    cat.commit_many(ops).map_err(|e| e.to_string())?;
                }
                CatalogAction::Undo => {
                    cat.undo()
                        .map_err(|e| e.to_string())?
                        .ok_or("nothing to undo")?;
                }
                CatalogAction::Pin(id, on) => commit(cat, Op::Pin { id, on })?,
                CatalogAction::Export(path) => {
                    let n = neowon_catalog::exchange::export_file(cat, path.as_ref())
                        .map_err(|e| format!("{path}: {e}"))?;
                    info!("catalog: exported {n} entities to {path}");
                }
                CatalogAction::Import(path) => {
                    let ids = neowon_catalog::exchange::import_file(cat, path.as_ref(), &at)
                        .map_err(|e| format!("{path}: {e}"))?;
                    info!("catalog: imported {} entities from {path}", ids.len());
                }
                CatalogAction::List(_) | CatalogAction::Window(_) | CatalogAction::Select(_) => {
                    unreachable!("handled above")
                }
            }
        }
    }
    Ok(())
}
