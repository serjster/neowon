//! Mutations: what the WAL records and the state folds. Every op is
//! validated before it changes anything, so it applies whole or not at
//! all.

use serde::{Deserialize, Serialize};

use crate::model::{Entity, Id};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Add an entity; its id must be fresh.
    Insert {
        entity: Entity,
    },
    /// Bring back a deleted entity under its old id (undo of a delete):
    /// the id must be tombstoned, and the tombstone is lifted.
    Restore {
        entity: Entity,
    },
    /// Replace a live entity wholesale (same id and kind): the exact undo
    /// of any single-entity edit.
    Replace {
        entity: Entity,
    },
    /// Rename a signal/source/emitter; the old name becomes an alias.
    Rename {
        id: Id,
        name: String,
        at: String,
    },
    Alias {
        id: Id,
        name: String,
        at: String,
    },
    Unalias {
        id: Id,
        name: String,
    },
    Tag {
        id: Id,
        tag: String,
        on: bool,
    },
    Pin {
        id: Id,
        on: bool,
    },
    /// Set one editable field (not id, provenance, pinned, tags, aliases)
    /// to a JSON value of the right type.
    Edit {
        id: Id,
        field: String,
        value: serde_json::Value,
    },
    /// Fold signal `from` into `to`: its observations and transmissions
    /// move to `to`, its names become `to`'s aliases, and `from` leaves a
    /// redirect tombstone.
    Merge {
        from: Id,
        to: Id,
        at: String,
    },
    /// Remove one entity. Refused while pinned, and while anything
    /// references it unless `cascade`, which removes the referrers too.
    Delete {
        id: Id,
        cascade: bool,
        at: String,
    },
    /// Remove many signals, skipping pinned ones. Refused while any has
    /// observations unless `cascade`.
    Purge {
        ids: Vec<Id>,
        cascade: bool,
        at: String,
    },
}

impl Op {
    /// Ops a user can take back within a session.
    pub fn undoable(&self) -> bool {
        !matches!(self, Op::Merge { .. } | Op::Purge { .. })
    }
}
