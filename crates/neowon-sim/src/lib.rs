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
    /// Vertical offset per channel as a fraction of full scale — shifts
    /// the raw codes exactly like the hardware zero DAC does.
    offsets: [f64; 2],
    seq: u64,
    /// Continuous time origin in seconds, so consecutive frames join
    /// seamlessly.
    t0: f64,
    /// Per-frame time advance override, seconds. `None` = one record length
    /// (free-running); WAV playback uses one UI tick so audio runs at 1x.
    frame_advance: Option<f64>,
    /// Emulate hardware peak detect: each output pair is the (min, max) of
    /// the signal over the interval those two samples span.
    peak: bool,
    /// Samples per record; WAV playback shortens records to exactly the
    /// audio advanced per tick so every sample is drawn once.
    frame_len: usize,
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
            offsets: [0.0, 0.0],
            seq: 0,
            t0: 0.0,
            frame_advance: None,
            peak: false,
            frame_len: SAMPLES,
            rng: Xorshift::default(),
        }
    }

    pub fn set_scenario(&mut self, s: Scenario) {
        // WAV playback advances one 30 fps UI tick per frame so the audio
        // plays at true speed (frames show an overlapping 5000-sample
        // window, exactly like a scope watching the line-out).
        (self.frame_advance, self.frame_len) = match &s {
            Scenario::XyWav { rate, .. } => {
                let len = (rate / 30.0).round().max(2.0) as usize;
                (Some(len as f64 / rate), len.min(SAMPLES))
            }
            _ => (None, SAMPLES),
        };
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

    /// Vertical offset of `ch` as a fraction of full scale (-0.5..=0.5).
    /// Shifts the raw codes like the hardware zero DAC; consumers recover
    /// true volts via `zero_volts`.
    pub fn set_offset(&mut self, ch: usize, offset: f64) {
        if ch < 2 {
            self.offsets[ch] = offset.clamp(-0.5, 0.5);
        }
    }

    pub fn time(&self) -> f64 {
        self.t0
    }

    /// Reposition the sample-time origin; used to align a trigger crossing
    /// with the requested horizontal trigger position.
    /// Hardware peak detect emulation (`AcqMode::Peak`).
    pub fn set_peak(&mut self, on: bool) {
        self.peak = on;
    }

    pub fn set_time(&mut self, t: f64) {
        self.t0 = t;
    }

    /// Noise-free scenario voltages at `t` (trigger searches use these).
    pub fn volts_quiet(&self, t: f64) -> [f64; 2] {
        self.scenario.sample_quiet(t)
    }

    /// Quantize one volt reading into the scope's i8 code, applying the
    /// zero-DAC offset exactly as `device.rs::configure_channel` does.
    fn quantize(&self, ch: usize, volts: f64, out: &mut Vec<i8>, clipped: &mut bool) {
        let lsb = self.ranges[ch] / 250.0;
        let pos0 = (250.0 * self.offsets[ch]).round();
        let q = (volts / lsb).round() + pos0;
        let r = q.clamp(-125.0, 125.0);
        if r != q {
            *clipped = true;
        }
        out.push(r as i8);
    }

    /// Peak detect: the instrument's ADC runs far faster than the storage
    /// rate and each stored *pair* keeps the extremes seen over its
    /// interval, which is what stops a fast signal aliasing away at slow
    /// time bases. Even index = min, odd = max (the VDS1022 convention).
    fn fill_peak(&mut self, n: usize, raws: &mut [Vec<i8>; 2], clipped: &mut [bool; 2]) {
        /// Sub-samples per output pair — the emulated ADC oversampling.
        const OVER: usize = 16;
        let dt = 1.0 / self.sample_rate;
        for k in 0..n.div_ceil(2) {
            let t0 = self.t0 + (2 * k) as f64 * dt;
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            for s in 0..OVER {
                let t = t0 + (s as f64 / OVER as f64) * 2.0 * dt;
                let v = self.scenario.sample(t, &mut self.rng);
                for ch in 0..2 {
                    lo[ch] = lo[ch].min(v[ch]);
                    hi[ch] = hi[ch].max(v[ch]);
                }
            }
            for ch in 0..2 {
                self.quantize(ch, lo[ch], &mut raws[ch], &mut clipped[ch]);
                if raws[ch].len() < n {
                    self.quantize(ch, hi[ch], &mut raws[ch], &mut clipped[ch]);
                }
            }
        }
    }

    pub fn next_frame(&mut self) -> CaptureFrame {
        let n = self.frame_len;
        let mut raws = [Vec::with_capacity(n), Vec::with_capacity(n)];
        let mut clipped = [false, false];
        if self.peak {
            self.fill_peak(n, &mut raws, &mut clipped);
        } else {
            for i in 0..n {
                let t = self.t0 + i as f64 / self.sample_rate;
                let v = self.scenario.sample(t, &mut self.rng);
                for ch in 0..2 {
                    self.quantize(ch, v[ch], &mut raws[ch], &mut clipped[ch]);
                }
            }
        }
        self.t0 += self.frame_advance.unwrap_or(n as f64 / self.sample_rate);
        self.seq += 1;
        let channels = (0..2)
            .filter(|&ch| self.enabled[ch])
            .map(|ch| ChannelCapture {
                ch,
                raw: std::mem::take(&mut raws[ch]),
                volts_per_lsb: self.ranges[ch] / 250.0,
                zero_volts: -self.offsets[ch] * self.ranges[ch],
                clipped: clipped[ch],
                freq_meter: self.scenario.fundamental(ch),
            })
            .collect();
        CaptureFrame {
            seq: self.seq,
            sample_rate: self.sample_rate,
            acq: if self.peak {
                AcqMode::Peak
            } else {
                AcqMode::Sample
            },
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

    /// The reported failure, reproduced and fixed in the simulator: sampling
    /// a 1 kHz signal at 500 S/s aliases it away, while peak detect keeps
    /// the envelope because each pair holds the extremes of its interval.
    #[test]
    fn peak_detect_survives_a_time_base_that_aliases() {
        let mut plain = SimSource::default();
        plain.set_scenario(Scenario::preset("sine-1k").unwrap());
        plain.set_enabled(0, true);
        plain.set_range(0, 2.0);
        plain.sample_rate = 500.0;

        let mut peak = SimSource::default();
        peak.set_scenario(Scenario::preset("sine-1k").unwrap());
        peak.set_enabled(0, true);
        peak.set_range(0, 2.0);
        peak.sample_rate = 500.0;
        peak.set_peak(true);

        let span = |f: &CaptureFrame| {
            let r = &f.channels[0].raw;
            (*r.iter().max().unwrap() as i32) - (*r.iter().min().unwrap() as i32)
        };
        let plain_span = span(&plain.next_frame());
        let peak_frame = peak.next_frame();
        assert_eq!(peak_frame.acq, AcqMode::Peak);
        let peak_span = span(&peak_frame);

        // The true amplitude is +-1 V on a +-1 V range = +-125 counts.
        assert!(
            peak_span > 200,
            "peak detect lost the envelope: span {peak_span}"
        );
        assert!(
            peak_span > plain_span * 2,
            "peak {peak_span} should dwarf aliased {plain_span}"
        );
    }

    #[test]
    fn peak_pairs_are_min_then_max() {
        let mut src = SimSource::default();
        src.set_scenario(Scenario::preset("sine-1k").unwrap());
        src.set_enabled(0, true);
        src.set_range(0, 2.0);
        src.sample_rate = 500.0;
        src.set_peak(true);
        let f = src.next_frame();
        let raw = &f.channels[0].raw;
        assert_eq!(raw.len(), SAMPLES);
        for k in 0..raw.len() / 2 {
            assert!(
                raw[2 * k] <= raw[2 * k + 1],
                "pair {k}: min {} > max {}",
                raw[2 * k],
                raw[2 * k + 1]
            );
        }
    }
}
