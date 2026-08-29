//! Simulated acquisition source — the virtual testbench generator.
//! Deterministic (seeded xorshift noise only), produces frames in the same
//! i8 encoding as real hardware so every downstream consumer is exercised
//! identically. Also the golden-signal source for DSP tests.

use neowon_core::{AcqMode, CaptureFrame, ChannelCapture};

pub mod backend;
pub mod figures;
pub mod scenario;
pub mod signal;

pub use backend::SimBackend;
pub use figures::XyFigure;
pub use scenario::Scenario;
pub use signal::{Component, SignalSpec, Xorshift};

/// Samples per record, matching the VDS1022 frame shape.
pub const SAMPLES: usize = 5000;

/// Deterministic signal generator over a [`Scenario`].
#[derive(Debug, Clone)]
pub struct SimSource {
    pub sample_rate: f64,
    scenario: Scenario,
    enabled: [bool; 2],
    /// Full-scale vertical range per channel in volts (10 divisions).
    ranges: [f64; 2],
    seq: u64,
    /// Continuous time origin in seconds, so consecutive frames join
    /// seamlessly.
    t0: f64,
    rng: Xorshift,
}

impl Default for SimSource {
    fn default() -> Self {
        Self::new(250_000.0, Scenario::default())
    }
}

impl SimSource {
    pub fn new(sample_rate: f64, scenario: Scenario) -> Self {
        Self {
            sample_rate,
            scenario,
            // CH1 on by default, like a freshly powered scope.
            enabled: [true, false],
            ranges: [10.0, 10.0],
            seq: 0,
            t0: 0.0,
            rng: Xorshift::default(),
        }
    }

    pub fn set_scenario(&mut self, s: Scenario) {
        self.scenario = s;
    }

    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    pub fn set_enabled(&mut self, ch: usize, on: bool) {
        if ch < 2 {
            self.enabled[ch] = on;
        }
    }

    /// Full-scale vertical range of `ch` in volts (10 divisions).
    pub fn set_range(&mut self, ch: usize, full_scale: f64) {
        if ch < 2 {
            self.ranges[ch] = full_scale;
        }
    }

    pub fn time(&self) -> f64 {
        self.t0
    }

    /// Reposition the sample-time origin; used to align a trigger crossing
    /// with the requested horizontal trigger position.
    pub fn set_time(&mut self, t: f64) {
        self.t0 = t;
    }

    /// Noise-free scenario voltages at `t` (trigger searches use these).
    pub fn volts_quiet(&self, t: f64) -> [f64; 2] {
        self.scenario.sample_quiet(t)
    }

    pub fn next_frame(&mut self) -> CaptureFrame {
        let mut raws = [Vec::with_capacity(SAMPLES), Vec::with_capacity(SAMPLES)];
        let mut clipped = [false, false];
        for i in 0..SAMPLES {
            let t = self.t0 + i as f64 / self.sample_rate;
            let v = self.scenario.sample(t, &mut self.rng);
            for ch in 0..2 {
                let lsb = self.ranges[ch] / 250.0;
                let q = (v[ch] / lsb).round();
                let r = q.clamp(-125.0, 125.0);
                if r != q {
                    clipped[ch] = true;
                }
                raws[ch].push(r as i8);
            }
        }
        self.t0 += SAMPLES as f64 / self.sample_rate;
        self.seq += 1;
        let channels = (0..2)
            .filter(|&ch| self.enabled[ch])
            .map(|ch| ChannelCapture {
                ch,
                raw: std::mem::take(&mut raws[ch]),
                volts_per_lsb: self.ranges[ch] / 250.0,
                zero_volts: 0.0,
                clipped: clipped[ch],
                freq_meter: self.scenario.fundamental(ch),
            })
            .collect();
        CaptureFrame {
            seq: self.seq,
            sample_rate: self.sample_rate,
            acq: AcqMode::Sample,
            channels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_dsp::{basic_stats, estimate_frequency};

    #[test]
    fn golden_probe_comp_signal() {
        let mut src = SimSource::default();
        let frame = src.next_frame();
        let cap = &frame.channels[0];
        assert_eq!(cap.raw.len(), SAMPLES);

        let stats = basic_stats(cap).unwrap();
        assert!((stats.vpp - 5.0).abs() < 0.3, "vpp {}", stats.vpp);
        assert!((stats.vavg - 2.5).abs() < 0.1, "vavg {}", stats.vavg);

        let f = estimate_frequency(&cap.raw, frame.sample_rate).unwrap();
        assert!((f - 1000.0).abs() < 10.0, "freq {f}");
    }

    #[test]
    fn frames_are_phase_continuous() {
        let mut src = SimSource::new(
            250_000.0,
            Scenario::PerChannel([
                SignalSpec {
                    // Noise-free so the comparison is exact.
                    components: vec![Component::Square {
                        freq: 1000.0,
                        amp: 2.5,
                        duty: 0.5,
                        phase: 0.0,
                    }],
                },
                SignalSpec::default(),
            ]),
        );
        let a = src.next_frame();
        let b = src.next_frame();
        // 250 kS/s, 1 kHz -> 250 samples/period; 5000 % 250 == 0, so frame b
        // must start exactly where a started (same phase).
        assert_eq!(a.channels[0].raw[0], b.channels[0].raw[0]);
        assert_eq!(a.seq + 1, b.seq);
    }

    #[test]
    fn deterministic() {
        let mut a = SimSource::default();
        let mut b = SimSource::default();
        assert_eq!(
            a.next_frame().channels[0].raw,
            b.next_frame().channels[0].raw
        );
    }
}
