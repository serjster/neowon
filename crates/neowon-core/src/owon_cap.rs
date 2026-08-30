//! Import of the OWON vendor `.cap` recording format (big-endian), as
//! written by the VDS1022 PC app v1.1.x. Layout reverse-engineered from
//! the vendor jar (`RecordFileIO`, `RecordControlTiny`); see
//! `docs/protocol-vds1022.md` §capture-files for the byte-level spec.
//!
//! Header (34 bytes): `"SPB" + machine name` (10 ASCII), machine type
//! i32 (VDS1022 = 100), record version i32 (current = 4), extend i32
//! (top byte must be 3), file size i32, timegap-ms i32, frame count i32
//! (the last three are zero if the recording was never sealed — then the
//! frame chain is walked to EOF). Frames: framelen i32, timebase index
//! i32, trigger position i32, peak u8 (v≥3), deep-memory len i32 (v≥4),
//! then channel blocks. Channel: ch u8, blocklen i32, inverse i32 (v≥1),
//! initPos i32, screendatalen i32, datalen i32, slowMove i32, pos0 i32,
//! voltbase index i32, probe index i32, freq f32, cycle f32, then
//! `datalen` raw i8 samples in the same ±125 = 10 div encoding the
//! live protocol uses.

use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use crate::{AcqMode, CaptureFrame, ChannelCapture, SharedFrame};

/// `[Voltbase]` table from the jar's `VDS1022ONE.txt`, volts per division.
const VOLTBASE: [f64; 10] = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
/// `[ProbeRate]` table.
const PROBE: [f64; 7] = [1.0, 10.0, 20.0, 50.0, 100.0, 500.0, 1000.0];
/// `[Timebase]` table, seconds per division (5 ns … 100 s).
const TIMEBASE: [f64; 32] = [
    5e-9, 10e-9, 20e-9, 50e-9, 100e-9, 200e-9, 500e-9, 1e-6, 2e-6, 5e-6, 10e-6, 20e-6, 50e-6,
    100e-6, 200e-6, 500e-6, 1e-3, 2e-3, 5e-3, 10e-3, 20e-3, 50e-3, 100e-3, 200e-3, 500e-3, 1.0,
    2.0, 5.0, 10.0, 20.0, 50.0, 100.0,
];

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("cap: {msg}"))
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            return Err(bad("truncated file"));
        };
        let s = &self.data[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// The sample rate the vendor app ran at for a given timebase: 5000
/// samples span the 20-division screen, clamped to the hardware's
/// 100 MS/s ceiling. (Reproduces both documented anchors: 100 MS/s at
/// ≤2.5 µs/div and the 2500 S/s roll threshold at 100 ms/div.)
fn sample_rate(timebase_idx: i32) -> f64 {
    let tb = TIMEBASE
        .get(timebase_idx.max(0) as usize)
        .copied()
        .unwrap_or(1e-3);
    (5000.0 / (20.0 * tb)).min(100e6)
}

pub fn read(path: &Path) -> io::Result<Vec<SharedFrame>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut data)?;
    let mut c = Cursor { data: &data, at: 0 };

    let header = c.take(10)?;
    if !header.starts_with(b"SPBVDS") {
        return Err(bad("not an OWON .cap file (bad header)"));
    }
    let _machine_type = c.i32()?;
    let version = c.i32()?;
    if !(0..=4).contains(&version) {
        return Err(bad("unknown record version"));
    }
    if version >= 2 {
        let extend = c.i32()?;
        if (extend >> 24) & 0xff != 3 {
            return Err(bad("not a record file (bad extend tag)"));
        }
    }
    let _filesize = c.i32()?;
    // The vendor's own record of the dead time between frames — the one
    // piece of gap information in the format. Third-party readers discard
    // it and butt the frames together; we place them on a real axis.
    let timegap_ms = c.i32()?;
    let counter = c.i32()?;

    let mut frames = Vec::new();
    // Sealed files carry the frame count; a crashed recording leaves it 0
    // and the chain is walked to EOF.
    while c.at < c.data.len() && (counter <= 0 || frames.len() < counter as usize) {
        let framelen = c.i32()?;
        if framelen < 0 {
            return Err(bad("negative frame length"));
        }
        let frame_start = c.at;
        let frame_end = frame_start
            .checked_add(framelen as usize)
            .filter(|&e| e <= c.data.len())
            .ok_or_else(|| bad("frame overruns file"))?;
        let timebase_idx = c.i32()?;
        let _trigger_pos = c.i32()?;
        let peak = if version >= 3 { c.u8()? != 0 } else { false };
        if version >= 4 {
            let _dm_len = c.i32()?;
        }

        let mut channels = Vec::new();
        while c.at < frame_end {
            let ch = c.u8()?;
            let blocklen = c.i32()?;
            let block_start = c.at;
            let block_end = block_start
                .checked_add(blocklen.max(0) as usize)
                .filter(|&e| e <= frame_end)
                .ok_or_else(|| bad("channel block overruns frame"))?;
            let inverse = if version >= 1 { c.i32()? } else { 0 };
            let _init_pos = c.i32()?;
            let _screendatalen = c.i32()?;
            let datalen = c.i32()?;
            let _slow_move = c.i32()?;
            let pos0 = c.i32()?;
            let vb_idx = c.i32()?;
            let probe_idx = c.i32()?;
            let freq = c.f32()?;
            let _cycle = c.f32()?;
            if datalen < 0 || c.at + datalen as usize > block_end {
                return Err(bad("sample data overruns channel block"));
            }
            let volts_div = VOLTBASE.get(vb_idx.max(0) as usize).copied().unwrap_or(1.0);
            let probe = PROBE.get(probe_idx.max(0) as usize).copied().unwrap_or(1.0);
            let volts_per_lsb = volts_div / 25.0 * probe;
            let sign = if inverse == 1 { -1i16 } else { 1 };
            let raw: Vec<i8> = c
                .take(datalen as usize)?
                .iter()
                .map(|&b| ((b as i8) as i16 * sign).clamp(-128, 127) as i8)
                .collect();
            let clipped = raw.iter().any(|&r| r.unsigned_abs() >= 125);
            channels.push(ChannelCapture {
                ch: ch as usize,
                raw,
                volts_per_lsb,
                zero_volts: -(pos0 as f64) * volts_per_lsb,
                clipped,
                freq_meter: (freq.is_finite() && freq > 0.0).then_some(freq as f64),
            });
            c.at = block_end;
        }
        c.at = frame_end;
        let rate = sample_rate(timebase_idx);
        let n = channels.first().map_or(0, |c: &ChannelCapture| c.raw.len());
        let idx = frames.len() as f64;
        // Frame start = index x (record duration + inter-frame gap). With no
        // gap recorded (a crashed recording leaves it 0) the frames are laid
        // end to end, which is the honest best guess.
        let gap = (timegap_ms.max(0) as f64) / 1000.0;
        frames.push(Arc::new(CaptureFrame {
            seq: frames.len() as u64 + 1,
            t_capture: Some(idx * (n as f64 / rate.max(1e-12) + gap)),
            sample_rate: sample_rate(timebase_idx),
            acq: if peak { AcqMode::Peak } else { AcqMode::Sample },
            channels,
        }));
    }
    if frames.is_empty() {
        return Err(bad("no frames in file"));
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only writer producing the vendor's v4 layout.
    struct W(Vec<u8>);

    impl W {
        fn i32(&mut self, v: i32) {
            self.0.extend_from_slice(&v.to_be_bytes());
        }
        fn f32(&mut self, v: f32) {
            self.0.extend_from_slice(&v.to_be_bytes());
        }
    }

    fn channel_block(w: &mut W, ch: u8, samples: &[i8], pos0: i32, vb: i32, probe: i32) {
        w.0.push(ch);
        // blocklen is the vendor's patch-back `endPtr - lenPos - 4`: the
        // bytes after this field — 40 of metadata plus the samples.
        w.i32(40 + samples.len() as i32);
        w.i32(0); // inverse
        w.i32(0); // initPos
        w.i32(samples.len() as i32); // screendatalen
        w.i32(samples.len() as i32); // datalen
        w.i32(0); // slowMove
        w.i32(pos0);
        w.i32(vb);
        w.i32(probe);
        w.f32(1000.0);
        w.f32(0.001);
        w.0.extend(samples.iter().map(|&s| s as u8));
    }

    fn cap_file(sealed: bool) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.0.extend_from_slice(b"SPBVDS1022");
        w.i32(100); // machine type
        w.i32(4); // record version
        w.i32(0x03000000); // extend
        let seal_at = w.0.len();
        w.i32(0); // filesize (patched by seal)
        w.i32(10); // timegap
        w.i32(0); // counter (patched by seal)
        for f in 0..2 {
            let frame_at = w.0.len();
            w.i32(0); // framelen placeholder
            w.i32(16); // timebase idx = 1 ms/div
            w.i32(0); // trigger pos
            w.0.push(0); // peak off
            w.i32(0); // DM len
            channel_block(&mut w, 0, &[0, 25, 50, -125 + f], 10, 4, 1); // 100mV ×10
            channel_block(&mut w, 1, &[1, -1], 0, 7, 0); // 1V ×1
            let framelen = (w.0.len() - frame_at - 4) as i32;
            w.0[frame_at..frame_at + 4].copy_from_slice(&framelen.to_be_bytes());
        }
        if sealed {
            let size = w.0.len() as i32;
            w.0[seal_at..seal_at + 4].copy_from_slice(&size.to_be_bytes());
            w.0[seal_at + 8..seal_at + 12].copy_from_slice(&2i32.to_be_bytes());
        }
        w.0
    }

    fn parse(bytes: &[u8], name: &str) -> Vec<SharedFrame> {
        let dir = std::env::temp_dir().join("neowon-cap-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        read(&path).unwrap()
    }

    #[test]
    fn parses_sealed_and_unsealed() {
        for (sealed, name) in [(true, "sealed.cap"), (false, "unsealed.cap")] {
            let frames = parse(&cap_file(sealed), name);
            assert_eq!(frames.len(), 2, "sealed={sealed}");
            let f = &frames[0];
            // 1 ms/div → 5000 / (20 × 1 ms) = 250 kS/s.
            assert_eq!(f.sample_rate, 250e3);
            assert_eq!(f.acq, AcqMode::Sample);
            assert_eq!(f.channels.len(), 2);
            let c0 = &f.channels[0];
            assert_eq!(c0.ch, 0);
            assert_eq!(c0.raw, vec![0, 25, 50, -125]);
            // voltbase 4 = 100 mV/div, probe idx 1 = ×10 → 40 mV/LSB.
            assert!((c0.volts_per_lsb - 0.04).abs() < 1e-12);
            assert!((c0.zero_volts - (-10.0 * 0.04)).abs() < 1e-12);
            assert!(c0.clipped);
            assert_eq!(c0.freq_meter, Some(1000.0));
            let c1 = &f.channels[1];
            assert_eq!(c1.ch, 1);
            assert!((c1.volts_per_lsb - 0.04).abs() < 1e-12); // 1 V/div ×1
            assert!(!c1.clipped);
        }
    }

    #[test]
    fn rejects_non_cap() {
        let dir = std::env::temp_dir().join("neowon-cap-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.cap");
        std::fs::write(&path, b"RIFFxxxxWAVE").unwrap();
        assert!(read(&path).is_err());
    }
}
