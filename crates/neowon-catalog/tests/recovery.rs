//! Recovery never destroys data it cannot prove is garbage.
//!
//! The only thing `open` may silently discard is a torn tail: the bytes
//! after the last whole, valid frame of a segment when no valid frame lies
//! beyond them. A segment whose *header* is torn that way holds no record
//! (an interrupted segment creation) and is set aside. Anything else — a
//! CRC-bad frame or a corrupt length field with valid frames after it, a
//! damaged header with records behind it — is corruption: `open` reports
//! it and leaves every byte where it was.

mod common;
use std::path::{Path, PathBuf};

use common::*;
use neowon_catalog::*;

/// A catalog holding five signals, all in its current segment (no
/// checkpoint since they were written). Returns the directory and the
/// current segment's path.
fn five_in_the_wal(name: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(name);
    let mut cat = Catalog::open(&dir).unwrap();
    cat.checkpoint_every = usize::MAX;
    for i in 0..5 {
        add(&mut cat, |id| {
            signal(id, &format!("S{i}"), 100e6 + i as f64, None)
        });
    }
    drop(cat); // no checkpoint: the five live only in the WAL
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let seg = dir.join(manifest["wal"].as_str().unwrap());
    (dir, seg)
}

/// Byte offset of every frame in a segment: `[header, rec1, rec2, …]`.
fn frames(path: &Path) -> Vec<usize> {
    let bytes = std::fs::read(path).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        out.push(at);
        at += 8 + u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    }
    out
}

fn signals(cat: &Catalog) -> usize {
    cat.state()
        .entities
        .values()
        .filter(|e| e.kind() == "signal")
        .count()
}

/// `open` must refuse, naming corruption, and leave the segment untouched.
fn refused_untouched(dir: &Path, seg: &Path, what: &str) {
    let before = std::fs::read(seg).unwrap();
    match Catalog::open(dir) {
        Err(Error::Corrupt(msg)) => println!("{what}: refused: {msg}"),
        Err(e) => panic!("{what}: refused, but not as corruption: {e}"),
        Ok(cat) => panic!(
            "{what}: opened with {} of 5 signals — acknowledged records dropped",
            signals(&cat)
        ),
    }
    assert_eq!(
        std::fs::read(seg).unwrap(),
        before,
        "{what}: the segment was rewritten"
    );
}

#[test]
fn a_crc_bad_frame_with_records_after_it_is_reported_not_truncated() {
    // Flip one bit inside record 3 of 5.
    let (dir, seg) = five_in_the_wal("recovery-crc-mid");
    let f = frames(&seg);
    let mut bytes = std::fs::read(&seg).unwrap();
    bytes[f[3] + 8 + 5] ^= 0x01;
    std::fs::write(&seg, &bytes).unwrap();
    refused_untouched(&dir, &seg, "crc-bad record 3 of 5");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_corrupt_length_field_with_records_after_it_is_reported() {
    // Record 3's length now claims the frame runs past the end of the file,
    // which is what a torn final frame looks like — but records 4 and 5
    // are still there, whole, behind it.
    let (dir, seg) = five_in_the_wal("recovery-len-mid");
    let f = frames(&seg);
    let mut bytes = std::fs::read(&seg).unwrap();
    bytes[f[3] + 2] ^= 0x10; // + 1 MiB
    std::fs::write(&seg, &bytes).unwrap();
    refused_untouched(&dir, &seg, "length of record 3 of 5");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_damaged_header_with_records_behind_it_is_reported() {
    let (dir, seg) = five_in_the_wal("recovery-header");
    let f = frames(&seg);
    let mut bytes = std::fs::read(&seg).unwrap();
    bytes[f[0] + 8 + 2] ^= 0x01;
    std::fs::write(&seg, &bytes).unwrap();
    refused_untouched(&dir, &seg, "header of a segment with 5 records");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_torn_tail_is_still_discarded() {
    // Each is an append that did not finish; none was acknowledged, so the
    // four records before it open and the catalog stays writable.
    type Tear = fn(&mut Vec<u8>, &[usize]);
    let tears: [(&str, Tear); 4] = [
        ("final frame cut short", |b, _| {
            b.truncate(b.len() - 3);
        }),
        ("final frame's header cut short", |b, f| {
            b.truncate(f[5] + 5)
        }),
        ("final frame fails its CRC", |b, f| b[f[5] + 8 + 5] ^= 0x01),
        ("final frame zero-filled", |b, f| {
            for x in &mut b[f[5]..] {
                *x = 0;
            }
        }),
    ];
    for (what, tear) in tears {
        let (dir, seg) = five_in_the_wal("recovery-torn");
        let f = frames(&seg);
        let mut bytes = std::fs::read(&seg).unwrap();
        tear(&mut bytes, &f);
        std::fs::write(&seg, &bytes).unwrap();
        let mut cat = Catalog::open(&dir).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(signals(&cat), 4, "{what}");
        assert_eq!(
            std::fs::metadata(&seg).unwrap().len(),
            f[5] as u64,
            "{what}: the tail is cut at the last whole frame"
        );
        add(&mut cat, |id| signal(id, "after", 1e6, None));
        drop(cat);
        assert_eq!(signals(&Catalog::open(&dir).unwrap()), 5, "{what}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_headerless_segment_that_is_not_current_is_set_aside() {
    // A sound catalog plus an interrupted segment creation beyond
    // its current segment — empty, 4 bytes, or half a header.
    let header_half = {
        let h = serde_json::to_vec(&wal::Header {
            format: wal::WAL_FORMAT.into(),
            schema: SCHEMA,
        })
        .unwrap();
        let f = wal::frame(&h);
        f[..f.len() / 2].to_vec()
    };
    for (what, stub) in [
        ("empty", Vec::new()),
        ("4 bytes", vec![0x2a, 0, 0, 0]),
        ("half a header", header_half),
    ] {
        let (dir, seg) = five_in_the_wal("recovery-stub");
        let n: u64 = seg
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches("wal-")
            .trim_end_matches(".log")
            .parse()
            .unwrap();
        let stub_path = dir.join(format!("wal-{}.log", n + 1));
        std::fs::write(&stub_path, &stub).unwrap();
        let mut cat = Catalog::open(&dir).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(signals(&cat), 5, "{what}");
        add(&mut cat, |id| signal(id, "after", 1e6, None));
        // The next checkpoint moves past it and sweeps it.
        cat.checkpoint().unwrap();
        assert!(!stub_path.exists(), "{what}: stub left after a checkpoint");
        drop(cat);
        assert_eq!(signals(&Catalog::open(&dir).unwrap()), 6, "{what}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn checkpoint_sweeps_only_its_own_files() {
    // The post-checkpoint sweep removes old segments, snapshots and its
    // own interrupted temps — nothing that merely resembles them.
    let (dir, _) = five_in_the_wal("recovery-sweep");
    let mine = [".manifest.json.123-4.tmp", ".snapshot-3.json.9-0.tmp"];
    let others = [
        "notes.txt",
        "snapshot-3.json.bak",
        "snapshot-old.json",
        ".scratch.tmp",
        "wal-9.log.bak",
    ];
    for n in mine.iter().chain(&others) {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut cat = Catalog::open(&dir).unwrap();
    cat.checkpoint().unwrap();
    for n in mine {
        assert!(!dir.join(n).exists(), "{n}: a catalog temp was left");
    }
    for n in others {
        assert!(dir.join(n).exists(), "{n}: not the catalog's, but deleted");
    }
    drop(cat);
    std::fs::remove_dir_all(&dir).unwrap();
}
