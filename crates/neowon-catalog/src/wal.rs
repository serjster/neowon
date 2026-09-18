//! The write-ahead log: length + CRC32 framed JSON records, fsynced before
//! a write is acknowledged. A segment starts with a header record naming
//! its format and schema, so an older segment is recognised (and
//! checkpointed forward) rather than misread.
//!
//! Frame: `len: u32 LE | crc32(payload): u32 LE | payload`. A frame that is
//! short or fails its CRC is a torn tail from a crash mid-append; it was
//! never acknowledged, so the reader stops there and the writer truncates
//! it away.

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
    if std::env::var("NEOWON_CATALOG_KILL").is_ok_and(|v| v == name) {
        std::process::abort();
    }
}

/// One framed record, as it sits in a segment.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 8);
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(&crc32(payload).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

/// Read a segment: its header and every whole, valid record, plus the
/// byte length they span (a torn tail beyond it is ignored).
pub fn read(path: &Path) -> Result<(Header, Vec<Record>, u64), Error> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let mut at = 0usize;
    let mut next = || -> Option<&[u8]> {
        let head = bytes.get(at..at + 8)?;
        let len = u32::from_le_bytes(head[0..4].try_into().ok()?) as usize;
        let crc = u32::from_le_bytes(head[4..8].try_into().ok()?);
        let payload = bytes.get(at + 8..at + 8 + len)?;
        if crc32(payload) != crc {
            return None;
        }
        at += 8 + len;
        Some(payload)
    };
    let header: Header = next()
        .and_then(|p| serde_json::from_slice(p).ok())
        .ok_or_else(|| Error::Corrupt(format!("{}: no valid header", path.display())))?;
    let mut records = Vec::new();
    while let Some(p) = next() {
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
    }
    Ok((header, records, at as u64))
}

pub struct Writer {
    file: File,
}

impl Writer {
    /// Start a new segment with its header, fsynced.
    pub fn create(path: &Path, schema: u32) -> Result<Self, Error> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        let header = serde_json::to_vec(&Header {
            format: WAL_FORMAT.into(),
            schema,
        })?;
        file.write_all(&frame(&header))?;
        file.sync_all()?;
        Ok(Self { file })
    }

    /// Append to an existing segment, cutting off a torn tail first.
    pub fn open(path: &Path, valid_len: u64) -> Result<Self, Error> {
        let mut file = OpenOptions::new().write(true).open(path)?;
        file.set_len(valid_len)?;
        file.seek(SeekFrom::End(0))?;
        file.sync_all()?;
        Ok(Self { file })
    }

    /// Append one record and fsync it; only then is the write durable.
    pub fn append(&mut self, rec: &Record) -> Result<(), Error> {
        let f = frame(&serde_json::to_vec(rec)?);
        if std::env::var("NEOWON_CATALOG_KILL").is_ok_and(|v| v == "wal-append") {
            // Crash half-way through the frame: the torn-tail case.
            let _ = self.file.write_all(&f[..f.len() / 2]);
            std::process::abort();
        }
        self.file.write_all(&f)?;
        self.file.sync_data()?;
        Ok(())
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
        let (h, recs, valid) = read(&path).unwrap();
        assert_eq!(h.schema, 1);
        assert_eq!(recs, vec![rec(1)]);
        let mut w = Writer::open(&path, valid).unwrap();
        w.append(&rec(3)).unwrap();
        assert_eq!(read(&path).unwrap().1, vec![rec(1), rec(3)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
