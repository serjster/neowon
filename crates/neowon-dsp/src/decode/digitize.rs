//! Turning a captured waveform into logic levels.
//!
//! This is where most decode failures actually happen: a decoder fed badly
//! digitized levels produces confident nonsense, and the fault looks like
//! the decoder's. Hysteresis is not optional — a single threshold on a
//! signal with any noise or ringing produces a burst of phantom edges at
//! every crossing.

/// Logic levels over time, one per input sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Digital {
    pub levels: Vec<bool>,
    pub sample_rate: f64,
}

impl Digital {
    /// Indices where the level changes, and what it changed to.
    pub fn edges(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.levels
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] != w[1])
            .map(|(i, w)| (i + 1, w[1]))
    }

    pub fn level_at(&self, i: usize) -> bool {
        self.levels.get(i).copied().unwrap_or(false)
    }

    /// Is the level steady across `a..b`? Used to check that a bit is
    /// actually stable where the protocol says it should be — an unstable
    /// mid-bit usually means the configured bit rate is wrong, which is
    /// worth telling the user instead of decoding rubbish.
    pub fn steady(&self, a: usize, b: usize) -> bool {
        let (a, b) = (a.min(self.levels.len()), b.min(self.levels.len()));
        if a >= b {
            return true;
        }
        let first = self.levels[a];
        self.levels[a..b].iter().all(|&l| l == first)
    }
}

/// Where to put the logic threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Threshold {
    /// Symmetric about the signal's own mid-point, with hysteresis as a
    /// fraction of its peak-to-peak range. The sane default: it adapts to
    /// whatever levels the probe actually sees.
    Relative { hysteresis: f64 },
    /// Fixed levels in raw counts, for signals whose idle level matters
    /// more than their midpoint.
    Absolute { low: i8, high: i8 },
}

impl Default for Threshold {
    fn default() -> Self {
        Threshold::Relative { hysteresis: 0.2 }
    }
}

/// Digitize `raw` into logic levels.
///
/// Returns `None` when the signal has no usable swing — better than
/// reporting a flat line as an idle bus and letting a decoder run on it.
pub fn digitize(raw: &[i8], sample_rate: f64, threshold: Threshold) -> Option<Digital> {
    if raw.len() < 2 {
        return None;
    }
    let (lo, hi) = match threshold {
        Threshold::Absolute { low, high } => (low as f64, high as f64),
        Threshold::Relative { hysteresis } => {
            let min = *raw.iter().min()? as f64;
            let max = *raw.iter().max()? as f64;
            let span = max - min;
            // Under a few counts of swing there is nothing to threshold;
            // a real logic signal on any sensible vertical setting is far
            // above this.
            if span < 8.0 {
                return None;
            }
            let mid = (max + min) / 2.0;
            let h = span * hysteresis.clamp(0.0, 0.49) / 2.0;
            (mid - h, mid + h)
        }
    };

    // Start from whichever side the first sample is closer to, so a capture
    // that begins mid-bit does not invent an edge at sample 0.
    let mut state = raw[0] as f64 >= (lo + hi) / 2.0;
    let mut levels = Vec::with_capacity(raw.len());
    for &v in raw {
        let v = v as f64;
        if state && v < lo {
            state = false;
        } else if !state && v > hi {
            state = true;
        }
        levels.push(state);
    }
    Some(Digital {
        levels,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square wave of `period` samples, amplitude ±100.
    fn square(n: usize, period: usize) -> Vec<i8> {
        (0..n)
            .map(|i| {
                if (i / (period / 2)).is_multiple_of(2) {
                    -100
                } else {
                    100
                }
            })
            .collect()
    }

    #[test]
    fn a_clean_square_digitizes_to_alternating_levels() {
        let d = digitize(&square(200, 20), 1e6, Threshold::default()).unwrap();
        let edges: Vec<_> = d.edges().collect();
        assert_eq!(edges.len(), 19, "one edge per half period after the first");
        // Evenly spaced, 10 samples apart.
        for w in edges.windows(2) {
            assert_eq!(w[1].0 - w[0].0, 10);
        }
    }

    #[test]
    fn hysteresis_rejects_noise_that_a_single_threshold_would_not() {
        // The case that matters in practice: an edge with finite slew, so
        // the signal dwells near the threshold, plus noise. A bare
        // threshold fires repeatedly while it crosses.
        let period = 20usize;
        let slew = 5usize;
        let raw: Vec<i8> = (0..200)
            .map(|i| {
                let phase = i % period;
                let high = phase >= period / 2;
                let into = if high { phase - period / 2 } else { phase };
                let base = if into < slew {
                    // Ramp through the middle.
                    let f = into as f64 / slew as f64;
                    let (from, to) = if high {
                        (-100.0, 100.0)
                    } else {
                        (100.0, -100.0)
                    };
                    from + (to - from) * f
                } else if high {
                    100.0
                } else {
                    -100.0
                };
                let noise = if i % 2 == 0 { 25.0 } else { -25.0 };
                (base + noise).clamp(-127.0, 127.0) as i8
            })
            .collect();

        let with = digitize(&raw, 1e6, Threshold::Relative { hysteresis: 0.45 }).unwrap();
        let without = digitize(&raw, 1e6, Threshold::Relative { hysteresis: 0.0 }).unwrap();
        assert!(
            without.edges().count() > with.edges().count(),
            "a bare threshold should chatter where hysteresis does not: \
             {} vs {}",
            without.edges().count(),
            with.edges().count()
        );
        // And hysteresis should land on roughly the real transition count.
        assert!(
            with.edges().count() <= 20,
            "hysteresis kept {} edges for 19 real transitions",
            with.edges().count()
        );
    }

    #[test]
    fn a_flat_line_is_not_an_idle_bus() {
        assert!(digitize(&[0i8; 100], 1e6, Threshold::default()).is_none());
        assert!(digitize(&[125i8; 100], 1e6, Threshold::default()).is_none());
    }

    #[test]
    fn steady_reports_instability_inside_a_bit() {
        let d = digitize(&square(200, 20), 1e6, Threshold::default()).unwrap();
        assert!(d.steady(2, 8), "well inside a half period");
        assert!(!d.steady(5, 15), "straddles an edge");
    }
}
