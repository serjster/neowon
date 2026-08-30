use std::sync::Arc;

/// One acquisition record: a set of simultaneously captured channel traces.
///
/// Frames are immutable once produced and shared by `Arc` between the
/// acquisition thread, DSP consumers, and the renderer.
#[derive(Debug, Clone)]
pub struct CaptureFrame {
    /// Monotonic sequence number assigned by the producing backend.
    pub seq: u64,
    /// When this record's **first sample** was taken, in seconds on a
    /// monotonic per-session clock. `None` when the producer cannot say.
    ///
    /// This is what lets consumers place records on a real time axis
    /// instead of assuming they are contiguous — they generally are not.
    /// Accuracy is producer-defined: a streaming source knows it exactly,
    /// while a triggered instrument that is polled over USB can only report
    /// arrival minus record duration, which is biased late by up to one poll
    /// interval.
    pub t_capture: Option<f64>,
    /// Actual sample rate of this record, in samples/second.
    pub sample_rate: f64,
    /// How the samples were produced (affects interpretation: peak-detect
    /// records are min/max pairs).
    pub acq: crate::AcqMode,
    pub channels: Vec<ChannelCapture>,
}

pub type SharedFrame = Arc<CaptureFrame>;

impl CaptureFrame {
    /// Seconds of signal in this record.
    pub fn duration(&self) -> f64 {
        let n = self.channels.first().map_or(0, |c| c.raw.len());
        n as f64 / self.sample_rate.max(1e-12)
    }

    /// Start time on the session clock, falling back to a contiguous
    /// estimate from `seq` when the producer could not stamp one. Loaders of
    /// stored captures use the fallback, so a file with no timestamps still
    /// lands on a sensible axis — it just cannot show real gaps.
    pub fn t_start(&self) -> f64 {
        self.t_capture
            .unwrap_or_else(|| self.seq as f64 * self.duration())
    }
}

/// A single channel's samples within a frame.
///
/// Samples use the scope convention: signed 8-bit, where ±125 spans the
/// full vertical range (10 divisions). Simulated backends produce the same
/// encoding so every consumer has one code path.
#[derive(Debug, Clone)]
pub struct ChannelCapture {
    /// Zero-based channel index.
    pub ch: usize,
    pub raw: Vec<i8>,
    /// Volts represented by one ADC count: `range * probe / 250`.
    pub volts_per_lsb: f64,
    /// Voltage at raw == 0 (screen center): `-offset_frac * range * probe`.
    pub zero_volts: f64,
    /// True if any sample sits at the ADC rails (|raw| >= 125).
    pub clipped: bool,
    /// Hardware frequency-meter reading, when the backend provides one.
    pub freq_meter: Option<f64>,
}

impl ChannelCapture {
    pub fn volts_at(&self, i: usize) -> f64 {
        self.raw[i] as f64 * self.volts_per_lsb + self.zero_volts
    }

    pub fn iter_volts(&self) -> impl Iterator<Item = f64> + '_ {
        self.raw
            .iter()
            .map(|&r| r as f64 * self.volts_per_lsb + self.zero_volts)
    }
}
