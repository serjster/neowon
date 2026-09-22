//! Unit tests for the MSC decoder (see `super`).
use super::*;
use crate::dab::fec::{depuncture_regions, eep_profile, uep_profile};

/// Row 10: the clause-12 delay permutation, pinned two ways. First the map
/// itself against table 21's rows; then behaviour: a marker at received
/// position `i` of frame `r` must come out in logical frame `r - D[i % 16]`.
#[test]
fn clause_12_delay_permutation_is_reproduced() {
    // Table 21, read as r' = r - R(ir mod 16), lists these delays.
    assert_eq!(
        DEINTERLEAVE_MAP,
        [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]
    );

    for k in 0..DEINTERLEAVE_DEPTH {
        let mut deinterleaver = TimeDeinterleaver::new(DEINTERLEAVE_DEPTH);
        let marker_frame = 20usize;
        let mut outputs: Vec<Option<Vec<i8>>> = Vec::new();
        for r in 0..40usize {
            let mut fragment = vec![0i8; DEINTERLEAVE_DEPTH];
            if r == marker_frame {
                fragment[k] = 1;
            }
            outputs.push(deinterleaver.push(&fragment));
        }
        let delay = DEINTERLEAVE_MAP[k];
        let target = marker_frame - delay;
        for (t, out) in outputs.iter().enumerate() {
            let frame = t as isize - DEINTERLEAVE_DEPTH as isize;
            let Some(out) = out else {
                continue;
            };
            let expected = if frame == target as isize { 1 } else { 0 };
            assert_eq!(
                out[k], expected,
                "k {k}: output frame {frame} at position {k}"
            );
        }
        assert!(outputs[target + DEINTERLEAVE_DEPTH].is_some(), "k {k}");
    }
}

/// The de-interleaver emits nothing until its ring has seen 16 frames, and
/// then emits one frame per input in order.
#[test]
fn deinterleaver_warms_up_for_16_frames() {
    let mut deinterleaver = TimeDeinterleaver::new(4);
    for r in 0..16u64 {
        assert_eq!(deinterleaver.push(&[r as i8; 4]), None);
    }
    let first = deinterleaver.push(&[16, 16, 16, 16]).expect("frame 16");
    // At receive index 16 the output is c_D for each position, i.e. the
    // frame whose index is the clause-12 delay of that position.
    assert_eq!(first, vec![0, 8, 4, 12]);
    assert_eq!(deinterleaver.received(), 17);
}

/// Time interleaving then de-interleaving round-trips a known sequence:
/// both directions implement clause 12, so a mismatch in either fails here.
#[test]
fn time_interleaver_round_trips_clause_12() {
    let profile = Profile::Uep(uep_profile(0).expect("32 kbit/s level 5"));
    let cu_bits = profile.punctured_bits().div_ceil(64) * 64;
    let mut interleaver = TimeInterleaver::new(cu_bits);
    let mut deinterleaver = TimeDeinterleaver::new(cu_bits);
    let mut frames = Vec::new();
    for r in 0..40u64 {
        let info: Vec<u8> = (0..profile.info_bits())
            .map(|i| ((i as u64 * 31 + r * 7).is_multiple_of(11)) as u8)
            .collect();
        let coded = interleaver.push(&info, profile);
        let soft: Vec<i8> = coded
            .iter()
            .map(|b| if *b == 1 { 100 } else { -100 })
            .collect();
        frames.push((info, deinterleaver.push(&soft)));
    }
    let mut after_warmup = 0;
    for r in DEINTERLEAVE_DEPTH..frames.len() {
        let (expected, _) = &frames[r - DEINTERLEAVE_DEPTH];
        let decoded = frames[r].1.as_ref().expect("after warm-up");
        let depunctured = profile.depuncture(decoded);
        let (mut bits, _) = viterbi_decode(&depunctured);
        energy_dispersal(&mut bits);
        assert_eq!(&bits, expected, "logical frame {}", r - DEINTERLEAVE_DEPTH);
        after_warmup += 1;
    }
    assert_eq!(after_warmup, 40 - DEINTERLEAVE_DEPTH);
}

/// The MSC FEC chain's soft-bit polarity, as an explicit test: inverting
/// the sign convention must not decode. This is the `tpeg-rust` trap on the
/// MSC side of the receiver.
#[test]
fn msc_polarity_is_detectable() {
    let profile = Profile::Eep(eep_profile(256, 2, 0).expect("3-A 256"));
    let info: Vec<u8> = (0..profile.info_bits())
        .map(|i| ((i * 5 + i / 7) % 3 == 0) as u8)
        .collect();
    let mut scrambled = info.clone();
    energy_dispersal(&mut scrambled);
    let mother = conv_encode(&scrambled);
    let regions = profile.regions();
    let punctured = crate::dab::fec::puncture_regions(&mother, &regions);
    let soft: Vec<i8> = punctured
        .iter()
        .map(|b| if *b == 1 { 100 } else { -100 })
        .collect();
    let decode = |soft: &[i8]| {
        let depunctured = depuncture_regions(soft, &regions);
        let (mut bits, _) = viterbi_decode(&depunctured);
        energy_dispersal(&mut bits);
        bits
    };
    assert_eq!(decode(&soft), info);
    let inverted: Vec<i8> = soft.iter().map(|s| -*s).collect();
    let wrong = decode(&inverted);
    let flipped = wrong.iter().zip(&info).filter(|(a, b)| a != b).count();
    assert!(
        flipped > 1000,
        "inverted soft bits decoded {flipped} errors"
    );
}

/// A FIC sub-channel entry resolves to the same profile the encoder would
/// use, for both protections, and an undefined combination is refused.
#[test]
fn sub_channel_plans_resolve_from_the_fic_table() {
    let eep = SubChannel {
        id: 3,
        start_cu: 0,
        size_cu: Some(96),
        protection: Protection::Eep {
            option: 0,
            level: 2,
        },
        bitrate_kbps: Some(256.0),
    };
    let decoder = SubChannelDecoder::new(&eep).expect("EEP 3-A 128");
    // 96 CUs at EEP 3-A is 6n CUs with n = 16, i.e. 128 kbit/s.
    assert_eq!(decoder.profile().info_bits(), 3072);

    let uep = SubChannel {
        id: 4,
        start_cu: 96,
        size_cu: Some(96),
        protection: Protection::Uep { table_index: 35 },
        bitrate_kbps: Some(128.0),
    };
    let decoder = SubChannelDecoder::new(&uep).expect("UEP index 35");
    match decoder.profile() {
        Profile::Uep(p) => {
            assert_eq!((p.bitrate_kbps, p.level), (128, 3));
            assert_eq!(p.padding, 4);
        }
        other => panic!("expected UEP, got {other:?}"),
    }

    // A UEP index whose table-8 size contradicts the signalled size is
    // refused rather than decoded against the wrong allocation.
    let wrong = SubChannel {
        size_cu: Some(64),
        ..uep
    };
    assert!(SubChannelDecoder::new(&wrong).is_none());
    // EEP with no size at all cannot be resolved.
    let unknown = SubChannel {
        size_cu: None,
        ..eep
    };
    assert!(SubChannelDecoder::new(&unknown).is_none());
}

/// The demux builds and drops handlers as the FIC table changes, and a
/// reconfiguration resets (never mixes) a handler's interleaver.
#[test]
fn demux_tracks_the_fic_table() {
    let mut ensemble = Ensemble::default();
    ensemble.sub_channels.insert(
        0,
        SubChannel {
            id: 0,
            start_cu: 0,
            size_cu: Some(96),
            protection: Protection::Eep {
                option: 0,
                level: 2,
            },
            bitrate_kbps: Some(256.0),
        },
    );
    let mut demux = MscDecoder::new();
    assert_eq!(demux.sync(&ensemble), 1);
    ensemble.sub_channels.get_mut(&0).unwrap().start_cu = 96;
    assert_eq!(demux.sync(&ensemble), 1);
    ensemble.sub_channels.clear();
    assert_eq!(demux.sync(&ensemble), 0);
    assert_eq!(demux.status().len(), 0);
}

/// `discard` (a gap in the CIF stream) restarts the clause-12 warm-up rather
/// than emitting a frame mixed across the hole.
#[test]
fn discard_restarts_the_interleaver_warmup() {
    use crate::dab::encoder::{MscEncoder, MscSubChannelSpec};

    let protection = Protection::Eep {
        option: 0,
        level: 2,
    };
    let mut ensemble = Ensemble::default();
    ensemble.sub_channels.insert(
        0,
        SubChannel {
            id: 0,
            start_cu: 0,
            size_cu: Some(96),
            protection,
            bitrate_kbps: Some(128.0),
        },
    );
    let mut demux = MscDecoder::new();
    assert_eq!(demux.sync(&ensemble), 1);
    let mut encoder = MscEncoder::new(
        vec![MscSubChannelSpec {
            id: 0,
            start_cu: 0,
            size_cu: 96,
            protection,
        }],
        false,
        11,
    );

    for _ in 0..DEINTERLEAVE_DEPTH {
        demux.push_cif(&encoder.next_cif().soft_bits());
    }
    assert!(demux.take_frames().is_empty(), "warm-up");
    demux.push_cif(&encoder.next_cif().soft_bits());
    assert_eq!(demux.take_frames().len(), 1);

    demux.discard();
    for _ in 0..DEINTERLEAVE_DEPTH {
        demux.push_cif(&encoder.next_cif().soft_bits());
    }
    assert!(demux.take_frames().is_empty(), "warm-up after the gap");
    demux.push_cif(&encoder.next_cif().soft_bits());
    assert_eq!(demux.take_frames().len(), 1);
}
