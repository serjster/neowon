//! The acquisition-backend abstraction: a device-agnostic config model, a
//! `Backend` trait implemented per instrument, and a `Supervisor` that owns a
//! backend on its own thread — including reconnect-and-replay when the
//! hardware goes away.

use std::time::Duration;

use neowon_core::{AcqMode, Coupling, SharedFrame, Slope, Sweep, TriggerKind};

pub mod supervisor;

pub use supervisor::{spawn, Command, Event, Supervisor};

/// What an instrument can do; the UI builds itself from this.
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub name: String,
    pub serial: String,
    pub channels: usize,
    /// Supported sample rates, ascending, S/s.
    pub sample_rates: Vec<f64>,
    /// Supported volts/div settings, ascending.
    pub volts_div: Vec<f64>,
    pub probes: Vec<f64>,
    /// Samples per record.
    pub record_len: usize,
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
        Self { enabled: false, volts_div: 1.0, coupling: Coupling::Dc, probe: 1.0, offset: 0.0 }
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
            kind: TriggerKind::Edge { slope: Slope::Rising },
            level: 0.0,
            sweep: Sweep::Auto,
            holdoff: 100e-9,
        }
    }
}

/// Function of the MULTI (aux) BNC port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiMode {
    TriggerOut,
    PassFailOut,
    TriggerIn,
}

/// Complete desired instrument state. Backends diff this against what they
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
                ChannelConfig { enabled: true, ..Default::default() },
                ChannelConfig::default(),
            ],
            sample_rate: 250e3,
            trigger: TriggerConfig { level: 2.5, ..Default::default() },
            position: 0.5,
            acq: AcqMode::Sample,
            running: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The connection is gone; the supervisor drops the backend and
    /// reconnects.
    #[error("fatal: {0}")]
    Fatal(String),
    /// Something recoverable; logged and carried on.
    #[error("transient: {0}")]
    Transient(String),
}

pub trait Backend: Send {
    fn capabilities(&self) -> &Capabilities;

    /// Drive the instrument to `cfg`. Called from the supervisor thread.
    fn apply(&mut self, cfg: &ScopeConfig) -> Result<(), BackendError>;

    /// Wait up to `budget` for the next frame. `Ok(None)` means no data yet.
    fn poll_frame(&mut self, budget: Duration) -> Result<Option<SharedFrame>, BackendError>;

    /// Periodic upkeep while not acquiring (keep-alives etc.).
    fn idle(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Force a trigger event on instruments that support it.
    fn force_trigger(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Configure the MULTI (aux) port; no-op on instruments without one.
    fn set_multi(&mut self, _mode: MultiMode) -> Result<(), BackendError> {
        Ok(())
    }

    /// Drive the pass/fail TTL output (MULTI port in pass-fail mode).
    fn set_pass_fail_output(&mut self, _level: bool) -> Result<(), BackendError> {
        Ok(())
    }

    /// Select a named stimulus/scenario on backends that generate their own
    /// signal (the simulator; a future AWG). Returns false if unknown or
    /// unsupported.
    fn set_stimulus(&mut self, _name: &str) -> Result<bool, BackendError> {
        Ok(false)
    }

    /// Stimulus names this backend accepts (empty = none).
    fn stimuli(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Probe the signal and pick sensible settings. Returns the new config
    /// (already applied to the instrument) or `None` if unsupported / no
    /// signal found.
    fn autoset(&mut self) -> Result<Option<ScopeConfig>, BackendError> {
        Ok(None)
    }
}
