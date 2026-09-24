//! The write-ahead log: length + CRC32 framed JSON records, fsynced before
//! a write is acknowledged. A segment starts with a header record naming
//! its format and schema, so an older segment is recognised (and
//! checkpointed forward) rather than misread.
//!
//! Frame: `len: u32 LE | crc32(payload): u32 LE | payload`. Recovery never
//! destroys what it cannot prove is garbage: the only bytes the reader
//! skips (and the writer truncates) are a torn tail — a short or CRC-bad
//! stretch at the end of a segment with no valid frame after it, the
//! unfinished end of an append that was never acknowledged. A damaged frame
//! with a valid frame behind it is corruption and is reported.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::Op;

pub const WAL_FORMAT: &str = "neowon-catalog-wal";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub schema: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    #[serde(flatten)]
    pub op: Op,
}

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const CRC: [u32; 256] = crc_table();

/// CRC-32 (IEEE 802.3), as zlib computes it.
pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| {
        CRC[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8)
    })
}

/// Crash-test hook: abort the process when `NEOWON_CATALOG_KILL` names
/// this point. Aborting (not panicking) is what a crash looks like: no
/// destructors, no flushes.
pub fn failpoint(name: &str) {
    if armed(name) {
        std::process::abort();
    }
}

/// Whether `NEOWON_CATALOG_KILL` names this point (for a failpoint that
/// writes part of something before it dies).
fn armed(name: &str) -> bool {
    std::env::var("NEOWON_CATALOG_KILL").is_ok_and(|v| v == name)
}

/// One framed record, as it sits in a segment.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 8);
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(&crc32(payload).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

/// The whole, CRC-valid frame at `at`: `(payload, next offset)`. A frame
/// with an empty payload is never written (every record is JSON), so a
/// zero-filled region does not read as one.
fn frame_at(bytes: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let head = bytes.get(at..at.checked_add(8)?)?;
    let len = u32::from_le_bytes(head[0..4].try_into().ok()?) as usize;
    let crc = u32::from_le_bytes(head[4..8].try_into().ok()?);
    let end = (at + 8).checked_add(len)?;
    let payload = bytes.get(at + 8..end)?;
    (len > 0 && crc32(payload) == crc).then_some((payload, end))
}

/// Split a segment at its first unreadable byte. `Ok(stop)` when nothing
/// from `stop` on is a valid frame — a torn tail, the one thing recovery may
/// discard. `Err` names the offset of a valid frame behind the damage:
/// that is corruption, and the bytes around it are not garbage.
fn torn_tail(bytes: &[u8], stop: usize) -> Result<usize, usize> {
    // A resync scan, paid only on the recovery path and only over the
    // tail. A random 8 bytes pass as a frame with odds of 2^-32; a false
    // hit reports corruption, which is the safe direction.
    match (stop + 1..bytes.len()).find(|&o| frame_at(bytes, o).is_some()) {
        Some(valid) => Err(valid),
        None => Ok(stop),
    }
}

/// Read a segment: its header, every record, and the byte length they
/// span. Beyond that length lies at most a torn tail — the unfinished end
/// of an append that was never acknowledged — which the writer cuts away.
///
/// `Ok(None)` is a segment whose header itself is torn: an interrupted
/// [`Writer::create`]. It holds no record, so it is garbage by proof.
///
/// A damaged frame with a valid frame anywhere after it is not a torn
/// tail — an acknowledged record sits behind it — so it is reported as
/// [`Error::Corrupt`] and nothing is truncated.
pub fn read(path: &Path) -> Result<Option<(Header, Vec<Record>, u64)>, Error> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let corrupt = |at: usize, valid: usize| {
        Error::Corrupt(format!(
            "{}: damaged frame at byte {at} with a valid frame at byte {valid} after it",
            path.display()
        ))
    };
    let Some((head, mut at)) = frame_at(&bytes, 0) else {
        return match torn_tail(&bytes, 0) {
            Ok(_) => Ok(None),
            Err(valid) => Err(corrupt(0, valid)),
        };
    };
    let header: Header = serde_json::from_slice(head)
        .map_err(|e| Error::Corrupt(format!("{}: header: {e}", path.display())))?;
    let mut records = Vec::new();
    while at < bytes.len() {
        let Some((p, next)) = frame_at(&bytes, at) else {
            torn_tail(&bytes, at).map_err(|valid| corrupt(at, valid))?;
            break;
        };
        let parsed = serde_json::from_slice::<serde_json::Value>(p).and_then(|mut v| {
            if header.schema == 0 {
                crate::migrate::value_v0(&mut v);
            }
            serde_json::from_value::<Record>(v)
        });
        match parsed {
            Ok(r) => records.push(r),
            // A valid frame with an unreadable op is not a torn write; it
            // is a format problem, and replaying past it would lose data.
            Err(e) => return Err(Error::Corrupt(format!("{}: {e}", path.display()))),
        }
        at = next;
    }
    Ok(Some((header, records, at as u64)))
}

pub struct Writer {
    file: File,
    /// fsyncs issued since this segment was opened. An fsync is what a
    /// commit costs, so this is the cost the batching is meant to cut —
    /// reported rather than asserted in prose, and the only way a test can
    /// tell "one sync for the batch" from "one sync each".
    syncs: u64,
}

impl Writer {
    /// Start a new segment with its header, fsynced.
    pub fn create(path: &Path, schema: u32) -> Result<Self, Error> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        let header = frame(&serde_json::to_vec(&Header {
            format: WAL_FORMAT.into(),
            schema,
        })?);
        if armed("segment-create") {
            // Crash half-way through the header: the segment exists but
            // names no format yet.
            let _ = file.write_all(&header[..header.len() / 2]);
            std::process::abort();
        }
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(Self { file, syncs: 1 })
    }

    /// Append to an existing segment, cutting off a torn tail first.
    pub fn open(path: &Path, valid_len: u64) -> Result<Self, Error> {
        let mut file = OpenOptions::new().write(true).open(path)?;
        file.set_len(valid_len)?;
        file.seek(SeekFrom::End(0))?;
        file.sync_all()?;
        Ok(Self { file, syncs: 1 })
    }

    /// Append one record and fsync it; only then is the write durable.
    pub fn append(&mut self, rec: &Record) -> Result<(), Error> {
        self.append_all(std::slice::from_ref(rec))
    }

    /// Append `recs` in order and fsync **once**.
    ///
    /// Durability is unchanged: none of the batch is acknowledged until the
    /// single `sync_data` returns, and a crash part-way leaves either a
    /// prefix of whole frames or a torn tail — both of which the reader
    /// already handles. What changes is the cost: an fsync per record would
    /// make a 200-entity import 200 disk barriers.
    pub fn append_all(&mut self, recs: &[Record]) -> Result<(), Error> {
        if recs.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::new();
        for rec in recs {
            buf.extend_from_slice(&frame(&serde_json::to_vec(rec)?));
        }
        if armed("wal-append") {
            // Crash half-way through the frame: the torn-tail case.
            let _ = self.file.write_all(&buf[..buf.len() / 2]);
            std::process::abort();
        }
        self.file.write_all(&buf)?;
        self.file.sync_data()?;
        self.syncs += 1;
        Ok(())
    }

    /// fsyncs issued on this segment. See [`Writer::syncs`] on the struct.
    pub fn syncs(&self) -> u64 {
        self.syncs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn torn_tail_is_ignored_and_truncated() {
        let dir = std::env::temp_dir().join(format!("neowon-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wal-1.log");
        let rec = |seq| Record {
            seq,
            op: Op::Pin {
                id: crate::model::Id(seq),
                on: true,
            },
        };
        let mut w = Writer::create(&path, 1).unwrap();
        w.append(&rec(1)).unwrap();
        w.append(&rec(2)).unwrap();
        drop(w);
        // Tear the last frame.
        let len = std::fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(len - 3)
            .unwrap();
        let (h, recs, valid) = read(&path).unwrap().unwrap();
        assert_eq!(h.schema, 1);
        assert_eq!(recs, vec![rec(1)]);
        let mut w = Writer::open(&path, valid).unwrap();
        w.append(&rec(3)).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().1, vec![rec(1), rec(3)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
