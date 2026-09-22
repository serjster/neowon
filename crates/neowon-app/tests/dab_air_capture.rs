//! Offline harness for the on-air DAB MSC / DAB+ transport work.
//!
//! Ignored and env-gated: set `NEOWON_IQ_CAPTURE` to a raw interleaved-f32 IQ
//! file (2.048 MS/s, Band III) and run
//!
//! ```text
//! NEOWON_IQ_CAPTURE=tmp-inspiration/dab-11c.f32 \
//!   cargo test -p neowon-app --release --test dab_air_capture -- --ignored --nocapture
//! ```
//!
//! It feeds the capture through `DabReceiver`, then treats each FIC-resolved
//! sub-channel's emitted bytes as a stream to be examined with the only
//! oracles a real transmitter offers: the DAB+ header **Fire code** and the
//! per-AU **CRC-16** (TS 102 563 clause 5.2). The scan is exhaustive over
//! byte offsets, so it measures what the MSC chain delivered, not whether a
//! particular sync policy happened to lock.
//!
//! `NEOWON_IQ_DUMP_DIR` writes each sub-channel's raw MSC byte stream.
//! `NEOWON_IQ_FIRST_FRAMES` limits transmission frames processed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use neowon_codec::dabplus::{crc16, fire_code};
use neowon_dsp::dab::DabReceiver;
use neowon_dsp::dab::msc::SubChannelDecoder;

/// Read an interleaved little-endian f32 (I, Q) capture.
fn read_iq(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read capture");
    assert_eq!(bytes.len() % 8, 0, "capture must be interleaved f32 pairs");
    bytes
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect()
}

/// Split a Fire-clean super frame's data region into AUs and count their CRCs.
/// Mirrors the clause-5.2 header syntax without the RS correction the
/// transport applies: the scan runs on the raw stream, so a wrong window can
/// only pass by chance.
fn au_crc_ok(window: &[u8], index: usize) -> (usize, usize) {
    let params = window[2];
    let (naus, first) = match (params & 0x40 != 0, params & 0x20 != 0) {
        (true, true) => (3, 6),
        (false, true) => (2, 5),
        (true, false) => (6, 11),
        (false, false) => (4, 8),
    };
    let size = 110 * index;
    let mut starts = vec![first];
    for k in 1..naus {
        let bit = 24 + 12 * (k - 1);
        let mut value = 0usize;
        for i in 0..12 {
            let at = bit + i;
            value = (value << 1) | ((window[at / 8] >> (7 - at % 8)) & 1) as usize;
        }
        starts.push(value);
    }
    starts.push(size);
    let mut ok = 0;
    for pair in starts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start + 2 > end || end > size {
            return (0, naus);
        }
        let stored = u16::from_be_bytes([window[end - 2], window[end - 1]]);
        if crc16(&window[start..end - 2]) == stored {
            ok += 1;
        }
    }
    (ok, naus)
}

#[test]
#[ignore = "requires NEOWON_IQ_CAPTURE (operator-recorded air capture); sim/offline only"]
fn air_capture_dabplus_transport() {
    let Some(path) = std::env::var_os("NEOWON_IQ_CAPTURE") else {
        eprintln!("set NEOWON_IQ_CAPTURE to run this harness");
        return;
    };
    let path = path.to_string_lossy().to_string();
    let limit: usize = std::env::var("NEOWON_IQ_FIRST_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let dump_dir = std::env::var("NEOWON_IQ_DUMP_DIR").ok().map(PathBuf::from);

    let iq = read_iq(&path);
    let samples = iq.len() / 2;
    eprintln!(
        "capture {path}: {} samples = {:.2} s",
        samples,
        samples as f64 / neowon_dsp::dab::SAMPLE_RATE
    );

    let mut receiver = DabReceiver::new();
    let mut indexes: BTreeMap<u8, u8> = BTreeMap::new();
    let mut dumps: BTreeMap<u8, Vec<u8>> = BTreeMap::new();

    let chunk = 1 << 20; // complex samples per push
    let mut at = 0usize;
    let mut frames_done = 0usize;
    while at < samples && frames_done < limit {
        let end = (at + chunk).min(samples);
        let block: Vec<f32> = iq[at * 2..end * 2].to_vec();
        frames_done += receiver.push_iq(&block);
        at = end;

        // The DAB+ index of every sub-channel, resolved exactly as the
        // transport's own `dabplus_index` resolves it from the FIC.
        for (id, sub) in &receiver.ensemble().sub_channels {
            if indexes.contains_key(id) {
                continue;
            }
            let Some(handler) = SubChannelDecoder::new(sub) else {
                continue;
            };
            let index = handler.profile().info_bits() / 8 / 24;
            if (1..=24).contains(&index) {
                indexes.insert(*id, index as u8);
            }
        }
        for frame in receiver.take_msc_frames() {
            dumps
                .entry(frame.sub_channel)
                .or_default()
                .extend_from_slice(&frame.bytes);
        }
    }

    let status = receiver.status();
    eprintln!(
        "receiver: frames_decoded {} rejected {} prs {:.3}",
        receiver.frames_decoded,
        receiver.frames_rejected,
        receiver.prs_metric()
    );
    eprintln!(
        "FIC: locked {} fib_crc {}/{} ({:.3}) eid {:?} label {:?}",
        status.locked,
        status.fib_crc_ok,
        status.fib_total,
        status.fib_crc_rate().unwrap_or(0.0),
        status.ensemble.eid,
        status.ensemble.label
    );

    if let Some(dir) = &dump_dir {
        std::fs::create_dir_all(dir).expect("dump dir");
        for (id, bytes) in &dumps {
            let out = dir.join(format!("subch{id}.bin"));
            std::fs::write(&out, bytes).expect("write dump");
        }
        eprintln!(
            "dumped {} sub-channel streams to {}",
            dumps.len(),
            dir.display()
        );
    }

    // Service names for the report: which SId sits on which SubChId.
    let mut names: BTreeMap<u8, String> = BTreeMap::new();
    for service in status.ensemble.services.values() {
        if let Some(sub) = service.sub_channel {
            names.insert(
                sub,
                format!(
                    "{} ({:04X})",
                    service.label.as_deref().unwrap_or("<unnamed>"),
                    service.sid
                ),
            );
        }
    }

    // The Fire/AU-CRC oracle, exhaustive over byte offsets.
    let mut total_sf = 0usize;
    let mut total_au = 0usize;
    let mut total_crc = 0usize;
    let mut first_window: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    let mut reported: BTreeMap<u8, (usize, usize, usize)> = BTreeMap::new();
    for (id, bytes) in &dumps {
        let Some(&index) = indexes.get(id) else {
            continue;
        };
        let index = index as usize;
        let sf = 120 * index;
        let hits: Vec<usize> = (0..bytes.len().saturating_sub(11))
            .filter(|&off| {
                let stored = u16::from_be_bytes([bytes[off], bytes[off + 1]]);
                stored != 0 && stored == fire_code(&bytes[off + 2..off + 11])
            })
            .collect();
        let mut runs: Vec<Vec<usize>> = Vec::new();
        for hit in hits {
            match runs.last_mut() {
                Some(run) if hit - *run.last().expect("non-empty") == sf => run.push(hit),
                _ => runs.push(vec![hit]),
            }
        }
        let mut full = 0usize;
        let mut aus = 0usize;
        let mut crc_ok = 0usize;
        for run in &runs {
            for &hit in run.iter().take(run.len().saturating_sub(1)) {
                let window = &bytes[hit..hit + sf];
                let (ok, n) = au_crc_ok(window, index);
                aus += n;
                crc_ok += ok;
                full += 1;
                first_window.entry(*id).or_insert_with(|| window.to_vec());
            }
        }
        total_sf += full;
        total_au += aus;
        total_crc += crc_ok;
        reported.insert(*id, (full, aus, crc_ok));
    }

    eprintln!("\nservice / sub-channel / index / Fire-clean superframes / AUs / AU CRCs:");
    for (id, (full, aus, crc_ok)) in &reported {
        let name = names.get(id).map(String::as_str).unwrap_or("<no service>");
        eprintln!(
            "  {name:<22} subCh {id:<2} index {:>2}: {full:>3} superframes, {aus:>4} AUs, {crc_ok:>4} CRCs ok",
            indexes.get(id).copied().unwrap_or(0)
        );
        if let Some(window) = first_window.get(id) {
            let params = window[2];
            eprintln!(
                "      first window: params {params:02x} dac {} kHz sbr {} stereo {} ps {} surround {}",
                if params & 0x40 != 0 { 48 } else { 32 },
                params & 0x20 != 0,
                params & 0x10 != 0,
                params & 0x08 != 0,
                params & 0x07,
            );
        }
    }
    eprintln!("\ntotal: {total_sf} Fire-clean superframes, {total_au} AUs, {total_crc} AU CRCs ok");

    assert!(
        total_sf > 0 && total_crc > 0,
        "the capture produced no Fire-clean, CRC-clean DAB+ superframe"
    );
}
