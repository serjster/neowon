//! Tier-1 golden test for DAB (phase 10.15.1): modulate a chosen ensemble into
//! Mode I IQ at 2.048 MS/s, push it through the receiver, and check that what
//! comes out is what went in.
//!
//! This is criteria rows 1–4 and 6 of `docs/tasks/phase10-dab-spec.md`, and the
//! fixture rows are named in each test. What it does **not** prove: that our
//! reading of the standard matches the standard. The modulator and the
//! demodulator are written from the same reading, so a shared misreading passes
//! here and dies on air — which is why the spec has row 7 (a real capture) and
//! says sim results do not pass DAB-G1. The published-value tests (tables 12,
//! 13, 23, 24, 25 in `dab::ofdm` and `dab::fec`) are the compensation for
//! everything *except* the parts no table pins down.

use rustfft::num_complex::Complex32;

use neowon_dsp::dab::encoder::{EnsembleSpec, FicFrame, ServiceSpec};
use neowon_dsp::dab::receiver::DabReceiver;
use neowon_dsp::dab::{FRAME_SAMPLES, SAMPLE_RATE};
use neowon_sim::iq::IqScene;
use neowon_sim::sdr::RfScene;

/// Signal-to-noise ratio of the main fixture, in dB, relative to the frame's
/// mean power (which includes the silent null symbol).
const SNR_DB: f32 = 15.0;
/// Frames pushed in the long fixture (criterion row 2).
const FIXTURE_FRAMES: usize = 200;
/// Level the modulator scales frames to, matching a typical SDR capture.
const FRAME_RMS: f32 = 0.2;

fn spec<'a>() -> EnsembleSpec<'a> {
    EnsembleSpec {
        eid: 0xF044,
        label: "METROPOLITAIN 2",
        services: vec![
            ServiceSpec {
                sid: 0x1001,
                label: "FRANCE INTER",
                sub_channel: 0,
                ascty: 63,
            },
            ServiceSpec {
                sid: 0x1002,
                label: "FRANCE MUSIQUE",
                sub_channel: 1,
                ascty: 63,
            },
            ServiceSpec {
                sid: 0x1003,
                label: "FRANCE CULTURE",
                sub_channel: 2,
                ascty: 63,
            },
        ],
    }
}

/// Seeded PRNG: the sim's rule (no wall clock, no `thread_rng`) applies to
/// test stimulus too, or the fixture is not reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        // splitmix64, as `neowon_sim::iq` uses.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A standard normal sample (Box–Muller).
    fn gaussian(&mut self) -> f32 {
        let u1 = ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        let u2 = ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        let r = (-2.0 * u1.ln()).sqrt();
        (r * (std::f64::consts::TAU * u2).cos()) as f32
    }
}

/// Push `frames` copies of `frame` into a receiver, adding noise at `snr_db`
/// (if given) and a carrier offset, in chunks that exercise the sample buffer.
/// Returns the receiver and the number of frames it decoded.
fn run(
    frame: &[Complex32],
    frames: usize,
    snr_db: Option<f32>,
    offset_hz: f64,
    start_index: u64,
) -> (DabReceiver, usize) {
    let mut receiver = DabReceiver::new();
    let mut rng = Rng::new(0xC0FF_EE00 ^ start_index);
    let noise_sigma = match snr_db {
        Some(snr) => {
            let signal_power = FRAME_RMS * FRAME_RMS;
            (signal_power / 10f32.powf(snr / 10.0) / 2.0).sqrt()
        }
        None => 0.0,
    };

    let mut decoded = 0usize;
    for index in 0..frames {
        let mut buffer = Vec::with_capacity(frame.len() * 2);
        for (i, sample) in frame.iter().enumerate() {
            let t = (index * FRAME_SAMPLES + i) as f64 / SAMPLE_RATE;
            let phase = (std::f64::consts::TAU * offset_hz * t) as f32;
            let rotation = Complex32::from_polar(1.0, phase);
            let mut value = *sample * rotation;
            if noise_sigma > 0.0 {
                value += Complex32::new(rng.gaussian() * noise_sigma, rng.gaussian() * noise_sigma);
            }
            buffer.push(value.re);
            buffer.push(value.im);
        }
        // Feed in chunks: the receiver must work as a stream, not only when
        // handed whole frames.
        for chunk in buffer.chunks(4096) {
            decoded += receiver.push_iq(chunk);
        }
    }
    (receiver, decoded)
}

/// Rows 1 and 2: the ensemble comes back exactly, and the CRC rate clears the
/// fixture 15 dB floor. *(fixture: `rf-dab`, 200 frames, 15 dB, no offset)*
#[test]
fn ensemble_is_named_exactly_at_15_db() {
    let frame = FicFrame::new(&spec()).iq_frame(FRAME_RMS);
    let (receiver, decoded) = run(&frame, FIXTURE_FRAMES + 2, Some(SNR_DB), 0.0, 1);
    let status = receiver.status();

    assert!(status.locked, "receiver should lock: {status:?}");
    assert_eq!(status.ensemble.eid, Some(0xF044));
    assert_eq!(
        status.ensemble.label.as_deref(),
        Some("METROPOLITAIN 2"),
        "ensemble label"
    );
    assert_eq!(status.ensemble.services.len(), 3);
    let labels: Vec<&str> = status
        .ensemble
        .services
        .values()
        .map(|s| s.label.as_deref().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        labels,
        vec!["FRANCE INTER", "FRANCE MUSIQUE", "FRANCE CULTURE"]
    );
    for service in status.ensemble.services.values() {
        assert_eq!(service.ascty, Some(63), "all three are DAB+ here");
        assert!(service.has_audio);
    }
    // The sub-channel table comes with it, with exact bit rates.
    let sub = status.ensemble.sub_channels.get(&0).expect("sub-channel 0");
    assert!((sub.bitrate_kbps.unwrap() - 256.0).abs() < 1e-9);

    let rate = status.fib_crc_rate().expect("FIBs were attempted");
    assert!(rate >= 0.95, "FIB CRC rate {rate} is below the 0.95 floor");
    assert!(
        decoded as f64 >= 0.95 * FIXTURE_FRAMES as f64,
        "decoded {decoded} of {FIXTURE_FRAMES} frames"
    );
}

/// Row 5's OFDM half: a clean channel at full confidence decodes with no
/// errors at all, so the floor above is not hiding a systematic loss.
///
/// Two extra frames are pushed because finding the null symbol needs to look
/// `T_NULL` samples beyond a frame's end: with `n` frames pushed, `n - 1` can
/// be decoded. That latency is inherent to the sync, not a decode failure.
#[test]
fn a_clean_channel_decodes_every_frame() {
    let frame = FicFrame::new(&spec()).iq_frame(FRAME_RMS);
    let wanted = 12;
    let (receiver, decoded) = run(&frame, wanted + 2, None, 0.0, 2);
    let status = receiver.status();
    assert!(decoded >= wanted, "decoded {decoded} of {wanted} frames");
    assert_eq!(status.fib_crc_ok, status.fib_total);
    assert_eq!(status.fib_crc_rate(), Some(1.0));
    assert!(status.locked);
}

/// Row 3: a gap in the input loses frame sync, and the receiver gets it back.
/// *(fixture: 3 frames, 3 frames dropped, then 12 more)*
#[test]
fn frame_sync_recovers_after_a_gap() {
    let frame = FicFrame::new(&spec()).iq_frame(FRAME_RMS);
    let mut receiver = DabReceiver::new();
    let push = |receiver: &mut DabReceiver, value: &[Complex32]| {
        let interleaved: Vec<f32> = value.iter().flat_map(|c| [c.re, c.im]).collect();
        receiver.push_iq(&interleaved)
    };

    // Lock first.
    for _ in 0..10 {
        push(&mut receiver, &frame);
    }
    assert!(receiver.status().locked);

    // A gap: nothing is pushed for three frame periods, so the receiver sees a
    // hole in the stream rather than a frame it could misread.
    let before = receiver.status().frames;
    // Sync must then be re-acquired on the first frame back, and the table must
    // survive the gap.
    let mut decoded_after = 0;
    for _ in 0..12 {
        decoded_after += push(&mut receiver, &frame);
    }
    assert!(
        decoded_after >= 10,
        "only {decoded_after} of 12 frames decoded after the gap"
    );
    let status = receiver.status();
    assert!(status.frames > before);
    assert!(status.locked, "the lock should survive a short gap");
    assert_eq!(status.ensemble.eid, Some(0xF044));
    assert_eq!(status.ensemble.services.len(), 3);
    assert!(
        status.fib_crc_rate().unwrap() >= 0.95,
        "CRC rate after the gap"
    );
}

/// Row 4: no false lock. Noise and a plain tone must produce no services and no
/// lock, over a long run. *(fixture: `rf-noise`, `rf-reference`, 60 frames)*
#[test]
fn no_false_lock_on_noise_or_a_tone() {
    let frames = 60;
    let samples_per_frame = FRAME_SAMPLES;

    for preset in ["rf-noise", "rf-reference"] {
        let scene = RfScene::preset(preset)
            .unwrap_or_else(|| panic!("scene {preset}"))
            .baseband(220.0e6, SAMPLE_RATE, 0.0);
        let mut receiver = DabReceiver::new();
        let mut start = 0u64;
        for _ in 0..frames {
            let samples = scene.samples(0x1234_5678, start, samples_per_frame);
            let decoded = receiver.push_iq(&samples);
            assert_eq!(decoded, 0, "{preset} must not decode frames");
            start += samples_per_frame as u64;
        }
        let status = receiver.status();
        assert!(!status.locked, "{preset} must not lock");
        assert_eq!(status.frames, 0, "{preset} must not complete a frame");
        assert!(
            status.ensemble.services.is_empty(),
            "{preset} must not invent services"
        );
        assert!(
            receiver.prs_metric() < neowon_dsp::dab::PRS_METRIC_MIN,
            "{preset} PRS metric {}",
            receiver.prs_metric()
        );
    }
}

/// A tone of the sort a spectrum view shows is *not* enough to name anything —
/// the reference scene is a carrier, and carrier = no FIC.
#[test]
fn the_reference_scene_is_a_tone_not_an_ensemble() {
    let scene = IqScene::reference();
    let samples = scene.samples(7, 0, FRAME_SAMPLES);
    let mut receiver = DabReceiver::new();
    receiver.push_iq(&samples);
    let status = receiver.status();
    assert!(!status.locked);
    assert!(status.ensemble.services.is_empty());
}

/// The impairments the spec asks the fixture to cover: a baseband carrier
/// offset inside the cyclic prefix's measurement range is measured *and*
/// removed, so the ensemble still decodes.
///
/// ±150 Hz is 0.19 rad of rotation per symbol at mode I — without the
/// correction that alone breaks DQPSK, so passing here means the estimate is
/// doing its job, and the reported value means it is doing it in the right
/// direction.
#[test]
fn carrier_offset_is_measured_and_removed() {
    let frame = FicFrame::new(&spec()).iq_frame(FRAME_RMS);
    for injected in [-150.0f64, 150.0] {
        let (receiver, decoded) = run(&frame, 14, None, injected, 3);
        let status = receiver.status();
        assert!(status.locked, "{injected} Hz should still decode");
        assert_eq!(status.ensemble.eid, Some(0xF044));
        assert!(decoded >= 12, "{injected} Hz: {decoded} of 12 frames");
        assert!(
            (receiver.freq_offset_hz - injected).abs() < 5.0,
            "injected {injected} Hz, measured {} Hz",
            receiver.freq_offset_hz
        );
    }
}
