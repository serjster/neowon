//! Deterministic complex (IQ) generator — the SDR half of the virtual
//! testbench (D8).
//!
//! Every sample is a pure function of `(seed, index)`: randomness comes from
//! counter-indexed [`splitmix64`] draws, never from a stateful PRNG, and
//! nothing on the sample path reads a clock. Output is also
//! **platform-independent to the bit**: the sample path uses only IEEE-754
//! basic operations (+, −, ×, ÷, `floor`), which every platform rounds
//! identically, and never the platform `sin`/`cos`/`ln`, which differ between
//! libms. So sine is a local polynomial and Gaussian noise is Irwin–Hall
//! (a sum of twelve uniforms), whose tails stop at ±6σ.

use neowon_core::{AcqMode, Acquisition, CaptureFrame, ChannelCapture, IqCal, SampleLayout};

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// Draw `index` of the SplitMix64 sequence seeded with `seed` — the same
/// value the sequential generator returns on its `index + 1`th call, but
/// computed directly, so any sample can be drawn without its predecessors.
pub fn splitmix64(seed: u64, index: u64) -> u64 {
    let mut z = seed.wrapping_add(index.wrapping_add(1).wrapping_mul(GOLDEN));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in `[0, 1)` with 53 bits of resolution.
fn unit(seed: u64, index: u64) -> f64 {
    (splitmix64(seed, index) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Draws one standard-normal value consumes.
const NORMAL_DRAWS: u64 = 12;

/// Approximately standard normal (Irwin–Hall, n = 12): exact mean 0 and
/// variance 1, support ±6.
fn normal(seed: u64, first: u64) -> f64 {
    let mut sum = 0.0;
    for j in 0..NORMAL_DRAWS {
        sum += unit(seed, first.wrapping_add(j));
    }
    sum - 6.0
}

/// `(cos, sin)` of `turns` full turns, from basic operations only.
/// Absolute error below 1e-13.
pub fn cos_sin_turns(turns: f64) -> (f64, f64) {
    // Reduce to the nearest quarter turn k and a residual |r| <= pi/4.
    let quarters = (turns - turns.floor()) * 4.0;
    let k = (quarters + 0.5).floor();
    let r = (quarters - k) * std::f64::consts::FRAC_PI_2;
    let r2 = r * r;
    // Taylor series to r^13 / r^14; the first omitted term is < 3e-14.
    let s = r
        * (1.0
            + r2 * (-1.0 / 6.0
                + r2 * (1.0 / 120.0
                    + r2 * (-1.0 / 5040.0
                        + r2 * (1.0 / 362_880.0
                            + r2 * (-1.0 / 39_916_800.0 + r2 * (1.0 / 6_227_020_800.0)))))));
    let c = 1.0
        + r2 * (-1.0 / 2.0
            + r2 * (1.0 / 24.0
                + r2 * (-1.0 / 720.0
                    + r2 * (1.0 / 40_320.0
                        + r2 * (-1.0 / 3_628_800.0
                            + r2 * (1.0 / 479_001_600.0 + r2 * (-1.0 / 87_178_291_200.0)))))));
    match k as u8 % 4 {
        0 => (c, s),
        1 => (-s, c),
        2 => (-c, -s),
        _ => (s, -c),
    }
}

/// One deterministic signal in an [`IqScene`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IqComponent {
    /// A complex exponential at `offset_hz` from the tuned centre (negative
    /// = below it), `amplitude` in full-scale units, starting at `phase`
    /// turns.
    Tone {
        offset_hz: f64,
        amplitude: f64,
        phase: f64,
    },
    /// A tone switched on for `duration_s` from `start_s` (sample time
    /// `index / sample_rate`), silent otherwise.
    Burst {
        offset_hz: f64,
        amplitude: f64,
        start_s: f64,
        duration_s: f64,
    },
    /// A linear sweep from `from_hz` to `to_hz` across `duration_s`,
    /// starting at `start_s`; silent outside it. Phase is
    /// `f0·τ + (f1 − f0)·τ² / (2T)` turns, τ the time into the sweep.
    Chirp {
        from_hz: f64,
        to_hz: f64,
        amplitude: f64,
        start_s: f64,
        duration_s: f64,
    },
}

impl IqComponent {
    /// `(amplitude, phase in turns)` at time `t`, or `None` while silent.
    fn at(&self, t: f64) -> Option<(f64, f64)> {
        let within = |start: f64, dur: f64| t >= start && t < start + dur;
        match *self {
            IqComponent::Tone {
                offset_hz,
                amplitude,
                phase,
            } => Some((amplitude, offset_hz * t + phase)),
            IqComponent::Burst {
                offset_hz,
                amplitude,
                start_s,
                duration_s,
            } => within(start_s, duration_s).then_some((amplitude, offset_hz * t)),
            IqComponent::Chirp {
                from_hz,
                to_hz,
                amplitude,
                start_s,
                duration_s,
            } => within(start_s, duration_s).then(|| {
                let tau = t - start_s;
                let turns = from_hz * tau + (to_hz - from_hz) * tau * tau / (2.0 * duration_s);
                (amplitude, turns)
            }),
        }
    }
}

/// What the simulated receiver sees: signals plus complex white noise, in
/// full-scale units (|I|, |Q| <= 1 is the ADC range).
#[derive(Debug, Clone, PartialEq)]
pub struct IqScene {
    /// IQ pairs per second.
    pub sample_rate: f64,
    pub components: Vec<IqComponent>,
    /// RMS of the complex noise, `sqrt(E|n|^2)`; each of I and Q carries
    /// `noise_rms / sqrt(2)`.
    pub noise_rms: f64,
}

impl IqScene {
    /// The scene the D8 fixture and `neowon sim iq` pin: a 0.5 FS tone
    /// 100 kHz above centre at 2.048 MS/s, in 0.05 FS noise. Changing it
    /// changes the fixture bytes, so treat it like a stimulus preset.
    pub fn reference() -> Self {
        Self {
            sample_rate: 2.048e6,
            components: vec![IqComponent::Tone {
                offset_hz: 100e3,
                amplitude: 0.5,
                phase: 0.0,
            }],
            noise_rms: 0.05,
        }
    }

    /// The I/Q pair at sample `index` for `seed`.
    pub fn sample(&self, seed: u64, index: u64) -> (f32, f32) {
        let (mut i, mut q) = (0.0f64, 0.0f64);
        let t = index as f64 / self.sample_rate;
        for c in &self.components {
            // Tones keep their original phase arithmetic (offset / rate ×
            // index) so the D8 fixture's bytes do not move.
            let (amplitude, turns) = match *c {
                IqComponent::Tone {
                    offset_hz,
                    amplitude,
                    phase,
                } => (
                    amplitude,
                    offset_hz / self.sample_rate * index as f64 + phase,
                ),
                _ => match c.at(t) {
                    Some(v) => v,
                    None => continue,
                },
            };
            let (cos, sin) = cos_sin_turns(turns);
            i += amplitude * cos;
            q += amplitude * sin;
        }
        if self.noise_rms > 0.0 {
            let sigma = self.noise_rms * std::f64::consts::FRAC_1_SQRT_2;
            let first = index.wrapping_mul(2 * NORMAL_DRAWS);
            i += sigma * normal(seed, first);
            q += sigma * normal(seed, first.wrapping_add(NORMAL_DRAWS));
        }
        (i as f32, q as f32)
    }

    /// `n` pairs from sample `start`, interleaved I, Q.
    pub fn samples(&self, seed: u64, start: u64, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(2 * n);
        for k in 0..n as u64 {
            let (i, q) = self.sample(seed, start + k);
            out.push(i);
            out.push(q);
        }
        out
    }

    /// A one-channel `Complex` stream frame of `n` pairs starting at sample
    /// `start`; `seq` numbers it and places it on the time axis.
    pub fn frame(&self, seed: u64, seq: u64, start: u64, n: usize) -> CaptureFrame {
        let data = self.samples(seed, start, n);
        let clipped = data.iter().any(|v| v.abs() >= 1.0);
        CaptureFrame::new(
            seq,
            Some(start as f64 / self.sample_rate),
            self.sample_rate,
            AcqMode::Sample,
            Acquisition::Stream { chunk: n },
            SampleLayout::Complex,
            vec![ChannelCapture {
                ch: 0,
                data,
                cal: IqCal::real(1.0, 0.0),
                clipped,
                freq_meter: None,
            }],
        )
        .expect("a sampled complex stream is a valid frame")
    }
}

/// Little-endian bytes of interleaved samples — the fixture and `sim iq`
/// file format.
pub fn to_le_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// FNV-1a, 64-bit: a cheap fingerprint of sample bytes for readouts
/// (`get iq`), not a security hash.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polynomial_sine_tracks_libm() {
        for k in 0..10_000 {
            let t = k as f64 / 10_000.0 * 3.0 - 1.0;
            let (c, s) = cos_sin_turns(t);
            let a = t * std::f64::consts::TAU;
            assert!((c - a.cos()).abs() < 1e-13, "cos at {t}");
            assert!((s - a.sin()).abs() < 1e-13, "sin at {t}");
        }
    }

    #[test]
    fn samples_are_indexable() {
        let scene = IqScene::reference();
        let run = scene.samples(9, 0, 64);
        let tail = scene.samples(9, 40, 24);
        assert_eq!(&run[80..], &tail[..]);
    }

    #[test]
    fn noise_has_its_stated_power() {
        let scene = IqScene {
            sample_rate: 1.0,
            components: Vec::new(),
            noise_rms: 0.2,
        };
        let n = 20_000;
        let data = scene.samples(3, 0, n);
        let power: f64 = data.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64;
        assert!((power.sqrt() - 0.2).abs() < 0.004, "rms {}", power.sqrt());
    }

    #[test]
    fn frame_is_a_complex_stream() {
        let f = IqScene::reference().frame(1, 0, 0, 256);
        assert_eq!(f.layout, SampleLayout::Complex);
        assert_eq!(f.channels[0].unit_count(f.layout), 256);
        assert!((f.duration() - 256.0 / 2.048e6).abs() < 1e-15);
    }

    #[test]
    fn bursts_and_chirps_are_gated_and_sweep() {
        let rate = 8192.0;
        let scene = IqScene {
            sample_rate: rate,
            components: vec![
                IqComponent::Burst {
                    offset_hz: 1000.0,
                    amplitude: 0.5,
                    start_s: 0.25,
                    duration_s: 0.01,
                },
                IqComponent::Chirp {
                    from_hz: -2000.0,
                    to_hz: -1000.0,
                    amplitude: 0.25,
                    start_s: 0.5,
                    duration_s: 0.25,
                },
            ],
            noise_rms: 0.0,
        };
        let mag = |k: u64| {
            let (i, q) = scene.sample(1, k);
            ((i * i + q * q) as f64).sqrt()
        };
        assert_eq!(mag(0), 0.0);
        assert!((mag(2048) - 0.5).abs() < 1e-6); // t = 0.25: burst on
        assert_eq!(mag(2048 + 82), 0.0); // 10 ms later: off
        assert!((mag(4096 + 100) - 0.25).abs() < 1e-6); // inside the chirp
        // Instantaneous frequency sweeps from -2 kHz to -1 kHz.
        let freq = |k: u64| {
            let (a, b) = (scene.sample(1, k), scene.sample(1, k + 1));
            let (re, im) = (
                (b.0 * a.0 + b.1 * a.1) as f64,
                (b.1 * a.0 - b.0 * a.1) as f64,
            );
            im.atan2(re) * rate / std::f64::consts::TAU
        };
        assert!((freq(4096) + 2000.0).abs() < 5.0, "{}", freq(4096));
        assert!((freq(6142) + 1000.0).abs() < 5.0, "{}", freq(6142));
    }

    #[test]
    fn fnv1a64_matches_published_vectors() {
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C);
    }
}
