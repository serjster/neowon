//! `Backend` implementation for the simulated source: paces frames at
//! ~30/s and maps the generic config onto the generator.

use std::sync::Arc;
use std::time::{Duration, Instant};

use neowon_backend::{Backend, BackendError, Capabilities, ScopeConfig};
use neowon_core::SharedFrame;

use crate::{SimChannel, SimSource, Waveform};

pub struct SimBackend {
    src: SimSource,
    caps: Capabilities,
    next_at: Instant,
    interval: Duration,
}

impl SimBackend {
    pub fn new() -> Self {
        Self {
            src: SimSource::default(),
            caps: Capabilities {
                name: "Simulated".into(),
                serial: "sim-0".into(),
                channels: 2,
                sample_rates: vec![2.5e3, 25e3, 250e3, 2.5e6, 25e6, 100e6],
                volts_div: vec![0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0],
                probes: vec![1.0, 10.0, 100.0],
                record_len: crate::SAMPLES,
            },
            next_at: Instant::now(),
            interval: Duration::from_millis(33),
        }
    }
}

impl Default for SimBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for SimBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &ScopeConfig) -> Result<(), BackendError> {
        self.src.sample_rate = cfg.sample_rate;
        self.src.channels = cfg
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| SimChannel {
                enabled: c.enabled,
                // CH1 mimics the probe-comp signal; CH2 a quieter sine.
                waveform: if i == 0 { Waveform::Square } else { Waveform::Sine },
                freq: if i == 0 { 1000.0 } else { 2500.0 },
                amplitude: if i == 0 { 5.0 } else { 1.0 },
                dc: if i == 0 { 2.5 } else { 0.0 },
                range: c.volts_div * 10.0 * c.probe,
                noise: 0.02,
            })
            .collect();
        Ok(())
    }

    fn poll_frame(&mut self, budget: Duration) -> Result<Option<SharedFrame>, BackendError> {
        let now = Instant::now();
        if now < self.next_at {
            let wait = (self.next_at - now).min(budget);
            std::thread::sleep(wait);
            if Instant::now() < self.next_at {
                return Ok(None);
            }
        }
        self.next_at = Instant::now() + self.interval;
        Ok(Some(Arc::new(self.src.next_frame())))
    }
}
