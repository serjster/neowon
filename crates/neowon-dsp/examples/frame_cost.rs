//! What one arriving SDR frame costs, priced against its own period.
//!
//! The SDR frame loop is claimed to keep up with the dongle; without a number
//! behind that, a ten-fold regression in any stage would be invisible. So this
//! runs the engine-free stages the app runs per frame, on
//! a deterministic sim scene, and prints ms/frame beside the frame's own
//! period and a 32 ms reference.
//!
//! Headless and sim-only — no window, no device, no USB. Wall-clock timing is
//! the measurement, so nothing here is asserted in a test: the numbers vary by
//! machine and build profile, which is exactly why they are printed with the
//! profile that produced them.
//!
//! ```text
//! cargo run --release -p neowon-dsp --example frame_cost
//! ```
//!
//! Not priced here: the app's display stages — `mask_dc`, `columns`, the
//! waterfall RGBA map and the texture upload. They live in `neowon-app`,
//! which is a binary crate with no library target, so an example cannot
//! reach them; a windowed probe is the rig for those (and for Bevy's own
//! frame time).

use std::time::{Duration, Instant};

use neowon_core::Modulation;
use neowon_dsp::{
    DemodMode, DetectConfig, Receiver, ReceiverConfig, Tracker, TrackerConfig, Window, detect,
    iq_spectrum,
};
use neowon_sim::{IqComponent, IqScene};

/// The app's display cadence: one frame every 32 ms is the budget every
/// stage below is measured against.
const PERIOD_MS: f64 = 32.0;
/// Repeats per stage. Enough to average out scheduler noise, few enough that
/// the whole example is a couple of seconds.
const REPS: u32 = 20;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Run `f` `REPS` times and return the mean and the worst, in ms.
fn time(mut f: impl FnMut()) -> (f64, f64) {
    // One untimed call so lazily-built tables and allocations are warm,
    // as they are in a running app after the first frame.
    f();
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..REPS {
        let t = Instant::now();
        f();
        let e = t.elapsed();
        total += e;
        worst = worst.max(e);
    }
    (ms(total) / REPS as f64, ms(worst))
}

fn row(stage: &str, mean: f64, worst: f64, period_ms: f64) {
    println!(
        "{stage:<28} {mean:>7.3} {worst:>8.3}   {:>6.2}%  {:>6.2}%",
        100.0 * mean / period_ms,
        100.0 * mean / PERIOD_MS,
    );
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "dev (opt-level as configured; run --release for the honest number)"
    } else {
        "release"
    };
    // The app's own working point: 2.048 MS/s in ~50 ms transfers, which is
    // what `stream_chunk_pairs` asks the dongle for at that rate.
    let rate = 2.048e6;
    let pairs = neowon_core::stream_chunk_pairs(rate);
    let frame_ms = 1e3 * pairs as f64 / rate;
    let scene = IqScene {
        sample_rate: rate,
        components: vec![
            IqComponent::Tone {
                offset_hz: 100e3,
                amplitude: 0.5,
                phase: 0.0,
            },
            IqComponent::Digital {
                modulation: Modulation::Qpsk,
                symbol_rate: 102.4e3,
                offset_hz: -400e3,
                amplitude: 0.4,
                rolloff: 0.35,
            },
        ],
        noise_rms: 0.05,
    };
    let seed = 1;
    let iq = scene.samples(seed, 0, pairs);
    let frame = scene.frame(seed, 0, 0, pairs);
    let fft = 4096;

    println!("neowon frame_cost — one SDR frame's DSP, {profile} build");
    println!(
        "{pairs} pairs at {:.3} MS/s = {frame_ms:.2} ms of signal per frame; \
         reference period {PERIOD_MS:.0} ms; {REPS} reps",
        rate / 1e6
    );
    println!();
    println!(
        "{:<28} {:>7} {:>8}   {:>6}  {:>6}",
        "stage", "mean ms", "worst ms", "/frame", "/32ms"
    );

    // 1. The spectrum every display and measurement is derived from.
    let (mean_spec, worst) = time(|| {
        std::hint::black_box(iq_spectrum(&iq, rate, Window::Hann, fft));
    });
    row("iq_spectrum (4096, Hann)", mean_spec, worst, frame_ms);

    // 2. Detection + tracking, as `sdr detect on` runs it.
    let cfg = DetectConfig {
        nfft: fft,
        blocks: (pairs / fft).max(1),
        // The simulator has no DC spike to guard against.
        dc_guard: 0,
        ..Default::default()
    };
    let mut tracker = Tracker::new(TrackerConfig {
        min_duration_s: 0.25,
        hold_s: 1.0,
        assoc_hz: 2.0 * rate / fft as f64,
    });
    let mut t0 = 0.0;
    let (mean_det, worst) = time(|| {
        let obs = detect(&iq, rate, 100e6, t0, &cfg);
        tracker.update(&obs, t0 + pairs as f64 / rate);
        t0 += pairs as f64 / rate;
    });
    row("detect + tracker.update", mean_det, worst, frame_ms);

    // 3. The survey's per-frame fold (`sdr survey`), over the same frame.
    let plan = neowon_dsp::SurveyPlan {
        start_hz: 88e6,
        stop_hz: 108e6,
        sample_rate: rate,
        ..Default::default()
    };
    let mut survey = neowon_dsp::Survey::new(plan);
    let (mean_sur, worst) = time(|| {
        std::hint::black_box(survey.feed(&frame));
    });
    row("survey::feed", mean_sur, worst, frame_ms);

    // 4. The audio demodulator (`sdr demod nfm`), streaming.
    let mut rx = Receiver::new(ReceiverConfig::new(DemodMode::Nfm, rate, 48e3));
    let mut audio = Vec::new();
    let (mean_demod, worst) = time(|| {
        audio.clear();
        rx.process(&iq, &mut audio);
    });
    row("demod Receiver::process", mean_demod, worst, frame_ms);

    // 5. The DAB consumer, which sees every frame when `sdr dab on`.
    let mut dab = neowon_dsp::dab::DabReceiver::new();
    let (mean_dab, worst) = time(|| {
        std::hint::black_box(dab.push_iq(&iq));
    });
    row("dab push_iq (+ decode)", mean_dab, worst, frame_ms);

    // 6. A fresh `FftPlanner` per `iq_spectrum` call, priced separately so
    //    caching the plan is judged against a number rather than an intuition.
    let (mean_plan, worst) = time(|| {
        std::hint::black_box(rustfft::FftPlanner::<f32>::new().plan_fft_forward(fft));
    });
    row("  of which: FftPlanner", mean_plan, worst, frame_ms);

    // The two realistic loads: a plain SDR display, and everything on.
    let display = mean_spec + mean_det + mean_sur;
    let all = display + mean_demod + mean_dab;
    println!();
    println!(
        "{:<28} {display:>7.3} {:>8}   {:>6.2}%  {:>6.2}%",
        "= display path",
        "-",
        100.0 * display / frame_ms,
        100.0 * display / PERIOD_MS
    );
    println!(
        "{:<28} {all:>7.3} {:>8}   {:>6.2}%  {:>6.2}%",
        "= every consumer on",
        "-",
        100.0 * all / frame_ms,
        100.0 * all / PERIOD_MS
    );
    println!();
    println!(
        "FftPlanner is {:.1}% of iq_spectrum and {:.2}% of the frame period: \
         caching the plan is not where the per-frame cost is.",
        100.0 * mean_plan / mean_spec,
        100.0 * mean_plan / frame_ms,
    );
    println!(
        "Verdict: {} the {frame_ms:.2} ms frame period ({:.1}x margin), \
         {} the {PERIOD_MS:.0} ms reference ({:.1}x).",
        if all < frame_ms { "inside" } else { "OVER" },
        frame_ms / all,
        if all < PERIOD_MS { "inside" } else { "OVER" },
        PERIOD_MS / all,
    );
}
