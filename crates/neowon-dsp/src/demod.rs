//! Streaming demodulators (Phase 10.10): channelise one tuned offset out of
//! the IQ stream and recover audio. Engine-free and deterministic — the
//! golden tests drive it from `neowon-sim`'s exact AM/FM generators, whose
//! closed forms define the expectation.
//!
//! Chain: NCO mix to baseband → decimating FIR channel filter → mode
//! demodulator (AM envelope with slow AGC; FM quadrature discriminator with
//! optional de-emphasis) → windowed-sinc resampler to the sound-card rate.
//! State persists across frames, so the app feeds it chunk by chunk.

use std::f64::consts::TAU;

/// How the tuned channel is turned into audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemodMode {
    Am,
    Nfm,
    Wfm,
}

impl DemodMode {
    pub const ALL: [DemodMode; 3] = [DemodMode::Am, DemodMode::Nfm, DemodMode::Wfm];

    /// UI label.
    pub fn label(self) -> &'static str {
        match self {
            DemodMode::Am => "AM",
            DemodMode::Nfm => "NFM",
            DemodMode::Wfm => "WFM",
        }
    }

    /// Script word (`sdr demod <verb>`), case-insensitive on parse.
    pub fn verb(self) -> &'static str {
        match self {
            DemodMode::Am => "am",
            DemodMode::Nfm => "nfm",
            DemodMode::Wfm => "wfm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "am" => Some(DemodMode::Am),
            "nfm" | "fm" => Some(DemodMode::Nfm),
            "wfm" => Some(DemodMode::Wfm),
            _ => None,
        }
    }

    /// Default channel width, Hz — a starting point the operator overrides.
    pub fn default_width_hz(self) -> f64 {
        match self {
            DemodMode::Am => 10e3,
            DemodMode::Nfm => 12.5e3,
            DemodMode::Wfm => 180e3,
        }
    }

    /// Audio band to keep, Hz.
    fn audio_cutoff_hz(self) -> f64 {
        match self {
            DemodMode::Am => 5e3,
            DemodMode::Nfm => 4e3,
            DemodMode::Wfm => 15e3,
        }
    }

    /// Intermediate rate to demodulate at: wide enough for the mode's
    /// deviation plus audio, near the sound card's rate for the narrow
    /// ones so the final resample is small.
    fn target_if_hz(self) -> f64 {
        match self {
            DemodMode::Am | DemodMode::Nfm => 48e3,
            DemodMode::Wfm => 240e3,
        }
    }
}

/// What the receiver is tuned to and how to demodulate it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceiverConfig {
    pub mode: DemodMode,
    /// Tuned channel offset from the hardware centre, Hz.
    pub offset_hz: f64,
    /// Channel width, Hz (the filter's passband).
    pub width_hz: f64,
    /// IQ rate at the input, pairs/s.
    pub sample_rate: f64,
    /// Audio rate the sink runs at.
    pub audio_rate: f64,
    /// FM de-emphasis time constant; `None` leaves the audio flat.
    pub deemphasis_tau_s: Option<f64>,
}

impl ReceiverConfig {
    pub fn new(mode: DemodMode, sample_rate: f64, audio_rate: f64) -> Self {
        Self {
            mode,
            offset_hz: 0.0,
            width_hz: mode.default_width_hz(),
            sample_rate,
            audio_rate,
            deemphasis_tau_s: Some(75e-6),
        }
    }
}

/// A streaming demodulator. Feed it IQ chunks in order; it emits audio at
/// `audio_rate` and keeps its filter/discriminator state across calls.
pub struct Receiver {
    cfg: ReceiverConfig,
    if_rate: f64,
    decim: usize,
    /// Channel filter at the input rate.
    h: Vec<f64>,
    hist_i: Vec<f64>,
    hist_q: Vec<f64>,
    /// Input index `hist[0]` corresponds to.
    base: i64,
    /// Next decimated-sample index to emit.
    m: i64,
    /// NCO phase, turns.
    nco: f64,
    prev: Option<(f64, f64)>,
    deemph: f64,
    am_mean: f64,
    resamp: Resampler,
    /// Channel power of the last chunk, dBFS.
    last_dbfs: f64,
}

impl Receiver {
    pub fn new(cfg: ReceiverConfig) -> Self {
        let mut r = Self {
            cfg,
            if_rate: 1.0,
            decim: 1,
            h: Vec::new(),
            hist_i: Vec::new(),
            hist_q: Vec::new(),
            base: 0,
            m: 0,
            nco: 0.0,
            prev: None,
            deemph: 0.0,
            am_mean: 0.0,
            resamp: Resampler::new(1.0, 1.0, 1.0),
            last_dbfs: f64::NEG_INFINITY,
        };
        r.build();
        r
    }

    /// Point the receiver at a new config. An offset-only change keeps the
    /// filter history (avoids a click); anything else rebuilds the channel.
    pub fn configure(&mut self, cfg: ReceiverConfig) {
        if self.cfg == cfg {
            return;
        }
        let rebuild = self.cfg.mode != cfg.mode
            || self.cfg.width_hz != cfg.width_hz
            || self.cfg.sample_rate != cfg.sample_rate
            || self.cfg.audio_rate != cfg.audio_rate;
        self.cfg = cfg;
        if rebuild {
            self.build();
        }
    }

    fn build(&mut self) {
        let fs = self.cfg.sample_rate.max(1.0);
        let target = self.cfg.mode.target_if_hz();
        self.decim = ((fs / target).round() as usize).max(1);
        self.if_rate = fs / self.decim as f64;

        // Channel low-pass at the input rate: keep `width/2`, reject by the
        // decimated Nyquist. Transition is `if_rate - width`, which bounds
        // the tap count for narrow channels too.
        let fc = (self.cfg.width_hz / 2.0).clamp(1.0, 0.45 * self.if_rate);
        let transition = (self.if_rate - 2.0 * fc).max(0.05 * self.if_rate);
        let taps = ((8.0 * fs / transition).ceil() as usize).clamp(31, 4001) | 1;
        let half = (taps / 2) as f64;
        let mut h: Vec<f64> = (0..taps)
            .map(|k| {
                let x = k as f64 - half;
                let sinc = if x.abs() < 1e-9 {
                    2.0 * fc / fs
                } else {
                    (TAU * (fc / fs) * x).sin() / (std::f64::consts::PI * x)
                };
                // Hamming window.
                let w = 0.54 + 0.46 * (std::f64::consts::PI * x / half).cos();
                sinc * w
            })
            .collect();
        let gain: f64 = h.iter().sum();
        for v in &mut h {
            *v /= gain;
        }
        self.h = h;

        self.hist_i.clear();
        self.hist_q.clear();
        self.base = 0;
        self.m = 0;
        self.prev = None;
        self.deemph = 0.0;
        self.am_mean = 0.0;
        self.last_dbfs = f64::NEG_INFINITY;
        self.resamp = Resampler::new(
            self.if_rate,
            self.cfg.audio_rate,
            self.cfg.mode.audio_cutoff_hz(),
        );
    }

    /// Recovered channel power of the last `process` call, dBFS.
    pub fn channel_dbfs(&self) -> f64 {
        self.last_dbfs
    }

    /// Demodulate `iq` (interleaved f32 I,Q) into `audio`. Appends to
    /// `audio`; the caller clears it.
    pub fn process(&mut self, iq: &[f32], audio: &mut Vec<f32>) {
        if self.h.is_empty() || self.cfg.sample_rate <= 0.0 {
            return;
        }
        // Mix to baseband and buffer.
        let phase_step = self.cfg.offset_hz / self.cfg.sample_rate;
        for p in iq.as_chunks::<2>().0 {
            let (s, c) = (TAU * self.nco).sin_cos();
            let (i, q) = (p[0] as f64, p[1] as f64);
            // (i + jq)·e^{-jθ}
            self.hist_i.push(i * c + q * s);
            self.hist_q.push(q * c - i * s);
            self.nco += phase_step;
            if self.nco >= 0.5 {
                self.nco -= 1.0;
            } else if self.nco < -0.5 {
                self.nco += 1.0;
            }
        }

        // Decimate: output at the filter's centre.
        let taps = self.h.len() as i64;
        let half = (taps - 1) / 2;
        let mut if_i = Vec::new();
        let mut if_q = Vec::new();
        loop {
            let center = self.m * self.decim as i64 + half;
            if center + half >= self.base + self.hist_i.len() as i64 {
                break;
            }
            let start = (center - half - self.base) as usize;
            let mut ai = 0.0;
            let mut aq = 0.0;
            for k in 0..taps as usize {
                ai += self.hist_i[start + k] * self.h[k];
                aq += self.hist_q[start + k] * self.h[k];
            }
            if_i.push(ai);
            if_q.push(aq);
            self.m += 1;
        }
        // Drop history before the next output's earliest tap.
        let drop = (self.m * self.decim as i64 - self.base).max(0) as usize;
        let drop = drop.min(self.hist_i.len());
        if drop > 0 {
            self.hist_i.drain(..drop);
            self.hist_q.drain(..drop);
            self.base += drop as i64;
        }

        if if_i.is_empty() {
            return;
        }
        let power: f64 = if_i
            .iter()
            .zip(&if_q)
            .map(|(i, q)| i * i + q * q)
            .sum::<f64>()
            / if_i.len() as f64;
        self.last_dbfs = 10.0 * (power + 1e-20).log10();

        let mut demod = Vec::with_capacity(if_i.len());
        match self.cfg.mode {
            DemodMode::Am => {
                // Envelope with a slow mean; the DC is the carrier.
                let alpha = 0.01;
                for (i, q) in if_i.iter().zip(&if_q) {
                    let a = (i * i + q * q).sqrt();
                    if self.am_mean <= 0.0 {
                        self.am_mean = a;
                    }
                    self.am_mean += alpha * (a - self.am_mean);
                    demod.push(((a - self.am_mean) / self.am_mean.max(1e-3)) as f32);
                }
            }
            DemodMode::Nfm | DemodMode::Wfm => {
                let scale = self.if_rate / TAU;
                let half_width = (self.cfg.width_hz / 2.0).max(1.0);
                let dt = 1.0 / self.if_rate;
                let alpha = self
                    .cfg
                    .deemphasis_tau_s
                    .map(|tau| dt / (tau + dt))
                    .unwrap_or(1.0);
                for (i, q) in if_i.iter().zip(&if_q) {
                    if let Some((pi, pq)) = self.prev {
                        // arg(z·conj(prev)) is the phase advance this sample.
                        let (re, im) = (i * pi + q * pq, q * pi - i * pq);
                        let d = im.atan2(re) * scale;
                        self.deemph += alpha * (d - self.deemph);
                        demod.push((self.deemph / half_width) as f32);
                    }
                    self.prev = Some((*i, *q));
                }
            }
        }

        self.resamp.process(&demod, audio);
    }
}

/// Streaming windowed-sinc resampler with an anti-alias low-pass. Used for
/// the last step from the demodulator's IF rate to the sound-card rate,
/// where the ratio is arbitrary (e.g. 227.5 kHz → 48 kHz).
struct Resampler {
    /// Input samples per output sample.
    step: f64,
    taps: usize,
    phases: usize,
    /// `phases × taps`, per-fraction kernels.
    kernel: Vec<f32>,
    hist: Vec<f32>,
    /// Position in `hist` of the next output sample.
    next: f64,
}

impl Resampler {
    fn new(in_rate: f64, out_rate: f64, cutoff_hz: f64) -> Self {
        let taps = 127usize;
        let phases = 512usize;
        let in_rate = in_rate.max(1.0);
        let out_rate = out_rate.max(1.0);
        let half = (taps / 2) as f64;
        // Never let the kernel pass above either Nyquist.
        let fc = (cutoff_hz / in_rate).clamp(1e-6, 0.4999);
        let mut kernel = vec![0f32; phases * taps];
        for p in 0..phases {
            let frac = p as f64 / phases as f64;
            let mut sum = 0.0f64;
            for k in 0..taps {
                // Distance from the output instant to the input sample.
                let x = k as f64 - half - frac;
                let sinc = if x.abs() < 1e-9 {
                    2.0 * fc
                } else {
                    (TAU * fc * x).sin() / (std::f64::consts::PI * x)
                };
                let u = (x + half) / taps as f64;
                let w = 0.42 - 0.5 * (TAU * u).cos() + 0.08 * (2.0 * TAU * u).cos();
                let v = sinc * w;
                kernel[p * taps + k] = v as f32;
                sum += v;
            }
            let g = 1.0 / sum;
            for k in 0..taps {
                kernel[p * taps + k] = (kernel[p * taps + k] as f64 * g) as f32;
            }
        }
        Self {
            step: in_rate / out_rate,
            taps,
            phases,
            kernel,
            hist: Vec::new(),
            next: 0.0,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        self.hist.extend_from_slice(input);
        let taps = self.taps;
        let half = (taps / 2) as i64;
        loop {
            let base = self.next.floor() as i64;
            if base - half < 0 {
                self.next += self.step;
                continue;
            }
            if base + half >= self.hist.len() as i64 {
                break;
            }
            let frac = self.next - base as f64;
            let ph = ((frac * self.phases as f64) as usize).min(self.phases - 1);
            let k0 = ph * taps;
            let b = base as usize;
            let mut acc = 0f64;
            for k in 0..taps {
                let idx = (b as i64 - half + k as i64) as usize;
                acc += self.hist[idx] as f64 * self.kernel[k0 + k] as f64;
            }
            out.push(acc as f32);
            self.next += self.step;
        }
        let keep = (self.next.floor() as i64 - half).max(0) as usize;
        if keep > 0 && keep <= self.hist.len() {
            self.hist.drain(..keep);
            self.next -= keep as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RMS of a slice.
    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// Frequency of a real tone by zero crossings over the middle half.
    fn tone_hz(a: &[f32], rate: f64) -> f64 {
        let a = &a[a.len() / 4..a.len() * 3 / 4];
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        let mut cross = 0u64;
        let mut prev = a[0] - mean;
        for &x in &a[1..] {
            let y = x - mean;
            if (prev < 0.0) != (y < 0.0) {
                cross += 1;
            }
            prev = y;
        }
        cross as f64 / 2.0 / (a.len() as f64 / rate)
    }

    #[test]
    fn resampler_keeps_a_tone_and_rejects_above_the_cutoff() {
        let (fin, fout, cutoff) = (227.5e3, 48e3, 15e3);
        let n = 64_000usize;
        let tone = |f: f64| -> Vec<f32> {
            (0..n)
                .map(|k| (TAU * f * k as f64 / fin).sin() as f32)
                .collect()
        };
        let run = |x: &[f32]| -> Vec<f32> {
            let mut r = Resampler::new(fin, fout, cutoff);
            let mut out = Vec::new();
            for chunk in x.chunks(4096) {
                r.process(chunk, &mut out);
            }
            out
        };
        let pass = run(&tone(1e3));
        assert!(
            (tone_hz(&pass, fout) - 1e3).abs() < 1.0,
            "{}",
            tone_hz(&pass, fout)
        );
        let stop_in = tone(30e3);
        let stop_out = run(&stop_in);
        let db = 20.0 * (rms(&stop_out) / rms(&stop_in)).log10();
        assert!(db < -40.0, "30 kHz rejected by only {db:.1} dB");
    }
}
