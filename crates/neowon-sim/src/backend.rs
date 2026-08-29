//! `Backend` implementation for the simulated source: paces frames at
//! ~30/s, maps the generic config onto the generator, and honors edge
//! triggering like real hardware (including starving on impossible levels).

use std::sync::Arc;
use std::time::{Duration, Instant};

use neowon_backend::{Backend, BackendError, Capabilities, ScopeConfig};
use neowon_core::{CaptureFrame, SharedFrame, Slope, Sweep, TriggerKind};

use crate::{SAMPLES, Scenario, SimSource};

pub struct SimBackend {
    src: SimSource,
    caps: Capabilities,
    cfg: ScopeConfig,
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
            cfg: ScopeConfig::default(),
            next_at: Instant::now(),
            interval: Duration::from_millis(33),
        }
    }

    /// Next record honoring the applied trigger. `None` = the trigger is
    /// starving, exactly like hardware in Normal sweep.
    fn next_record(&mut self) -> Option<CaptureFrame> {
        let trig = self.cfg.trigger;
        let slope = match trig.kind {
            TriggerKind::Edge { slope } => slope,
            // Only edge triggering is simulated; pulse/slope/video kinds
            // free-run like Auto.
            _ => return Some(self.src.next_frame()),
        };
        if trig.sweep == Sweep::Auto {
            return Some(self.src.next_frame());
        }
        self.triggered_record(trig.source, slope, trig.level)
    }

    /// Find the first edge crossing `level` with `slope` (noise-free scan of
    /// up to 2 record lengths), then regenerate the record time-shifted so
    /// the crossing lands on the trigger position index.
    fn triggered_record(
        &mut self,
        source: usize,
        slope: Slope,
        level: f64,
    ) -> Option<CaptureFrame> {
        if source > 1 || !self.cfg.channels[source].enabled {
            return None;
        }
        let trig_idx = (self.cfg.position * SAMPLES as f64)
            .round()
            .clamp(1.0, (SAMPLES - 2) as f64);
        let dt = 1.0 / self.src.sample_rate;
        let t0 = self.src.time();
        let mut prev = self.src.volts_quiet(t0)[source];
        for i in 1..(2 * SAMPLES) {
            let v = self.src.volts_quiet(t0 + i as f64 * dt)[source];
            let crossed = match slope {
                Slope::Rising => prev < level && v >= level,
                Slope::Falling => prev > level && v <= level,
            };
            if crossed {
                let frac = (level - prev) / (v - prev);
                let crossing = (i - 1) as f64 + frac;
                self.src.set_time(t0 + (crossing - trig_idx) * dt);
                return Some(self.src.next_frame());
            }
            prev = v;
        }
        None
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
        for (i, c) in cfg.channels.iter().enumerate().take(2) {
            self.src.set_enabled(i, c.enabled);
            self.src.set_range(i, c.volts_div * 10.0 * c.probe);
        }
        self.cfg = cfg.clone();
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
        Ok(self.next_record().map(Arc::new))
    }

    fn set_stimulus(&mut self, name: &str) -> Result<bool, BackendError> {
        match Scenario::preset(name) {
            Some(s) => {
                self.src.set_scenario(s);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn stimuli(&self) -> Vec<&'static str> {
        Scenario::PRESETS.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_backend::{ChannelConfig, TriggerConfig};

    fn armed(level: f64, sweep: Sweep) -> SimBackend {
        let mut b = SimBackend::new();
        assert!(b.set_stimulus("sine-1k").unwrap());
        let cfg = ScopeConfig {
            sample_rate: 250e3,
            channels: vec![
                ChannelConfig {
                    enabled: true,
                    volts_div: 1.0,
                    ..Default::default()
                },
                ChannelConfig::default(),
            ],
            trigger: TriggerConfig {
                source: 0,
                kind: TriggerKind::Edge {
                    slope: Slope::Rising,
                },
                level,
                sweep,
                holdoff: 100e-9,
            },
            position: 0.5,
            ..Default::default()
        };
        b.apply(&cfg).unwrap();
        b
    }

    #[test]
    fn normal_sweep_aligns_trigger() {
        let mut b = armed(0.0, Sweep::Normal);
        let frame = b.poll_frame(Duration::from_millis(200)).unwrap().unwrap();
        let raw = &frame.channels[0].raw;
        // Crossing placed at index 2500; 0 V = 0 counts at 10 V range.
        assert!(raw[2500].unsigned_abs() <= 4, "raw[2500] = {}", raw[2500]);
        assert!(raw[2510] > raw[2490], "not rising at the trigger point");
    }

    #[test]
    fn normal_sweep_starves_on_impossible_level() {
        let mut b = armed(10.0, Sweep::Normal);
        assert!(b.poll_frame(Duration::from_millis(200)).unwrap().is_none());
    }

    #[test]
    fn auto_sweep_ignores_level() {
        let mut b = armed(10.0, Sweep::Auto);
        assert!(b.poll_frame(Duration::from_millis(200)).unwrap().is_some());
    }

    #[test]
    fn stimulus_selection() {
        let mut b = SimBackend::new();
        assert!(b.set_stimulus("xy-heart").unwrap());
        assert!(!b.set_stimulus("no-such").unwrap());
        assert!(b.stimuli().contains(&"xy-heart"));
    }
}
