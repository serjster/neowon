//! Simulated SDR: deterministic RF scenes behind the `Backend` trait.
//!
//! A scene places emitters at absolute RF frequencies; tuning selects which
//! of them fall inside `centre ± rate/2` and turns them into the baseband
//! `IqScene`. Samples come from the D8 generator, so they are a pure
//! function of (seed, sample index): pacing uses the wall clock, the signal
//! never does. Gain and AGC are accepted and ignored (the scene is already
//! in full-scale units); ppm shifts the band the way a real crystal
//! correction does, by (centre)·ppm upwards.
//!
//! Scene names are a stable API like the scope's stimulus presets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use neowon_backend::{
    Acquisition, Backend, BackendError, Capabilities, InstrumentConfig, SdrCaps, SdrConfig,
};
use neowon_core::SharedFrame;

use crate::iq::{IqComponent, IqScene};

/// Pairs per frame (32 ms at 2.048 MS/s).
pub const CHUNK_PAIRS: usize = 64 * 1024;

/// A transmitter in a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub freq_hz: f64,
    /// Full-scale units.
    pub amplitude: f64,
}

const fn em(freq_hz: f64, amplitude: f64) -> Emitter {
    Emitter { freq_hz, amplitude }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RfScene {
    pub emitters: Vec<Emitter>,
    /// Complex noise RMS, full-scale units.
    pub noise_rms: f64,
}

impl RfScene {
    pub const PRESETS: [&'static str; 4] = ["rf-reference", "rf-fm-band", "rf-hf", "rf-noise"];

    pub fn preset(name: &str) -> Option<Self> {
        let (emitters, noise_rms) = match name {
            // Tuned to the default 100 MHz this is IqScene::reference.
            "rf-reference" => (vec![em(100.1e6, 0.5)], 0.05),
            "rf-fm-band" => (
                vec![
                    em(88.5e6, 0.10),
                    em(91.3e6, 0.05),
                    em(94.9e6, 0.20),
                    em(98.3e6, 0.15),
                    em(99.4e6, 0.30),
                    em(101.7e6, 0.08),
                    em(104.1e6, 0.12),
                    em(106.6e6, 0.25),
                ],
                0.02,
            ),
            "rf-hf" => (
                vec![em(7.1e6, 0.05), em(9.64e6, 0.2), em(11.8e6, 0.1)],
                0.01,
            ),
            "rf-noise" => (Vec::new(), 0.05),
            _ => return None,
        };
        Some(Self {
            emitters,
            noise_rms,
        })
    }

    /// The baseband this scene produces tuned to `centre_hz` at `rate`
    /// pairs/s with `ppm` crystal correction.
    pub fn baseband(&self, centre_hz: f64, rate: f64, ppm: f64) -> IqScene {
        let shift = centre_hz * ppm * 1e-6;
        IqScene {
            sample_rate: rate,
            components: self
                .emitters
                .iter()
                .map(|e| e.freq_hz - centre_hz + shift)
                .zip(&self.emitters)
                .filter(|(off, _)| off.abs() < rate / 2.0)
                .map(|(offset_hz, e)| IqComponent::Tone {
                    offset_hz,
                    amplitude: e.amplitude,
                    phase: 0.0,
                })
                .collect(),
            noise_rms: self.noise_rms,
        }
    }
}

pub struct SimSdrBackend {
    caps: Capabilities,
    scene: RfScene,
    cfg: SdrConfig,
    baseband: IqScene,
    seed: u64,
    /// Next sample index; frames are contiguous in it.
    index: u64,
    seq: u64,
    next_at: Instant,
}

impl SimSdrBackend {
    pub fn new() -> Self {
        let scene = RfScene::preset("rf-reference").expect("preset");
        let cfg = SdrConfig::default();
        Self {
            caps: Capabilities::Sdr(SdrCaps {
                name: "Simulated SDR".into(),
                serial: "sim-sdr-0".into(),
                tuner: "sim".into(),
                freq_range_hz: (500e3, 1.766e9),
                sample_rates: vec![
                    250e3, 1.024e6, 1.536e6, 1.792e6, 1.92e6, 2.048e6, 2.16e6, 2.4e6, 2.56e6,
                    2.88e6, 3.2e6,
                ],
                gains_db: vec![
                    0.0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9,
                    25.4, 28.0, 29.7, 32.8, 33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5,
                    48.0, 49.6,
                ],
                acquisition: Acquisition::Stream { chunk: CHUNK_PAIRS },
            }),
            baseband: scene.baseband(cfg.centre_hz, cfg.sample_rate, cfg.ppm),
            scene,
            cfg,
            seed: 1,
            index: 0,
            seq: 0,
            next_at: Instant::now(),
        }
    }

    fn rebuild(&mut self) {
        self.baseband = self
            .scene
            .baseband(self.cfg.centre_hz, self.cfg.sample_rate, self.cfg.ppm);
    }
}

impl Default for SimSdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for SimSdrBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &InstrumentConfig) -> Result<(), BackendError> {
        let c = cfg
            .sdr()
            .ok_or_else(|| BackendError::Transient("scope config sent to an SDR backend".into()))?;
        if c.sample_rate <= 0.0 {
            return Err(BackendError::Transient(
                "sample rate must be positive".into(),
            ));
        }
        self.cfg = c.clone();
        self.rebuild();
        Ok(())
    }

    fn poll_frame(&mut self, budget: Duration) -> Result<Option<SharedFrame>, BackendError> {
        if !self.cfg.running {
            std::thread::sleep(budget);
            return Ok(None);
        }
        let now = Instant::now();
        if now < self.next_at {
            std::thread::sleep((self.next_at - now).min(budget));
            if Instant::now() < self.next_at {
                return Ok(None);
            }
        }
        let period = Duration::from_secs_f64(CHUNK_PAIRS as f64 / self.cfg.sample_rate);
        // Real time, without accumulating a backlog if we fell behind.
        self.next_at = (self.next_at + period).max(Instant::now());
        let frame = self
            .baseband
            .frame(self.seed, self.seq, self.index, CHUNK_PAIRS);
        self.index += CHUNK_PAIRS as u64;
        self.seq += 1;
        Ok(Some(Arc::new(frame)))
    }

    fn set_stimulus(&mut self, name: &str) -> Result<bool, BackendError> {
        match RfScene::preset(name) {
            Some(s) => {
                self.scene = s;
                self.rebuild();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn stimuli(&self) -> Vec<&'static str> {
        RfScene::PRESETS.to_vec()
    }

    fn set_seed(&mut self, seed: u64) -> Result<bool, BackendError> {
        self.seed = seed;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use neowon_backend::SdrConfig;
    use neowon_core::SampleLayout;

    use super::*;

    fn tuned(centre_hz: f64) -> InstrumentConfig {
        InstrumentConfig::Sdr(SdrConfig {
            centre_hz,
            ..Default::default()
        })
    }

    fn next(b: &mut SimSdrBackend) -> SharedFrame {
        loop {
            if let Some(f) = b.poll_frame(Duration::from_millis(100)).unwrap() {
                return f;
            }
        }
    }

    #[test]
    fn every_preset_resolves() {
        for name in RfScene::PRESETS {
            assert!(RfScene::preset(name).is_some(), "{name}");
        }
    }

    #[test]
    fn default_scene_is_the_d8_reference() {
        let b = SimSdrBackend::new();
        assert_eq!(b.baseband, IqScene::reference());
    }

    #[test]
    fn tuning_selects_emitters_in_band() {
        let s = RfScene::preset("rf-fm-band").unwrap();
        let bb = s.baseband(99.0e6, 2.048e6, 0.0);
        let offs: Vec<f64> = bb
            .components
            .iter()
            .filter_map(|c| match c {
                IqComponent::Tone { offset_hz, .. } => Some(*offset_hz),
                _ => None,
            })
            .collect();
        assert_eq!(offs.len(), 2);
        assert!((offs[0] + 0.7e6).abs() < 1.0 && (offs[1] - 0.4e6).abs() < 1.0);
    }

    #[test]
    fn ppm_raises_the_band() {
        let s = RfScene::preset("rf-reference").unwrap();
        let IqComponent::Tone { offset_hz, .. } = s.baseband(100e6, 2.048e6, 10.0).components[0]
        else {
            panic!("rf-reference is a tone");
        };
        assert!((offset_hz - (100e3 + 1000.0)).abs() < 1e-6);
    }

    #[test]
    fn frames_are_contiguous_and_reseedable() {
        let mut b = SimSdrBackend::new();
        b.apply(&tuned(100e6)).unwrap();
        let (f0, f1) = (next(&mut b), next(&mut b));
        assert_eq!(f0.layout, SampleLayout::Complex);
        assert!((f1.t_start() - (f0.t_start() + f0.duration())).abs() < 1e-12);
        // Sample 0 of seed 1 is the fixture's first pair.
        let d = &f0.channels[0].data;
        assert_eq!(d[..2], IqScene::reference().samples(1, 0, 1)[..]);
        b.set_seed(2).unwrap();
        let f2 = next(&mut b);
        let expect = IqScene::reference().samples(2, 2 * CHUNK_PAIRS as u64, 1);
        assert_eq!(f2.channels[0].data[..2], expect[..]);
    }
}
