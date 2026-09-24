//! Signal detection over IQ: frame the capture, estimate the
//! noise floor with a rolling median across frequency, keep the bins well
//! above it, group them into clusters, and report each as a
//! `SignalObservation` whose edges are its 99% occupied band. A `Tracker`
//! then gives observations identity over time and debounces transients.
//!
//! Engine-free and deterministic: the same IQ always gives the same
//! observations.

use neowon_core::SignalObservation;

use crate::fft::Window;
use crate::iq::{IqSpectrum, average, stft};
use crate::modmeas::{Band, channel_power_dbfs, occupied_band, snr_db};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectConfig {
    /// Bins per spectrum.
    pub nfft: usize,
    /// A frame is `nfft · blocks` pairs. Within and across frames, blocks
    /// overlap by half and run continuously over the capture; each frame
    /// averages the `2 · blocks` that start inside it. Averaging keeps
    /// noise-only bins under the threshold, and the overlap leaves no
    /// instant of the signal only at a block's tapered edge, which would
    /// hide an event shorter than a block.
    pub blocks: usize,
    pub window: Window,
    /// Width of the floor window as a fraction of the band. The floor is
    /// the window's lower quartile, so a signal must fill three quarters
    /// of it to lift the floor: 1/4 of the band lets a signal up to ~1/5
    /// of the band wide (a 200 kHz FM channel at 1 MS/s) stand out.
    pub floor_span: f64,
    /// The floor is never taken below this, dBFS per bin: every real
    /// receiver has noise, and without a limit a noise-free input would
    /// report window leakage 150 dB down as signals.
    pub floor_min_dbfs: f64,
    /// A bin counts when it is this far above the floor, dB.
    pub threshold_db: f64,
    /// Clusters separated by at most this many quiet bins merge.
    pub gap_bins: usize,
    /// Bins either side of DC never count (the RTL2832's DC spike); 0 for
    /// the simulator.
    pub dc_guard: usize,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            nfft: 256,
            blocks: 4,
            window: Window::Hann,
            floor_span: 0.25,
            floor_min_dbfs: -120.0,
            threshold_db: 12.0,
            gap_bins: 2,
            dc_guard: 0,
        }
    }
}

impl DetectConfig {
    pub fn frame_len(&self) -> usize {
        self.nfft * self.blocks
    }
}

/// The quantile the floor takes of its window.
pub const FLOOR_QUANTILE: f64 = 0.25;
/// Standard-normal quantile at `FLOOR_QUANTILE`.
const FLOOR_Z: f64 = -0.674_489_750;

/// Noise floor per bin, dB: the lower quartile of the bins within
/// `width / 2` of each bin, corrected to the mean of noise averaged over
/// `blocks` blocks. A lower quartile ignores signals that fill up to three
/// quarters of the window (a median would be lifted by any signal wider
/// than half of it). Averaged noise power is chi-square with `2·blocks`
/// degrees of freedom, whose quantile the Wilson–Hilferty approximation
/// gives; dividing it out makes SNRs unbiased.
///
/// Computed per segment of `width / 8` bins and interpolated between
/// segment centres, so it costs a few sorts of `width` values, not one
/// per bin.
pub fn floor(db: &[f64], width: usize, blocks: usize) -> Vec<f64> {
    let n = db.len();
    let seg = (width / 8).max(1);
    let half = (width / 2).max(1);
    let segs = n.div_ceil(seg);
    let centre = |j: usize| (j * seg + (seg - 1).min(n - 1 - j * seg) / 2) as f64;
    let mut buf = Vec::with_capacity(width + seg);
    let level: Vec<f64> = (0..segs)
        .map(|j| {
            let c = centre(j) as usize;
            let (a, b) = (c.saturating_sub(half), (c + half).min(n - 1));
            buf.clear();
            buf.extend_from_slice(&db[a..=b]);
            buf.sort_by(f64::total_cmp);
            buf[((buf.len() - 1) as f64 * FLOOR_QUANTILE).round() as usize]
        })
        .collect();
    let nu = 2.0 * blocks.max(1) as f64;
    let h = 2.0 / (9.0 * nu);
    let ratio = (1.0 - h + FLOOR_Z * h.sqrt()).powi(3).max(1e-3);
    let correction = -10.0 * ratio.log10();
    (0..n)
        .map(|k| {
            let x = k as f64;
            let j = ((x - centre(0)) / seg as f64)
                .floor()
                .clamp(0.0, (segs - 1) as f64) as usize;
            let v = if j + 1 < segs {
                let (x0, x1) = (centre(j), centre(j + 1));
                let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                level[j] + t * (level[j + 1] - level[j])
            } else {
                level[j]
            };
            v + correction
        })
        .collect()
}

/// Observations in one spectrum. `centre_hz` makes them absolute;
/// `t_start..t_end` is the frame they came from.
pub fn detect_spectrum(
    s: &IqSpectrum,
    centre_hz: f64,
    t_start: f64,
    t_end: f64,
    cfg: &DetectConfig,
) -> Vec<SignalObservation> {
    detect_timed(s, centre_hz, &|_, _, _| (t_start, t_end), cfg)
}

/// When a cluster (bins `a..=b`) was present within a frame: from the
/// frame's block spectra, the span of the blocks whose band power clears
/// the band's floor by the threshold. Falls back to the whole frame when
/// no single block does (a signal only the average reveals).
fn block_span(
    blocks: &[Vec<f64>],
    (a, b): (usize, usize),
    floor_lin: f64,
    threshold_db: f64,
    clock: &FrameClock,
) -> (f64, f64) {
    let gate = floor_lin * 10f64.powf(threshold_db / 10.0);
    let hot: Vec<usize> = (0..blocks.len())
        .filter(|&i| blocks[i][a..=b].iter().sum::<f64>() > gate)
        .collect();
    match (hot.first(), hot.last()) {
        (Some(&f), Some(&l)) => (
            clock.t0 + f as f64 * clock.hop_s,
            clock.t0 + l as f64 * clock.hop_s + clock.block_s,
        ),
        _ => (clock.t0, clock.t0 + clock.frame_s),
    }
}

/// Where a frame's blocks sit in time.
struct FrameClock {
    t0: f64,
    hop_s: f64,
    block_s: f64,
    frame_s: f64,
}

/// `detect_spectrum` with the observation times chosen per cluster by
/// `when(a, b, floor_lin)` (bins `a..=b`, the floor's linear power summed
/// over them).
fn detect_timed(
    s: &IqSpectrum,
    centre_hz: f64,
    when: &dyn Fn(usize, usize, f64) -> (f64, f64),
    cfg: &DetectConfig,
) -> Vec<SignalObservation> {
    let width = ((s.len() as f64 * cfg.floor_span) as usize).max(8);
    let fl: Vec<f64> = floor(&s.power_db, width, s.blocks)
        .into_iter()
        .map(|f| f.max(cfg.floor_min_dbfs))
        .collect();
    let dc = s.len() / 2;
    let hot: Vec<bool> = (0..s.len())
        .map(|k| {
            (cfg.dc_guard == 0 || k.abs_diff(dc) > cfg.dc_guard)
                && s.power_db[k] - fl[k] > cfg.threshold_db
        })
        .collect();

    // Runs of hot bins, merging runs split by short quiet gaps.
    let mut clusters: Vec<(usize, usize)> = Vec::new();
    for k in (0..s.len()).filter(|&k| hot[k]) {
        match clusters.last_mut() {
            Some((_, b)) if k - *b <= cfg.gap_bins + 1 => *b = k,
            _ => clusters.push((k, k)),
        }
    }

    clusters
        .into_iter()
        .map(|(a, b)| {
            let lin: Vec<f64> = s.power_db[a..=b]
                .iter()
                .map(|&d| 10f64.powf(d / 10.0))
                .collect();
            let (lo, hi) = occupied_band(&lin, 0.99);
            let edge = |x: f64| s.offset_hz(a) + (x - 0.5) * s.bin_hz;
            let total: f64 = lin.iter().sum();
            let centroid = lin
                .iter()
                .enumerate()
                .map(|(i, p)| p * s.offset_hz(a + i))
                .sum::<f64>()
                / total;
            let band = Band { a, b };
            let floor_lin: f64 = fl[a..=b].iter().map(|&d| 10f64.powf(d / 10.0)).sum();
            let (t_start, t_end) = when(a, b, floor_lin);
            SignalObservation {
                centre_hz: centre_hz + centroid,
                lo_hz: centre_hz + edge(lo),
                hi_hz: centre_hz + edge(hi),
                power_dbfs: channel_power_dbfs(s, band, cfg.window),
                snr_db: snr_db(s, band, &fl),
                t_start,
                t_end,
            }
        })
        .collect()
}

/// Frame `iq` (interleaved, starting at time `t0`) and detect in each
/// whole frame. A frame's blocks may read up to half a block past its end.
/// Observation times are those of the blocks the signal is in (hop
/// resolution), not the whole frame, so a transient's duration is not
/// inflated to the frame length.
pub fn detect(
    iq: &[f32],
    rate: f64,
    centre_hz: f64,
    t0: f64,
    cfg: &DetectConfig,
) -> Vec<SignalObservation> {
    let (len, hop) = (cfg.frame_len(), cfg.nfft / 2);
    let pairs = iq.len() / 2;
    let dt = len as f64 / rate;
    (0..pairs / len)
        .filter_map(|f| {
            // Blocks starting in [f·len, (f+1)·len).
            let end = ((f + 1) * len - hop + cfg.nfft).min(pairs);
            let blocks = stft(&iq[2 * f * len..2 * end], cfg.window, cfg.nfft, hop)?;
            let s = average(&blocks, rate)?;
            let t = t0 + f as f64 * dt;
            let clock = FrameClock {
                t0: t,
                hop_s: hop as f64 / rate,
                block_s: cfg.nfft as f64 / rate,
                frame_s: dt,
            };
            let when =
                |a, b, floor_lin| block_span(&blocks, (a, b), floor_lin, cfg.threshold_db, &clock);
            Some(detect_timed(&s, centre_hz, &when, cfg))
        })
        .flatten()
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerConfig {
    /// A track is active once it has spanned this long.
    pub min_duration_s: f64,
    /// A track unseen for this long is dropped.
    pub hold_s: f64,
    /// Extra frequency slack when matching an observation to a track, Hz.
    pub assoc_hz: f64,
}

/// A signal followed over time: stable identity, first/last sighting, the
/// band it has covered, and its latest observation.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: u64,
    pub first_seen: f64,
    pub last_seen: f64,
    /// Union of the observed bands, Hz.
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub hits: usize,
    pub last: SignalObservation,
}

impl Track {
    pub fn duration(&self) -> f64 {
        self.last_seen - self.first_seen
    }

    pub fn bandwidth_hz(&self) -> f64 {
        self.hi_hz - self.lo_hz
    }
}

#[derive(Debug, Clone)]
pub struct Tracker {
    pub cfg: TrackerConfig,
    tracks: Vec<Track>,
    next_id: u64,
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self {
            cfg,
            tracks: Vec::new(),
            next_id: 1,
        }
    }

    /// Fold in one frame's observations: each extends the nearest track
    /// whose latest band it overlaps (± `assoc_hz`), else starts a track.
    /// Tracks unseen for `hold_s` before `now` are dropped.
    pub fn update(&mut self, obs: &[SignalObservation], now: f64) {
        let mut taken = vec![false; self.tracks.len()];
        for o in obs {
            let slack = self.cfg.assoc_hz;
            let best = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| {
                    !taken[*i] && o.lo_hz <= t.last.hi_hz + slack && o.hi_hz >= t.last.lo_hz - slack
                })
                .min_by(|a, b| {
                    let d = |t: &Track| (t.last.centre_hz - o.centre_hz).abs();
                    d(a.1).total_cmp(&d(b.1))
                })
                .map(|(i, _)| i);
            match best {
                Some(i) => {
                    taken[i] = true;
                    let t = &mut self.tracks[i];
                    t.last_seen = t.last_seen.max(o.t_end);
                    t.lo_hz = t.lo_hz.min(o.lo_hz);
                    t.hi_hz = t.hi_hz.max(o.hi_hz);
                    t.hits += 1;
                    t.last = o.clone();
                }
                None => {
                    self.tracks.push(Track {
                        id: self.next_id,
                        first_seen: o.t_start,
                        last_seen: o.t_end,
                        lo_hz: o.lo_hz,
                        hi_hz: o.hi_hz,
                        hits: 1,
                        last: o.clone(),
                    });
                    taken.push(true);
                    self.next_id += 1;
                }
            }
        }
        let hold = self.cfg.hold_s;
        self.tracks.retain(|t| now - t.last_seen <= hold);
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Tracks that have lasted `min_duration_s`: the debounced set.
    pub fn active(&self) -> impl Iterator<Item = &Track> {
        let min = self.cfg.min_duration_s;
        self.tracks
            .iter()
            .filter(move |t| t.duration() >= min - 1e-9)
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_ignores_a_signal_filling_most_of_its_window() {
        // A 400-bin plateau 40 dB up in a 4096-bin band: the median over a
        // narrow window would sit on the plateau; the quartile over a
        // quarter of the band stays on the noise.
        let mut db = vec![-70.0; 4096];
        for v in &mut db[1800..2200] {
            *v = -30.0;
        }
        let f = floor(&db, 1024, 64);
        assert!(
            f.iter().all(|&v| (v - -70.0).abs() < 0.5),
            "{:?}",
            &f[1990..2010]
        );
    }

    #[test]
    fn floor_reads_the_mean_of_averaged_noise() {
        // Chi-square(2K)/2K noise with K = 16: the corrected quartile must
        // land on the mean (0 dB) within a few tenths of a dB.
        let k = 16;
        let noise: Vec<f64> = (0..4096)
            .map(|i| {
                let s: f64 = (0..k)
                    .map(|j| {
                        let u = (neowon_sim_free_unit(i * k + j) + 1e-12).ln();
                        -u
                    })
                    .sum();
                10.0 * (s / k as f64).log10()
            })
            .collect();
        let f = floor(&noise, 1024, k);
        let mean = f.iter().sum::<f64>() / f.len() as f64;
        assert!(mean.abs() < 0.3, "{mean}");
    }

    /// A deterministic uniform in (0, 1) without pulling in the simulator.
    fn neowon_sim_free_unit(i: usize) -> f64 {
        let mut z = (i as u64)
            .wrapping_add(1)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn tracker_debounces_and_expires() {
        let obs = |t: f64| SignalObservation {
            centre_hz: 1000.0,
            lo_hz: 990.0,
            hi_hz: 1010.0,
            power_dbfs: -20.0,
            snr_db: 30.0,
            t_start: t,
            t_end: t + 0.1,
        };
        let mut tr = Tracker::new(TrackerConfig {
            min_duration_s: 0.2,
            hold_s: 0.3,
            assoc_hz: 50.0,
        });
        tr.update(&[obs(0.0)], 0.1);
        assert_eq!(tr.active().count(), 0);
        tr.update(&[obs(0.1)], 0.2);
        assert_eq!(tr.active().count(), 1);
        assert_eq!(tr.tracks()[0].id, 1);
        tr.update(&[], 0.6);
        assert!(tr.tracks().is_empty());
    }
}
