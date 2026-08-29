//! Simulated acquisition source. Deterministic (seeded LCG noise), produces
//! frames in the same encoding as real hardware so every downstream consumer
//! is exercised identically. Also the golden-signal source for DSP tests.

use neowon_core::{CaptureFrame, ChannelCapture};

pub mod backend;
pub use backend::SimBackend;

pub const SAMPLES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Square,
    Triangle,
}

#[derive(Debug, Clone)]
pub struct SimChannel {
    pub enabled: bool,
    pub waveform: Waveform,
    /// Signal frequency in Hz.
    pub freq: f64,
    /// Peak-to-peak amplitude in volts.
    pub amplitude: f64,
    /// DC offset of the signal itself, volts.
    pub dc: f64,
    /// Full-scale vertical range in volts (10 divisions).
    pub range: f64,
    /// RMS noise in volts.
    pub noise: f64,
}

impl Default for SimChannel {
    fn default() -> Self {
        // Mirrors the probe-comp test signal: 1 kHz, 0..5 V square.
        Self {
            enabled: true,
            waveform: Waveform::Square,
            freq: 1000.0,
            amplitude: 5.0,
            dc: 2.5,
            range: 10.0,
            noise: 0.02,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimSource {
    pub sample_rate: f64,
    pub channels: Vec<SimChannel>,
    seq: u64,
    /// Continuous phase in seconds, so consecutive frames join seamlessly.
    t0: f64,
    rng: u64,
}

impl Default for SimSource {
    fn default() -> Self {
        Self::new(250_000.0, vec![SimChannel::default()])
    }
}

impl SimSource {
    pub fn new(sample_rate: f64, channels: Vec<SimChannel>) -> Self {
        Self { sample_rate, channels, seq: 0, t0: 0.0, rng: 0x9E37_79B9_7F4A_7C15 }
    }

    fn next_noise(&mut self) -> f64 {
        // xorshift64* — deterministic, no rand dependency.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let u = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
        u - 0.5
    }

    pub fn next_frame(&mut self) -> CaptureFrame {
        let mut channels = Vec::new();
        for ci in 0..self.channels.len() {
            let ch = self.channels[ci].clone();
            if !ch.enabled {
                continue;
            }
            let lsb = ch.range / 250.0;
            let mut raw = Vec::with_capacity(SAMPLES);
            let mut clipped = false;
            for i in 0..SAMPLES {
                let t = self.t0 + i as f64 / self.sample_rate;
                let phase = (t * ch.freq).fract();
                let unit = match ch.waveform {
                    Waveform::Sine => (phase * std::f64::consts::TAU).sin() * 0.5,
                    Waveform::Square => {
                        if phase < 0.5 { 0.5 } else { -0.5 }
                    }
                    Waveform::Triangle => {
                        if phase < 0.5 { 2.0 * phase - 0.5 } else { 1.5 - 2.0 * phase }
                    }
                };
                let volts = unit * ch.amplitude + ch.dc + self.next_noise() * ch.noise * 3.46;
                let q = (volts / lsb).round();
                let r = q.clamp(-125.0, 125.0);
                if r != q {
                    clipped = true;
                }
                raw.push(r as i8);
            }
            channels.push(ChannelCapture {
                ch: ci,
                raw,
                volts_per_lsb: lsb,
                zero_volts: 0.0,
                clipped,
                freq_meter: Some(ch.freq),
            });
        }
        self.t0 += SAMPLES as f64 / self.sample_rate;
        self.seq += 1;
        CaptureFrame { seq: self.seq, sample_rate: self.sample_rate, channels }
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
            vec![SimChannel { noise: 0.0, ..SimChannel::default() }],
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
        assert_eq!(a.next_frame().channels[0].raw, b.next_frame().channels[0].raw);
    }
}
