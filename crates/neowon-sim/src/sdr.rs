//! Simulated SDR: deterministic RF scenes behind the `Backend` trait.
//!
//! A scene places emitters at absolute RF frequencies; tuning selects which
//! of them fall inside `centre ± rate/2` and turns them into the baseband
//! `IqScene`. Samples come from the `iq` generator, so they are a pure
//! function of (seed, sample index): pacing uses the wall clock, the signal
//! never does. Gain and AGC are accepted and ignored (the scene is already
//! in full-scale units); ppm shifts the band the way a real crystal
//! correction does, by (centre)·ppm upwards.
//!
//! Scene names are a stable API like the scope's stimulus presets.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use neowon_backend::{
    Acquisition, Backend, BackendError, Capabilities, InstrumentConfig, SdrCaps, SdrConfig,
    sdr_config,
};
use neowon_core::ladders::{RTL_SAMPLE_RATES, r82xx_gains_db};
use neowon_core::{SharedFrame, stream_chunk_pairs};

use neowon_core::Modulation;

use crate::iq::{IqBuffer, IqComponent, IqScene};

/// What an emitter transmits. One kind, not a set of independent
/// `Option`s that could all be set at once with the baseband silently
/// preferring one of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmitterKind {
    Carrier,
    Am {
        depth: f64,
        tone_hz: f64,
    },
    Fm {
        deviation_hz: f64,
        tone_hz: f64,
    },
    Digital {
        modulation: Modulation,
        symbol_rate: f64,
        rolloff: f64,
    },
}

/// A transmitter in a scene: where it sits, how loud it is, and what it
/// transmits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub freq_hz: f64,
    /// Full-scale units (for a digital emitter, the symbol amplitude after
    /// a matched filter).
    pub amplitude: f64,
    pub kind: EmitterKind,
}

/// A carrier (see `dig` for a digital emitter).
pub const fn em(freq_hz: f64, amplitude: f64) -> Emitter {
    Emitter {
        freq_hz,
        amplitude,
        kind: EmitterKind::Carrier,
    }
}

const fn dig(freq_hz: f64, amplitude: f64, m: Modulation, symbol_rate: f64) -> Emitter {
    Emitter {
        freq_hz,
        amplitude,
        kind: EmitterKind::Digital {
            modulation: m,
            symbol_rate,
            rolloff: 0.35,
        },
    }
}

const fn am(freq_hz: f64, amplitude: f64, depth: f64, tone_hz: f64) -> Emitter {
    Emitter {
        freq_hz,
        amplitude,
        kind: EmitterKind::Am { depth, tone_hz },
    }
}

const fn fm(freq_hz: f64, amplitude: f64, deviation_hz: f64, tone_hz: f64) -> Emitter {
    Emitter {
        freq_hz,
        amplitude,
        kind: EmitterKind::Fm {
            deviation_hz,
            tone_hz,
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RfScene {
    pub emitters: Vec<Emitter>,
    /// Complex noise RMS, full-scale units. Applied on top of `buffer` too.
    pub noise_rms: f64,
    /// A pre-modulated stream the app installed (see [`install_scene`]);
    /// `rf-dab` carries the Mode I ensemble here.
    pub buffer: Option<IqBuffer>,
}

/// Scenes the embedding app hands the sim under a preset name.
///
/// The sim cannot synthesise a DAB ensemble — that needs `neowon_dsp`, which
/// must not become a sim dependency — so the app builds
/// the IQ and installs it here; `RfScene::preset` then resolves the stable
/// preset name like any built-in. The registry is process-global because the
/// backend lives on the supervisor thread; the app installs before it sends
/// the `stimulus` command, and tests run in separate processes.
static INSTALLED: OnceLock<Mutex<BTreeMap<String, RfScene>>> = OnceLock::new();

/// Install `scene` under `name`, replacing a previous install. Idempotent
/// for the same content; without an install, `preset("rf-dab")` is silence.
pub fn install_scene(name: &str, scene: RfScene) {
    INSTALLED
        .get_or_init(Default::default)
        .lock()
        .expect("scene registry")
        .insert(name.to_string(), scene);
}

pub fn installed_scene(name: &str) -> Option<RfScene> {
    INSTALLED
        .get_or_init(Default::default)
        .lock()
        .expect("scene registry")
        .get(name)
        .cloned()
}

impl RfScene {
    pub const PRESETS: [&'static str; 8] = [
        "rf-reference",
        "rf-fm-band",
        "rf-hf",
        "rf-noise",
        "rf-digital",
        "rf-am",
        "rf-fm",
        "rf-dab",
    ];

    pub fn preset(name: &str) -> Option<Self> {
        if let Some(installed) = installed_scene(name) {
            return Some(installed);
        }
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
            // Symbol rates divide 2.048 MS/s (20 and 40 samples/symbol).
            // Symbol SNR ≈ 25 dB for QPSK, 28 dB for 16QAM.
            "rf-digital" => (
                vec![
                    dig(100.3e6, 0.9, Modulation::Qpsk, 102.4e3),
                    dig(99.6e6, 1.25, Modulation::Qam16, 51.2e3),
                ],
                0.05,
            ),
            // A 1 kHz tone on each: the demod oracle's stimulus.
            "rf-am" => (vec![am(100.1e6, 0.5, 0.5, 1000.0)], 0.02),
            "rf-fm" => (vec![fm(100.1e6, 0.5, 3000.0, 1000.0)], 0.02),
            // The app installs the real ensemble (an IQ buffer); before that
            // there is nothing honest to play, so it is silent rather than a
            // different signal wearing the name.
            "rf-dab" => (Vec::new(), 0.0),
            _ => return None,
        };
        Some(Self {
            emitters,
            noise_rms,
            buffer: None,
        })
    }

    /// The baseband this scene produces tuned to `centre_hz` at `rate`
    /// pairs/s with `ppm` crystal correction.
    pub fn baseband(&self, centre_hz: f64, rate: f64, ppm: f64) -> IqScene {
        let shift = centre_hz * ppm * 1e-6;
        let mut components: Vec<IqComponent> = self
            .emitters
            .iter()
            .map(|e| e.freq_hz - centre_hz + shift)
            .zip(&self.emitters)
            .filter(|(off, _)| off.abs() < rate / 2.0)
            .map(|(offset_hz, e)| match e.kind {
                EmitterKind::Digital {
                    modulation,
                    symbol_rate,
                    rolloff,
                } => IqComponent::Digital {
                    modulation,
                    symbol_rate,
                    offset_hz,
                    amplitude: e.amplitude,
                    rolloff,
                },
                EmitterKind::Am { depth, tone_hz } => IqComponent::Am {
                    offset_hz,
                    amplitude: e.amplitude,
                    depth,
                    tone_hz,
                },
                EmitterKind::Fm {
                    deviation_hz,
                    tone_hz,
                } => IqComponent::Fm {
                    offset_hz,
                    amplitude: e.amplitude,
                    deviation_hz,
                    tone_hz,
                },
                EmitterKind::Carrier => IqComponent::Tone {
                    offset_hz,
                    amplitude: e.amplitude,
                    phase: 0.0,
                },
            })
            .collect();
        // The buffer is baseband already: no RF offset is applied.
        if let Some(buffer) = &self.buffer {
            components.push(IqComponent::Buffer(*buffer));
        }
        IqScene {
            sample_rate: rate,
            components,
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
                // The dongle's own ladders, from their one home in core:
                // a sim that accepted a rate the hardware refuses would
                // be a lie the tests could not catch.
                sample_rates: RTL_SAMPLE_RATES.to_vec(),
                gains_db: r82xx_gains_db(),
                acquisition: Acquisition::Stream {
                    chunk: stream_chunk_pairs(cfg.sample_rate),
                },
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

    pub fn with_scene(scene: RfScene) -> Self {
        let mut b = Self::new();
        b.scene = scene;
        b.rebuild();
        b
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
        let c = sdr_config(cfg)?;
        if c.sample_rate <= 0.0 {
            return Err(BackendError::Transient(
                "sample rate must be positive".into(),
            ));
        }
        self.cfg = c.clone();
        if let Capabilities::Sdr(caps) = &mut self.caps {
            caps.acquisition = Acquisition::Stream {
                chunk: stream_chunk_pairs(c.sample_rate),
            };
        }
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
        let chunk = stream_chunk_pairs(self.cfg.sample_rate);
        let period = Duration::from_secs_f64(chunk as f64 / self.cfg.sample_rate);
        // Real time, without accumulating a backlog if we fell behind.
        self.next_at = (self.next_at + period).max(Instant::now());
        let frame = self.baseband.frame(self.seed, self.seq, self.index, chunk);
        self.index += chunk as u64;
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
        let bb = s.baseband(100e6, 2.048e6, 10.0);
        let IqComponent::Tone { offset_hz, .. } = &bb.components[0] else {
            panic!("rf-reference is a tone");
        };
        assert!((*offset_hz - (100e3 + 1000.0)).abs() < 1e-6);
    }

    /// The app's hand-off path: an installed scene is what `preset` returns,
    /// and the backend plays its buffer as the sample stream.
    #[test]
    fn an_installed_scene_is_the_preset_and_plays_its_buffer() {
        static SAMPLES: [f32; 4] = [0.25, -0.25, 0.5, -0.5];
        let buffer = IqBuffer {
            samples: &SAMPLES,
            sample_rate: 2.048e6,
        };
        install_scene(
            "rf-buffer-test",
            RfScene {
                emitters: Vec::new(),
                noise_rms: 0.0,
                buffer: Some(buffer),
            },
        );
        let scene = RfScene::preset("rf-buffer-test").expect("installed");
        assert!(scene.buffer.is_some());
        let mut b = SimSdrBackend::new();
        b.apply(&tuned(100e6)).unwrap();
        assert!(b.set_stimulus("rf-buffer-test").unwrap());
        let f = next(&mut b);
        assert_eq!(&f.channels[0].data[..2], &[0.25, -0.25]);
    }

    /// Without an app install the `rf-dab` name is present and resolves to
    /// silence — an absent ensemble, never a different signal wearing it.
    #[test]
    fn uninstalled_rf_dab_is_silence() {
        let scene = RfScene::preset("rf-dab").expect("the preset name exists");
        let bb = scene.baseband(100e6, 2.048e6, 0.0);
        assert!(bb.components.is_empty());
        assert_eq!(bb.noise_rms, 0.0);
    }

    #[test]
    fn frames_are_contiguous_and_reseedable() {
        let mut b = SimSdrBackend::new();
        b.apply(&tuned(100e6)).unwrap();
        let (f0, f1) = (next(&mut b), next(&mut b));
        assert_eq!(f0.layout(), SampleLayout::Complex);
        assert!((f1.t_start() - (f0.t_start() + f0.duration())).abs() < 1e-12);
        // Sample 0 of seed 1 is the fixture's first pair.
        let d = &f0.channels[0].data;
        assert_eq!(d[..2], IqScene::reference().samples(1, 0, 1)[..]);
        b.set_seed(2).unwrap();
        let f2 = next(&mut b);
        let chunk = stream_chunk_pairs(2.048e6) as u64;
        let expect = IqScene::reference().samples(2, 2 * chunk, 1);
        assert_eq!(f2.channels[0].data[..2], expect[..]);
    }

    /// The frame carries the shared time-based chunk, not a fixed pair
    /// count: the same policy the RTL transport uses, so the simulator
    /// shows the display cadence the hardware will have at any rate.
    #[test]
    fn frame_size_follows_the_rate() {
        let mut b = SimSdrBackend::new();
        b.apply(&InstrumentConfig::Sdr(SdrConfig {
            sample_rate: 250e3,
            ..Default::default()
        }))
        .unwrap();
        let f = next(&mut b);
        assert_eq!(f.channels[0].data.len() / 2, stream_chunk_pairs(250e3));
        assert!((f.duration() - 0.065536).abs() < 1e-12, "{}", f.duration());
        b.apply(&InstrumentConfig::Sdr(SdrConfig::default()))
            .unwrap();
        let f = next(&mut b);
        assert_eq!(f.channels[0].data.len() / 2, 102_400);
        assert!((f.duration() - 0.05).abs() < 1e-12, "{}", f.duration());
    }
}
