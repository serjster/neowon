//! Minimal RIFF/WAVE PCM16 reader + writer — enough for the XY demo files
//! and capture export, with no external dependency.

use std::io::{self, Read, Write};
use std::path::Path;

/// Read a 16-bit PCM WAV; returns (sample_rate, per-frame channel samples
/// normalized to -1.0..1.0). Mono files duplicate the channel.
pub fn read_pcm16(path: &Path) -> io::Result<(u32, Vec<(f32, f32)>)> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if buf.len() < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let (mut rate, mut channels) = (0u32, 0u16);
    let mut data: Option<&[u8]> = None;
    let mut off = 12;
    while off + 8 <= buf.len() {
        let id = &buf[off..off + 4];
        let size = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
        let body = buf
            .get(off + 8..off + 8 + size)
            .ok_or_else(|| bad("truncated chunk"))?;
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err(bad("short fmt chunk"));
                }
                let format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                if format != 1 || bits != 16 || channels == 0 || channels > 2 {
                    return Err(bad("only mono/stereo 16-bit PCM is supported"));
                }
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are word-aligned.
        off += 8 + size + (size & 1);
    }
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if rate == 0 {
        return Err(bad("no fmt chunk"));
    }
    let stride = 2 * channels as usize;
    let frames = data
        .chunks_exact(stride)
        .map(|f| {
            let l = i16::from_le_bytes(f[0..2].try_into().unwrap()) as f32 / 32768.0;
            let r = if channels == 2 {
                i16::from_le_bytes(f[2..4].try_into().unwrap()) as f32 / 32768.0
            } else {
                l
            };
            (l, r)
        })
        .collect();
    Ok((rate, frames))
}

/// Write 16-bit PCM (1 or 2 channels; truncated to the shortest).
pub fn write_pcm16(path: &Path, rate: u32, channels: &[&[i16]]) -> io::Result<()> {
    assert!(matches!(channels.len(), 1 | 2), "1 or 2 channels");
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let ch = channels.len() as u16;
    let data_len = (n * ch as usize * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&(ch * 2).to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        for c in channels {
            out.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_stereo() {
        let dir = std::env::temp_dir().join("neowon-wav-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.wav");
        let left: Vec<i16> = (0..1000).map(|i| (i * 13 % 32767) as i16).collect();
        let right: Vec<i16> = (0..1000).map(|i| -((i * 7 % 32767) as i16)).collect();
        write_pcm16(&path, 48_000, &[&left, &right]).unwrap();
        let (rate, frames) = read_pcm16(&path).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(frames.len(), 1000);
        for (i, &(l, r)) in frames.iter().enumerate() {
            assert!((l - left[i] as f32 / 32768.0).abs() < 1e-6);
            assert!((r - right[i] as f32 / 32768.0).abs() < 1e-6);
        }
    }
}
