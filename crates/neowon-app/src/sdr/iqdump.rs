//! Raw IQ capture (`sdr iqdump <path> <seconds>`): write the complex frames
//! the streaming consumers receive to a file, so a hardware session can be
//! replayed offline. Format: interleaved `f32` little-endian, I then Q, at
//! the link's sample rate at capture time (`get sdr` reports it); no header,
//! because the reader is an instrument test, not an interchange format.
//!
//! The dump taps the same frames `sdr::dab::feed` sees (every frame that
//! arrives, not the latest-wins display path), so a capture is exactly what
//! the decoder was fed.
//!
//! The stream goes to a temp beside `path` and is renamed into place when
//! the dump completes or is stopped (`neowon_core::atomic_file`), so the
//! file appears whole; a dump that fails part-way, or is still running
//! when the app exits, leaves no file.

use std::io::Write;

use neowon_core::CaptureFrame;
use neowon_core::atomic_file::AtomicFile;

use super::SdrState;

pub struct IqDump {
    file: AtomicFile,
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
        let file =
            AtomicFile::create(path).map_err(|e| format!("iqdump: cannot write {path}: {e}"))?;
        Ok(Self {
            file,
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

    /// Put what was written in place at `path`, whole.
    fn finish(self) -> Result<String, String> {
        self.file
            .commit()
            .map_err(|e| format!("iqdump: finish {}: {e}", self.path))?;
        Ok(self.path)
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
            let (pairs, rate) = (dump.written_pairs, dump.sample_rate);
            match dump.finish() {
                Ok(path) => {
                    tracing::info!("iqdump: {path} complete, {pairs} pairs at {rate:.0} Hz")
                }
                Err(e) => tracing::error!("{e}"),
            }
        }
        Ok(false) => sdr.iq_dump = Some(dump),
        Err(e) => tracing::error!("{e}"),
    }
}

/// Stop an active dump, putting what was written in place.
pub fn stop(sdr: &mut SdrState) -> Option<String> {
    let dump = sdr.iq_dump.take()?;
    let path = dump.path.clone();
    if let Err(e) = dump.finish() {
        tracing::error!("{e}");
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_writes_exactly_the_requested_pairs() {
        let path = std::env::temp_dir().join(format!("neowon-iqdump-{}.f32", std::process::id()));
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let mut dump = IqDump::start(&path, 0.5, 8.0).expect("start");
        assert!(!dump.write(&[1.0, 2.0, 3.0, 4.0]).expect("write"));
        assert!(dump.write(&[5.0, 6.0, 7.0, 8.0]).expect("write"));
        assert_eq!(dump.written_pairs, 4);
        assert!(
            !std::path::Path::new(&path).exists(),
            "the dump appeared before it was whole"
        );
        dump.finish().expect("finish");
        let bytes = std::fs::read(&path).expect("read back");
        assert_eq!(bytes.len(), 4 * 8);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(bytes[28..32].try_into().unwrap()), 8.0);
        let _ = std::fs::remove_file(&path);
    }
}
