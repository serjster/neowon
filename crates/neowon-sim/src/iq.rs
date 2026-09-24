//! Deterministic complex (IQ) generator — the SDR half of the virtual
//! testbench.
//!
//! Every sample is a pure function of `(seed, index)`: randomness comes from
//! counter-indexed [`splitmix64`] draws, never from a stateful PRNG, and
//! nothing on the sample path reads a clock. Output is also
//! **platform-independent to the bit**: the sample path uses only IEEE-754
//! basic operations (+, −, ×, ÷, `floor`), which every platform rounds
//! identically, and never the platform `sin`/`cos`/`ln`, which differ between
//! libms. So sine is a local polynomial and Gaussian noise is Irwin–Hall
//! (a sum of twelve uniforms), whose tails stop at ±6σ.

use neowon_core::{
    AcqMode, Acquisition, CaptureFrame, ChannelCapture, IqCal, Modulation, SampleLayout,
};

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

/// A pre-modulated IQ stream the embedding app hands the sim: interleaved
/// I, Q samples in full-scale units, at the rate they were generated for.
///
/// This is how a signal the sim cannot synthesise itself reaches it — the
/// `rf-dab` scene, whose Mode I ensemble the app builds with
/// `neowon_dsp::dab::encoder` (the sim has no DSP dependency). The samples
/// are `'static` because an installed scene lives for the process; the
/// buffer replays from sample 0 and loops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IqBuffer {
    pub samples: &'static [f32],
    /// The buffer's native sample rate; a scene at another rate reads it by
    /// nearest sample.
    pub sample_rate: f64,
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
    /// `index / sample_rate`), silent otherwise. `rise_s` shapes the
    /// switching with raised-cosine edges (0 = rectangular); real bursts
    /// have finite edges, and a rectangular gate's sinc² spectrum has no
    /// occupied bandwidth of its own, only one set by the noise floor.
    Burst {
        offset_hz: f64,
        amplitude: f64,
        start_s: f64,
        duration_s: f64,
        rise_s: f64,
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
    /// Amplitude modulation by a tone: `amplitude·(1 + depth·cos(2π·tone·t))`
    /// on a carrier at `offset_hz`.
    Am {
        offset_hz: f64,
        amplitude: f64,
        depth: f64,
        tone_hz: f64,
    },
    /// Frequency modulation by a tone: instantaneous frequency
    /// `offset_hz + deviation_hz·cos(2π·tone·t)`, constant envelope.
    Fm {
        offset_hz: f64,
        amplitude: f64,
        deviation_hz: f64,
        tone_hz: f64,
    },
    /// A continuous digitally modulated carrier: Gray-labelled symbols
    /// drawn from the seed (`symbol_bits`), root-raised-cosine shaped with
    /// roll-off `rolloff`, at `offset_hz` from centre. `amplitude` is the
    /// symbol amplitude after a unit-energy matched filter, so a noise RMS
    /// of `amplitude · 10^(−snr/20)` gives that Es/N0 at the decisions.
    /// Per-sample power is `amplitude² / sps`.
    Digital {
        modulation: Modulation,
        symbol_rate: f64,
        offset_hz: f64,
        amplitude: f64,
        rolloff: f64,
    },
    /// A pre-modulated IQ buffer, replayed and looped ([`IqBuffer`]).
    /// `offset_hz` is not applied: the buffer already is the baseband.
    Buffer(IqBuffer),
}

/// Symbols either side of the current one that shape a sample.
pub const RRC_SPAN: i64 = 12;
/// Keeps symbol draws independent of the noise draws of the same seed.
const SYMBOL_SALT: u64 = 0x5EED_0F5E_B01D_A7A5;

/// The label of symbol `n` for `seed` (low bits used per modulation).
pub fn symbol_bits(seed: u64, n: i64) -> u32 {
    splitmix64(seed ^ SYMBOL_SALT, n as u64) as u32
}

/// Pulse values for S samples per symbol: row r (the sample's phase within
/// its symbol) holds, at column j, `h((r + (RRC_SPAN − j)·S) / S)` for the
/// 2·RRC_SPAN + 1 symbols around it. Built once per (S, β).
fn rrc_table(s: usize, beta: f64) -> std::sync::Arc<Vec<f64>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Tables = Mutex<HashMap<(usize, u64), Arc<Vec<f64>>>>;
    static TABLES: OnceLock<Tables> = OnceLock::new();
    let mut map = TABLES
        .get_or_init(Default::default)
        .lock()
        .expect("rrc table lock");
    map.entry((s, beta.to_bits()))
        .or_insert_with(|| {
            let w = 2 * RRC_SPAN as usize + 1;
            let mut t = Vec::with_capacity(s * w);
            for r in 0..s as i64 {
                for j in 0..w as i64 {
                    // Symbol n = n0 − SPAN + j; offset k − n·S = r + (SPAN − j)·S.
                    let num = r + (RRC_SPAN - j) * s as i64;
                    t.push(rrc(num as f64 / s as f64, beta));
                }
            }
            Arc::new(t)
        })
        .clone()
}

/// Root-raised-cosine pulse at `tau` symbol periods, roll-off `beta`,
/// unit energy (∫h² dτ = 1), from basic operations only.
pub fn rrc(tau: f64, beta: f64) -> f64 {
    use std::f64::consts::PI;
    let sin_pi = |x: f64| cos_sin_turns(x / 2.0).1;
    let cos_pi = |x: f64| cos_sin_turns(x / 2.0).0;
    if tau.abs() < 1e-12 {
        return 1.0 - beta + 4.0 * beta / PI;
    }
    if beta > 0.0 && (tau.abs() - 1.0 / (4.0 * beta)).abs() < 1e-12 {
        let x = 1.0 / (4.0 * beta);
        return beta
            * std::f64::consts::FRAC_1_SQRT_2
            * ((1.0 + 2.0 / PI) * sin_pi(x) + (1.0 - 2.0 / PI) * cos_pi(x));
    }
    let num = sin_pi(tau * (1.0 - beta)) + 4.0 * beta * tau * cos_pi(tau * (1.0 + beta));
    let den = PI * tau * (1.0 - (4.0 * beta * tau) * (4.0 * beta * tau));
    num / den
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
                rise_s,
            } => within(start_s, duration_s).then(|| {
                let tau = (t - start_s).min(start_s + duration_s - t);
                let env = if rise_s > 0.0 && tau < rise_s {
                    // 0.5·(1 − cos(π·τ/rise)); cos of π·x is x/2 turns.
                    0.5 * (1.0 - cos_sin_turns(tau / rise_s / 2.0).0)
                } else {
                    1.0
                };
                (amplitude * env, offset_hz * t)
            }),
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
            IqComponent::Am {
                offset_hz,
                amplitude,
                depth,
                tone_hz,
            } => Some((
                amplitude * (1.0 + depth * cos_sin_turns(tone_hz * t).0),
                offset_hz * t,
            )),
            // Phase: ∫ f dt = offset·t + deviation·sin(2π·tone·t)/(2π·tone).
            IqComponent::Fm {
                offset_hz,
                amplitude,
                deviation_hz,
                tone_hz,
            } => Some((
                amplitude,
                offset_hz * t
                    + deviation_hz * cos_sin_turns(tone_hz * t).1
                        / (std::f64::consts::TAU * tone_hz),
            )),
            // Complex-valued; `IqScene::sample` builds it directly.
            IqComponent::Digital { .. } => None,
            // A buffer is not a phase model; `IqScene::sample` reads it.
            IqComponent::Buffer(_) => None,
        }
    }

    /// The baseband (pre-carrier) value of a `Digital` component at
    /// sample `index`: Σ a_n h(u − n), u = index · symbol_rate / rate,
    /// scaled so the matched filter recovers `amplitude · a_n`.
    ///
    /// With an integer number of samples per symbol S, every pulse offset
    /// is exactly `(index − n·S) / S`, so the pulse values come from a
    /// table of those same exact values (fast enough for a live scene);
    /// otherwise each is computed directly. Either way a sample is a pure
    /// function of (seed, index).
    fn digital(&self, seed: u64, index: u64, rate: f64) -> (f64, f64) {
        let IqComponent::Digital {
            modulation,
            symbol_rate,
            amplitude,
            rolloff,
            ..
        } = *self
        else {
            return (0.0, 0.0);
        };
        let sps = rate / symbol_rate;
        let (mut i, mut q) = (0.0, 0.0);
        let mut add = |n: i64, h: f64| {
            let (a, b) = modulation.point(symbol_bits(seed, n));
            i += a * h;
            q += b * h;
        };
        if sps.fract() == 0.0 && (1.0..=4096.0).contains(&sps) {
            let s = sps as i64;
            let k = index as i64;
            let n0 = k.div_euclid(s);
            let r = k.rem_euclid(s) as usize;
            let table = rrc_table(s as usize, rolloff);
            let row = &table[r * (2 * RRC_SPAN as usize + 1)..][..2 * RRC_SPAN as usize + 1];
            for (j, &h) in row.iter().enumerate() {
                add(n0 - RRC_SPAN + j as i64, h);
            }
        } else {
            let u = index as f64 / rate * symbol_rate;
            let n0 = u.floor() as i64;
            for n in n0 - RRC_SPAN..=n0 + RRC_SPAN {
                add(n, rrc(u - n as f64, rolloff));
            }
        }
        let g = amplitude / sps.sqrt();
        (i * g, q * g)
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
    /// The scene the IQ fixture and `neowon sim iq` pin: a 0.5 FS tone
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

    pub fn sample(&self, seed: u64, index: u64) -> (f32, f32) {
        let (mut i, mut q) = (0.0f64, 0.0f64);
        let t = index as f64 / self.sample_rate;
        for c in &self.components {
            if let IqComponent::Buffer(buffer) = c {
                let pairs = buffer.samples.len() / 2;
                if pairs == 0 {
                    continue;
                }
                // At the buffer's native rate the index is the sample; at any
                // other scene rate it is read by nearest sample, like a
                // differently clocked converter.
                let k = if (buffer.sample_rate - self.sample_rate).abs() < 0.5 {
                    index
                } else {
                    (index as f64 * buffer.sample_rate / self.sample_rate) as u64
                };
                let k = (k % pairs as u64) as usize;
                i += buffer.samples[2 * k] as f64;
                q += buffer.samples[2 * k + 1] as f64;
                continue;
            }
            if let IqComponent::Digital { offset_hz, .. } = *c {
                let (a, b) = c.digital(seed, index, self.sample_rate);
                let (cos, sin) = cos_sin_turns(offset_hz * t);
                i += a * cos - b * sin;
                q += a * sin + b * cos;
                continue;
            }
            // Tones keep their original phase arithmetic (offset / rate ×
            // index) so the fixture's bytes do not move.
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

    /// A buffer component is the app's hand-off for signals the sim cannot
    /// synthesise: it replays exactly and loops, at its native rate.
    #[test]
    fn a_buffer_component_replays_and_loops() {
        static SAMPLES: [f32; 4] = [1.0, 2.0, 3.0, 4.0];
        let scene = IqScene {
            sample_rate: 4.0,
            components: vec![IqComponent::Buffer(IqBuffer {
                samples: &SAMPLES,
                sample_rate: 4.0,
            })],
            noise_rms: 0.0,
        };
        assert_eq!(scene.sample(0, 0), (1.0, 2.0));
        assert_eq!(scene.sample(0, 1), (3.0, 4.0));
        assert_eq!(scene.sample(0, 2), (1.0, 2.0), "the buffer loops");
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
        assert_eq!(f.layout(), SampleLayout::Complex);
        assert_eq!(f.channels[0].unit_count(f.layout()), 256);
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
                    rise_s: 0.0,
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
    fn rrc_has_unit_energy_and_nyquist_zeros_when_squared() {
        // ∫h² = 1 (Riemann sum at 64 samples per symbol over ±12 symbols).
        let e: f64 = (-12 * 64..=12 * 64)
            .map(|k| rrc(k as f64 / 64.0, 0.35).powi(2) / 64.0)
            .sum();
        assert!((e - 1.0).abs() < 2e-3, "{e}");
        // h * h is a raised cosine: zero at non-zero integer symbol lags.
        let rc = |lag: f64| -> f64 {
            (-12 * 64..=12 * 64)
                .map(|k| {
                    let t = k as f64 / 64.0;
                    rrc(t, 0.35) * rrc(t - lag, 0.35) / 64.0
                })
                .sum()
        };
        assert!(
            rc(1.0).abs() < 3e-3 && rc(2.0).abs() < 3e-3,
            "{} {}",
            rc(1.0),
            rc(2.0)
        );
        // The special points are continuous with their neighbourhoods.
        let x = 1.0 / (4.0 * 0.35);
        assert!((rrc(x, 0.35) - rrc(x + 1e-7, 0.35)).abs() < 1e-5);
        assert!((rrc(0.0, 0.35) - rrc(1e-7, 0.35)).abs() < 1e-5);
    }

    #[test]
    fn rrc_table_matches_the_direct_pulse() {
        let t = rrc_table(10, 0.35);
        let w = 2 * RRC_SPAN as usize + 1;
        for r in 0..10 {
            for j in 0..w {
                let tau = (r as f64 + (RRC_SPAN - j as i64) as f64 * 10.0) / 10.0;
                assert!((t[r * w + j] - rrc(tau, 0.35)).abs() < 1e-15);
            }
        }
    }

    #[test]
    fn digital_power_is_amplitude_squared_over_sps() {
        let scene = IqScene {
            sample_rate: 1e6,
            components: vec![IqComponent::Digital {
                modulation: Modulation::Qam16,
                symbol_rate: 100e3,
                offset_hz: 0.0,
                amplitude: 0.5,
                rolloff: 0.35,
            }],
            noise_rms: 0.0,
        };
        let d = scene.samples(1, 0, 20_000);
        let p = d.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / 20_000.0;
        assert!((p - 0.25 / 10.0).abs() < 0.001, "{p}");
    }

    #[test]
    fn am_envelope_and_fm_frequency_follow_their_tones() {
        let rate = 48_000.0;
        let am = IqScene {
            sample_rate: rate,
            components: vec![IqComponent::Am {
                offset_hz: 1000.0,
                amplitude: 0.4,
                depth: 0.5,
                tone_hz: 100.0,
            }],
            noise_rms: 0.0,
        };
        let env = |k: u64| {
            let (i, q) = am.sample(1, k);
            ((i * i + q * q) as f64).sqrt()
        };
        assert!((env(0) - 0.6).abs() < 1e-6 && (env(240) - 0.2).abs() < 1e-6); // t = 0, half a tone period
        let fm = IqScene {
            sample_rate: rate,
            components: vec![IqComponent::Fm {
                offset_hz: 0.0,
                amplitude: 0.5,
                deviation_hz: 3000.0,
                tone_hz: 100.0,
            }],
            noise_rms: 0.0,
        };
        let f = |k: u64| {
            let (a, b) = (fm.sample(1, k), fm.sample(1, k + 1));
            let (re, im) = (
                (b.0 * a.0 + b.1 * a.1) as f64,
                (b.1 * a.0 - b.0 * a.1) as f64,
            );
            im.atan2(re) * rate / std::f64::consts::TAU
        };
        assert!((f(0) - 3000.0).abs() < 20.0, "{}", f(0)); // peak deviation at t = 0
        assert!((f(240) + 3000.0).abs() < 20.0, "{}", f(240));
    }

    #[test]
    fn fnv1a64_matches_published_vectors() {
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C);
    }
}
