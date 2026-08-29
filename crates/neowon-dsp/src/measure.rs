use neowon_core::ChannelCapture;

/// Amplitude statistics of one trace, in volts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasicStats {
    pub vmin: f64,
    pub vmax: f64,
    pub vpp: f64,
    pub vavg: f64,
    pub vrms: f64,
}

pub fn basic_stats(cap: &ChannelCapture) -> Option<BasicStats> {
    if cap.raw.is_empty() {
        return None;
    }
    let mut min = i32::MAX;
    let mut max = i32::MIN;
    let mut sum = 0i64;
    let mut sq = 0i64;
    for &r in &cap.raw {
        let v = r as i32;
        min = min.min(v);
        max = max.max(v);
        sum += v as i64;
        sq += (v as i64) * (v as i64);
    }
    let n = cap.raw.len() as f64;
    let lsb = cap.volts_per_lsb;
    let mean_raw = sum as f64 / n;
    Some(BasicStats {
        vmin: min as f64 * lsb + cap.zero_volts,
        vmax: max as f64 * lsb + cap.zero_volts,
        vpp: (max - min) as f64 * lsb,
        vavg: mean_raw * lsb + cap.zero_volts,
        // RMS of the absolute signal (including its DC content).
        vrms: {
            let mean_sq = sq as f64 / n;
            let zero = cap.zero_volts / lsb; // in raw units
            // E[(x + z)^2] = E[x^2] + 2 z E[x] + z^2, all in raw units.
            let total = mean_sq + 2.0 * zero * mean_raw + zero * zero;
            total.max(0.0).sqrt() * lsb
        },
    })
}

/// Estimate the dominant frequency by hysteresis threshold crossings at the
/// waveform midpoint. Robust for periodic signals with at least two full
/// periods in the record; returns None otherwise.
pub fn estimate_frequency(raw: &[i8], sample_rate: f64) -> Option<f64> {
    if raw.len() < 8 || sample_rate <= 0.0 {
        return None;
    }
    let (min, max) = raw
        .iter()
        .fold((i32::MAX, i32::MIN), |(lo, hi), &r| {
            (lo.min(r as i32), hi.max(r as i32))
        });
    let span = max - min;
    if span < 8 {
        return None; // flat line — no measurable signal
    }
    let mid = (max + min) as f64 / 2.0;
    let hyst = (span as f64 / 8.0).max(2.0);
    let hi_th = mid + hyst / 2.0;
    let lo_th = mid - hyst / 2.0;

    // Rising crossings with hysteresis, linearly interpolated for sub-sample
    // resolution.
    let mut armed = false;
    let mut first: Option<f64> = None;
    let mut last: Option<f64> = None;
    let mut count = 0u32;
    let mut prev = raw[0] as f64;
    for (i, &r) in raw.iter().enumerate().skip(1) {
        let v = r as f64;
        if v < lo_th {
            armed = true;
        } else if armed && prev < hi_th && v >= hi_th {
            let frac = if v > prev { (hi_th - prev) / (v - prev) } else { 0.0 };
            let t = (i - 1) as f64 + frac;
            if first.is_none() {
                first = Some(t);
            }
            last = Some(t);
            count += 1;
            armed = false;
        }
        prev = v;
    }
    let (first, last) = (first?, last?);
    if count < 2 || last <= first {
        return None;
    }
    let periods = (count - 1) as f64;
    Some(periods * sample_rate / (last - first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_core::ChannelCapture;

    fn square(n: usize, samples_per_period: f64, lo: i8, hi: i8) -> Vec<i8> {
        (0..n)
            .map(|i| {
                let phase = (i as f64 / samples_per_period).fract();
                if phase < 0.5 { hi } else { lo }
            })
            .collect()
    }

    #[test]
    fn freq_of_square_wave() {
        // 1 kHz at 250 kS/s -> 250 samples/period, 5000 samples = 20 periods.
        let raw = square(5000, 250.0, 0, 125);
        let f = estimate_frequency(&raw, 250_000.0).unwrap();
        assert!((f - 1000.0).abs() < 5.0, "estimated {f}");
    }

    #[test]
    fn freq_of_sine_wave() {
        let raw: Vec<i8> = (0..5000)
            .map(|i| ((i as f64 / 100.0 * std::f64::consts::TAU).sin() * 100.0) as i8)
            .collect();
        // period = 100 samples @ 1 MS/s -> 10 kHz
        let f = estimate_frequency(&raw, 1_000_000.0).unwrap();
        assert!((f - 10_000.0).abs() < 50.0, "estimated {f}");
    }

    #[test]
    fn stats_of_unipolar_square() {
        // 0..5 V square on a 10 V full-scale range: raw 0..125, lsb = 10/250.
        let cap = ChannelCapture {
            ch: 0,
            raw: square(5000, 250.0, 0, 125),
            volts_per_lsb: 10.0 / 250.0,
            zero_volts: 0.0,
            clipped: false,
            freq_meter: None,
        };
        let s = basic_stats(&cap).unwrap();
        assert_eq!(s.vmin, 0.0);
        assert_eq!(s.vmax, 5.0);
        assert_eq!(s.vpp, 5.0);
        assert!((s.vavg - 2.5).abs() < 0.01);
        assert!((s.vrms - (0.5f64 * 25.0).sqrt()).abs() < 0.05);
    }

    #[test]
    fn no_freq_on_flat_line() {
        assert!(estimate_frequency(&[3i8; 5000], 250_000.0).is_none());
    }
}
