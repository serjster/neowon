//! Phase 10.1 golden fixtures for detection and tracking (spec table:
//! seed 7, N 8192). The spec leaves the sample rate implicit; it is
//! 8192 Hz here, so N is one second, a frame (`nfft 256 × 4 blocks`) is
//! 125 ms and one bin is 32 Hz. SNR is signal power over total noise power.
//! Each row prints a JSON readout.
//!
//! `cargo test -p neowon-dsp --test detect_golden -- --nocapture`

use neowon_core::SignalObservation;
use neowon_dsp::iq::iq_spectrum;
use neowon_dsp::modmeas::occupied_band;
use neowon_dsp::{DetectConfig, Tracker, TrackerConfig, Window, detect};
use neowon_sim::{IqComponent, IqScene};

const RATE: f64 = 8192.0;
const N: usize = 8192;
const SEED: u64 = 7;
const BIN: f64 = RATE / 256.0;
const AMP: f64 = 0.5;

fn noise_for(snr_db: Option<f64>) -> f64 {
    snr_db.map_or(0.0, |s| (AMP * AMP / 10f64.powf(s / 10.0)).sqrt())
}

fn cfg() -> DetectConfig {
    DetectConfig::default()
}

/// Two frames to become active; forgotten after two quiet frames.
fn tracker() -> Tracker {
    Tracker::new(TrackerConfig {
        min_duration_s: 0.25,
        hold_s: 0.25,
        assoc_hz: 2.0 * BIN,
    })
}

/// Detect, then feed the tracker frame by frame. Also returns the most
/// tracks that were active after any frame.
fn run(
    components: Vec<IqComponent>,
    snr_db: Option<f64>,
) -> (Vec<SignalObservation>, Tracker, usize) {
    let scene = IqScene {
        sample_rate: RATE,
        components,
        noise_rms: noise_for(snr_db),
    };
    let obs = detect(&scene.samples(SEED, 0, N), RATE, 0.0, 0.0, &cfg());
    let mut tr = tracker();
    let frame = cfg().frame_len() as f64 / RATE;
    let mut most_active = 0;
    for f in 0..N / cfg().frame_len() {
        let t = f as f64 * frame;
        // An observation belongs to the frame its first block started in.
        let now: Vec<_> = obs
            .iter()
            .filter(|o| o.t_start >= t && o.t_start < t + frame)
            .cloned()
            .collect();
        tr.update(&now, t + frame);
        most_active = most_active.max(tr.active().count());
    }
    (obs, tr, most_active)
}

fn readout(row: &str, obs: &[SignalObservation], tr: &Tracker) {
    let active: Vec<String> = tr
        .active()
        .map(|t| {
            format!(
                "{{\"id\":{},\"lo_hz\":{:.1},\"hi_hz\":{:.1}}}",
                t.id, t.lo_hz, t.hi_hz
            )
        })
        .collect();
    println!(
        r#"{{"row":"{row}","observations":{},"tracks":{},"active":[{}]}}"#,
        obs.len(),
        tr.tracks().len(),
        active.join(",")
    );
}

fn tone(offset_hz: f64) -> IqComponent {
    IqComponent::Tone {
        offset_hz,
        amplitude: AMP,
        phase: 0.0,
    }
}

#[test]
fn tone_1khz_clean_is_one_peak_at_its_frequency() {
    let (obs, tr, _) = run(vec![tone(1000.0)], None);
    readout("tone", &obs, &tr);
    // One observation per frame, all within half a bin of 1 kHz.
    assert_eq!(obs.len(), N / cfg().frame_len(), "{obs:?}");
    for o in &obs {
        assert!((o.centre_hz - 1000.0).abs() <= BIN / 2.0, "{o:?}");
        assert!((o.power_dbfs - 20.0 * AMP.log10()).abs() < 0.1, "{o:?}");
    }
    assert_eq!(tr.tracks().len(), 1);
}

#[test]
fn burst_10ms_at_half_time_is_present_with_its_own_bandwidth() {
    // "10 ms burst at 0.5": centred on the capture's midpoint, with
    // raised-cosine edges across its whole length (a Hann envelope): a
    // rectangular gate's occupied bandwidth would be set by the noise.
    let burst = IqComponent::Burst {
        offset_hz: 1000.0,
        amplitude: AMP,
        start_s: 0.495,
        duration_s: 0.010,
        rise_s: 0.005,
    };
    let (obs, tr, _) = run(vec![burst], Some(30.0));
    readout("burst", &obs, &tr);
    // The truth is the burst's own 99% bandwidth, from its exact energy
    // spectrum (the isolated, noise-free burst over the whole second,
    // 1 Hz bins) — not from the detector.
    let alone = IqScene {
        sample_rate: RATE,
        components: vec![burst],
        noise_rms: 0.0,
    };
    let exact = iq_spectrum(&alone.samples(SEED, 0, N), RATE, Window::Rectangle, N).unwrap();
    let lin: Vec<f64> = exact
        .power_db
        .iter()
        .map(|d| 10f64.powf(d / 10.0))
        .collect();
    let (lo, hi) = occupied_band(&lin, 0.99);
    let truth = (hi - lo) * exact.bin_hz;
    // The observation carrying the burst is the strongest one; others are
    // its edge leaking into the next frame, tens of dB down.
    let o = obs
        .iter()
        .max_by(|a, b| a.power_dbfs.total_cmp(&b.power_dbfs))
        .expect("burst not detected");
    println!(
        r#"{{"row":"burst","truth_obw99_hz":{truth:.1},"measured_hz":{:.1}}}"#,
        o.bandwidth_hz()
    );
    assert!(
        (o.bandwidth_hz() - truth).abs() <= BIN,
        "{} vs {truth}",
        o.bandwidth_hz()
    );
    assert!(o.lo_hz <= 1000.0 && o.hi_hz >= 1000.0, "{o:?}");
}

#[test]
fn chirp_1_to_2_khz_spans_its_sweep() {
    let chirp = IqComponent::Chirp {
        from_hz: 1000.0,
        to_hz: 2000.0,
        amplitude: AMP,
        start_s: 0.0,
        duration_s: 1.0,
    };
    let (obs, tr, _) = run(vec![chirp], Some(30.0));
    readout("chirp", &obs, &tr);
    assert_eq!(tr.tracks().len(), 1, "{:?}", tr.tracks());
    let t = &tr.tracks()[0];
    assert!((t.bandwidth_hz() - 1000.0).abs() <= 2.0 * BIN, "{t:?}");
    assert!(
        (t.lo_hz - 1000.0).abs() <= 2.0 * BIN && (t.hi_hz - 2000.0).abs() <= 2.0 * BIN,
        "{t:?}"
    );
}

#[test]
fn noise_only_has_no_peaks() {
    let (obs, tr, _) = run(Vec::new(), Some(0.0));
    readout("noise", &obs, &tr);
    assert!(obs.is_empty(), "{obs:?}");
}

#[test]
fn transient_shorter_than_min_duration_never_becomes_active() {
    // Half of min_duration (one frame), aligned to frame 2.
    let transient = IqComponent::Burst {
        offset_hz: -1500.0,
        amplitude: AMP,
        start_s: 0.25,
        duration_s: 0.125,
        rise_s: 0.0,
    };
    let (obs, tr, most_active) = run(vec![transient], Some(30.0));
    readout("transient", &obs, &tr);
    assert!(!obs.is_empty(), "the transient is observed");
    // Never active at any point, not merely forgotten by the end.
    assert_eq!(most_active, 0, "{obs:?}");
    // Its measured span is its true 125 ms to within a block either side.
    let (first, last) = obs.iter().fold((f64::MAX, 0.0f64), |(a, b), o| {
        (a.min(o.t_start), b.max(o.t_end))
    });
    let block = 256.0 / RATE;
    assert!(
        (first - 0.25).abs() <= block && (last - 0.375).abs() <= block,
        "{first}..{last}"
    );
}
