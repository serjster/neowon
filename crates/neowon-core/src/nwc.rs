//! neowon's own capture file format (`.nwc`): the recorder's frame ring,
//! zstd-compressed, every field the acquisition produced — lossless where
//! WAV/CSV exports are not.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic  b"NWCAP2\0\0"           8 bytes (v1 files still read)
//! flags  u32                      bit0 = payload is a zstd stream
//! payload (zstd):
//!   per frame:  seq u64 · sample_rate f64 · acq u8 (0/1/2) · avg u8
//!               n_channels u8 · [v2] t_flag u8 · t_capture f64
//!     per channel: ch u8 · volts_per_lsb f64 · zero_volts f64
//!                  clipped u8 · freq_flag u8 · freq f64 · n u32 · raw [i8; n]
//! frames until decompressed EOF
//! ```

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::{AcqMode, CaptureFrame, ChannelCapture, SharedFrame};

const MAGIC: &[u8; 8] = b"NWCAP2\0\0";
/// Version 1 had no capture timestamps. Still readable; frames come back
/// with `t_capture: None` and fall back to a contiguous axis.
const MAGIC_V1: &[u8; 8] = b"NWCAP1\0\0";
const FLAG_ZSTD: u32 = 1;
/// Sanity bounds so a corrupt file errors instead of allocating wildly.
const MAX_CHANNELS: u8 = 8;
const MAX_SAMPLES: u32 = 64 * 1024 * 1024;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("nwc: {msg}"))
}

pub fn write(path: &Path, frames: &[SharedFrame]) -> io::Result<()> {
    let mut file = io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(MAGIC)?;
    file.write_all(&FLAG_ZSTD.to_le_bytes())?;
    let mut z = zstd::stream::Encoder::new(file, 0)?;
    for frame in frames {
        z.write_all(&frame.seq.to_le_bytes())?;
        z.write_all(&frame.sample_rate.to_le_bytes())?;
        let (acq, avg) = match frame.acq {
            AcqMode::Sample => (0u8, 0u8),
            AcqMode::Peak => (1, 0),
            AcqMode::Average(n) => (2, n),
        };
        z.write_all(&[acq, avg, frame.channels.len() as u8])?;
        z.write_all(&[frame.t_capture.is_some() as u8])?;
        z.write_all(&frame.t_capture.unwrap_or(0.0).to_le_bytes())?;
        for c in &frame.channels {
            z.write_all(&[c.ch as u8])?;
            z.write_all(&c.volts_per_lsb.to_le_bytes())?;
            z.write_all(&c.zero_volts.to_le_bytes())?;
            z.write_all(&[c.clipped as u8, c.freq_meter.is_some() as u8])?;
            z.write_all(&c.freq_meter.unwrap_or(0.0).to_le_bytes())?;
            z.write_all(&(c.raw.len() as u32).to_le_bytes())?;
            let bytes: Vec<u8> = c.raw.iter().map(|&v| v as u8).collect();
            z.write_all(&bytes)?;
        }
    }
    z.finish()?.flush()
}

pub fn read(path: &Path) -> io::Result<Vec<SharedFrame>> {
    let mut file = io::BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    let version = if &magic == MAGIC {
        2
    } else if &magic == MAGIC_V1 {
        1
    } else {
        return Err(bad("not an .nwc file (bad magic)"));
    };
    let mut flags = [0u8; 4];
    file.read_exact(&mut flags)?;
    if u32::from_le_bytes(flags) & FLAG_ZSTD == 0 {
        return Err(bad("unknown payload encoding"));
    }
    let mut z = zstd::stream::Decoder::new(file)?;
    let mut frames = Vec::new();
    loop {
        // Frame boundary: EOF here is the normal end of the stream.
        let mut seq = [0u8; 8];
        match z.read_exact(&mut seq) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let sample_rate = f64::from_le_bytes(read_a(&mut z)?);
        let [acq, avg, n_channels] = read_a::<3>(&mut z)?;
        let t_capture = if version >= 2 {
            let [t_flag] = read_a::<1>(&mut z)?;
            let t = f64::from_le_bytes(read_a(&mut z)?);
            (t_flag != 0).then_some(t)
        } else {
            None
        };
        if n_channels > MAX_CHANNELS {
            return Err(bad("channel count out of range"));
        }
        let acq = match acq {
            0 => AcqMode::Sample,
            1 => AcqMode::Peak,
            2 => AcqMode::Average(avg),
            _ => return Err(bad("unknown acquisition mode")),
        };
        let mut channels = Vec::with_capacity(n_channels as usize);
        for _ in 0..n_channels {
            let [ch] = read_a::<1>(&mut z)?;
            let volts_per_lsb = f64::from_le_bytes(read_a(&mut z)?);
            let zero_volts = f64::from_le_bytes(read_a(&mut z)?);
            let [clipped, freq_flag] = read_a::<2>(&mut z)?;
            let freq = f64::from_le_bytes(read_a(&mut z)?);
            let n = u32::from_le_bytes(read_a(&mut z)?);
            if n > MAX_SAMPLES {
                return Err(bad("sample count out of range"));
            }
            let mut raw = vec![0u8; n as usize];
            z.read_exact(&mut raw)?;
            channels.push(ChannelCapture {
                ch: ch as usize,
                raw: raw.into_iter().map(|b| b as i8).collect(),
                volts_per_lsb,
                zero_volts,
                clipped: clipped != 0,
                freq_meter: (freq_flag != 0).then_some(freq),
            });
        }
        frames.push(Arc::new(CaptureFrame {
            seq: u64::from_le_bytes(seq),
            t_capture,
            sample_rate,
            acq,
            channels,
        }));
    }
    Ok(frames)
}

fn read_a<const N: usize>(r: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_frames() -> Vec<SharedFrame> {
        vec![
            Arc::new(CaptureFrame {
                seq: 7,
                t_capture: Some(1.5),
                sample_rate: 250e3,
                acq: AcqMode::Average(16),
                channels: vec![
                    ChannelCapture {
                        ch: 0,
                        raw: (0..500).map(|i| ((i * 7) % 251 - 125) as i8).collect(),
                        volts_per_lsb: 0.01,
                        zero_volts: -0.5,
                        clipped: true,
                        freq_meter: Some(999.9),
                    },
                    ChannelCapture {
                        ch: 1,
                        raw: vec![1, -1, 127, -128],
                        volts_per_lsb: 0.2,
                        zero_volts: 0.0,
                        clipped: false,
                        freq_meter: None,
                    },
                ],
            }),
            Arc::new(CaptureFrame {
                seq: 8,
                t_capture: None,
                sample_rate: 2.5e3,
                acq: AcqMode::Sample,
                channels: vec![],
            }),
        ]
    }

    #[test]
    fn round_trips_field_exact() {
        let dir = std::env::temp_dir().join("neowon-nwc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.nwc");
        let frames = sample_frames();
        write(&path, &frames).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.len(), frames.len());
        for (a, b) in frames.iter().zip(&back) {
            assert_eq!(a.seq, b.seq);
            assert_eq!(a.sample_rate, b.sample_rate);
            assert_eq!(a.acq, b.acq);
            assert_eq!(a.channels.len(), b.channels.len());
            for (ca, cb) in a.channels.iter().zip(&b.channels) {
                assert_eq!(ca.ch, cb.ch);
                assert_eq!(ca.raw, cb.raw);
                assert_eq!(ca.volts_per_lsb, cb.volts_per_lsb);
                assert_eq!(ca.zero_volts, cb.zero_volts);
                assert_eq!(ca.clipped, cb.clipped);
                assert_eq!(ca.freq_meter, cb.freq_meter);
            }
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = std::env::temp_dir().join("neowon-nwc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.nwc");
        std::fs::write(&path, b"not a capture file").unwrap();
        assert!(read(&path).is_err());
    }
}
