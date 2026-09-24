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

use neowon_core::atomic_file::AtomicFile;
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

/// Write `bytes` to `path` atomically (`neowon_core::atomic_file`: temp,
/// fsync, rename), then fsync the directory. The failpoints sit on either
/// side of the rename.
fn replace_file(dir: &Path, name: &str, bytes: &[u8], failpoints: bool) -> Result<(), Error> {
    let mut f = AtomicFile::create(dir.join(name))?;
    f.write_all(bytes)?;
    f.flush()?;
    if failpoints {
        failpoint("before-rename");
    }
    f.commit()?;
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

fn snapshot_no(name: &str) -> Option<u64> {
    name.strip_prefix("snapshot-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// A temp `replace_file` left for one of this store's own files
/// (`.<target>.<pid>-<n>.tmp`, `neowon_core::atomic_file`'s pattern), and
/// nothing that merely ends in `.tmp`.
fn own_temp(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    let Some((target, tag)) = rest.rsplit_once('.') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    neowon_core::atomic_file::is_temp(name)
        && tag
            .split_once('-')
            .is_some_and(|(a, b)| digits(a) && digits(b))
        && (target == "manifest.json" || snapshot_no(target).is_some())
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
            let Some((header, records, valid)) = wal::read(&dir.join(name))? else {
                // A torn header: `Writer::create` died before the header
                // was whole, so no record was ever appended. Not the
                // manifest's segment: a checkpoint was interrupted; set it
                // aside (the next checkpoint numbers past it and sweeps
                // it). The manifest's own: recreated below.
                if *name == manifest.wal {
                    std::fs::remove_file(dir.join(name))?;
                }
                continue;
            };
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

    pub fn next_id(&mut self) -> crate::Id {
        self.state.alloc_id()
    }

    /// Validate, apply and durably log `op`; returns its sequence number
    /// once it is on disk.
    pub fn commit(&mut self, op: Op) -> Result<u64, Error> {
        Ok(*self
            .commit_many(vec![op])?
            .last()
            .expect("one op in, one seq out"))
    }

    /// Validate, apply and durably log `ops` in order, with **one** fsync for
    /// the whole batch. Returns their sequence numbers.
    ///
    /// Semantics match a `commit` loop exactly, including the failure shape:
    /// if an op is refused, everything before it is still committed (and
    /// durable) and the error is returned. The saving is the fsync count,
    /// which `wal_syncs` reports — without it a `catalog bulk tag` over 50
    /// ids or a 200-entity import would pay one disk barrier per entity.
    pub fn commit_many(&mut self, ops: Vec<Op>) -> Result<Vec<u64>, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let mut recs: Vec<Record> = Vec::with_capacity(ops.len());
        let mut inverses: Vec<Option<Op>> = Vec::with_capacity(ops.len());
        let mut refused = None;
        for op in ops {
            let inverse = self.state.inverse(&op);
            if let Err(e) = self.state.apply(&op) {
                // Whatever was applied before this op is real; log it, then
                // report the refusal.
                refused = Some(e);
                break;
            }
            // Sequence numbers stay contiguous across the batch.
            let seq = self.seq + recs.len() as u64 + 1;
            inverses.push(inverse);
            recs.push(Record { seq, op });
        }
        if let Err(e) = self.wal.append_all(&recs) {
            self.poisoned = true;
            return Err(e);
        }
        let seqs: Vec<u64> = recs.iter().map(|r| r.seq).collect();
        for (rec, inverse) in recs.iter().zip(inverses) {
            self.seq = rec.seq;
            match inverse {
                Some(inverse) => self.undo.push(inverse),
                // No exact inverse (a merge, purge or cascade, or an op
                // whose restore would be refused or partial): the history
                // before it cannot be taken back safely, and skipping the
                // entry would let the next undo rewind something older.
                None => self.undo.clear(),
            }
            self.since_checkpoint += 1;
        }
        if self.since_checkpoint >= self.checkpoint_every {
            self.checkpoint()?;
        }
        match refused {
            Some(e) => Err(e),
            None => Ok(seqs),
        }
    }

    /// fsyncs the current WAL segment has paid for. A commit's cost is its
    /// fsync, so this is how "one sync per batch" is checked rather than
    /// claimed. Resets when a checkpoint starts a new segment.
    pub fn wal_syncs(&self) -> u64 {
        self.wal.syncs()
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
        // Number past every segment on disk rather than delete a leftover
        // from an interrupted checkpoint: until the manifest swap below,
        // nothing proves a segment the manifest does not name is garbage.
        let next_seg = std::fs::read_dir(&self.dir)?
            .filter_map(|e| segment_no(e.ok()?.file_name().to_str()?))
            .chain(segment_no(&self.manifest.wal))
            .max()
            .unwrap_or(0)
            + 1;
        let seg = format!("wal-{next_seg}.log");
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
            // Everything a segment or snapshot held is in `snap` now; the
            // sweep touches only names this store creates.
            let stale_seg = segment_no(&name).is_some() && name != seg;
            let stale_snap = snapshot_no(&name).is_some() && name != snap;
            // A temp a killed writer left: the lock proves it is dead.
            let stale_tmp = own_temp(&name);
            if stale_seg || stale_snap || stale_tmp {
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
