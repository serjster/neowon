//! P0.1 driver check (Phase 10 D2 / SDR-G1) on the in-tree RTL-SDR driver.
//! Hardware only — run it by hand with the dongle attached:
//!
//! `cargo run -p neowon-sdr --example p01 [-- --json <path>]`
//!
//! The checks are self-referential so they hold anywhere: survey the FM
//! broadcast band for its strongest carrier, then
//! - stream: delivered pairs/s within 1% of the set rate at 2.048 and
//!   2.4 MS/s, nothing dropped;
//! - tune: a live retune by +300 kHz moves that carrier by −300 kHz;
//! - gain: a 0 / 14.4 / 29.7 / 49.6 dB sweep raises total power at every
//!   step and by more than 10 dB end to end. The size of each step is
//!   scene-dependent (at low gain the dongle's own noise dominates):
//!   `librtlsdr-rs` measures the same +14.5 dB for 0 → 29.7 dB on the same
//!   station, so an absolute threshold would test the band, not the driver;
//! - ppm: +100 vs −100 ppm moves the band by 2·(f + IF)·100e-6 (±10%) —
//!   the correction acts on the LO, which sits at centre + IF;
//! - RTL AGC: at 0 dB tuner gain, raises power by more than 5 dB;
//! - HF direct sampling (Q branch): some HF spot shows a peak > 10 dB;
//! - leaving HF: the device is left untuned (not failed) and retunes.
//!
//! Offsets are whole-spectrum shifts (cross-correlation of averaged dB
//! spectra, sub-bin interpolated): everything in band moves with the LO, so
//! this does not depend on one station's modulation or on neighbours
//! drifting in and out of a window around it. Rates are timed from chunk
//! arrivals, not from the capture window, which is only chunk-accurate.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use neowon_sdr::rtl::{DirectSampling, RtlSdr, Stream, list};

const NFFT: usize = 4096;
const SHIFT: u32 = 300_000;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let json_out = std::env::args().skip_while(|a| a != "--json").nth(1);

    for d in list()? {
        println!(
            "{:04x}:{:04x} {} / {} serial={:?}",
            d.vendor_id,
            d.product_id,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
            d.serial
        );
    }
    let t_open = Instant::now();
    let mut sdr = RtlSdr::open(None)?;
    let open_ms = t_open.elapsed().as_secs_f64() * 1e3;
    println!("open {open_ms:.0} ms: tuner {:?}", sdr.tuner());
    let mut rate = sdr.set_sample_rate(2_048_000)?;
    sdr.set_gain(Some(297))?;
    sdr.set_center_freq(89_000_000)?;
    let stream = sdr.stream()?;

    // Survey 88–108 MHz for the strongest carrier.
    let mut best = (0.0f64, f64::MIN);
    for step in 0..10u32 {
        let centre = 89_000_000 + step * 2_000_000;
        sdr.set_center_freq(centre)?;
        let (off, snr) = peak(&spectrum(&take(&stream, 0.3)?.iq), rate);
        if snr > best.1 {
            best = (centre as f64 + off, snr);
        }
    }
    let carrier = best.0;
    println!(
        "strongest carrier {:.4} MHz ({:.1} dB)",
        carrier / 1e6,
        best.1
    );

    // Stream throughput.
    let mut pairs_per_s = Vec::new();
    for want in [2_400_000u32, 2_048_000] {
        rate = sdr.set_sample_rate(want)?;
        let _ = take(&stream, 0.3)?;
        let r = take(&stream, 3.0)?.pairs_per_s;
        println!(
            "stream {rate}: {r:.0} pairs/s ({:+.3}%)",
            (r / rate as f64 - 1.0) * 100.0
        );
        pairs_per_s.push((rate, r));
    }

    // Tune.
    let c0 = carrier as u32 - SHIFT;
    sdr.set_center_freq(c0)?;
    let before = spectrum(&take(&stream, 1.0)?.iq);
    sdr.set_center_freq(c0 + SHIFT)?;
    let after = spectrum(&take(&stream, 1.0)?.iq);
    let moved = shift_hz(&before, &after, rate, 700);
    println!("tune: band moved {:+.2} kHz", moved / 1e3);

    // ppm, on the carrier.
    let mut at = Vec::new();
    for ppm in [100, -100, 0] {
        sdr.set_ppm(ppm)?;
        at.push(spectrum(&take(&stream, 1.0)?.iq));
    }
    // +100 ppm: the tuner takes its crystal as fast, programs the LO low,
    // and the band appears higher. Signed on purpose.
    let ppm_moved = shift_hz(&at[1], &at[0], rate, 100);
    let ppm_expect = 2.0 * (carrier + sdr.tuner_if_hz() as f64) * 100e-6;
    println!(
        "ppm: -100 -> +100 moved the band {:+.2} kHz (expect +{:.2}); 0 -> +100 {:+.2} kHz",
        ppm_moved / 1e3,
        ppm_expect / 1e3,
        shift_hz(&at[2], &at[0], rate, 100) / 1e3
    );

    // Gain and RTL AGC.
    let mut sweep = Vec::new();
    let mut clip = 0.0;
    for g in [0, 144, 297, 496] {
        sdr.set_gain(Some(g))?;
        let c = take(&stream, 0.5)?;
        clip = c.clip;
        sweep.push(power_dbfs(&c.iq));
    }
    let (g0, g30) = (sweep[0], sweep[2]);
    sdr.set_gain(Some(0))?;
    sdr.set_rtl_agc(true)?;
    let g0agc = power_dbfs(&take(&stream, 0.8)?.iq);
    sdr.set_rtl_agc(false)?;
    let g0after = power_dbfs(&take(&stream, 0.8)?.iq);
    sdr.set_gain(Some(297))?;
    println!("RTL AGC at 0 dB: on {g0agc:.1} dBFS, off again {g0after:.1} dBFS");
    println!(
        "gain sweep 0/14.4/29.7/49.6 dB: {:.1?} dBFS; clip at 49.6 dB {:.4}%",
        sweep,
        clip * 100.0
    );

    // HF direct sampling on the V3's Q input.
    sdr.set_direct_sampling(DirectSampling::Q)?;
    let mut hf_best = (0.0, f64::MIN);
    for f in [6_000_000u32, 7_100_000, 9_600_000, 11_800_000, 15_300_000] {
        sdr.set_center_freq(f)?;
        let c = take(&stream, 0.7)?;
        let (off, snr) = peak(&spectrum(&c.iq), rate);
        println!(
            "HF {:.1} MHz: {:.1} dBFS, peak {:+.1} kHz at {snr:.1} dB",
            f as f64 / 1e6,
            power_dbfs(&c.iq),
            off / 1e3
        );
        if snr > hf_best.1 {
            hf_best = (f as f64 + off, snr);
        }
    }
    // Leave HF while tuned at 15.3 MHz: must succeed and leave it untuned.
    sdr.set_direct_sampling(DirectSampling::Off)?;
    let untuned = sdr.center_freq() == 0;
    sdr.set_center_freq(carrier as u32)?;
    let (_, back_snr) = peak(&spectrum(&take(&stream, 1.0)?.iq), rate);
    println!("HF exit: untuned {untuned}; back on the carrier at {back_snr:.1} dB");

    let overflows = stream.overflows();
    drop(stream);

    let stream_ok = overflows == 0
        && pairs_per_s
            .iter()
            .all(|&(set, got)| (got / set as f64 - 1.0).abs() < 0.01);
    let tune_ok = (moved + SHIFT as f64).abs() < 10_000.0;
    let gain_ok = sweep.windows(2).all(|w| w[1] > w[0] + 1.0) && sweep[3] - sweep[0] > 10.0;
    let ppm_ok = (ppm_moved / ppm_expect - 1.0).abs() < 0.1;
    let agc_ok = g0agc - g0 > 5.0;
    let hf_ok = hf_best.1 > 10.0;
    let hf_exit_ok = untuned && back_snr > 10.0;
    let pass = stream_ok && tune_ok && gain_ok && ppm_ok && agc_ok && hf_ok && hf_exit_ok;
    let json = format!(
        concat!(
            r#"{{"driver":"neowon-sdr::rtl","serial":{:?},"tuner":"{:?}","open_ms":{:.0},"#,
            r#""pairs_per_s":{:?},"overflows":{},"carrier_hz":{:.0},"tune_moved_hz":{:.0},"#,
            r#""gain_sweep_dbfs":{:.1?},"gain_0_to_297_db":{:.1},"ppm_moved_hz":{:.0},"ppm_expect_hz":{:.0},"#,
            r#""agc_delta_db":{:.1},"agc_off_db":{:.1},"hf_peak_hz":{:.0},"hf_peak_db":{:.1},"hf_exit_untuned":{},"#,
            r#""stream_ok":{},"tune_ok":{},"gain_ok":{},"ppm_ok":{},"agc_ok":{},"hf_ok":{},"#,
            r#""hf_exit_ok":{},"pass":{}}}"#
        ),
        sdr.info().serial.clone().unwrap_or_default(),
        sdr.tuner(),
        open_ms,
        pairs_per_s
            .iter()
            .map(|&(s, g)| [s as u64, g.round() as u64])
            .collect::<Vec<_>>(),
        overflows,
        carrier,
        moved,
        sweep,
        g30 - g0,
        ppm_moved,
        ppm_expect,
        g0agc - g0,
        g0after - g0,
        hf_best.0,
        hf_best.1,
        untuned,
        stream_ok,
        tune_ok,
        gain_ok,
        ppm_ok,
        agc_ok,
        hf_ok,
        hf_exit_ok,
        pass
    );
    println!("{json}");
    if let Some(path) = json_out {
        std::fs::write(&path, format!("{json}\n"))?;
    }
    if !pass {
        bail!("P0.1 FAILED");
    }
    Ok(())
}

struct Capture {
    /// Interleaved I, Q in full-scale units.
    iq: Vec<f32>,
    /// Pairs delivered after the first chunk, per second between the first
    /// and last chunk arrivals.
    pairs_per_s: f64,
    clip: f64,
}

/// Discard what is queued and the chunk in flight when the caller changed
/// a setting (it straddles the change: measured latency is one chunk, and
/// at 29.7 dB its old samples outweigh a 0 dB window ~100:1), then collect
/// `secs` of stream.
fn take(stream: &Stream, secs: f64) -> Result<Capture> {
    while stream.recv_timeout(Duration::ZERO)?.is_some() {}
    let _ = stream.recv_timeout(Duration::from_millis(500))?;
    let start = Instant::now();
    let mut raw = Vec::new();
    let (mut first, mut last, mut after_first) = (None, start, 0usize);
    while start.elapsed().as_secs_f64() < secs {
        if let Some(b) = stream.recv_timeout(Duration::from_millis(20))? {
            let now = Instant::now();
            if first.is_some() {
                after_first += b.len();
            } else {
                first = Some(now);
            }
            last = now;
            raw.extend_from_slice(&b);
        }
    }
    let span = first.map_or(0.0, |f| (last - f).as_secs_f64());
    let clip = raw.iter().filter(|&&b| b == 0 || b == 255).count() as f64 / raw.len().max(1) as f64;
    Ok(Capture {
        iq: raw.iter().map(|&b| (b as f32 - 127.5) / 127.5).collect(),
        pairs_per_s: after_first as f64 / 2.0 / span.max(1e-9),
        clip,
    })
}

fn power_dbfs(iq: &[f32]) -> f64 {
    let p = iq.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / (iq.len() / 2).max(1) as f64;
    10.0 * p.max(1e-20).log10()
}

fn bin_hz(k: usize, rate: u32) -> f64 {
    (k as f64 - NFFT as f64 / 2.0) * rate as f64 / NFFT as f64
}

/// Hann-windowed averaged power spectrum in dB, DC at bin NFFT/2.
fn spectrum(iq: &[f32]) -> Vec<f64> {
    let win: Vec<f64> = (0..NFFT)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / NFFT as f64).cos())
        .collect();
    let mut acc = vec![0.0; NFFT];
    let mut blocks = 0;
    for chunk in iq.as_chunks::<{ 2 * NFFT }>().0 {
        let mut re: Vec<f64> = (0..NFFT).map(|i| chunk[2 * i] as f64 * win[i]).collect();
        let mut im: Vec<f64> = (0..NFFT)
            .map(|i| chunk[2 * i + 1] as f64 * win[i])
            .collect();
        fft(&mut re, &mut im);
        for k in 0..NFFT {
            acc[(k + NFFT / 2) % NFFT] += re[k] * re[k] + im[k] * im[k];
        }
        blocks += 1;
    }
    acc.iter()
        .map(|p| 10.0 * (p / blocks.max(1) as f64).max(1e-30).log10())
        .collect()
}

/// Strongest bin away from DC and the band edges: (Hz, dB over median).
fn peak(spec: &[f64], rate: u32) -> (f64, f64) {
    let mut sorted = spec.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[NFFT / 2];
    let (k, p) = spec
        .iter()
        .enumerate()
        .filter(|(k, _)| (*k as i64 - NFFT as i64 / 2).abs() > 8)
        .filter(|(k, _)| *k > NFFT / 10 && *k < NFFT * 9 / 10)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, &p)| (k, p))
        .unwrap_or((NFFT / 2, median));
    (bin_hz(k, rate), p - median)
}

/// How far the band moved from `a` to `b`, Hz: the lag (within ±`max_bins`)
/// maximising the correlation of the mean-removed dB spectra over the
/// middle 80% of the band, refined by a parabola through the peak.
fn shift_hz(a: &[f64], b: &[f64], rate: u32, max_bins: i64) -> f64 {
    let centre = |s: &[f64]| {
        let m = s.iter().sum::<f64>() / s.len() as f64;
        s.iter().map(|v| v - m).collect::<Vec<_>>()
    };
    let (a, b) = (centre(a), centre(b));
    let (lo, hi) = (NFFT as i64 / 10 + max_bins, NFFT as i64 * 9 / 10 - max_bins);
    let corr = |lag: i64| -> f64 {
        (lo..hi)
            .map(|k| a[k as usize] * b[(k + lag) as usize])
            .sum()
    };
    let best = (-max_bins..=max_bins)
        .max_by(|&x, &y| corr(x).total_cmp(&corr(y)))
        .expect("lags");
    let (cm, c0, cp) = (corr(best - 1), corr(best), corr(best + 1));
    let frac = 0.5 * (cm - cp) / (cm - 2.0 * c0 + cp);
    (best as f64 + frac) * rate as f64 / NFFT as f64
}

/// In-place iterative radix-2 FFT.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}
