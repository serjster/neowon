//! Labelled IQ datasets (Phase 10.8): recipes, builders, receiver
//! impairments, wideband compositing, hierarchical metadata and SigMF
//! export — every random choice drawn from the recipe's declared seed.
//!
//! Example `i` uses seed `splitmix64(recipe.seed, i)`, and every parameter
//! it draws is a counter draw from that seed, so an example is a pure
//! function of (recipe, i): the same recipe builds bit-identical bytes on
//! every platform (the generator and impairments use IEEE basic
//! operations only).
//!
//! Metadata records what was actually applied (the drawn values, not the
//! recipe's ranges), and each signal's occupancy rectangle comes from its
//! definition: an RRC signal occupies exactly `Rs·(1 + β)`, AM by a tone
//! `±tone`, CW nothing, FM by convention Carson's `2·(Δf + tone)` (≈98% of
//! its power; FM has no finite occupied band).

use neowon_core::Modulation;
use serde::{Deserialize, Serialize};

use crate::iq::{IqComponent, IqScene, cos_sin_turns, splitmix64};

/// Class labels a recipe may ask for (the classifier's preset labels,
/// less noise).
pub const LABELS: [&str; 8] = ["cw", "am", "fm", "bpsk", "qpsk", "8psk", "16qam", "64qam"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Impairments {
    /// Largest |I/Q gain imbalance| drawn, dB.
    pub iq_gain_db: f64,
    /// Largest |I/Q phase imbalance| drawn, degrees.
    pub iq_phase_deg: f64,
    /// Largest DC offset magnitude drawn, full-scale units.
    pub dc_offset: f64,
}

/// A dataset's definition. As JSON (`Recipe::from_json`), omitted fields
/// take the defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub name: String,
    pub seed: u64,
    pub sample_rate: f64,
    /// I/Q pairs per example.
    pub samples: usize,
    pub examples: usize,
    pub classes: Vec<String>,
    /// Signals per example, inclusive; more than one is wideband
    /// compositing (non-overlapping rectangles).
    pub signals_per_example: (usize, usize),
    /// Per signal: its power over the total noise power, dB.
    pub snr_db: (f64, f64),
    pub symbol_rate_hz: (f64, f64),
    pub rolloff: f64,
    /// Signal centres are drawn within ±this.
    pub max_offset_hz: f64,
    pub noise_rms: f64,
    pub impairments: Impairments,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            name: "neowon-default".into(),
            seed: 1,
            sample_rate: 1e6,
            samples: 16 * 1024,
            examples: 8,
            classes: LABELS.iter().map(|s| s.to_string()).collect(),
            signals_per_example: (1, 1),
            snr_db: (10.0, 30.0),
            symbol_rate_hz: (25e3, 125e3),
            rolloff: 0.35,
            max_offset_hz: 250e3,
            noise_rms: 0.05,
            impairments: Impairments {
                iq_gain_db: 0.5,
                iq_phase_deg: 3.0,
                dc_offset: 0.01,
            },
        }
    }
}

impl Recipe {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let r: Recipe = serde_json::from_str(text).map_err(|e| e.to_string())?;
        validate(&r)?;
        Ok(r)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalMeta {
    pub label: String,
    pub centre_hz: f64,
    /// Occupancy rectangle: `lo_hz..hi_hz` over `t_start_s..t_stop_s`.
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub t_start_s: f64,
    pub t_stop_s: f64,
    pub snr_db: f64,
    pub amplitude: f64,
    pub symbol_rate_hz: Option<f64>,
    pub rolloff: Option<f64>,
    pub tone_hz: Option<f64>,
    pub depth: Option<f64>,
    pub deviation_hz: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppliedImpairments {
    /// Linear I gain (Q is the reference).
    pub iq_gain: f64,
    /// Q mixes in `sin(phase)` of I, radians.
    pub iq_phase_rad: f64,
    pub dc_i: f64,
    pub dc_q: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExampleMeta {
    pub index: usize,
    pub seed: u64,
    pub signals: Vec<SignalMeta>,
    pub impairments: AppliedImpairments,
}

pub struct Example {
    /// Interleaved I, Q.
    pub iq: Vec<f32>,
    pub meta: ExampleMeta,
}

/// Signal `j` of an example renders from `splitmix64(seed, SIGNAL_SALT + j)`.
const SIGNAL_SALT: u64 = 1 << 32;

/// Which parts to include (the full example has all; the others let a
/// test check the metadata against its causes).
#[derive(Debug, Clone, Copy)]
pub struct Parts {
    pub noise: bool,
    pub impairments: bool,
}

impl Parts {
    pub const ALL: Parts = Parts {
        noise: true,
        impairments: true,
    };
}

/// Counter draws from one example's seed.
struct Draws {
    seed: u64,
    k: u64,
}

impl Draws {
    fn unit(&mut self) -> f64 {
        self.k += 1;
        (splitmix64(self.seed, self.k) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    fn range(&mut self, (lo, hi): (f64, f64)) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn signed(&mut self, max: f64) -> f64 {
        (2.0 * self.unit() - 1.0) * max
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n - 1)
    }
}

fn modulation(label: &str) -> Option<Modulation> {
    match label {
        "bpsk" => Some(Modulation::Bpsk),
        "qpsk" => Some(Modulation::Qpsk),
        "8psk" => Some(Modulation::Psk8),
        "16qam" => Some(Modulation::Qam16),
        "64qam" => Some(Modulation::Qam64),
        _ => None,
    }
}

/// Draw one signal of `label` centred at `centre` with power `p`, and its
/// metadata (occupancy from the definition).
fn signal(
    d: &mut Draws,
    r: &Recipe,
    label: &str,
    centre: f64,
    snr_db: f64,
) -> (IqComponent, SignalMeta) {
    let p = r.noise_rms * r.noise_rms * 10f64.powf(snr_db / 10.0);
    let duration = r.samples as f64 / r.sample_rate;
    let mut meta = SignalMeta {
        label: label.into(),
        centre_hz: centre,
        lo_hz: centre,
        hi_hz: centre,
        t_start_s: 0.0,
        t_stop_s: duration,
        snr_db,
        amplitude: 0.0,
        symbol_rate_hz: None,
        rolloff: None,
        tone_hz: None,
        depth: None,
        deviation_hz: None,
    };
    let comp = match label {
        "cw" => {
            meta.amplitude = p.sqrt();
            IqComponent::Tone {
                offset_hz: centre,
                amplitude: meta.amplitude,
                phase: d.unit(),
            }
        }
        "am" => {
            let (tone, depth) = (d.range((500.0, 5e3)), d.range((0.2, 0.9)));
            meta.amplitude = (p / (1.0 + depth * depth / 2.0)).sqrt();
            (meta.tone_hz, meta.depth) = (Some(tone), Some(depth));
            (meta.lo_hz, meta.hi_hz) = (centre - tone, centre + tone);
            IqComponent::Am {
                offset_hz: centre,
                amplitude: meta.amplitude,
                depth,
                tone_hz: tone,
            }
        }
        "fm" => {
            let (tone, dev) = (d.range((500.0, 5e3)), d.range((5e3, 50e3)));
            meta.amplitude = p.sqrt();
            (meta.tone_hz, meta.deviation_hz) = (Some(tone), Some(dev));
            let carson = dev + tone;
            (meta.lo_hz, meta.hi_hz) = (centre - carson, centre + carson);
            IqComponent::Fm {
                offset_hz: centre,
                amplitude: meta.amplitude,
                deviation_hz: dev,
                tone_hz: tone,
            }
        }
        _ => {
            let m = modulation(label).expect("recipe labels are validated");
            let rs = d.range(r.symbol_rate_hz);
            // Per-sample power of the digital source is amplitude² / sps.
            meta.amplitude = (p * r.sample_rate / rs).sqrt();
            (meta.symbol_rate_hz, meta.rolloff) = (Some(rs), Some(r.rolloff));
            let half = rs * (1.0 + r.rolloff) / 2.0;
            (meta.lo_hz, meta.hi_hz) = (centre - half, centre + half);
            IqComponent::Digital {
                modulation: m,
                symbol_rate: rs,
                offset_hz: centre,
                amplitude: meta.amplitude,
                rolloff: r.rolloff,
            }
        }
    };
    (comp, meta)
}

/// Check a recipe before building from it.
pub fn validate(r: &Recipe) -> Result<(), String> {
    if let Some(l) = r.classes.iter().find(|c| !LABELS.contains(&c.as_str())) {
        return Err(format!("unknown class {l:?}; use {LABELS:?}"));
    }
    if r.classes.is_empty()
        || r.signals_per_example.0 == 0
        || r.signals_per_example.0 > r.signals_per_example.1
    {
        return Err("need classes and 1 <= min signals <= max signals".into());
    }
    if r.max_offset_hz + r.symbol_rate_hz.1 >= r.sample_rate / 2.0 {
        return Err("offsets plus bandwidth must stay inside the sample rate".into());
    }
    Ok(())
}

/// Build example `index` with the chosen `parts`.
pub fn build(r: &Recipe, index: usize, parts: Parts) -> Example {
    let seed = splitmix64(r.seed, index as u64);
    let mut d = Draws { seed, k: 0 };
    let (lo, hi) = r.signals_per_example;
    let count = lo + d.pick(hi - lo + 1);
    let mut comps = Vec::new();
    let mut metas: Vec<SignalMeta> = Vec::new();
    for _ in 0..count {
        let label = r.classes[d.pick(r.classes.len())].clone();
        let snr = d.range(r.snr_db);
        // Up to 32 tries for a centre whose rectangle overlaps no other
        // (with a 5 kHz guard); give up on the signal otherwise.
        for _ in 0..32 {
            let centre = d.signed(r.max_offset_hz);
            let mut probe = Draws { seed, k: d.k };
            let (_, m) = signal(&mut probe, r, &label, centre, snr);
            let clear = metas
                .iter()
                .all(|o| m.hi_hz + 5e3 < o.lo_hz || m.lo_hz - 5e3 > o.hi_hz);
            if clear {
                let (c, m) = signal(&mut d, r, &label, centre, snr);
                comps.push(c);
                metas.push(m);
                break;
            }
        }
    }
    let applied = AppliedImpairments {
        iq_gain: 10f64.powf(d.signed(r.impairments.iq_gain_db) / 20.0),
        iq_phase_rad: d.signed(r.impairments.iq_phase_deg).to_radians(),
        dc_i: d.signed(r.impairments.dc_offset),
        dc_q: d.signed(r.impairments.dc_offset),
    };
    // Each signal renders from its own seed (two digital signals must not
    // share a symbol stream); the noise from the example's. Summed in f32,
    // which is as platform-exact as the parts.
    let noise = IqScene {
        sample_rate: r.sample_rate,
        components: Vec::new(),
        noise_rms: if parts.noise { r.noise_rms } else { 0.0 },
    };
    let mut iq = noise.samples(seed, 0, r.samples);
    for (j, c) in comps.into_iter().enumerate() {
        let scene = IqScene {
            sample_rate: r.sample_rate,
            components: vec![c],
            noise_rms: 0.0,
        };
        let part = scene.samples(splitmix64(seed, SIGNAL_SALT + j as u64), 0, r.samples);
        for (a, b) in iq.iter_mut().zip(part) {
            *a += b;
        }
    }
    if parts.impairments {
        let (c, s) = cos_sin_turns(applied.iq_phase_rad / std::f64::consts::TAU);
        for p in iq.chunks_exact_mut(2) {
            let (i, q) = (p[0] as f64, p[1] as f64);
            p[0] = (applied.iq_gain * i + applied.dc_i) as f32;
            p[1] = (s * i + c * q + applied.dc_q) as f32;
        }
    }
    Example {
        iq,
        meta: ExampleMeta {
            index,
            seed,
            signals: metas,
            impairments: applied,
        },
    }
}

/// The whole dataset as one SigMF recording: `cf32_le` data (examples
/// back to back) and its metadata JSON, one annotation per signal. The
/// recipe and each example's metadata ride in `neowon:` fields.
pub fn sigmf(r: &Recipe) -> Result<(Vec<u8>, Vec<u8>), String> {
    validate(r)?;
    let mut data = Vec::with_capacity(r.examples * r.samples * 8);
    let mut annotations = Vec::new();
    let mut examples = Vec::new();
    for i in 0..r.examples {
        let ex = build(r, i, Parts::ALL);
        let start = i * r.samples;
        data.extend(ex.iq.iter().flat_map(|v| v.to_le_bytes()));
        for s in &ex.meta.signals {
            annotations.push(serde_json::json!({
                "core:sample_start": start as u64 + (s.t_start_s * r.sample_rate).round() as u64,
                "core:sample_count": ((s.t_stop_s - s.t_start_s) * r.sample_rate).round() as u64,
                "core:freq_lower_edge": s.lo_hz,
                "core:freq_upper_edge": s.hi_hz,
                "core:label": s.label,
                "neowon:example": i,
                "neowon:snr_db": s.snr_db,
            }));
        }
        examples.push(ex.meta);
    }
    let meta = serde_json::json!({
        "global": {
            "core:datatype": "cf32_le",
            "core:sample_rate": r.sample_rate,
            "core:version": "1.0.0",
            "core:recorder": format!("neowon-sim {}", env!("CARGO_PKG_VERSION")),
            "core:description": format!("neowon dataset {}", r.name),
            "neowon:recipe": r,
            "neowon:examples": examples,
        },
        "captures": [{ "core:sample_start": 0 }],
        "annotations": annotations,
    });
    let meta = serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?;
    Ok((data, meta))
}
