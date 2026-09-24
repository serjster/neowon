//! The acquisition-backend abstraction: a device-agnostic config model, a
//! `Backend` trait implemented per instrument, and a `Supervisor` that owns a
//! backend on its own thread — including reconnect-and-replay when the
//! hardware goes away.

use std::time::Duration;

use neowon_core::SharedFrame;
pub use neowon_core::{
    Acquisition, Capabilities, ChannelConfig, InstrumentConfig, ScopeCaps, ScopeConfig, SdrCaps,
    SdrConfig, SdrGain, TriggerConfig,
};

pub mod supervisor;

pub use supervisor::{Command, Event, Supervisor, spawn};

/// Function of the MULTI (aux) BNC port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiMode {
    TriggerOut,
    PassFailOut,
    TriggerIn,
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

/// The scope half of `cfg`, or the error a scope backend returns when
/// handed an SDR config.
pub fn scope_config(cfg: &InstrumentConfig) -> Result<&ScopeConfig, BackendError> {
    cfg.scope()
        .ok_or_else(|| BackendError::Transient("SDR config sent to a scope backend".into()))
}

/// The SDR half of `cfg`, or the error an SDR backend returns when handed
/// a scope config. The mirror of [`scope_config`]: one refusal, one
/// message, whichever instrument the backend drives.
pub fn sdr_config(cfg: &InstrumentConfig) -> Result<&SdrConfig, BackendError> {
    cfg.sdr()
        .ok_or_else(|| BackendError::Transient("scope config sent to an SDR backend".into()))
}

pub trait Backend: Send {
    fn capabilities(&self) -> &Capabilities;

    /// Drive the instrument to `cfg`. Called from the supervisor thread.
    /// A config for the other instrument mode is an error (see
    /// [`scope_config`] and [`sdr_config`]).
    fn apply(&mut self, cfg: &InstrumentConfig) -> Result<(), BackendError>;

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

    /// Reseed the deterministic generator on generating backends. Returns
    /// false where there is none (hardware).
    fn set_seed(&mut self, _seed: u64) -> Result<bool, BackendError> {
        Ok(false)
    }

    /// Probe the signal and pick sensible settings. Returns the new config
    /// (already applied to the instrument) or `None` if unsupported / no
    /// signal found.
    fn autoset(&mut self) -> Result<Option<ScopeConfig>, BackendError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_backends_refuse_sdr_config() {
        let scope = InstrumentConfig::from(ScopeConfig::default());
        assert_eq!(scope_config(&scope).unwrap(), &ScopeConfig::default());
        let sdr = InstrumentConfig::from(SdrConfig::default());
        assert!(matches!(
            scope_config(&sdr),
            Err(BackendError::Transient(_))
        ));
    }

    #[test]
    fn sdr_backends_refuse_scope_config() {
        let sdr = InstrumentConfig::from(SdrConfig::default());
        assert_eq!(sdr_config(&sdr).unwrap(), &SdrConfig::default());
        let scope = InstrumentConfig::from(ScopeConfig::default());
        assert!(matches!(
            sdr_config(&scope),
            Err(BackendError::Transient(_))
        ));
    }
}
