//! The RTL-SDR `Backend` driven through the `Supervisor`, as the app will
//! drive it. Hardware only:
//!
//! `cargo run -p neowon-sdr --example backend [-- <vhf_hz> <hf_hz>]`
//!
//! Checks: frames arrive complex and contiguous in time (timestamps advance
//! by exactly each frame's duration, so no chunk was dropped); a VHF
//! station and an HF station both show a peak, the latter through the
//! automatic switch to direct sampling; a +200 kHz retune moves the VHF
//! peak by -200 kHz (±25 kHz: an FM peak bin wanders with modulation).

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use neowon_backend::{Event, InstrumentConfig, SdrConfig, SdrGain, spawn};
use neowon_core::SampleLayout;
use neowon_dsp::{Window, iq_spectrum};
use neowon_sdr::RtlBackend;

/// Collect `secs` of frames, skipping the first `skip` (settling after a
/// config change). Returns interleaved IQ and whether timestamps were
/// contiguous.
fn collect(
    events: &crossbeam_channel::Receiver<Event>,
    skip: usize,
    secs: f64,
) -> (Vec<f32>, f64, bool) {
    let (mut iq, mut rate, mut contiguous) = (Vec::new(), 0.0, true);
    let mut next_t: Option<f64> = None;
    let mut seen = 0;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        let Ok(ev) = events.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        if let Event::Frame(f) = ev {
            assert_eq!(f.layout, SampleLayout::Complex);
            let t = f.t_capture.unwrap_or(f64::NAN);
            if let Some(n) = next_t
                && (t - n).abs() > 1e-6
            {
                contiguous = false;
            }
            next_t = Some(t + f.duration());
            seen += 1;
            if seen > skip {
                iq.extend_from_slice(&f.channels[0].data);
                rate = f.sample_rate;
            }
        }
    }
    (iq, rate, contiguous)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<f64>());
    let vhf = args.next().transpose()?.unwrap_or(99.4e6);
    let hf = args.next().transpose()?.unwrap_or(9.74e6);

    let sup = spawn(|| {
        RtlBackend::open(None)
            .map(|b| Box::new(b) as Box<dyn neowon_backend::Backend>)
            .map_err(|e| e.to_string())
    });
    let caps = loop {
        match sup.events.recv_timeout(Duration::from_secs(5))? {
            Event::Connected(c) => break c,
            Event::Disconnected(e) => bail!("connect: {e}"),
            _ => {}
        }
    };
    println!(
        "connected: {} {} ({:?})",
        caps.name(),
        caps.serial(),
        caps.sdr().map(|s| &s.tuner)
    );

    let mut cfg = SdrConfig {
        centre_hz: vhf,
        sample_rate: 2.048e6,
        gain: SdrGain::Manual(29.7),
        ..Default::default()
    };
    let report = |label: &str, cfg: &SdrConfig| -> Result<(f64, f64, bool)> {
        sup.apply(InstrumentConfig::Sdr(cfg.clone()));
        let (iq, rate, contiguous) = collect(&sup.events, 2, 1.5);
        let s = iq_spectrum(&iq, rate, Window::Hann, 4096)
            .ok_or_else(|| anyhow::anyhow!("no frames"))?;
        let (off, db) = s.peak(8).unwrap();
        let snr = db - s.median_db();
        println!(
            "{label:>10} {:>9.4} MHz: {} blocks, peak {:+8.1} kHz {snr:5.1} dB over floor, contiguous {contiguous}",
            cfg.centre_hz / 1e6,
            s.blocks,
            off / 1e3
        );
        Ok((off, snr, contiguous))
    };

    let (a, snr_a, c1) = report("VHF", &cfg)?;
    cfg.centre_hz += 200e3;
    let (b, _, c2) = report("retuned", &cfg)?;
    cfg.centre_hz = hf;
    let (_, snr_hf, c3) = report("HF", &cfg)?;
    cfg.centre_hz = vhf;
    let (_, snr_back, c4) = report("VHF again", &cfg)?;
    println!(
        "dropped by the supervisor: {}",
        sup.dropped.load(std::sync::atomic::Ordering::Relaxed)
    );

    // An FM station's peak bin wanders ±5 kHz with its modulation (seen:
    // -4.5 and +5.0 kHz on the same station), so allow ±25 kHz.
    let moved_ok = ((b - a) + 200e3).abs() < 25e3;
    let pass = snr_a > 10.0 && moved_ok && snr_hf > 10.0 && snr_back > 10.0 && c1 && c2 && c3 && c4;
    println!(
        "retune moved the peak {:+.1} kHz (expect -200); pass {pass}",
        (b - a) / 1e3
    );
    if !pass {
        bail!("backend check FAILED");
    }
    Ok(())
}
