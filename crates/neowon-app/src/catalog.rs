//! The signal catalog in the app (Phase 10.2): the open `neowon_catalog`,
//! its script actions, and its control-socket readouts. The catalog lives
//! at `NEOWON_CATALOG` or `~/.neowon/catalog` and is the single writer's:
//! a second app on the same directory is refused, not raced.
//!
//! `catalog add` with no arguments files the strongest live detection;
//! `catalog observe` files every active track against the catalogued
//! signal whose band it overlaps.

use bevy::log::{error, info};
use bevy::prelude::*;
use neowon_catalog::{
    Catalog, Entity, Id, ObsRecord, Observation, Op, ProvKind, Provenance, Signal,
};

use crate::Link;
use crate::sdr::SdrState;

#[derive(Resource)]
pub struct CatalogState {
    pub cat: Option<Catalog>,
    pub path: std::path::PathBuf,
    /// Case-insensitive substring the list shows (`catalog list <text>`).
    pub filter: String,
    pub window: bool,
    pub selected: Option<Id>,
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
        }
    }

    /// Live signals matching the filter, ordered by (creation time, id).
    pub fn signals(&self) -> Vec<&Signal> {
        let Some(cat) = &self.cat else {
            return Vec::new();
        };
        let f = self.filter.to_lowercase();
        let mut v: Vec<&Signal> = cat
            .state()
            .entities
            .values()
            .filter_map(|e| match e {
                Entity::Signal(s) => Some(s),
                _ => None,
            })
            .filter(|s| {
                f.is_empty()
                    || s.name.to_lowercase().contains(&f)
                    || s.tags.iter().any(|t| t.to_lowercase().contains(&f))
                    || s.aliases.iter().any(|a| a.name.to_lowercase().contains(&f))
            })
            .collect();
        v.sort_by(|a, b| {
            a.provenance
                .timestamp
                .cmp(&b.provenance.timestamp)
                .then(a.id.cmp(&b.id))
        });
        v
    }

    pub fn observations_of(&self, id: Id) -> usize {
        self.cat
            .as_ref()
            .and_then(|c| c.state().history(id).ok())
            .map_or(0, |h| h.len())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogAction {
    List(String),
    /// A signal: explicit (`centre_hz`, optional name) or, with no
    /// arguments, the strongest live detection.
    Add(Option<(f64, String)>),
    Observe,
    Rename(Id, String),
    Delete(Id, bool),
    Purge(Vec<Id>, bool),
    Merge(Id, Id),
    Tag(Id, String, bool),
    Alias(Id, String),
    Edit(Id, String, String),
    /// `bulk tag|untag|pin|unpin|delete <ids> [tag]`.
    Bulk(String, Vec<Id>, String),
    Undo,
    Pin(Id, bool),
    Export(String),
    Import(String),
    Window(bool),
    Select(Option<Id>),
}

fn ids(s: &str) -> Result<Vec<Id>, String> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|x| x.parse())
        .collect()
}

/// `catalog <verb> …` (the words after `catalog`).
pub fn parse<'a>(
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<CatalogAction, String> {
    let verb = next()?;
    let mut rest = Vec::new();
    while let Ok(w) = next() {
        rest.push(w);
    }
    let arg = |i: usize| {
        rest.get(i)
            .copied()
            .ok_or_else(|| format!("catalog {verb}: missing argument"))
    };
    let tail = |i: usize| rest.get(i..).map(|r| r.join(" ")).unwrap_or_default();
    let id = |i: usize| -> Result<Id, String> { arg(i)?.parse() };
    let cascade = || rest.contains(&"cascade");
    Ok(match verb {
        "list" => CatalogAction::List(tail(0)),
        "add" if rest.is_empty() => CatalogAction::Add(None),
        "add" => CatalogAction::Add(Some((crate::sdr::parse_hz(arg(0)?)?, tail(1)))),
        "observe" => CatalogAction::Observe,
        "rename" => CatalogAction::Rename(id(0)?, tail(1)),
        "delete" => CatalogAction::Delete(id(0)?, cascade()),
        "purge" => CatalogAction::Purge(ids(arg(0)?)?, cascade()),
        "merge" => CatalogAction::Merge(id(0)?, id(1)?),
        "tag" => CatalogAction::Tag(id(0)?, arg(1)?.to_string(), true),
        "untag" => CatalogAction::Tag(id(0)?, arg(1)?.to_string(), false),
        "alias" => CatalogAction::Alias(id(0)?, tail(1)),
        "edit" => CatalogAction::Edit(id(0)?, arg(1)?.to_string(), tail(2)),
        "bulk" => CatalogAction::Bulk(arg(0)?.to_string(), ids(arg(1)?)?, tail(2)),
        "undo" => CatalogAction::Undo,
        "pin" => CatalogAction::Pin(id(0)?, true),
        "unpin" => CatalogAction::Pin(id(0)?, false),
        "export" => CatalogAction::Export(tail(0)),
        "import" => CatalogAction::Import(tail(0)),
        "window" => CatalogAction::Window(matches!(arg(0)?, "on" | "1")),
        "select" => CatalogAction::Select(arg(0)?.parse().ok()),
        other => return Err(format!("unknown catalog verb {other:?}")),
    })
}

fn user(at: &str) -> Provenance {
    Provenance::user(at)
}

/// A signal and its first observation from a detector track.
fn from_track(cat: &mut Catalog, t: &neowon_dsp::Track, at: &str) -> Result<Id, String> {
    let o = &t.last;
    let id = cat.next_id();
    let name = format!("{:.4} MHz", o.centre_hz / 1e6);
    let mut p = Provenance::user(at);
    p.kind = ProvKind::Decoder;
    p.tool = "neowon detect".into();
    p.input_ref = Some(format!("track:{}", t.id));
    commit(
        cat,
        Op::Insert {
            entity: Entity::Signal(Signal {
                id,
                name,
                centre_hz: o.centre_hz,
                bandwidth_hz: o.bandwidth_hz(),
                modulation: None,
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
                    let t = sdr
                        .tracker
                        .active()
                        .max_by(|a, b| a.last.power_dbfs.total_cmp(&b.last.power_dbfs))
                        .ok_or("no active detection to add")?
                        .clone();
                    let id = from_track(cat, &t, &at)?;
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
                        commit(cat, op)?;
                    }
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

fn esc(s: &str) -> String {
    format!("\"{}\"", crate::control::escape(s))
}

/// `get catalog`: the listed signals and the catalog's health.
pub fn catalog_json(st: &CatalogState) -> String {
    let Some(cat) = &st.cat else {
        return format!(
            r#"{{"ok":false,"error":"no catalog open at {}"}}"#,
            st.path.display()
        );
    };
    let rows: Vec<String> = st
        .signals()
        .iter()
        .map(|s| {
            let tags: Vec<String> = s.tags.iter().map(|t| esc(t)).collect();
            let aliases: Vec<String> = s.aliases.iter().map(|a| esc(&a.name)).collect();
            format!(
                concat!(
                    r#"{{"id":{},"name":{},"centre_hz":{},"bandwidth_hz":{},"tags":[{}],"#,
                    r#""aliases":[{}],"pinned":{},"observations":{}}}"#
                ),
                s.id.0,
                esc(&s.name),
                s.centre_hz,
                s.bandwidth_hz,
                tags.join(","),
                aliases.join(","),
                s.pinned,
                st.observations_of(s.id)
            )
        })
        .collect();
    format!(
        r#"{{"ok":true,"path":{},"seq":{},"entities":{},"integrity":{},"signals":[{}]}}"#,
        esc(&st.path.display().to_string()),
        cat.seq(),
        cat.state().entities.len(),
        cat.state().integrity().len(),
        rows.join(",")
    )
}

/// `get history <id>`: a signal's observations, redirects followed.
pub fn history_json(st: &CatalogState, id: &str) -> String {
    let (Some(cat), Ok(id)) = (&st.cat, id.parse::<Id>()) else {
        return r#"{"ok":false,"error":"no catalog, or a bad id"}"#.into();
    };
    match cat.state().history(id) {
        Ok(h) => {
            let rows: Vec<String> = h
                .iter()
                .map(|o| {
                    format!(
                        r#"{{"id":{},"signal":{},"t_start":{},"centre_hz":{},"power_dbfs":{},"snr_db":{}}}"#,
                        o.id.0, o.signal.0, o.obs.t_start, o.obs.centre_hz, o.obs.power_dbfs, o.obs.snr_db
                    )
                })
                .collect();
            format!(
                r#"{{"ok":true,"canonical":{},"rows":[{}]}}"#,
                cat.state().resolve(id).map_or(0, |i| i.0),
                rows.join(",")
            )
        }
        Err(e) => format!(r#"{{"ok":false,"error":{}}}"#, esc(&e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words<'a>(s: &'a str) -> impl FnMut() -> Result<&'a str, String> {
        let mut w = s.split_whitespace();
        move || w.next().ok_or_else(|| "missing argument".to_string())
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(parse(&mut words("add")).unwrap(), CatalogAction::Add(None));
        assert_eq!(
            parse(&mut words("add 99.4M BBC Radio 2")).unwrap(),
            CatalogAction::Add(Some((99.4e6, "BBC Radio 2".into())))
        );
        assert_eq!(
            parse(&mut words("rename #3 Radio Two")).unwrap(),
            CatalogAction::Rename(Id(3), "Radio Two".into())
        );
        assert_eq!(
            parse(&mut words("purge 1,2,3 cascade")).unwrap(),
            CatalogAction::Purge(vec![Id(1), Id(2), Id(3)], true)
        );
        assert_eq!(
            parse(&mut words("merge 4 #5")).unwrap(),
            CatalogAction::Merge(Id(4), Id(5))
        );
        assert_eq!(
            parse(&mut words("bulk tag 1,2 fm")).unwrap(),
            CatalogAction::Bulk("tag".into(), vec![Id(1), Id(2)], "fm".into())
        );
        assert!(parse(&mut words("frobnicate")).is_err());
        assert!(parse(&mut words("merge 4")).is_err());
    }
}
