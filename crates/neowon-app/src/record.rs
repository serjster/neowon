//! Capture recording + export. Records the stream of records into memory
//! and exports the concatenated samples as WAV (16-bit PCM at the
//! acquisition rate — CH1 = left, CH2 = right when both are on), raw i8
//! per channel, or CSV. Records are discrete acquisitions, so consecutive
//! frames are generally not phase-continuous unless the source is (roll
//! mode / WAV playback).

use bevy::prelude::*;
use neowon_core::SharedFrame;

use crate::Link;

/// Bounded so a forgotten recording can't eat the machine: ~20 min at
/// 36 records/s.
const MAX_FRAMES: usize = 40_000;

#[derive(Resource, Default)]
pub struct Recorder {
    pub on: bool,
    pub frames: Vec<SharedFrame>,
    last_seq: u64,
    /// Last export destination, for the UI.
    pub last_export: Option<String>,
}

impl Recorder {
    pub fn clear(&mut self) {
        self.frames.clear();
        self.last_seq = 0;
    }

    pub fn samples_per_channel(&self) -> usize {
        self.frames
            .iter()
            .map(|f| f.channels.first().map_or(0, |c| c.raw.len()))
            .sum()
    }

    pub fn seconds(&self) -> f64 {
        let rate = self
            .frames
            .last()
            .map(|f| f.sample_rate)
            .unwrap_or(1.0)
            .max(1.0);
        self.samples_per_channel() as f64 / rate
    }

    /// Concatenated samples of channel slot `ch`, as 16-bit PCM (i8 << 8).
    fn pcm16(&self, ch: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(self.samples_per_channel());
        for f in &self.frames {
            if let Some(cap) = f.channels.iter().find(|c| c.ch == ch) {
                out.extend(cap.raw.iter().map(|&r| (r as i16) << 8));
            }
        }
        out
    }

    fn rate(&self) -> u32 {
        // The last frame's rate: a recording that started just before a
        // stimulus/timebase switch should be stamped with where it ended up.
        self.frames
            .last()
            .map(|f| f.sample_rate.round() as u32)
            .unwrap_or(48_000)
    }

    /// Export as WAV: stereo when both channels recorded, else mono.
    pub fn export_wav(&self, path: &std::path::Path) -> std::io::Result<()> {
        let ch0 = self.pcm16(0);
        let ch1 = self.pcm16(1);
        if !ch0.is_empty() && !ch1.is_empty() {
            neowon_core::wav::write_pcm16(path, self.rate(), &[&ch0, &ch1])
        } else {
            let mono = if ch0.is_empty() { &ch1 } else { &ch0 };
            neowon_core::wav::write_pcm16(path, self.rate(), &[mono])
        }
    }

    /// Export raw i8 samples, one `<stem>_chN.raw` file per recorded channel.
    pub fn export_raw(&self, base: &std::path::Path) -> std::io::Result<Vec<String>> {
        let mut written = Vec::new();
        for ch in 0..3 {
            let data: Vec<u8> = self
                .frames
                .iter()
                .filter_map(|f| f.channels.iter().find(|c| c.ch == ch))
                .flat_map(|c| c.raw.iter().map(|&r| r as u8))
                .collect();
            if data.is_empty() {
                continue;
            }
            let path = base.with_file_name(format!(
                "{}_ch{}.raw",
                base.file_stem().unwrap_or_default().to_string_lossy(),
                ch + 1
            ));
            std::fs::write(&path, &data)?;
            written.push(path.display().to_string());
        }
        Ok(written)
    }

    /// Export CSV: time plus one volts column per recorded channel.
    pub fn export_csv(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        let rate = self.rate() as f64;
        writeln!(f, "t,ch1_v,ch2_v")?;
        let mut i = 0usize;
        for frame in &self.frames {
            let c0 = frame.channels.iter().find(|c| c.ch == 0);
            let c1 = frame.channels.iter().find(|c| c.ch == 1);
            let n = c0.or(c1).map_or(0, |c| c.raw.len());
            for k in 0..n {
                let v = |c: Option<&neowon_core::ChannelCapture>| {
                    c.and_then(|c| {
                        c.raw
                            .get(k)
                            .map(|&r| r as f64 * c.volts_per_lsb + c.zero_volts)
                    })
                    .map_or(String::new(), |v| format!("{v:.6}"))
                };
                writeln!(f, "{:.9},{},{}", i as f64 / rate, v(c0), v(c1))?;
                i += 1;
            }
        }
        Ok(())
    }
}

/// Default export directory: `~/neowon-captures`.
pub fn export_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(home).join("neowon-captures");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Timestamped default file stem.
pub fn default_stem() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("capture-{secs}")
}

/// Append new records while recording is on.
pub fn record_frames(link: Res<Link>, mut rec: ResMut<Recorder>) {
    if !rec.on {
        return;
    }
    let Some(frame) = &link.latest else { return };
    if frame.seq == rec.last_seq || rec.frames.len() >= MAX_FRAMES {
        return;
    }
    rec.last_seq = frame.seq;
    rec.frames.push(frame.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_core::{AcqMode, CaptureFrame, ChannelCapture};
    use std::sync::Arc;

    fn frame(seq: u64, vals: &[i8]) -> SharedFrame {
        Arc::new(CaptureFrame {
            seq,
            sample_rate: 1000.0,
            acq: AcqMode::Sample,
            channels: vec![ChannelCapture {
                ch: 0,
                raw: vals.to_vec(),
                volts_per_lsb: 0.01,
                zero_volts: 0.0,
                clipped: false,
                freq_meter: None,
            }],
        })
    }

    #[test]
    fn wav_export_round_trips() {
        let mut rec = Recorder::default();
        rec.frames.push(frame(1, &[0, 50, -50, 100]));
        rec.frames.push(frame(2, &[-100, 25, 0, 0]));
        assert_eq!(rec.samples_per_channel(), 8);
        let dir = std::env::temp_dir().join("neowon-rec-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.wav");
        rec.export_wav(&path).unwrap();
        let (rate, frames) = neowon_core::wav::read_pcm16(&path).unwrap();
        assert_eq!(rate, 1000);
        assert_eq!(frames.len(), 8);
        // i8 50 << 8 = 12800 -> 12800/32768
        assert!((frames[1].0 - 12800.0 / 32768.0).abs() < 1e-4);
    }
}
