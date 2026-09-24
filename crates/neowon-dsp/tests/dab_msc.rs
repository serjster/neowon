//! Tier-2 golden tests for DAB MSC: modulate a chosen ensemble
//! — FIC plus real sub-channel content — into Mode I IQ at 2.048 MS/s, push it
//! through the receiver, and check that the sub-channel bytes that come out are
//! the bytes the encoder was told to send.
//!
//! This is criteria rows 8, 9 and 12 of `docs/tasks/phase10-dab-spec.md`:
//!
//! - **row 8** — bit-exact sub-channel bytes for EEP (option A, level 3) and
//!   UEP (level 3) at 15 dB. A "frame" here is a *sub-channel logical frame*
//!   (24 ms), which is what the MSC decoder emits; 55 transmission frames
//!   yield exactly 200 of them per sub-channel after the 16-CIF clause-12
//!   warm-up and the front end's one-frame sync lookahead.
//! - **row 9** — the same fixture's payload CRC rate, ≥ 95%. The CRC is the
//!   sim's oracle transport (annex E's CRC-16 over each logical frame); EN 300
//!   401 has no MSC CRC at this layer, so the receiver checks it only when told
//!   to, which is what this fixture does.
//! - **row 12** — a multi-segment DLS label reassembled exactly, including a
//!   segment whose data group spans two audio frames.
//!
//! What it does **not** prove: that our reading of the standard matches the
//! standard. The modulator and demodulator share an author and a reading, and
//! only a hardware run (tier 1's row 7, already passed) removes that class of
//! error for the front end. The table tests in `dab::fec` pin tables 8, 13, 15,
//! 18 and 20 against the standard's printed values as compensation.

use std::collections::BTreeMap;

use neowon_dsp::dab::encoder::{
    EnsembleSpec, FicFrame, MscEncoder, MscSubChannelSpec, ServiceSpec, SubChannelSpec,
};
use neowon_dsp::dab::pad::PadParser;
use neowon_dsp::dab::receiver::DabReceiver;
use neowon_dsp::dab::{CIF_SOFT_BITS, FRAME_SAMPLES, Protection, SAMPLE_RATE};

/// Signal-to-noise ratio of the main fixture, in dB.
const SNR_DB: f32 = 15.0;
/// Level the modulator scales frames to, matching a typical SDR capture.
const FRAME_RMS: f32 = 0.2;
/// Transmission frames pushed: 4 CIFs each, 16 CIFs of de-interleaver warm-up
/// and one frame of sync lookahead leave exactly 200 logical frames.
const TRANSMISSION_FRAMES: usize = 55;
/// The two sub-channels: EEP 3-A at 256 kbit/s and UEP index 35 (128 kbit/s,
/// level 3) — the two protections row 8 names.
const EEP: Protection = Protection::Eep {
    option: 0,
    level: 2,
};
const UEP: Protection = Protection::Uep { table_index: 35 };
const EEP_CU: u16 = 192;
const UEP_CU: u16 = 96;

/// Seeded PRNG: the sim's rule (no wall clock, no `thread_rng`) applies to test
/// stimulus too, or the fixture is not reproducible.
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

fn sub_channels() -> Vec<SubChannelSpec> {
    vec![
        SubChannelSpec {
            id: 0,
            start_cu: 0,
            size_cu: EEP_CU,
            protection: EEP,
        },
        SubChannelSpec {
            id: 1,
            start_cu: EEP_CU,
            size_cu: UEP_CU,
            protection: UEP,
        },
    ]
}

fn spec<'a>() -> EnsembleSpec<'a> {
    EnsembleSpec {
        eid: 0xF044,
        label: "MSC FIXTURE",
        services: vec![
            ServiceSpec {
                sid: 0x1001,
                label: "EEP SERVICE",
                sub_channel: 0,
                ascty: 63,
            },
            ServiceSpec {
                sid: 0x1002,
                label: "UEP SERVICE",
                sub_channel: 1,
                ascty: 63,
            },
        ],
        sub_channels: sub_channels(),
    }
}

fn msc_specs() -> Vec<MscSubChannelSpec> {
    sub_channels()
        .into_iter()
        .map(|sub| MscSubChannelSpec {
            id: sub.id,
            start_cu: sub.start_cu,
            size_cu: sub.size_cu,
            protection: sub.protection,
        })
        .collect()
}

/// Rows 8 and 9: the same fixture serves both — 200 logical frames per
/// sub-channel at 15 dB, byte-exact against the encoder, with the oracle CRC
/// rate above the 0.95 floor.
#[test]
fn subchannel_bytes_are_bit_exact_and_crc_clean_at_15_db() {
    let fic = FicFrame::new(&spec());
    let mut encoder = MscEncoder::new(msc_specs(), true, 0x5EED_1234);
    let mut receiver = DabReceiver::new();
    receiver.enable_msc_payload_crc();

    let mut rng = Rng::new(0xC0FF_EE00);
    let signal_power = FRAME_RMS * FRAME_RMS;
    let noise_sigma = (signal_power / 10f32.powf(SNR_DB / 10.0) / 2.0).sqrt();

    let mut expected: BTreeMap<u8, Vec<Vec<u8>>> = BTreeMap::new();
    let mut decoded: BTreeMap<u8, Vec<Vec<u8>>> = BTreeMap::new();
    for _ in 0..TRANSMISSION_FRAMES {
        let cifs = encoder.next_cifs(4);
        let mut msc_bits = Vec::with_capacity(4 * CIF_SOFT_BITS);
        for cif in &cifs {
            msc_bits.extend_from_slice(cif.transmitted_bits());
            for (id, payload) in &cif.payloads {
                expected.entry(*id).or_default().push(payload.clone());
            }
        }
        let iq = fic.iq_frame_with_msc(&msc_bits, FRAME_RMS);

        let mut buffer = Vec::with_capacity(iq.len() * 2);
        for sample in iq.iter() {
            let value = *sample
                + rustfft::num_complex::Complex32::new(
                    rng.gaussian() * noise_sigma,
                    rng.gaussian() * noise_sigma,
                );
            buffer.push(value.re);
            buffer.push(value.im);
        }
        for chunk in buffer.chunks(4096) {
            receiver.push_iq(chunk);
        }
        for frame in receiver.take_msc_frames() {
            decoded
                .entry(frame.sub_channel)
                .or_default()
                .push(frame.bytes);
        }
    }

    let status = receiver.status();
    assert!(status.locked, "the FIC should lock: {status:?}");
    for (id, sample_rate) in [(0u8, 256.0), (1u8, 128.0)] {
        let expected = &expected[&id];
        let got = &decoded[&id];
        assert!(
            got.len() >= 200,
            "sub-channel {id}: only {} logical frames",
            got.len()
        );
        for (index, bytes) in got.iter().enumerate() {
            assert_eq!(
                bytes.len() as f64,
                sample_rate * 24.0 / 8.0,
                "sub-channel {id} frame {index} length"
            );
            assert_eq!(bytes, &expected[index], "sub-channel {id} frame {index}");
        }
        let counters = status.msc.get(&id).expect("counter for sub-channel");
        assert_eq!(counters.frames, got.len() as u64);
        assert_eq!(counters.crc_checks, got.len() as u64);
        let rate = 1.0 - counters.crc_failures as f64 / counters.crc_checks as f64;
        assert!(
            rate >= 0.95,
            "sub-channel {id}: MSC CRC rate {rate} below the 0.95 floor"
        );
    }
}

/// On air there is no MSC CRC to check: a receiver that was not told about the
/// oracle transport reports zero checks (and zero failures), never a made-up
/// rate. The bytes still come out.
#[test]
fn without_the_oracle_transport_no_crc_is_claimed() {
    let fic = FicFrame::new(&spec());
    let mut encoder = MscEncoder::new(msc_specs(), false, 7);
    let mut receiver = DabReceiver::new();
    let mut decoded = 0usize;
    for _ in 0..24 {
        let cifs = encoder.next_cifs(4);
        let mut msc_bits = Vec::with_capacity(4 * CIF_SOFT_BITS);
        for cif in &cifs {
            msc_bits.extend_from_slice(cif.transmitted_bits());
        }
        let iq = fic.iq_frame_with_msc(&msc_bits, FRAME_RMS);
        let interleaved: Vec<f32> = iq.iter().flat_map(|c| [c.re, c.im]).collect();
        receiver.push_iq(&interleaved);
        decoded += receiver.take_msc_frames().len();
    }
    assert!(decoded > 0);
    let status = receiver.status();
    for (id, counters) in &status.msc {
        assert_eq!(counters.crc_checks, 0, "sub-channel {id}");
        assert_eq!(counters.crc_failures, 0, "sub-channel {id}");
        assert!(counters.frames > 0, "sub-channel {id} frames");
    }
}

/// Row 12: a multi-segment DLS label reassembled exactly, including one
/// segment whose data group is split across two frames.
#[test]
fn dls_multi_segment_round_trip_spans_two_frames() {
    // The é is EBU Latin 0x82: the label is charset 0.
    let segments: [Vec<u8>; 3] = [
        b"Now playing: Caf".to_vec(),
        [&[0x82u8][..], b" - Perfect "].concat(),
        b"Day".to_vec(),
    ];
    let expected = "Now playing: Café - Perfect Day";

    let mut parser = PadParser::new();
    assert_eq!(parser.dls(), None);
    // Frame 0: the start of segment 0 (its data group is 20 bytes; the first
    // 12 arrive now).
    let frame0 = variable_xpad(&[(2, &dls_group(true, true, false, 0, &segments[0])[..12])]);
    assert_eq!(parser.push_pad_region(&pad_region(&frame0)), None);
    // Frame 1: the rest of segment 0, then all of segment 1 (still no last
    // segment, so nothing is published).
    let seg0 = dls_group(true, true, false, 0, &segments[0]);
    let seg1 = dls_group(true, false, false, 1, &segments[1]);
    assert_eq!(seg0.len(), 20);
    let frame1 = variable_xpad(&[(3, &seg0[12..]), (2, &seg1)]);
    assert_eq!(parser.push_pad_region(&pad_region(&frame1)), None);
    assert_eq!(parser.dls(), None, "partial labels are never published");
    // Frame 2: the last segment completes the message.
    let seg2 = dls_group(true, false, true, 2, &segments[2]);
    let frame2 = variable_xpad(&[(2, &seg2)]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&frame2)),
        Some(expected.to_string())
    );
    assert_eq!(parser.dls(), Some(expected));
}

/// One DLS data group (segment): header, characters, annex-E CRC.
fn dls_group(toggle: bool, first: bool, last: bool, number: u8, text: &[u8]) -> Vec<u8> {
    assert!(text.len() <= 16);
    let mut group = vec![0u8; 2];
    group[0] =
        (toggle as u8) << 7 | (first as u8) << 6 | (last as u8) << 5 | (text.len() as u8 - 1);
    group[1] = if first { 0x00 } else { (number & 0x07) << 4 };
    group.extend_from_slice(text);
    let crc = neowon_dsp::dab::fec::crc16(&group);
    group.push((crc >> 8) as u8);
    group.push(crc as u8);
    group
}

/// A variable-size X-PAD field with one sub-field per `(app_type, data)`, each
/// padded to a legal length code and the list closed by an end marker.
fn variable_xpad(subfields: &[(u8, &[u8])]) -> Vec<u8> {
    const LENGTHS: [usize; 8] = [4, 6, 8, 12, 16, 24, 32, 48];
    let mut xpad = Vec::new();
    let mut chosen = Vec::new();
    for (app, data) in subfields {
        let len = *LENGTHS
            .iter()
            .find(|len| **len >= data.len())
            .expect("sub-field fits a length code");
        let code = LENGTHS.iter().position(|l| *l == len).unwrap() as u8;
        chosen.push((len, data.to_vec()));
        xpad.push((code << 5) | app);
    }
    xpad.push(0x00); // end marker
    for (len, data) in chosen {
        xpad.extend_from_slice(&data);
        xpad.resize(xpad.len() + (len - data.len()), 0x00);
    }
    xpad
}

/// Wrap a logical X-PAD field in a transmission-order PAD region: X-PAD
/// reversed, then the two F-PAD bytes (variable-size indicator, CI flag set).
fn pad_region(xpad: &[u8]) -> Vec<u8> {
    let mut region: Vec<u8> = xpad.iter().rev().copied().collect();
    region.push(0x20);
    region.push(0x02);
    region
}

/// The fixture's arithmetic is worth keeping honest: the front end decodes
/// `n - 1` of `n` pushed frames, each with four 24 ms CIFs, and the clause-12
/// warm-up costs 16 of them.
#[test]
fn fixture_arithmetic_is_consistent() {
    assert_eq!((TRANSMISSION_FRAMES - 1) * 4 - 16, 200);
    assert_eq!(CIF_SOFT_BITS, 864 * 64);
    assert_eq!(FRAME_SAMPLES, 196_608);
    assert_eq!(SAMPLE_RATE, 2_048_000.0);
}

/// Regression for an on-air defect: **a time-varying echo that
/// moves the null symbol's power dip starved the clause-12 delay line.**
///
/// A DAB transmitter's guard interval (EN 300 401 table 22: `Delta = 504`
/// samples at 2.048 MS/s) absorbs echoes, but under SFN/multipath the null
/// symbol's dip is no longer where the power minimum says: the receiver that
/// re-derived its frame start from the dip every frame landed on a moving grid,
/// rejected most frames, and discarded the MSC de-interleaver on each rejection
/// — so a sub-channel never completed the 16 logical frames of clause-12
/// continuity and *no* logical frame was correct, while the FIC (no inter-frame
/// memory) stayed perfectly clean. On a real 11C capture that was 0 valid DAB+
/// superframes with a locked, 100%-CRC FIC; this fixture is the same shape in
/// the small: the echo delay alternates between two inside-guard values, frame
/// to frame, and every logical frame must still come out byte-exact.
///
/// The static-AWGN fixture above (`subchannel_bytes_are_bit_exact_…`) cannot
/// catch this class: with a single fixed path the dip never moves and
/// re-searching every frame finds the same grid, which is why this test carries
/// the impairment.
#[test]
fn multipath_echo_keeps_the_clause_12_chain_contiguous() {
    // Both delays are inside the guard interval (504 samples) and one frame
    // apart in phase, so the differential demodulator still sees a valid
    // channel and the only thing that moves is the power dip.
    const DELAYS: [usize; 2] = [128, 384];
    const ECHO_GAIN: f32 = 0.7;

    let fic = FicFrame::new(&spec());
    let mut encoder = MscEncoder::new(msc_specs(), true, 0x5EED_1234);
    let mut receiver = DabReceiver::new();
    receiver.enable_msc_payload_crc();

    let mut rng = Rng::new(0xC0FF_EE00);
    let signal_power = FRAME_RMS * FRAME_RMS;
    let noise_sigma = (signal_power / 10f32.powf(SNR_DB / 10.0) / 2.0).sqrt();

    let mut expected: BTreeMap<u8, Vec<Vec<u8>>> = BTreeMap::new();
    let mut decoded: BTreeMap<u8, Vec<Vec<u8>>> = BTreeMap::new();
    for frame in 0..TRANSMISSION_FRAMES {
        let cifs = encoder.next_cifs(4);
        let mut msc_bits = Vec::with_capacity(4 * CIF_SOFT_BITS);
        for cif in &cifs {
            msc_bits.extend_from_slice(cif.transmitted_bits());
            for (id, payload) in &cif.payloads {
                expected.entry(*id).or_default().push(payload.clone());
            }
        }
        let iq = fic.iq_frame_with_msc(&msc_bits, FRAME_RMS);

        let delay = DELAYS[frame % DELAYS.len()];
        let mut echoed = vec![rustfft::num_complex::Complex32::new(0.0, 0.0); delay];
        echoed.extend(iq.iter().copied());
        let mut buffer = Vec::with_capacity(iq.len() * 2);
        for (index, sample) in iq.iter().enumerate() {
            let value = *sample
                + echoed[index] * ECHO_GAIN
                + rustfft::num_complex::Complex32::new(
                    rng.gaussian() * noise_sigma,
                    rng.gaussian() * noise_sigma,
                );
            buffer.push(value.re);
            buffer.push(value.im);
        }
        for chunk in buffer.chunks(4096) {
            receiver.push_iq(chunk);
        }
        for frame in receiver.take_msc_frames() {
            decoded
                .entry(frame.sub_channel)
                .or_default()
                .push(frame.bytes);
        }
    }

    let status = receiver.status();
    assert!(status.locked, "the FIC should lock: {status:?}");
    for (id, sample_rate) in [(0u8, 256.0), (1u8, 128.0)] {
        let expected = &expected[&id];
        let got = &decoded[&id];
        assert!(
            got.len() >= 200,
            "sub-channel {id}: only {} logical frames — the delay line was starved",
            got.len()
        );
        for (index, bytes) in got.iter().enumerate() {
            assert_eq!(
                bytes.len() as f64,
                sample_rate * 24.0 / 8.0,
                "sub-channel {id} frame {index} length"
            );
            assert_eq!(bytes, &expected[index], "sub-channel {id} frame {index}");
        }
        let counters = status.msc.get(&id).expect("counter for sub-channel");
        assert_eq!(counters.frames, got.len() as u64);
        assert_eq!(counters.crc_checks, got.len() as u64);
        let rate = 1.0 - counters.crc_failures as f64 / counters.crc_checks as f64;
        assert!(
            rate >= 0.95,
            "sub-channel {id}: MSC CRC rate {rate} below the 0.95 floor"
        );
    }
}

/// The receiver must also survive a caller that hands it exactly one frame per
/// call: the PRS refinement moves a coarse start by up to `COARSE_SPAN`
/// samples, and a candidate near the end of the buffered samples once walked
/// the symbol loops past the buffer (index out of bounds). The clamps in
/// `refine_frame_start` are what this pins; the impairment is the same echo as
/// the regression above so the coarse dip is genuinely misplaced.
#[test]
fn frame_sized_feeds_never_overrun_the_sample_buffer() {
    const DELAYS: [usize; 2] = [128, 384];
    let fic = FicFrame::new(&spec());
    let mut encoder = MscEncoder::new(msc_specs(), true, 7);
    let mut receiver = DabReceiver::new();
    for frame in 0..6 {
        let cifs = encoder.next_cifs(4);
        let mut msc_bits = Vec::with_capacity(4 * CIF_SOFT_BITS);
        for cif in &cifs {
            msc_bits.extend_from_slice(cif.transmitted_bits());
        }
        let iq = fic.iq_frame_with_msc(&msc_bits, FRAME_RMS);
        let delay = DELAYS[frame % DELAYS.len()];
        let mut echoed = vec![rustfft::num_complex::Complex32::new(0.0, 0.0); delay];
        echoed.extend(iq.iter().copied());
        let interleaved: Vec<f32> = iq
            .iter()
            .enumerate()
            .flat_map(|(index, sample)| {
                let value = *sample * 0.4 + echoed[index] * 0.6;
                [value.re, value.im]
            })
            .collect();
        receiver.push_iq(&interleaved);
        receiver.take_msc_frames();
    }
}
