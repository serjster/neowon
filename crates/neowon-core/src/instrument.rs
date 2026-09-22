//! The instrument surface every backend describes itself with (D6): what it
//! can do (`Capabilities`) and the complete state the host wants it in
//! (`InstrumentConfig`). An instrument is either a scope or an SDR; the two
//! share delivery (`Acquisition`) and identity, and nothing else.
//!
//! These live in core rather than `neowon-backend` so capability and config
//! types never depend on the crate that drives them; `neowon-backend`
//! re-exports them.

use crate::{AcqMode, Coupling, Slope, Sweep, TriggerKind};

/// How samples reach the host. This is the difference that most divides
/// instruments, and it decides what the horizontal controls can mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquisition {
    /// Discrete records of a fixed length, separated by dead time the
    /// instrument spends transferring and re-arming. The time one record
    /// covers is `samples / sample_rate`, so spanning more time costs
    /// sample rate — a USB scope with a small buffer.
    Record { samples: usize },
    /// A continuous sample stream, handed over in chunks that are
    /// contiguous in time. There is no record and no dead time; how much
    /// time you can see is bounded only by host memory — a sound card, an
    /// SDR, a logic analyser in streaming mode.
    Stream { chunk: usize },
}

impl Acquisition {
    /// Samples per delivered frame, whichever kind this is.
    pub fn frame_len(self) -> usize {
        match self {
            Acquisition::Record { samples } => samples,
            Acquisition::Stream { chunk } => chunk,
        }
    }

    pub fn is_stream(self) -> bool {
        matches!(self, Acquisition::Stream { .. })
    }
}

/// How much time one streamed frame should carry. A display draws one row
/// per frame, so a fixed chunk count makes the cadence fall with the
/// sample rate; sizing by time keeps it near 20 rows/s everywhere.
const STREAM_FRAME_SECS: f64 = 0.05;
/// Smallest/largest streamed frame, in pairs: 32 KiB…256 KiB of i8 I,Q,
/// the transfer sizes the USB streaming backends carry.
const STREAM_CHUNK_MIN_PAIRS: usize = 16 * 1024;
const STREAM_CHUNK_MAX_PAIRS: usize = 128 * 1024;
/// USB bulk transfers are whole 64-byte packets, i.e. 32 i8 I,Q pairs.
const STREAM_CHUNK_ALIGN_PAIRS: usize = 32;

/// Pairs in one streamed frame at `rate_hz` pairs/s: about
/// `STREAM_FRAME_SECS` of samples, clamped to the bounds above and aligned
/// to the 64-byte transfer grid. Shared by the hardware and simulated SDR
/// backends so both deliver the same frame cadence at any rate.
pub fn stream_chunk_pairs(rate_hz: f64) -> usize {
    let pairs = (rate_hz * STREAM_FRAME_SECS).round().max(0.0) as usize;
    pairs
        .clamp(STREAM_CHUNK_MIN_PAIRS, STREAM_CHUNK_MAX_PAIRS)
        .next_multiple_of(STREAM_CHUNK_ALIGN_PAIRS)
}

/// What a scope can do; the scope UI builds itself from this.
#[derive(Debug, Clone)]
pub struct ScopeCaps {
    pub name: String,
    pub serial: String,
    pub channels: usize,
    /// Supported sample rates, ascending, S/s.
    pub sample_rates: Vec<f64>,
    /// Supported volts/div settings, ascending. Empty when the input range
    /// is not adjustable (a sound card has one full scale).
    pub volts_div: Vec<f64>,
    pub probes: Vec<f64>,
    /// How samples arrive.
    pub acquisition: Acquisition,
    /// Does the instrument find trigger events itself? When false the host
    /// has to, or the display free-runs.
    pub hardware_trigger: bool,
}

impl ScopeCaps {
    /// Samples in one delivered frame. Named for the record it usually is,
    /// but a streaming source answers with its chunk size so the
    /// single-frame display keeps working.
    pub fn record_len(&self) -> usize {
        self.acquisition.frame_len()
    }
}

/// What an SDR receiver can do. Always a complex stream.
#[derive(Debug, Clone)]
pub struct SdrCaps {
    pub name: String,
    pub serial: String,
    /// Tuner chip, as the driver names it (e.g. "R820T").
    pub tuner: String,
    /// Tunable centre-frequency range, Hz, inclusive.
    pub freq_range_hz: (f64, f64),
    /// Supported IQ sample rates, ascending, pairs/s.
    pub sample_rates: Vec<f64>,
    /// Discrete manual tuner gains, ascending, dB.
    pub gains_db: Vec<f64>,
    /// How frames arrive. A stream whose chunk follows the sample rate
    /// advertises the current rate's size here; each frame's own `acq`
    /// carries the chunk it was actually delivered with.
    pub acquisition: Acquisition,
}

/// What an instrument can do. The variant is the instrument's mode.
#[derive(Debug, Clone)]
pub enum Capabilities {
    Scope(ScopeCaps),
    Sdr(SdrCaps),
}

impl Capabilities {
    pub fn name(&self) -> &str {
        match self {
            Capabilities::Scope(c) => &c.name,
            Capabilities::Sdr(c) => &c.name,
        }
    }

    pub fn serial(&self) -> &str {
        match self {
            Capabilities::Scope(c) => &c.serial,
            Capabilities::Sdr(c) => &c.serial,
        }
    }

    pub fn acquisition(&self) -> Acquisition {
        match self {
            Capabilities::Scope(c) => c.acquisition,
            Capabilities::Sdr(c) => c.acquisition,
        }
    }

    pub fn scope(&self) -> Option<&ScopeCaps> {
        match self {
            Capabilities::Scope(c) => Some(c),
            Capabilities::Sdr(_) => None,
        }
    }

    pub fn sdr(&self) -> Option<&SdrCaps> {
        match self {
            Capabilities::Sdr(c) => Some(c),
            Capabilities::Scope(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelConfig {
    pub enabled: bool,
    /// Volts per division at the instrument input (before probe factor).
    pub volts_div: f64,
    pub coupling: Coupling,
    pub probe: f64,
    /// Vertical offset as a fraction of full scale, -0.5..=0.5.
    pub offset: f64,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            volts_div: 1.0,
            coupling: Coupling::Dc,
            probe: 1.0,
            offset: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerConfig {
    /// Zero-based source channel.
    pub source: usize,
    pub kind: TriggerKind,
    /// Level in volts (at the probe tip); the edge/pulse level.
    pub level: f64,
    pub sweep: Sweep,
    /// Trigger holdoff in seconds.
    pub holdoff: f64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            source: 0,
            kind: TriggerKind::Edge {
                slope: Slope::Rising,
            },
            level: 0.0,
            sweep: Sweep::Auto,
            holdoff: 100e-9,
        }
    }
}

/// Complete desired scope state. Backends diff this against what they
/// last applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeConfig {
    pub channels: Vec<ChannelConfig>,
    pub sample_rate: f64,
    pub trigger: TriggerConfig,
    /// Horizontal trigger position, fraction of the record (0.5 = centered).
    pub position: f64,
    pub acq: AcqMode,
    pub running: bool,
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            channels: vec![
                ChannelConfig {
                    enabled: true,
                    ..Default::default()
                },
                ChannelConfig::default(),
            ],
            sample_rate: 250e3,
            trigger: TriggerConfig {
                level: 2.5,
                ..Default::default()
            },
            position: 0.5,
            acq: AcqMode::Sample,
            running: true,
        }
    }
}

/// Tuner gain: the tuner's own gain control, or a fixed setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SdrGain {
    Auto,
    /// dB; backends snap to the nearest entry of `SdrCaps::gains_db`.
    Manual(f64),
}

/// Complete desired SDR state. Demodulation and squelch are host-side and
/// do not belong here.
#[derive(Debug, Clone, PartialEq)]
pub struct SdrConfig {
    pub centre_hz: f64,
    /// IQ pairs/s.
    pub sample_rate: f64,
    pub gain: SdrGain,
    /// The demodulator chip's digital AGC (RTL2832), separate from the
    /// tuner gain.
    pub agc: bool,
    /// Frequency correction, parts per million.
    pub ppm: f64,
    pub running: bool,
}

impl Default for SdrConfig {
    fn default() -> Self {
        Self {
            centre_hz: 100e6,
            sample_rate: 2.048e6,
            gain: SdrGain::Auto,
            agc: false,
            ppm: 0.0,
            running: true,
        }
    }
}

/// Complete desired instrument state; the variant must match the
/// backend's `Capabilities`.
#[derive(Debug, Clone, PartialEq)]
pub enum InstrumentConfig {
    Scope(ScopeConfig),
    Sdr(SdrConfig),
}

impl InstrumentConfig {
    pub fn running(&self) -> bool {
        match self {
            InstrumentConfig::Scope(c) => c.running,
            InstrumentConfig::Sdr(c) => c.running,
        }
    }

    pub fn scope(&self) -> Option<&ScopeConfig> {
        match self {
            InstrumentConfig::Scope(c) => Some(c),
            InstrumentConfig::Sdr(_) => None,
        }
    }

    pub fn sdr(&self) -> Option<&SdrConfig> {
        match self {
            InstrumentConfig::Sdr(c) => Some(c),
            InstrumentConfig::Scope(_) => None,
        }
    }
}

impl From<ScopeConfig> for InstrumentConfig {
    fn from(c: ScopeConfig) -> Self {
        InstrumentConfig::Scope(c)
    }
}

impl From<SdrConfig> for InstrumentConfig {
    fn from(c: SdrConfig) -> Self {
        InstrumentConfig::Sdr(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chunks_are_time_sized_bounded_and_aligned() {
        // Anchors: at 250 kS/s the floor binds (65.5 ms per frame), at
        // 1.024 and 2.048 MS/s the frame is the plain 50 ms.
        assert_eq!(stream_chunk_pairs(250e3), 16_384);
        assert_eq!(stream_chunk_pairs(1.024e6), 51_200);
        assert_eq!(stream_chunk_pairs(2.048e6), 102_400);
        // Bounds, including degenerate rates.
        assert_eq!(stream_chunk_pairs(0.0), 16_384);
        assert_eq!(stream_chunk_pairs(1e9), 131_072);
        // Non-decreasing in rate, on the 64-byte (32-pair) grid.
        let rates = [250e3, 1.024e6, 1.536e6, 1.92e6, 2.048e6, 2.56e6, 3.2e6];
        for w in rates.windows(2) {
            assert!(
                stream_chunk_pairs(w[0]) <= stream_chunk_pairs(w[1]),
                "{w:?}"
            );
        }
        for r in rates {
            assert_eq!(stream_chunk_pairs(r) % 32, 0, "{r}");
        }
    }
}
