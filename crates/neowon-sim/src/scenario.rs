//! Scenarios: what the virtual bench generator drives on the two channels,
//! plus the named presets addressable from the UI, scripts, and tests.
//! Preset names are a stable API — see AGENTS.md.

use std::f64::consts::PI;
use std::sync::Arc;

use crate::figures::XyFigure;
use crate::signal::{Component, SignalSpec, Xorshift};

/// Two-channel stimulus definition.
#[derive(Debug, Clone, PartialEq)]
pub enum Scenario {
    PerChannel([SignalSpec; 2]),
    /// CH1 = amp*x(t), CH2 = amp*y(t) for the figure at `freq` Hz.
    Xy {
        figure: XyFigure,
        freq: f64,
        amp: f64,
    },
    /// Looping stereo waveform playback: CH1 = amp*left, CH2 = amp*right —
    /// the oscilloscope-music / vector-graphics format (L = X, R = Y).
    XyWav {
        samples: Arc<Vec<(f32, f32)>>,
        rate: f64,
        amp: f64,
    },
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario::preset("probe-comp").expect("probe-comp preset exists")
    }
}

impl Scenario {
    /// Preset names, a stable API shared by UI, scripts, and tests.
    pub const PRESETS: [&'static str; 14] = [
        "probe-comp",
        "sine-1k",
        "dc-1v",
        "two-tone",
        "chirp",
        "am",
        "fm",
        "trapezoid",
        "noise",
        "xy-circle",
        "xy-lissajous-3-2",
        "xy-rose-5",
        "xy-heart",
        "xy-butterfly",
    ];

    pub fn preset(name: &str) -> Option<Scenario> {
        let quiet = SignalSpec::default();
        let ch1 = |components: Vec<Component>| {
            Scenario::PerChannel([SignalSpec { components }, quiet.clone()])
        };
        match name {
            // Mirrors the hardware probe-comp output: 1 kHz 0..5 V square on
            // CH1, 2.5 kHz 1 Vpp sine on CH2, a little noise on both.
            "probe-comp" => Some(Scenario::PerChannel([
                SignalSpec {
                    components: vec![
                        Component::Square {
                            freq: 1000.0,
                            amp: 2.5,
                            duty: 0.5,
                            phase: 0.0,
                        },
                        Component::Dc { level: 2.5 },
                        Component::Noise { rms: 0.02 },
                    ],
                },
                SignalSpec {
                    components: vec![
                        Component::Sine {
                            freq: 2500.0,
                            amp: 0.5,
                            phase: 0.0,
                        },
                        Component::Noise { rms: 0.02 },
                    ],
                },
            ])),
            "sine-1k" => Some(ch1(vec![
                Component::Sine {
                    freq: 1000.0,
                    amp: 1.0,
                    phase: 0.0,
                },
                Component::Noise { rms: 0.005 },
            ])),
            "dc-1v" => Some(ch1(vec![Component::Dc { level: 1.0 }])),
            "two-tone" => Some(ch1(vec![
                Component::Sine {
                    freq: 1000.0,
                    amp: 1.0,
                    phase: 0.0,
                },
                Component::Sine {
                    freq: 3500.0,
                    amp: 0.4,
                    phase: 0.0,
                },
            ])),
            "chirp" => Some(ch1(vec![Component::Chirp {
                f0: 100.0,
                f1: 10_000.0,
                period: 20e-3,
                amp: 1.0,
            }])),
            "am" => Some(ch1(vec![Component::Am {
                carrier: 10_000.0,
                mod_freq: 500.0,
                depth: 0.5,
                amp: 1.0,
            }])),
            "fm" => Some(ch1(vec![Component::Fm {
                carrier: 10_000.0,
                mod_freq: 500.0,
                deviation: 2000.0,
                amp: 1.0,
            }])),
            "trapezoid" => Some(ch1(vec![Component::Trapezoid {
                freq: 1000.0,
                amp: 1.0,
                duty: 0.5,
                edge: 200e-6,
            }])),
            "noise" => Some(ch1(vec![Component::Noise { rms: 0.3 }])),
            "xy-circle" => Some(Scenario::Xy {
                figure: XyFigure::Circle,
                freq: 1000.0,
                amp: 1.5,
            }),
            "xy-lissajous-3-2" => Some(Scenario::Xy {
                figure: XyFigure::Lissajous {
                    a: 3,
                    b: 2,
                    phase: PI / 2.0,
                },
                freq: 1000.0,
                amp: 1.5,
            }),
            "xy-rose-5" => Some(Scenario::Xy {
                figure: XyFigure::Rose { k: 5 },
                freq: 1000.0,
                amp: 1.5,
            }),
            "xy-heart" => Some(Scenario::Xy {
                figure: XyFigure::Heart,
                freq: 1000.0,
                amp: 1.5,
            }),
            "xy-butterfly" => Some(Scenario::Xy {
                figure: XyFigure::Butterfly,
                freq: 600.0,
                amp: 1.5,
            }),
            _ => None,
        }
    }

    /// Both channels' voltages at time `t`, noise included.
    pub fn sample(&self, t: f64, rng: &mut Xorshift) -> [f64; 2] {
        match self {
            Scenario::PerChannel(specs) => [specs[0].sample(t, rng), specs[1].sample(t, rng)],
            Scenario::Xy { figure, freq, amp } => {
                let (x, y) = figure.sample(std::f64::consts::TAU * freq * t);
                [amp * x, amp * y]
            }
            Scenario::XyWav { .. } => self.sample_quiet(t),
        }
    }

    /// Both channels' voltages at time `t`, noise excluded.
    pub fn sample_quiet(&self, t: f64) -> [f64; 2] {
        match self {
            Scenario::PerChannel(specs) => [specs[0].sample_quiet(t), specs[1].sample_quiet(t)],
            Scenario::Xy { figure, freq, amp } => {
                let (x, y) = figure.sample(std::f64::consts::TAU * freq * t);
                [amp * x, amp * y]
            }
            Scenario::XyWav { samples, rate, amp } => {
                if samples.is_empty() {
                    return [0.0, 0.0];
                }
                let idx = ((t * rate).floor() as i64).rem_euclid(samples.len() as i64) as usize;
                let (x, y) = samples[idx];
                [amp * x as f64, amp * y as f64]
            }
        }
    }

    /// Unambiguous tone frequency for the frequency-meter display.
    pub fn fundamental(&self, ch: usize) -> Option<f64> {
        match self {
            Scenario::PerChannel(specs) => specs.get(ch.min(1))?.fundamental(),
            Scenario::Xy { .. } | Scenario::XyWav { .. } => None,
        }
    }

    /// Load a stereo WAV as an XY playback scenario.
    pub fn from_wav(path: &std::path::Path, amp: f64) -> std::io::Result<Scenario> {
        let (rate, samples) = neowon_core::wav::read_pcm16(path)?;
        Ok(Scenario::XyWav {
            samples: Arc::new(samples),
            rate: rate as f64,
            amp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_constructs() {
        for name in Scenario::PRESETS {
            assert!(Scenario::preset(name).is_some(), "{name}");
        }
        assert!(Scenario::preset("no-such-preset").is_none());
    }

    #[test]
    fn probe_comp_matches_hardware_signal() {
        let s = Scenario::preset("probe-comp").unwrap();
        // Mid high-dwell of the CH1 square: ~ +5 V minus nothing.
        let v = s.sample_quiet(250e-6); // quarter into the 1 kHz period
        assert!((v[0] - 5.0).abs() < 1e-9, "{v:?}");
        // Low dwell: 0 V.
        let v = s.sample_quiet(750e-6);
        assert!(v[0].abs() < 1e-9, "{v:?}");
    }
}
