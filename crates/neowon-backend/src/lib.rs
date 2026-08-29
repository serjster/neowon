//! The acquisition-backend abstraction: a device-agnostic config model, a
//! `Backend` trait implemented per instrument, and a `Supervisor` that owns a
//! backend on its own thread — including reconnect-and-replay when the
//! hardware goes away.

use std::time::Duration;

use neowon_core::{Coupling, SharedFrame, Slope, Sweep};

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
    pub slope: Slope,
    /// Level in volts (at the probe tip).
    pub level: f64,
    pub sweep: Sweep,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self { source: 0, slope: Slope::Rising, level: 0.0, sweep: Sweep::Auto }
    }
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
}
