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
    let raw = raw_stats(&cap.raw)?;
    let lsb = cap.volts_per_lsb;
    Some(BasicStats {
        vmin: raw.min as f64 * lsb + cap.zero_volts,
        vmax: raw.max as f64 * lsb + cap.zero_volts,
        vpp: (raw.max - raw.min) as f64 * lsb,
        vavg: raw.mean * lsb + cap.zero_volts,
        vrms: {
            let zero = cap.zero_volts / lsb;
            let total = raw.mean_sq + 2.0 * zero * raw.mean + zero * zero;
            total.max(0.0).sqrt() * lsb
        },
    })
}

struct RawStats {
    min: i32,
    max: i32,
    mean: f64,
    mean_sq: f64,
}

fn raw_stats(raw: &[i8]) -> Option<RawStats> {
    if raw.is_empty() {
        return None;
    }
    let (mut min, mut max, mut sum, mut sq) = (i32::MAX, i32::MIN, 0i64, 0i64);
    for &r in raw {
        let v = r as i32;
        min = min.min(v);
        max = max.max(v);
        sum += v as i64;
        sq += (v * v) as i64;
    }
    let n = raw.len() as f64;
    Some(RawStats {
        min,
        max,
        mean: sum as f64 / n,
        mean_sq: sq as f64 / n,
    })
}

/// The full automatic measurement set of one trace. Voltages in volts, times
/// in seconds, duty as a fraction, over/preshoot as a fraction of Vamp.
/// Timing fields are `None` when the record doesn't contain enough edges.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measurements {
    pub vmin: f64,
    pub vmax: f64,
    pub vpp: f64,
    pub vtop: f64,
    pub vbase: f64,
    pub vamp: f64,
    pub vavg: f64,
    /// `None` on an envelope record: the RMS of a min/max envelope is not
    /// the signal's RMS (it is biased high by construction).
    pub vrms: Option<f64>,
    pub overshoot: Option<f64>,
    pub preshoot: Option<f64>,
    pub period: Option<f64>,
    pub freq: Option<f64>,
    pub rise: Option<f64>,
    pub fall: Option<f64>,
    pub pwidth: Option<f64>,
    pub nwidth: Option<f64>,
    pub pduty: Option<f64>,
    pub nduty: Option<f64>,
}

/// A mid-level crossing, in fractional sample time.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    t: f64,
    rising: bool,
}

/// Interpolated threshold crossings with hysteresis around `mid`.
fn crossings(raw: &[i8], mid: f64, hyst: f64) -> Vec<Crossing> {
    let hi_th = mid + hyst / 2.0;
    let lo_th = mid - hyst / 2.0;
    let mut out = Vec::new();
    // armed_low: we've seen the signal below lo_th since the last rising edge.
    let mut armed_low = false;
    let mut armed_high = false;
    let mut prev = raw[0] as f64;
    for (i, &r) in raw.iter().enumerate().skip(1) {
        let v = r as f64;
        if v < lo_th {
            armed_low = true;
        }
        if v > hi_th {
            armed_high = true;
        }
        if armed_low && prev < hi_th && v >= hi_th {
            let frac = if v > prev {
                (hi_th - prev) / (v - prev)
            } else {
                0.0
            };
            out.push(Crossing {
                t: (i - 1) as f64 + frac,
                rising: true,
            });
            armed_low = false;
        } else if armed_high && prev > lo_th && v <= lo_th {
            let frac = if v < prev {
                (prev - lo_th) / (prev - v)
            } else {
                0.0
            };
            out.push(Crossing {
                t: (i - 1) as f64 + frac,
                rising: false,
            });
            armed_high = false;
        }
        prev = v;
    }
    out
}

/// Vtop/Vbase by histogram mode above/below the midpoint (i8 samples make a
/// natural 256-bin histogram). Falls back to max/min for signals without flat
/// levels (e.g. sine).
fn top_base(raw: &[i8], min: i32, max: i32) -> (f64, f64) {
    let mid = ((min + max) / 2) as i8;
    let mut hist = [0u32; 256];
    for &r in raw {
        hist[(r as i32 + 128) as usize] += 1;
    }
    let significant = (raw.len() / 32).max(4) as u32;
    let mode_in = |lo: i32, hi: i32| -> Option<i32> {
        let mut best = None;
        let mut best_n = significant;
        for v in lo..=hi {
            let n = hist[(v + 128) as usize];
            if n >= best_n {
                best_n = n;
                best = Some(v);
            }
        }
        best
    };
    let top = mode_in(mid as i32 + 1, max).unwrap_or(max) as f64;
    let base = mode_in(min, mid as i32).unwrap_or(min) as f64;
    (top, base)
}

/// Time from `from_level` to `to_level` on the edge starting at crossing `c`,
/// searching outward from the mid crossing. Returns fractional samples.
fn edge_time(raw: &[i8], c: &Crossing, low_level: f64, high_level: f64) -> Option<f64> {
    let n = raw.len();
    let idx = c.t as usize;
    let (start_level, end_level) = if c.rising {
        (low_level, high_level)
    } else {
        (high_level, low_level)
    };
    // Walk backward to where the edge started.
    let mut a = idx;
    loop {
        let v = raw[a] as f64;
        let done = if c.rising {
            v <= start_level
        } else {
            v >= start_level
        };
        if done {
            break;
        }
        if a == 0 {
            return None;
        }
        a -= 1;
    }
    // Walk forward to where the edge finished.
    let mut b = idx + 1;
    loop {
        if b >= n {
            return None;
        }
        let v = raw[b] as f64;
        let done = if c.rising {
            v >= end_level
        } else {
            v <= end_level
        };
        if done {
            break;
        }
        b += 1;
    }
    let interp = |i0: usize, level: f64| -> f64 {
        let v0 = raw[i0] as f64;
        let v1 = raw[i0 + 1] as f64;
        if (v1 - v0).abs() < 1e-12 {
            i0 as f64
        } else {
            i0 as f64 + ((level - v0) / (v1 - v0)).clamp(0.0, 1.0)
        }
    };
    let t0 = interp(a, start_level);
    let t1 = interp(b - 1, end_level);
    (t1 > t0).then_some(t1 - t0)
}

/// Measure an **envelope** record — hardware peak detect, or our own deep
/// view, both of which store interleaved (min, max) pairs rather than
/// successive samples.
///
/// Only the metrics that survive min/max decimation are reported. Extrema do
/// survive, so amplitudes are exact; anything derived from the spacing of
/// samples does not, because adjacent entries are two views of the *same*
/// instant rather than two instants. Measuring such a record as if it were a
/// waveform is what produces the classic nonsense — every pair reads as a
/// full cycle, so the frequency comes out at half the column rate whatever
/// the signal is, and edge-time searches terminate after one step.
pub fn measure_envelope(cap: &ChannelCapture) -> Option<Measurements> {
    if cap.raw.len() < 2 {
        return None;
    }
    let lsb = cap.volts_per_lsb;
    let z = cap.zero_volts;
    // Extrema survive min/max decimation whatever the interleave phase is,
    // so take them over the whole record rather than over one series.
    let all = raw_stats(&cap.raw)?;

    // For the flat-top/flat-bottom histograms the two series must be told
    // apart, and which one holds the maxima is NOT fixed: the phase of the
    // pairing shifts between records on the VDS1022 (observed on hardware —
    // consecutive records reported Vmax and Vmin swapped). Identify it from
    // the data instead of trusting a convention.
    let a: Vec<i8> = cap.raw.iter().step_by(2).copied().collect();
    let b: Vec<i8> = cap.raw.iter().skip(1).step_by(2).copied().collect();
    let (sa, sb) = (raw_stats(&a)?, raw_stats(&b)?);
    let (maxs, mins, hi, lo) = if sa.mean >= sb.mean {
        (&a, &b, sa, sb)
    } else {
        (&b, &a, sb, sa)
    };
    let (top_r, _) = top_base(maxs, hi.min, hi.max);
    let (_, base_r) = top_base(mins, lo.min, lo.max);

    let mut m = Measurements {
        vmin: all.min as f64 * lsb + z,
        vmax: all.max as f64 * lsb + z,
        vpp: (all.max - all.min) as f64 * lsb,
        vtop: top_r * lsb + z,
        vbase: base_r * lsb + z,
        vamp: (top_r - base_r) * lsb,
        // The envelope's midline: the signal's mean for a symmetric wave,
        // and an honest centre otherwise.
        vavg: (lo.mean + hi.mean) / 2.0 * lsb + z,
        vrms: None,
        ..Default::default()
    };
    let amp_r = top_r - base_r;
    if amp_r >= 8.0 {
        m.overshoot = Some((all.max as f64 - top_r) / amp_r);
        m.preshoot = Some((base_r - all.min as f64) / amp_r);
    }
    Some(m)
}

pub fn measure(cap: &ChannelCapture, sample_rate: f64) -> Option<Measurements> {
    let raw = &cap.raw;
    let rs = raw_stats(raw)?;
    let lsb = cap.volts_per_lsb;
    let z = cap.zero_volts;
    let dt = 1.0 / sample_rate;

    let (top_r, base_r) = top_base(raw, rs.min, rs.max);
    let mut m = Measurements {
        vmin: rs.min as f64 * lsb + z,
        vmax: rs.max as f64 * lsb + z,
        vpp: (rs.max - rs.min) as f64 * lsb,
        vtop: top_r * lsb + z,
        vbase: base_r * lsb + z,
        vamp: (top_r - base_r) * lsb,
        vavg: rs.mean * lsb + z,
        vrms: Some({
            let zero = z / lsb;
            (rs.mean_sq + 2.0 * zero * rs.mean + zero * zero)
                .max(0.0)
                .sqrt()
                * lsb
        }),
        ..Default::default()
    };
    let amp_r = top_r - base_r;
    if amp_r >= 8.0 {
        m.overshoot = Some((rs.max as f64 - top_r) / amp_r);
        m.preshoot = Some((base_r - rs.min as f64) / amp_r);
    }

    let span = (rs.max - rs.min) as f64;
    if span < 8.0 || sample_rate <= 0.0 {
        return Some(m); // no measurable dynamic signal
    }
    let mid = (rs.max + rs.min) as f64 / 2.0;
    let hyst = (span / 8.0).max(2.0);
    let cr = crossings(raw, mid, hyst);

    // Period: average same-direction crossing spacing.
    let rises: Vec<f64> = cr.iter().filter(|c| c.rising).map(|c| c.t).collect();
    let falls: Vec<f64> = cr.iter().filter(|c| !c.rising).map(|c| c.t).collect();
    if rises.len() >= 2 {
        let period = (rises.last().unwrap() - rises[0]) / (rises.len() - 1) as f64 * dt;
        m.period = Some(period);
        m.freq = Some(1.0 / period);
    } else if falls.len() >= 2 {
        let period = (falls.last().unwrap() - falls[0]) / (falls.len() - 1) as f64 * dt;
        m.period = Some(period);
        m.freq = Some(1.0 / period);
    }

    // Widths: average duration between alternating crossings.
    let (mut pw, mut pn, mut nw, mut nn) = (0.0, 0u32, 0.0, 0u32);
    for pair in cr.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.rising == b.rising {
            continue;
        }
        let w = (b.t - a.t) * dt;
        if a.rising {
            pw += w;
            pn += 1;
        } else {
            nw += w;
            nn += 1;
        }
    }
    if pn > 0 {
        m.pwidth = Some(pw / pn as f64);
    }
    if nn > 0 {
        m.nwidth = Some(nw / nn as f64);
    }
    if let (Some(p), Some(t)) = (m.pwidth, m.period) {
        m.pduty = Some(p / t);
    }
    if let (Some(n), Some(t)) = (m.nwidth, m.period) {
        m.nduty = Some(n / t);
    }

    // Rise/fall: 10%..90% of Vamp, averaged over edges.
    let low10 = base_r + 0.1 * amp_r;
    let high90 = base_r + 0.9 * amp_r;
    let acc = |rising: bool| -> Option<f64> {
        let (mut sum, mut n) = (0.0, 0u32);
        for c in cr.iter().filter(|c| c.rising == rising) {
            if let Some(t) = edge_time(raw, c, low10, high90) {
                sum += t;
                n += 1;
            }
        }
        (n > 0).then(|| sum / n as f64 * dt)
    };
    if amp_r >= 8.0 {
        m.rise = acc(true);
        m.fall = acc(false);
    }
    Some(m)
}

/// Estimate the dominant frequency by hysteresis threshold crossings at the
/// waveform midpoint. Robust for periodic signals with at least two full
/// periods in the record; returns None otherwise.
pub fn estimate_frequency(raw: &[i8], sample_rate: f64) -> Option<f64> {
    if raw.len() < 8 || sample_rate <= 0.0 {
        return None;
    }
    let rs = raw_stats(raw)?;
    let span = rs.max - rs.min;
    if span < 8 {
        return None;
    }
    let mid = (rs.max + rs.min) as f64 / 2.0;
    let hyst = (span as f64 / 8.0).max(2.0);
    let rises: Vec<f64> = crossings(raw, mid, hyst)
        .into_iter()
        .filter(|c| c.rising)
        .map(|c| c.t)
        .collect();
    if rises.len() < 2 {
        return None;
    }
    let dt = rises.last().unwrap() - rises[0];
    (dt > 0.0).then(|| (rises.len() - 1) as f64 * sample_rate / dt)
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

    fn cap(raw: Vec<i8>, lsb: f64) -> ChannelCapture {
        ChannelCapture {
            ch: 0,
            raw,
            volts_per_lsb: lsb,
            zero_volts: 0.0,
            clipped: false,
            freq_meter: None,
        }
    }

    #[test]
    fn freq_of_square_wave() {
        let raw = square(5000, 250.0, 0, 125);
        let f = estimate_frequency(&raw, 250_000.0).unwrap();
        assert!((f - 1000.0).abs() < 5.0, "estimated {f}");
    }

    #[test]
    fn freq_of_sine_wave() {
        let raw: Vec<i8> = (0..5000)
            .map(|i| ((i as f64 / 100.0 * std::f64::consts::TAU).sin() * 100.0) as i8)
            .collect();
        let f = estimate_frequency(&raw, 1_000_000.0).unwrap();
        assert!((f - 10_000.0).abs() < 50.0, "estimated {f}");
    }

    #[test]
    fn stats_of_unipolar_square() {
        let c = cap(square(5000, 250.0, 0, 125), 10.0 / 250.0);
        let s = basic_stats(&c).unwrap();
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

    #[test]
    fn full_measurements_of_square() {
        // 1 kHz, 25% duty square at 250 kS/s: 250 samples/period, 62.5 high.
        let raw: Vec<i8> = (0..5000)
            .map(|i| {
                if (i as f64 / 250.0).fract() < 0.25 {
                    100
                } else {
                    -50
                }
            })
            .collect();
        let m = measure(&cap(raw, 0.01), 250e3).unwrap();
        assert!((m.vtop - 1.0).abs() < 0.02, "vtop {}", m.vtop);
        assert!((m.vbase + 0.5).abs() < 0.02, "vbase {}", m.vbase);
        assert!((m.vamp - 1.5).abs() < 0.04, "vamp {}", m.vamp);
        let f = m.freq.unwrap();
        assert!((f - 1000.0).abs() < 5.0, "freq {f}");
        let duty = m.pduty.unwrap();
        assert!((duty - 0.25).abs() < 0.02, "pduty {duty}");
        let pw = m.pwidth.unwrap();
        assert!((pw - 250e-6).abs() < 10e-6, "pwidth {pw}");
        // Instant edges at this rate: rise under 2 samples.
        assert!(m.rise.unwrap() < 2.0 / 250e3, "rise {:?}", m.rise);
        assert_eq!(m.overshoot, Some(0.0));
    }

    #[test]
    fn measurements_of_sine_use_extremes() {
        let raw: Vec<i8> = (0..5000)
            .map(|i| ((i as f64 / 500.0 * std::f64::consts::TAU).sin() * 100.0) as i8)
            .collect();
        let m = measure(&cap(raw, 0.01), 250e3).unwrap();
        // Sine has no flat top: histogram peaks at the extremes.
        assert!(m.vtop > 0.9, "vtop {}", m.vtop);
        assert!(m.vbase < -0.9, "vbase {}", m.vbase);
        let d = m.pduty.unwrap();
        assert!((d - 0.5).abs() < 0.03, "duty {d}");
    }

    #[test]
    fn envelope_measurements_keep_amplitudes_and_drop_timings() {
        // A 1 kHz square seen as min/max pairs: alternating -100/+100.
        let raw: Vec<i8> = (0..5000)
            .map(|i| if i % 2 == 0 { -100 } else { 100 })
            .collect();
        let cap = cap(raw, 0.01);
        let e = measure_envelope(&cap).unwrap();

        // Extrema survive min/max decimation exactly.
        assert!((e.vmax - 1.0).abs() < 1e-9, "vmax {}", e.vmax);
        assert!((e.vmin + 1.0).abs() < 1e-9, "vmin {}", e.vmin);
        assert!((e.vpp - 2.0).abs() < 1e-9, "vpp {}", e.vpp);

        // Nothing derived from sample spacing is reported.
        assert_eq!(e.freq, None);
        assert_eq!(e.period, None);
        assert_eq!(e.rise, None);
        assert_eq!(e.fall, None);
        assert_eq!(e.pwidth, None);
        assert_eq!(e.pduty, None);
        assert_eq!(e.vrms, None);

        // Measured as if it were a waveform, the same record claims a
        // frequency of half the sample rate — the nonsense this avoids.
        let bogus = measure(&cap, 250e3).unwrap();
        assert!(
            bogus.freq.unwrap() > 100e3,
            "expected the aliased reading, got {:?}",
            bogus.freq
        );
    }

    #[test]
    fn envelope_measurement_ignores_the_interleave_phase() {
        // The same envelope with the pairs offset by one sample: the
        // VDS1022 shifts this phase between records, so the result must not
        // depend on it (a naive even=min assumption reports a negative Vpp
        // on the shifted record).
        let a: Vec<i8> = (0..5000)
            .map(|i| if i % 2 == 0 { -100 } else { 100 })
            .collect();
        let b: Vec<i8> = (0..5000)
            .map(|i| if i % 2 == 0 { 100 } else { -100 })
            .collect();
        let ma = measure_envelope(&cap(a, 0.01)).unwrap();
        let mb = measure_envelope(&cap(b, 0.01)).unwrap();
        for m in [&ma, &mb] {
            assert!(m.vpp > 0.0, "vpp must be positive, got {}", m.vpp);
            assert!((m.vpp - 2.0).abs() < 1e-9, "vpp {}", m.vpp);
            assert!((m.vmax - 1.0).abs() < 1e-9, "vmax {}", m.vmax);
            assert!((m.vmin + 1.0).abs() < 1e-9, "vmin {}", m.vmin);
        }
        assert_eq!(ma.vtop, mb.vtop);
        assert_eq!(ma.vbase, mb.vbase);
    }
}
