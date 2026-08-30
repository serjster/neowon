//! Component-based signal model. A `SignalSpec` is a sum of `Component`s,
//! sampled deterministically in time; the only randomness is the seeded
//! `Xorshift` stream, so identical setups produce identical samples forever.

use std::f64::consts::TAU;

/// xorshift64* PRNG — deterministic, no rand dependency.
#[derive(Debug, Clone)]
pub struct Xorshift(u64);

impl Default for Xorshift {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl Xorshift {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Uniform in [-0.5, 0.5).
    pub fn next_uniform(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let u = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
        u - 0.5
    }
}

/// One summand of a signal. Amplitudes are peak volts unless noted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Component {
    Sine {
        freq: f64,
        amp: f64,
        phase: f64,
    },
    Square {
        freq: f64,
        amp: f64,
        duty: f64,
        phase: f64,
    },
    Triangle {
        freq: f64,
        amp: f64,
        phase: f64,
    },
    /// Sawtooth rising from -amp to +amp once per period.
    Ramp {
        freq: f64,
        amp: f64,
        phase: f64,
    },
    /// Square with linear 0-100% edges taking `edge` seconds.
    Trapezoid {
        freq: f64,
        amp: f64,
        duty: f64,
        edge: f64,
    },
    Dc {
        level: f64,
    },
    /// A repeating 8-N-1 UART frame stream, idle high. Exists so the
    /// protocol decoders can be exercised — and demonstrated — without
    /// hardware or a second instrument to generate the traffic.
    Uart {
        baud: f64,
        /// Bytes sent, then the line idles for `gap_bits` before
        /// repeating. Fixed-size so `Component` stays `Copy`.
        bytes: [u8; 8],
        len: u8,
        gap_bits: f64,
        /// Volts for a mark (idle/1); a space is its negation.
        amp: f64,
    },
    Noise {
        rms: f64,
    },
    /// Linear frequency sweep f0->f1 over `period`, repeating; phase
    /// phi(t) = 2*pi*(f0*t + (f1-f0)/(2*period)*t^2) for t in [0, period).
    Chirp {
        f0: f64,
        f1: f64,
        period: f64,
        amp: f64,
    },
    Am {
        carrier: f64,
        mod_freq: f64,
        depth: f64,
        amp: f64,
    },
    /// FM: amp * sin(2*pi*carrier*t + (deviation/mod_freq)*sin(2*pi*mod_freq*t))
    Fm {
        carrier: f64,
        mod_freq: f64,
        deviation: f64,
        amp: f64,
    },
}

impl Component {
    /// Value at time `t` (seconds), noise included.
    pub fn sample(&self, t: f64, rng: &mut Xorshift) -> f64 {
        match self {
            Component::Noise { rms } => rng.next_uniform() * rms * 12f64.sqrt(),
            _ => self.sample_quiet(t),
        }
    }

    /// Value at time `t` without any noise contribution.
    pub fn sample_quiet(&self, t: f64) -> f64 {
        match self {
            Component::Uart {
                baud,
                bytes,
                len,
                gap_bits,
                amp,
            } => {
                let bytes = &bytes[..(*len as usize).min(8)];
                // One frame is start + 8 data + stop; the stream repeats
                // after gap_bits of idle, so a record always contains whole
                // frames wherever it starts.
                let bit = 1.0 / baud.max(1e-9);
                let per_byte = 10.0;
                let total = bytes.len() as f64 * per_byte + gap_bits;
                let pos = ((t / bit) % total + total) % total;
                let idx = (pos / per_byte).floor() as usize;
                if idx >= bytes.len() {
                    return *amp; // idle
                }
                let within = pos - idx as f64 * per_byte;
                let mark = match within.floor() as usize {
                    0 => false,                                      // start
                    k if k <= 8 => bytes[idx] & (1 << (k - 1)) != 0, // LSB first
                    _ => true,                                       // stop
                };
                if mark { *amp } else { -*amp }
            }
            Component::Sine { freq, amp, phase } => amp * (TAU * freq * t + phase).sin(),
            Component::Square {
                freq,
                amp,
                duty,
                phase,
            } => {
                let p = ((freq * t + phase / TAU).fract() + 1.0).fract();
                if p < *duty { *amp } else { -*amp }
            }
            Component::Triangle { freq, amp, phase } => {
                let p = ((freq * t + phase / TAU).fract() + 1.0).fract();
                let unit = if p < 0.5 {
                    4.0 * p - 1.0
                } else {
                    3.0 - 4.0 * p
                };
                amp * unit
            }
            Component::Ramp { freq, amp, phase } => {
                let p = ((freq * t + phase / TAU).fract() + 1.0).fract();
                amp * (2.0 * p - 1.0)
            }
            Component::Trapezoid {
                freq,
                amp,
                duty,
                edge,
            } => {
                if *edge <= 0.0 {
                    return Component::Square {
                        freq: *freq,
                        amp: *amp,
                        duty: *duty,
                        phase: 0.0,
                    }
                    .sample_quiet(t);
                }
                // Moving average of the square over a window of `edge`
                // seconds: (F(t) - F(t-edge)) / edge, F = cumulative area.
                let period = 1.0 / freq;
                let square_area = |u: f64| -> f64 {
                    let k = (u / period).floor();
                    let r = u - k * period;
                    let hi = period * duty;
                    let partial = if r <= hi {
                        amp * r
                    } else {
                        amp * (2.0 * hi - r)
                    };
                    k * amp * period * (2.0 * duty - 1.0) + partial
                };
                (square_area(t) - square_area(t - edge)) / edge
            }
            Component::Dc { level } => *level,
            Component::Noise { .. } => 0.0,
            Component::Chirp {
                f0,
                f1,
                period,
                amp,
            } => {
                let tp = ((t % period) + period) % period;
                amp * (TAU * (f0 * tp + (f1 - f0) / (2.0 * period) * tp * tp)).sin()
            }
            Component::Am {
                carrier,
                mod_freq,
                depth,
                amp,
            } => amp * (1.0 + depth * (TAU * mod_freq * t).sin()) * (TAU * carrier * t).sin(),
            Component::Fm {
                carrier,
                mod_freq,
                deviation,
                amp,
            } => {
                let arg = TAU * carrier * t + (deviation / mod_freq) * (TAU * mod_freq * t).sin();
                amp * arg.sin()
            }
        }
    }

    /// Fundamental frequency when the component is periodic and unambiguous.
    pub fn fundamental(&self) -> Option<f64> {
        match self {
            Component::Sine { freq, .. }
            | Component::Square { freq, .. }
            | Component::Triangle { freq, .. }
            | Component::Ramp { freq, .. }
            | Component::Trapezoid { freq, .. } => Some(*freq),
            Component::Uart { baud, .. } => Some(*baud),
            Component::Am { carrier, .. } | Component::Fm { carrier, .. } => Some(*carrier),
            Component::Dc { .. } | Component::Noise { .. } | Component::Chirp { .. } => None,
        }
    }
}

/// A sum of components.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalSpec {
    pub components: Vec<Component>,
}

impl SignalSpec {
    /// Value at time `t` (seconds), noise included.
    pub fn sample(&self, t: f64, rng: &mut Xorshift) -> f64 {
        self.components.iter().map(|c| c.sample(t, rng)).sum()
    }

    /// Value at time `t` without any noise contribution.
    pub fn sample_quiet(&self, t: f64) -> f64 {
        self.components.iter().map(|c| c.sample_quiet(t)).sum()
    }

    /// The fundamental when exactly one periodic component is present.
    pub fn fundamental(&self) -> Option<f64> {
        let mut found = None;
        for c in &self.components {
            if let Some(f) = c.fundamental() {
                if found.is_some() {
                    return None;
                }
                found = Some(f);
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_levels_and_duty() {
        let sq = Component::Square {
            freq: 1000.0,
            amp: 1.0,
            duty: 0.25,
            phase: 0.0,
        };
        assert_eq!(sq.sample_quiet(0.0), 1.0);
        assert_eq!(sq.sample_quiet(0.249e-3), 1.0);
        assert_eq!(sq.sample_quiet(0.251e-3), -1.0);
    }

    #[test]
    fn trapezoid_edge_is_linear() {
        // 1 kHz, duty 0.5, 200 us edges: mid-edge (t = 100 us into the rise)
        // sits at 0 V; the dwell plateaus reach the full +-amp.
        let tr = Component::Trapezoid {
            freq: 1000.0,
            amp: 1.0,
            duty: 0.5,
            edge: 200e-6,
        };
        let mid = tr.sample_quiet(100e-6);
        assert!(mid.abs() < 1e-9, "mid-edge {mid}");
        assert!((tr.sample_quiet(300e-6) - 1.0).abs() < 1e-9);
        assert!((tr.sample_quiet(700e-6) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn trapezoid_degenerates_to_square() {
        let tr = Component::Trapezoid {
            freq: 1000.0,
            amp: 1.0,
            duty: 0.5,
            edge: 0.0,
        };
        assert_eq!(tr.sample_quiet(100e-6), 1.0);
        assert_eq!(tr.sample_quiet(600e-6), -1.0);
    }

    #[test]
    fn chirp_instantaneous_frequency_advances() {
        let ch = Component::Chirp {
            f0: 100.0,
            f1: 10_000.0,
            period: 20e-3,
            amp: 1.0,
        };
        // Zero-crossing spacing shrinks as the sweep runs.
        let crossings = |from: f64| {
            let mut n = 0;
            let mut prev = ch.sample_quiet(from);
            for i in 1..=1000 {
                let v = ch.sample_quiet(from + i as f64 * 1e-6);
                if prev < 0.0 && v >= 0.0 {
                    n += 1;
                }
                prev = v;
            }
            n
        };
        let early = crossings(0.0);
        let late = crossings(15e-3);
        assert!(late > 4 * early, "early {early}, late {late}");
    }

    #[test]
    fn noise_has_requested_rms() {
        let mut rng = Xorshift::default();
        let n = Component::Noise { rms: 0.3 };
        let mut sq = 0.0;
        let n_samples = 1_000_000;
        for _ in 0..n_samples {
            let v = n.sample(0.0, &mut rng);
            sq += v * v;
        }
        let rms = (sq / n_samples as f64).sqrt();
        assert!((rms - 0.3).abs() < 0.005, "rms {rms}");
    }

    #[test]
    fn fundamental_is_single_tone_only() {
        let one = SignalSpec {
            components: vec![
                Component::Sine {
                    freq: 1000.0,
                    amp: 1.0,
                    phase: 0.0,
                },
                Component::Noise { rms: 0.01 },
            ],
        };
        assert_eq!(one.fundamental(), Some(1000.0));
        let two = SignalSpec {
            components: vec![
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
            ],
        };
        assert_eq!(two.fundamental(), None);
    }
}
