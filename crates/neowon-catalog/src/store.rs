//! The on-disk catalog: a manifest, one snapshot, and WAL segments, owned
//! by a single writer (an exclusive file lock).
//!
//! ```text
//! <dir>/lock              exclusive lock: one writer per catalog
//! <dir>/manifest.json     {"format":"neowon-catalog","schema":1,
//!                          "last_seq":N,"snapshot":…,"wal":…}
//! <dir>/snapshot-N.json   the state as of seq N
//! <dir>/wal-M.log         records after it (framed, fsynced)
//! ```
//!
//! Durability: a commit returns only after its WAL frame is fsynced, so
//! an acknowledged write survives any crash. Replay applies only records
//! above the manifest's `last_seq`, so replaying twice is harmless. A
//! checkpoint writes the new snapshot and WAL segment first, then swaps
//! the manifest (fsync temp → rename → fsync directory): a crash anywhere
//! leaves either the old manifest (old snapshot + old WAL, all still
//! present) or the new one (both complete).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::migrate;
use crate::op::Op;
use crate::state::State;
use crate::wal::{self, Record, Writer, failpoint};
use crate::{Error, FORMAT, SCHEMA};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    /// Absent in v0 catalogs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    pub last_seq: u64,
    pub snapshot: Option<String>,
    pub wal: String,
}

pub struct Catalog {
    dir: PathBuf,
    state: State,
    manifest: Manifest,
    wal: Writer,
    seq: u64,
    since_checkpoint: usize,
    /// Checkpoint after this many commits (and on close).
    pub checkpoint_every: usize,
    undo: Vec<Op>,
    /// Set when a commit changed memory but could not reach disk: the
    /// in-memory state is then ahead of what a reopen would see, so every
    /// further operation is refused until the catalog is reopened.
    poisoned: bool,
    _lock: File,
}

fn fsync_dir(dir: &Path) -> Result<(), Error> {
    // Makes renames and creations durable (a no-op where the platform
    // cannot open directories).
    if let Ok(d) = File::open(dir) {
        d.sync_all()?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: temp file, fsync, rename, fsync
/// the directory. The failpoints sit on either side of the rename.
fn replace_file(dir: &Path, name: &str, bytes: &[u8], failpoints: bool) -> Result<(), Error> {
    let tmp = dir.join(format!("{name}.tmp"));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    if failpoints {
        failpoint("before-rename");
    }
    std::fs::rename(&tmp, dir.join(name))?;
    if failpoints {
        failpoint("after-rename-before-fsync");
    }
    fsync_dir(dir)
}

fn segment_no(name: &str) -> Option<u64> {
    name.strip_prefix("wal-")?
        .strip_suffix(".log")?
        .parse()
        .ok()
}

impl Catalog {
    /// Open (creating if absent) the catalog in `dir`, replaying its WAL.
    /// A v0 catalog is migrated and re-saved at the current schema; a
    /// newer schema is refused.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))?;
        lock.try_lock().map_err(|_| Error::Locked(dir.clone()))?;

        let manifest_path = dir.join("manifest.json");
        let (mut manifest, fresh) = if manifest_path.exists() {
            let m: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
            if m.format != FORMAT {
                return Err(Error::Corrupt(format!(
                    "not a catalog: format {:?}",
                    m.format
                )));
            }
            (m, false)
        } else {
            let m = Manifest {
                format: FORMAT.into(),
                schema: Some(SCHEMA),
                last_seq: 0,
                snapshot: None,
                wal: "wal-1.log".into(),
            };
            (m, true)
        };
        let schema = manifest.schema.unwrap_or(0);
        if schema > SCHEMA {
            return Err(Error::TooNew(schema));
        }

        let mut state = match &manifest.snapshot {
            Some(name) => {
                let bytes = std::fs::read(dir.join(name))?;
                if schema == 0 {
                    migrate::snapshot_v0(&bytes)?
                } else {
                    serde_json::from_slice(&bytes)?
                }
            }
            None => State::default(),
        };

        // Replay every segment in order; the watermark skips what the
        // snapshot already holds, so leftovers from an interrupted
        // checkpoint are harmless.
        let mut segments: Vec<(u64, String)> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| Some((segment_no(&n)?, n)))
            .collect();
        segments.sort();
        let mut seq = manifest.last_seq;
        let mut current_len = None;
        for (_, name) in &segments {
            let (header, records, valid) = wal::read(&dir.join(name))?;
            if header.format != wal::WAL_FORMAT || header.schema > SCHEMA {
                return Err(Error::TooNew(header.schema));
            }
            for r in records {
                if r.seq <= seq {
                    continue;
                }
                state
                    .apply(&r.op)
                    .map_err(|e| Error::Corrupt(format!("{name}: replaying seq {}: {e}", r.seq)))?;
                seq = r.seq;
            }
            if *name == manifest.wal {
                current_len = Some(valid);
            }
        }

        let wal = match current_len {
            Some(valid) => Writer::open(&dir.join(&manifest.wal), valid)?,
            None => {
                let w = Writer::create(&dir.join(&manifest.wal), SCHEMA)?;
                fsync_dir(&dir)?;
                w
            }
        };
        if fresh {
            replace_file(
                &dir,
                "manifest.json",
                &serde_json::to_vec_pretty(&manifest)?,
                false,
            )?;
        }
        let migrated = schema < SCHEMA;
        manifest.schema = Some(schema);
        let mut cat = Self {
            dir,
            state,
            manifest,
            wal,
            seq,
            since_checkpoint: 0,
            checkpoint_every: 1000,
            undo: Vec::new(),
            poisoned: false,
            _lock: lock,
        };
        if migrated {
            // Re-save at the current schema: the v0 tail is folded into a
            // v1 snapshot rather than left in an old-format segment.
            cat.checkpoint()?;
        }
        Ok(cat)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A fresh id for a new entity.
    pub fn next_id(&mut self) -> crate::Id {
        self.state.alloc_id()
    }

    /// Validate, apply and durably log `op`; returns its sequence number
    /// once it is on disk.
    pub fn commit(&mut self, op: Op) -> Result<u64, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let inverse = self.state.inverse(&op);
        self.state.apply(&op)?;
        let rec = Record {
            seq: self.seq + 1,
            op,
        };
        if let Err(e) = self.wal.append(&rec) {
            self.poisoned = true;
            return Err(e);
        }
        self.seq = rec.seq;
        if rec.op.undoable() {
            self.undo.extend(inverse);
        } else {
            // History before a merge or purge cannot be taken back safely.
            self.undo.clear();
        }
        self.since_checkpoint += 1;
        if self.since_checkpoint >= self.checkpoint_every {
            self.checkpoint()?;
        }
        Ok(rec.seq)
    }

    /// Take back the last undoable op of this session (logged as a new
    /// op, so the WAL stays append-only). `Ok(None)` when there is none.
    pub fn undo(&mut self) -> Result<Option<u64>, Error> {
        let Some(op) = self.undo.pop() else {
            return Ok(None);
        };
        let before = self.undo.len();
        let seq = self.commit(op)?;
        // The inverse's own inverse is not a redo entry.
        self.undo.truncate(before);
        Ok(Some(seq))
    }

    /// Fold the WAL into a new snapshot and start a fresh segment.
    pub fn checkpoint(&mut self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let snap = format!("snapshot-{}.json", self.seq);
        replace_file(&self.dir, &snap, &serde_json::to_vec(&self.state)?, false)?;
        let next_seg = segment_no(&self.manifest.wal).unwrap_or(0) + 1;
        let seg = format!("wal-{next_seg}.log");
        let _ = std::fs::remove_file(self.dir.join(&seg)); // leftover from an interrupted checkpoint
        let wal = Writer::create(&self.dir.join(&seg), SCHEMA)?;
        fsync_dir(&self.dir)?;
        let manifest = Manifest {
            format: FORMAT.into(),
            schema: Some(SCHEMA),
            last_seq: self.seq,
            snapshot: Some(snap.clone()),
            wal: seg.clone(),
        };
        replace_file(
            &self.dir,
            "manifest.json",
            &serde_json::to_vec_pretty(&manifest)?,
            true,
        )?;
        // Committed: the old snapshot and segments are no longer needed.
        for e in std::fs::read_dir(&self.dir)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let stale_seg = segment_no(&name).is_some() && name != seg;
            let stale_snap = name.starts_with("snapshot-") && name != snap;
            if stale_seg || stale_snap {
                let _ = std::fs::remove_file(e.path());
            }
        }
        self.manifest = manifest;
        self.wal = wal;
        self.since_checkpoint = 0;
        Ok(())
    }

    /// Checkpoint and release the lock.
    pub fn close(mut self) -> Result<(), Error> {
        self.checkpoint()
    }
}
