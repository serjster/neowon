//! What the IQ fixture proves, what it does not, and the two checks that
//! close the gap for the path every golden in this directory runs on.
//!
//! # The limits of the existing fixture
//!
//! `crates/neowon-sim/tests/iq_determinism.rs` pins
//! `IqScene::reference()` — one 0.5 FS tone in 0.05 FS noise — seed 1,
//! 1024 pairs, byte for byte. That is a **determinism and portability**
//! check and nothing more. Specifically it does not prove:
//!
//! 1. **Correctness.** A generator that emitted the wrong frequency, the
//!    wrong amplitude or the wrong noise power would pass it, as long as it
//!    did so identically on every run and every platform. The fixture's
//!    authority is "these bytes are what this code produced", not "these
//!    bytes are right".
//! 2. **Anything about the modulated path.** The reference scene has one
//!    `Tone` component. `Digital` — symbol draw, RRC pulse shaping, the
//!    table/direct split in `IqComponent::digital`, the carrier rotation —
//!    is not exercised by a single byte of it, and that path is what
//!    `classify_golden`, `mod_estimators` and `demod_golden` measure
//!    against. A platform that rounded the RRC table differently would
//!    move every golden number here while `iq_determinism` stayed green.
//! 3. **Statistical claims.** 1024 pairs is 0.5 ms at 2.048 MS/s. Noise
//!    power, spectral flatness, symbol balance: none of them are measured
//!    over a window that short, and the sim's Irwin–Hall noise has no tails
//!    past ±6σ by construction, so a golden that depends on rare outliers
//!    is measuring the generator, not the estimator.
//!
//! The two tests below close 1 and 2 for the modulated path: one
//! closed-form property (the matched filter returns the transmitted
//! symbols, a fact about the pulse shaping that no recording can fake) and
//! one cross-platform byte pin over the `Digital` path. 3 is a stated
//! limit, not a defect — a golden that needs a distribution states its own
//! N, as `classify_golden` does.
//!
//! These live here rather than beside the tone fixture in `neowon-sim`
//! because `neowon-dsp`'s goldens are what the `Digital` path's bytes are
//! load-bearing for.
//!
//! `cargo test -p neowon-dsp --test iq_fixture -- --nocapture`

mod common;

use neowon_core::Modulation;
use neowon_sim::iq::{RRC_SPAN, fnv1a64, rrc, symbol_bits, to_le_bytes};
use neowon_sim::{IqComponent, IqScene};

const RATE: f64 = 1e6;
const RS: f64 = 100e3;
/// Samples per symbol — integer, so the generator takes its table path.
const SPS: i64 = (RATE / RS) as i64;
const BETA: f64 = 0.35;
const AMP: f64 = 0.5;
const SEED: u64 = 1;
const PAIRS: usize = 1024;

/// The scene this file pins: QPSK, 100 ksym/s, roll-off 0.35, at 1 MS/s,
/// on the carrier (no offset) and noise-free. Deliberately **not**
/// `IqScene::reference()`: a tone says nothing about pulse shaping. Change
/// it and the digest below changes, so treat it like a stimulus preset.
fn digital_reference() -> IqScene {
    IqScene {
        sample_rate: RATE,
        components: vec![IqComponent::Digital {
            modulation: Modulation::Qpsk,
            symbol_rate: RS,
            offset_hz: 0.0,
            amplitude: AMP,
            rolloff: BETA,
        }],
        noise_rms: 0.0,
    }
}

/// The `Digital` path's cross-platform byte pin.
///
/// The bytes are pinned by their FNV-1a 64 digest rather than a committed
/// blob — the digest covers every one of the 8 192 bytes, and a text
/// literal diffs and reviews where a binary file does not. The first pair
/// is pinned as exact `f32` bits beside it so a failure says *how* the
/// bytes moved, not only that they did.
///
/// Regenerate: run the command in this file's header and copy the printed
/// `digest` / `first_pair_bits`. A legitimate change to
/// [`digital_reference`] or to the generator is regenerated that way and
/// reviewed as a diff of these two literals.
#[test]
fn the_digital_path_is_byte_identical_across_platforms() {
    // Measured on aarch64-apple-darwin by the header command.
    const DIGEST: u64 = 0x2969_C92E_A83F_481B;
    const FIRST_PAIR_BITS: [u32; 2] = [0xBDD8_CF70, 0x3DE4_F7FE];

    let samples = digital_reference().samples(SEED, 0, PAIRS);
    let bytes = to_le_bytes(&samples);
    let digest = fnv1a64(&bytes);
    let first = [samples[0].to_bits(), samples[1].to_bits()];

    let document = format!(
        r#"{{"row":"digital_bytes","seed":{SEED},"pairs":{PAIRS},"bytes":{},"digest":"0x{digest:016X}","first_pair_bits":["0x{:08X}","0x{:08X}"],"first_pair":[{},{}]}}"#,
        bytes.len(),
        first[0],
        first[1],
        samples[0],
        samples[1]
    );
    println!("{document}");
    common::file_readout("iq-digital-bytes", &document);

    assert_eq!(bytes.len(), PAIRS * 2 * 4, "one f32 per component");
    assert_eq!(
        first, FIRST_PAIR_BITS,
        "the first I/Q pair moved: {} {}",
        samples[0], samples[1]
    );
    assert_eq!(
        digest, DIGEST,
        "the Digital path's bytes moved (0x{digest:016X})"
    );
}

/// The closed-form property the tone fixture cannot state: pulse shaping is
/// Nyquist, so matched-filtering the stream and sampling at the symbol
/// instants returns the transmitted symbols scaled by `amplitude` — the
/// contract `IqComponent::Digital`'s documentation claims ("`amplitude` is
/// the symbol amplitude after a unit-energy matched filter").
///
/// This is not a recording: the expectation comes from the constellation
/// and the seed, through an independent filter written here from
/// `neowon_sim::iq::rrc`, and it would fail for a generator that shaped,
/// scaled or ordered its symbols differently while staying perfectly
/// deterministic.
#[test]
fn the_matched_filter_returns_the_transmitted_symbols() {
    let span = RRC_SPAN * SPS;
    // Unit-energy discrete matched filter: Σ(h/√S)² ≈ (1/S)Σh² = 1.
    let taps: Vec<f64> = (-span..=span)
        .map(|d| rrc(d as f64 / SPS as f64, BETA) / (SPS as f64).sqrt())
        .collect();

    // Symbol n sits at sample n·SPS — the grid is absolute, set by the
    // sample index, not by this window. Only symbols with a whole filter
    // span of samples either side are measurable; the rest are truncated by
    // the block's edges, which is an artefact of the window, not of the
    // generator.
    let symbols = PAIRS as i64 * 8;
    let scene = digital_reference();
    let samples = scene.samples(SEED, 0, (symbols * SPS) as usize);
    let first = RRC_SPAN;
    let last = symbols - RRC_SPAN;

    let mut worst = 0.0f64;
    let mut checked = 0u32;
    for m in first..last {
        let centre = m * SPS;
        let (mut i, mut q) = (0.0, 0.0);
        for (t, &h) in taps.iter().enumerate() {
            let k = (centre - span + t as i64) as usize;
            i += samples[2 * k] as f64 * h;
            q += samples[2 * k + 1] as f64 * h;
        }
        let (ai, aq) = Modulation::Qpsk.point(symbol_bits(SEED, m));
        let error = ((i - AMP * ai).powi(2) + (q - AMP * aq).powi(2)).sqrt();
        worst = worst.max(error);
        checked += 1;
    }
    // Residual ISI: both the generator and this filter truncate the pulse
    // at ±RRC_SPAN symbols, and the Nyquist identity holds exactly only for
    // the untruncated pulse. Measured by the header command:
    // 0.00157 against a symbol amplitude of 0.5, i.e. 0.31 %. The bound is
    // 0.002 — 27 % over the measured worst case, and every structural
    // failure (no shaping, a √S scale error, a symbol grid off by one)
    // lands two to three orders above it.
    let document = format!(
        r#"{{"row":"matched_filter","symbols_checked":{checked},"amplitude":{AMP},"worst_error":{worst:.5},"bound":0.002}}"#
    );
    println!("{document}");
    common::file_readout("iq-matched-filter", &document);

    assert!(checked > 8_000, "only {checked} symbols measured");
    assert!(
        worst <= 0.002,
        "worst symbol error {worst:.5} at amplitude {AMP}"
    );
}
