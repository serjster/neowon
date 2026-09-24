//! neowon's own capture file format (`.nwc`): the recorder's frame ring,
//! zstd-compressed, every field the acquisition produced — lossless where
//! WAV/CSV exports are not.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic  b"NWCAP3\0\0"           8 bytes (v1/v2 files still read)
//! flags  u32                      bit0 = payload is a zstd stream
//! payload (zstd):
//!   per frame:  seq u64 · sample_rate f64 · acq u8 (0/1/2) · avg u8
//!               layout u8 (0=real, 1=complex) · n_channels u8
//!               [v2+] t_flag u8 · t_capture f64
//!     per channel [v3]: ch u8 · scale_i f64 · scale_q f64
//!                       offset_i f64 · offset_q f64
//!                       clipped u8 · freq_flag u8 · freq f64 · n u32
//!                       data [f32; n]
//!     per channel [v1/v2]: ch u8 · volts_per_lsb f64 · zero_volts f64
//!                       clipped u8 · freq_flag u8 · freq f64 · n u32 · raw [i8; n]
//! frames until decompressed EOF
//! ```
//!
//! `n` is the scalar count of `data` (for complex frames that is twice the
//! I/Q pair count). v1/v2 channels are read back as `Real` frames.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::{AcqMode, Acquisition, CaptureFrame, ChannelCapture, IqCal, SampleLayout, SharedFrame};

const MAGIC_V3: &[u8; 8] = b"NWCAP3\0\0";
const MAGIC_V2: &[u8; 8] = b"NWCAP2\0\0";
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

fn layout_byte(layout: SampleLayout) -> u8 {
    match layout {
        SampleLayout::Real => 0,
        SampleLayout::Complex => 1,
    }
}

fn layout_from_byte(b: u8) -> io::Result<SampleLayout> {
    match b {
        0 => Ok(SampleLayout::Real),
        1 => Ok(SampleLayout::Complex),
        _ => Err(bad("unknown sample layout")),
    }
}

pub fn write(path: &Path, frames: &[SharedFrame]) -> io::Result<()> {
    // The capture appears at `path` only once every frame is in it.
    let mut file = crate::atomic_file::AtomicFile::create(path)?;
    file.write_all(MAGIC_V3)?;
    file.write_all(&FLAG_ZSTD.to_le_bytes())?;
    let mut z = zstd::stream::Encoder::new(file, 0)?;
    for frame in frames {
        z.write_all(&frame.seq.to_le_bytes())?;
        z.write_all(&frame.sample_rate.to_le_bytes())?;
        let (acq, avg) = match frame.acq() {
            AcqMode::Sample => (0u8, 0u8),
            AcqMode::Peak => (1, 0),
            AcqMode::Average(n) => (2, n),
        };
        z.write_all(&[
            acq,
            avg,
            layout_byte(frame.layout()),
            frame.channels.len() as u8,
        ])?;
        z.write_all(&[frame.t_capture.is_some() as u8])?;
        z.write_all(&frame.t_capture.unwrap_or(0.0).to_le_bytes())?;
        for c in &frame.channels {
            z.write_all(&[c.ch as u8])?;
            z.write_all(&c.cal.scale_i.to_le_bytes())?;
            z.write_all(&c.cal.scale_q.to_le_bytes())?;
            z.write_all(&c.cal.offset_i.to_le_bytes())?;
            z.write_all(&c.cal.offset_q.to_le_bytes())?;
            z.write_all(&[c.clipped as u8, c.freq_meter.is_some() as u8])?;
            z.write_all(&c.freq_meter.unwrap_or(0.0).to_le_bytes())?;
            z.write_all(&(c.data.len() as u32).to_le_bytes())?;
            let mut bytes = Vec::with_capacity(c.data.len() * 4);
            for &v in &c.data {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            z.write_all(&bytes)?;
        }
    }
    z.finish()?.commit()
}

pub fn read(path: &Path) -> io::Result<Vec<SharedFrame>> {
    let mut file = io::BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    let version = if &magic == MAGIC_V3 {
        3
    } else if &magic == MAGIC_V2 {
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
        let (acq_b, avg, layout_b, n_channels) = if version >= 3 {
            let [a, b, l, n] = read_a::<4>(&mut z)?;
            (a, b, l, n)
        } else {
            let [a, b, n] = read_a::<3>(&mut z)?;
            (a, b, 0u8, n)
        };
        let layout = layout_from_byte(layout_b)?;
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
        let acq = match acq_b {
            0 => AcqMode::Sample,
            1 => AcqMode::Peak,
            2 => AcqMode::Average(avg),
            _ => return Err(bad("unknown acquisition mode")),
        };
        let mut channels = Vec::with_capacity(n_channels as usize);
        for _ in 0..n_channels {
            let [ch] = read_a::<1>(&mut z)?;
            let data: Vec<f32>;
            let cal: IqCal;
            let clipped: bool;
            let freq_meter: Option<f64>;
            if version >= 3 {
                let scale_i = f64::from_le_bytes(read_a(&mut z)?);
                let scale_q = f64::from_le_bytes(read_a(&mut z)?);
                let offset_i = f64::from_le_bytes(read_a(&mut z)?);
                let offset_q = f64::from_le_bytes(read_a(&mut z)?);
                let [clipped_b, freq_flag] = read_a::<2>(&mut z)?;
                let freq = f64::from_le_bytes(read_a(&mut z)?);
                let n = u32::from_le_bytes(read_a(&mut z)?);
                if n > MAX_SAMPLES {
                    return Err(bad("sample count out of range"));
                }
                let mut raw = vec![0u8; n as usize * 4];
                z.read_exact(&mut raw)?;
                data = raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                cal = IqCal {
                    scale_i,
                    scale_q,
                    offset_i,
                    offset_q,
                };
                clipped = clipped_b != 0;
                freq_meter = (freq_flag != 0).then_some(freq);
            } else {
                let volts_per_lsb = f64::from_le_bytes(read_a(&mut z)?);
                let zero_volts = f64::from_le_bytes(read_a(&mut z)?);
                let [clipped_b, freq_flag] = read_a::<2>(&mut z)?;
                let freq = f64::from_le_bytes(read_a(&mut z)?);
                let n = u32::from_le_bytes(read_a(&mut z)?);
                if n > MAX_SAMPLES {
                    return Err(bad("sample count out of range"));
                }
                let mut raw = vec![0u8; n as usize];
                z.read_exact(&mut raw)?;
                data = raw.into_iter().map(|b| b as i8 as f32).collect();
                cal = IqCal::real(volts_per_lsb, zero_volts);
                clipped = clipped_b != 0;
                freq_meter = (freq_flag != 0).then_some(freq);
            }
            channels.push(ChannelCapture {
                ch: ch as usize,
                data,
                cal,
                clipped,
                freq_meter,
            });
        }
        // A file is not trusted to hold a legal layout x acq combination:
        // it goes through the same constructor a backend does. The
        // delivery a stored frame implies is its layout's — complex data
        // only ever came off a stream.
        let units = channels.first().map_or(0, |c| c.unit_count(layout));
        let delivery = match layout {
            SampleLayout::Complex => Acquisition::Stream { chunk: units },
            SampleLayout::Real => Acquisition::Record { samples: units },
        };
        let frame = CaptureFrame::new(
            u64::from_le_bytes(seq),
            t_capture,
            sample_rate,
            acq,
            delivery,
            layout,
            channels,
        )
        .map_err(|e| bad(&e.to_string()))?;
        frames.push(Arc::new(frame));
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
            Arc::new(
                CaptureFrame::new(
                    7,
                    Some(1.5),
                    250e3,
                    AcqMode::Average(16),
                    Acquisition::Record { samples: 500 },
                    SampleLayout::Real,
                    vec![
                        ChannelCapture {
                            ch: 0,
                            data: (0..500).map(|i| ((i * 7) % 251 - 125) as f32).collect(),
                            cal: IqCal::real(0.01, -0.5),
                            clipped: true,
                            freq_meter: Some(999.9),
                        },
                        ChannelCapture {
                            ch: 1,
                            data: vec![1.0, -1.0, 127.0, -128.0],
                            cal: IqCal::real(0.2, 0.0),
                            clipped: false,
                            freq_meter: None,
                        },
                    ],
                )
                .unwrap(),
            ),
            Arc::new(
                CaptureFrame::new(
                    8,
                    None,
                    2.5e3,
                    AcqMode::Sample,
                    Acquisition::Record { samples: 0 },
                    SampleLayout::Real,
                    vec![],
                )
                .unwrap(),
            ),
        ]
    }

    #[test]
    fn round_trips_field_exact() {
        let dir = crate::test_scratch("nwc");
        let path = dir.join("rt.nwc");
        let frames = sample_frames();
        write(&path, &frames).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.len(), frames.len());
        for (a, b) in frames.iter().zip(&back) {
            assert_eq!(a.seq, b.seq);
            assert_eq!(a.sample_rate, b.sample_rate);
            assert_eq!(a.acq(), b.acq());
            assert_eq!(a.layout(), b.layout());
            assert_eq!(a.channels.len(), b.channels.len());
            for (ca, cb) in a.channels.iter().zip(&b.channels) {
                assert_eq!(ca.ch, cb.ch);
                assert_eq!(ca.data, cb.data);
                assert_eq!(ca.cal, cb.cal);
                assert_eq!(ca.clipped, cb.clipped);
                assert_eq!(ca.freq_meter, cb.freq_meter);
            }
        }
    }

    #[test]
    fn complex_calibration_round_trips() {
        let dir = crate::test_scratch("nwc");
        let path = dir.join("iq.nwc");
        let frames = vec![Arc::new(
            CaptureFrame::new(
                1,
                Some(0.0),
                1e6,
                AcqMode::Sample,
                Acquisition::Stream { chunk: 2 },
                SampleLayout::Complex,
                vec![ChannelCapture {
                    ch: 0,
                    data: vec![0.5, -0.25, 1.0, -1.0],
                    cal: IqCal {
                        scale_i: 0.25,
                        scale_q: 0.3,
                        offset_i: -1.5,
                        offset_q: 2.5,
                    },
                    clipped: false,
                    freq_meter: None,
                }],
            )
            .unwrap(),
        )];
        write(&path, &frames).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back[0].layout(), SampleLayout::Complex);
        assert_eq!(back[0].channels[0].data, frames[0].channels[0].data);
        assert_eq!(back[0].channels[0].cal, frames[0].channels[0].cal);
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = crate::test_scratch("nwc");
        let path = dir.join("bad.nwc");
        std::fs::write(&path, b"not a capture file").unwrap();
        assert!(read(&path).is_err());
    }

    /// Write a one-frame legacy stream by hand; `version` selects the v1
    /// (no timestamp) or v2 layout.
    fn write_legacy(path: &Path, version: u8) {
        let magic = if version == 1 { MAGIC_V1 } else { MAGIC_V2 };
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(magic).unwrap();
        file.write_all(&FLAG_ZSTD.to_le_bytes()).unwrap();
        let mut z = zstd::stream::Encoder::new(file, 0).unwrap();
        z.write_all(&5u64.to_le_bytes()).unwrap();
        z.write_all(&1000.0f64.to_le_bytes()).unwrap();
        z.write_all(&[0u8, 0u8, 1u8]).unwrap(); // acq Sample, avg, n_channels
        if version >= 2 {
            z.write_all(&[0u8]).unwrap(); // no timestamp
            z.write_all(&0.0f64.to_le_bytes()).unwrap();
        }
        z.write_all(&[0u8]).unwrap(); // ch
        z.write_all(&0.02f64.to_le_bytes()).unwrap(); // volts_per_lsb
        z.write_all(&(-1.5f64).to_le_bytes()).unwrap(); // zero_volts
        z.write_all(&[0u8, 0u8]).unwrap(); // clipped, freq_flag
        z.write_all(&0.0f64.to_le_bytes()).unwrap();
        z.write_all(&3u32.to_le_bytes()).unwrap();
        z.write_all(&[0u8, 200u8, 156u8]).unwrap(); // 0, -56, -100 as i8
        z.finish().unwrap().flush().unwrap();
    }

    #[test]
    fn reads_legacy_i8_channels_as_real() {
        let dir = crate::test_scratch("nwc");
        for version in [1u8, 2] {
            let path = dir.join(format!("v{version}.nwc"));
            write_legacy(&path, version);
            let frames = read(&path).unwrap();
            let f = &frames[0];
            assert_eq!(f.layout(), SampleLayout::Real);
            assert_eq!(f.channels[0].cal, IqCal::real(0.02, -1.5));
            assert_eq!(f.channels[0].data, vec![0.0, -56.0, -100.0]);
            // v1 has no timestamp; v2 has an explicit "none".
            assert_eq!(f.t_capture, None);
        }
    }
}
