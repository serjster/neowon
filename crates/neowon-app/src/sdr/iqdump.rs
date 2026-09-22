//! Raw IQ capture (`sdr iqdump <path> <seconds>`): write the complex frames
//! the streaming consumers receive to a file, so a hardware session can be
//! replayed offline. Format: interleaved `f32` little-endian, I then Q, at
//! the link's sample rate at capture time (`get sdr` reports it); no header,
//! because the reader is an instrument test, not an interchange format.
//!
//! The dump taps the same frames `sdr::dab::feed` sees (every frame that
//! arrives, not the latest-wins display path), so a capture is exactly what
//! the decoder was fed.

use std::fs::File;
use std::io::{BufWriter, Write};

use neowon_core::CaptureFrame;

use super::SdrState;

pub struct IqDump {
    file: BufWriter<File>,
    pub path: String,
    pub sample_rate: f64,
    pub remaining_pairs: u64,
    pub written_pairs: u64,
}

impl IqDump {
    pub fn start(path: &str, seconds: f64, sample_rate: f64) -> Result<Self, String> {
        if !(0.1..=600.0).contains(&seconds) {
            return Err(format!("iqdump: {seconds} s outside 0.1..=600"));
        }
        let file = File::create(path).map_err(|e| format!("iqdump: cannot write {path}: {e}"))?;
        Ok(Self {
            file: BufWriter::new(file),
            path: path.to_string(),
            sample_rate,
            remaining_pairs: (seconds * sample_rate).round() as u64,
            written_pairs: 0,
        })
    }

    /// Append one frame's interleaved pairs. Returns `true` when the dump has
    /// written the requested length.
    fn write(&mut self, interleaved: &[f32]) -> Result<bool, String> {
        let pairs = (interleaved.len() / 2) as u64;
        let take = pairs.min(self.remaining_pairs) as usize;
        let mut bytes = Vec::with_capacity(take * 8);
        for value in &interleaved[..take * 2] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("iqdump: write {}: {e}", self.path))?;
        self.written_pairs += take as u64;
        self.remaining_pairs = self.remaining_pairs.saturating_sub(take as u64);
        Ok(self.remaining_pairs == 0)
    }
}

/// Feed one arriving complex frame to the active dump, if any. Finishing or
/// failing ends the dump: a capture that silently stopped short would be
/// worse than none.
pub fn write(sdr: &mut SdrState, frame: &CaptureFrame) {
    let Some(mut dump) = sdr.iq_dump.take() else {
        return;
    };
    match dump.write(&frame.channels[0].data) {
        Ok(true) => {
            if let Err(e) = dump.file.flush() {
                tracing::error!("iqdump: flush {}: {e}", dump.path);
            }
            tracing::info!(
                "iqdump: {} complete, {} pairs at {:.0} Hz",
                dump.path,
                dump.written_pairs,
                dump.sample_rate
            );
        }
        Ok(false) => sdr.iq_dump = Some(dump),
        Err(e) => tracing::error!("{e}"),
    }
}

/// Stop an active dump, flushing what was written.
pub fn stop(sdr: &mut SdrState) -> Option<String> {
    let dump = sdr.iq_dump.take()?;
    if let Err(e) = dump.file.into_inner() {
        tracing::error!("iqdump: flush {}: {e}", dump.path);
    }
    Some(dump.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_writes_exactly_the_requested_pairs() {
        let path = std::env::temp_dir().join(format!("neowon-iqdump-{}.f32", std::process::id()));
        let path = path.to_string_lossy().to_string();
        let mut dump = IqDump::start(&path, 0.5, 8.0).expect("start");
        assert!(!dump.write(&[1.0, 2.0, 3.0, 4.0]).expect("write"));
        assert!(dump.write(&[5.0, 6.0, 7.0, 8.0]).expect("write"));
        assert_eq!(dump.written_pairs, 4);
        dump.file.flush().expect("flush");
        let bytes = std::fs::read(&path).expect("read back");
        assert_eq!(bytes.len(), 4 * 8);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(bytes[28..32].try_into().unwrap()), 8.0);
        let _ = std::fs::remove_file(&path);
    }
}
