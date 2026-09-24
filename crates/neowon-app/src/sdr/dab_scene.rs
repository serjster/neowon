//! The `rf-dab` sim scene: a real Mode I ensemble, composed in the app.
//!
//! The sim may not depend on `neowon-dsp`, so this module builds the ensemble with
//! `neowon_dsp::dab::encoder`, modulates it into IQ, and installs it with
//! [`neowon_sim::sdr::install_scene`] under the stable preset name `rf-dab`.
//! The app calls [`install`] when the operator selects that scene; nothing
//! here touches hardware.
//!
//! The ensemble: five services over five sub-channels.
//!
//! * Sub-channel 0 carries a DLS PAD stream (song/artist strings, one whose
//!   first data group spans two logical frames). No audio codec is involved:
//!   its logical frames are PAD regions directly, which is the transport the
//!   parser takes (`PadParser::push_pad_region`).
//! * Sub-channel 3 is a real DAB+ programme: the committed
//!   `dabplus_heaacv2.sf` super-frame stream (three 120 ms super frames,
//!   libfdk-aac HE-AAC v2 at `subchannel_index` 10), carried on a 60-CU EEP
//!   3-A sub-channel (80 kbit/s useful — TS 102 563 clause 5.1's rate).
//!   [`asc_override`] supplies the fixture's own AudioSpecificConfig,
//!   because no available encoder emits the 960-line transform DAB+
//!   mandates and the fixture is 1024-line; a real stream takes the
//!   standard-derived config instead.
//! * Sub-channel 4 is DAB classic: the committed `tone.mp2` (17 MPEG-1
//!   Layer II frames at 128 kbit/s) on a 96-CU UEP sub-channel — 384 bytes
//!   per 24 ms logical frame, exactly one MP2 frame each.
//!
//! **Determinism.** The IQ is a pure function of these constants: no wall
//! clock, no `thread_rng`, a seeded oracle encoder, and fixed byte sources.
//! The buffer is a whole number of Mode I frames, and every chosen stream
//! cycles on whole logical frames (the DAB+ fixture on whole super frames) —
//! and, because the clause-12 time interleaver carries 16 logical frames of
//! state, every chosen stream's cycle must *divide* the buffer. Only then is
//! re-playing the buffer a continuation of the transmitter's state rather
//! than a cut across it. `chosen_channels` asserts that invariant.
//!
//! The pieces: the transmitter chain for a chosen byte source is
//! `dab_scene/chain.rs`, the DLS PAD programme `dab_scene/dls.rs`; this file
//! holds the ensemble layout, the buffer composition and its loop-seam rule.

use std::sync::OnceLock;

use neowon_dsp::dab::encoder::{
    EnsembleSpec, FicFrame, MscEncoder, MscSubChannelSpec, ServiceSpec, SubChannelSpec,
};
use neowon_dsp::dab::msc::DEINTERLEAVE_DEPTH;
use neowon_dsp::dab::{CIFS_PER_FRAME, Protection, SAMPLE_RATE};
use neowon_sim::sdr::install_scene;
use neowon_sim::{IqBuffer, RfScene};

mod chain;
mod dls;
#[cfg(test)]
mod loop_tests;
#[cfg(test)]
mod tests;

use chain::{ChosenChannel, profile_of};
use dls::dls_payloads;

pub const EID: u16 = 0x1046;
pub const ENSEMBLE_LABEL: &str = "NEOWON SIM";
/// `(SId, label, SubChId, ASCTy)`: the DLS carrier and two real audio
/// programmes (one DAB+, one DAB classic).
pub const SERVICES: [(u16, &str, u8, u8); 5] = [
    (0x1001, "NEOWON ONE", 0, 63),
    (0x1002, "NEOWON TWO", 1, 63),
    (0x1003, "NEOWON THREE", 2, 0),
    (0x1004, "NEOWON DAB+", 3, 63),
    (0x1005, "NEOWON MP2", 4, 0),
];
/// The labels the DLS stream cycles, in order. The first is long enough that
/// its first segment's data group spans two logical frames (clause 7.4.5.2).
pub const DLS_TEXTS: [&str; 3] = [
    "Now playing: Café - Perfect Day",
    "Neowon Radio - rf-dab test",
    "DLS from the virtual ensemble",
];
/// The sub-channel that carries the DLS PAD stream (service 0x1001).
pub const DLS_SUB_CHANNEL: u8 = 0;
/// The DAB+ audio programme (service 0x1004, sub-channel 3).
pub const DABPLUS_SID: u16 = 0x1004;
pub const DABPLUS_SUB_CHANNEL: u8 = 3;
/// The DAB classic programme's sub-channel 4 (service 0x1005).
pub const MP2_SUB_CHANNEL: u8 = 4;

const PROTECTION_EEP_3A: Protection = Protection::Eep {
    option: 0,
    level: 2,
};
const PROTECTION_UEP_35: Protection = Protection::Uep { table_index: 35 };
const PROTECTION_EEP_2A: Protection = Protection::Eep {
    option: 0,
    level: 1,
};
/// 192 CUs = 256 kbit/s useful at EEP 3-A.
const EEP_3A_CU: u16 = 192;
/// 60 CUs = 80 kbit/s useful at EEP 3-A: `subchannel_index` 10, the rate the
/// committed DAB+ fixture was framed for (TS 102 563 clause 5.1).
const EEP_3A_DABPLUS_CU: u16 = 60;
/// 96 CUs = 128 kbit/s useful at UEP table index 35 (level 3).
const UEP_35_CU: u16 = 96;
/// 64 CUs = 64 kbit/s useful at EEP 2-A.
const EEP_2A_CU: u16 = 64;
const SUB_CHANNELS: [SubChannelSpec; 5] = [
    SubChannelSpec {
        id: 0,
        start_cu: 0,
        size_cu: EEP_3A_CU,
        protection: PROTECTION_EEP_3A,
    },
    SubChannelSpec {
        id: 1,
        start_cu: EEP_3A_CU,
        size_cu: UEP_35_CU,
        protection: PROTECTION_UEP_35,
    },
    SubChannelSpec {
        id: 2,
        start_cu: EEP_3A_CU + UEP_35_CU,
        size_cu: EEP_2A_CU,
        protection: PROTECTION_EEP_2A,
    },
    SubChannelSpec {
        id: DABPLUS_SUB_CHANNEL,
        start_cu: EEP_3A_CU + UEP_35_CU + EEP_2A_CU,
        size_cu: EEP_3A_DABPLUS_CU,
        protection: PROTECTION_EEP_3A,
    },
    SubChannelSpec {
        id: MP2_SUB_CHANNEL,
        start_cu: EEP_3A_CU + UEP_35_CU + EEP_2A_CU + EEP_3A_DABPLUS_CU,
        size_cu: UEP_35_CU,
        protection: PROTECTION_UEP_35,
    },
];
/// Transmission frames in the buffer: 1.44 s at 2.048 MS/s. A whole number
/// of DAB+ fixture periods (four), so the loop seam is a super-frame
/// boundary, and long enough that the clause-12 warm-up does not dominate.
const TRANSMISSION_FRAMES: usize = 15;
/// Transmission frames generated and discarded so the kept buffer starts
/// after the clause-12 interleaver's zero-history transient. Replaying the
/// buffer is only a valid continuation of the transmitter's state when its
/// first frame already has 16 logical frames of history behind it —
/// otherwise every loop seam corrupts the first super frames after it.
const WARMUP_FRAMES: usize = DEINTERLEAVE_DEPTH / CIFS_PER_FRAME;
/// Frame RMS, a typical SDR capture level.
const FRAME_RMS: f32 = 0.2;
/// Oracle payload seed: the non-chosen sub-channels carry this generator's
/// bytes, which the decoder must also recover (they are not asserted here).
const SEED: u64 = 0x0DAB_5EED;

/// The audio fixture bytes the scene carries. Copies of the committed
/// `neowon-codec` acceptance fixtures; provenance, hashes and the
/// 1024-vs-960 note are in `tests/fixtures/README.md`.
const DABPLUS_SF: &[u8] = include_bytes!("../../tests/fixtures/dabplus_heaacv2.sf");
const DABPLUS_ASC: &[u8] = include_bytes!("../../tests/fixtures/dabplus_heaacv2.asc");
const TONE_MP2: &[u8] = include_bytes!("../../tests/fixtures/tone.mp2");
/// MP2 frames of the fixture the scene cycles. The fixture's 17 frames do not
/// divide the 60-logical-frame buffer, so cycling all of it corrupts the
/// frames straddling each loop seam (the interleaver periodicity argument
/// above); 15 divides 60 and keeps most of the fixture.
const MP2_LOOP_FRAMES: usize = 15;
/// One MPEG-1 Layer II frame at 128 kbit/s, 48 kHz.
const MP2_FRAME_BYTES: usize = 384;

/// The fixture's own AudioSpecificConfig for the scene's DAB+ programme, or
/// `None` for any other ensemble/service. The app's playback resolves this
/// as its per-programme ASC override (`sdr::dab_audio`): the fixture is a
/// 1024-line libfdk stream, while the standard-derived config would ask for
/// the 960-line transform the fixture does not have.
#[must_use]
pub fn asc_override(eid: Option<u16>, sid: u16) -> Option<&'static [u8]> {
    (eid == Some(EID) && sid == DABPLUS_SID).then_some(DABPLUS_ASC)
}

/// Install the composed scene under the stable `rf-dab` name. Builds once
/// per process; the second call is a no-op.
pub fn install() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| install_scene("rf-dab", scene()));
}

/// The scene the sim receives: no emitters, no noise, one IQ buffer. The
/// samples live in a `OnceLock` because the sim's component borrows them for
/// the process (`IqBuffer` is `&'static [f32]`); the scene is installed once.
pub fn scene() -> RfScene {
    static SAMPLES: OnceLock<Vec<f32>> = OnceLock::new();
    RfScene {
        emitters: Vec::new(),
        noise_rms: 0.0,
        buffer: Some(IqBuffer {
            samples: SAMPLES.get_or_init(iq_samples),
            sample_rate: SAMPLE_RATE,
        }),
    }
}

/// `TRANSMISSION_FRAMES` whole Mode I frames, interleaved I, Q at full scale.
pub fn iq_samples() -> Vec<f32> {
    let spec = ensemble_spec();
    let fic = FicFrame::new(&spec);
    let msc_specs: Vec<MscSubChannelSpec> = SUB_CHANNELS
        .iter()
        .map(|sub| MscSubChannelSpec {
            id: sub.id,
            start_cu: sub.start_cu,
            size_cu: sub.size_cu,
            protection: sub.protection,
        })
        .collect();
    let mut msc = MscEncoder::new(msc_specs, false, SEED);
    let mut channels = chosen_channels();

    let mut samples = Vec::with_capacity(TRANSMISSION_FRAMES * neowon_dsp::dab::FRAME_SAMPLES * 2);
    for index in 0..TRANSMISSION_FRAMES + WARMUP_FRAMES {
        let mut frame_bits = Vec::with_capacity(4 * neowon_dsp::dab::CIF_SOFT_BITS);
        for cif in msc.next_cifs(CIFS_PER_FRAME) {
            let mut bits = cif.bits;
            for channel in channels.iter_mut() {
                let encoded = channel.advance();
                bits[channel.start_bit..channel.start_bit + encoded.len()]
                    .copy_from_slice(&encoded);
            }
            frame_bits.extend_from_slice(&bits);
        }
        if index < WARMUP_FRAMES {
            continue;
        }
        for sample in fic.iq_frame_with_msc(&frame_bits, FRAME_RMS) {
            samples.push(sample.re);
            samples.push(sample.im);
        }
    }
    samples
}

fn ensemble_spec() -> EnsembleSpec<'static> {
    EnsembleSpec {
        eid: EID,
        label: ENSEMBLE_LABEL,
        services: SERVICES
            .iter()
            .map(|(sid, label, sub_channel, ascty)| ServiceSpec {
                sid: *sid,
                label,
                sub_channel: *sub_channel,
                ascty: *ascty,
            })
            .collect(),
        sub_channels: SUB_CHANNELS.to_vec(),
    }
}

/// The sub-channels whose payload is chosen rather than the encoder's
/// pseudo-random filler: the DLS carrier and the two audio programmes.
fn chosen_channels() -> Vec<ChosenChannel> {
    let dls_profile = profile_of(PROTECTION_EEP_3A, EEP_3A_CU);
    let dls_source = dls_payloads(
        CIFS_PER_FRAME * TRANSMISSION_FRAMES,
        dls_profile.info_bits() / 8,
    )
    .concat();
    let mp2_source = TONE_MP2[..MP2_LOOP_FRAMES * MP2_FRAME_BYTES].to_vec();
    let chosen = vec![
        ChosenChannel::new(&sub(DLS_SUB_CHANNEL), dls_source),
        ChosenChannel::new(&sub(DABPLUS_SUB_CHANNEL), DABPLUS_SF.to_vec()),
        ChosenChannel::new(&sub(MP2_SUB_CHANNEL), mp2_source),
    ];
    // Re-playing the buffer is a valid continuation of the transmitter only
    // when each stream's cycle divides it: otherwise the receiver's
    // de-interleaver mixes two payload phases across the wrap and the frames
    // after the seam decode as garbage.
    let buffer_frames = CIFS_PER_FRAME * TRANSMISSION_FRAMES;
    for channel in &chosen {
        assert!(
            buffer_frames.is_multiple_of(channel.period_frames()),
            "a chosen stream cycles every {} logical frames, which does not \
             divide the {buffer_frames}-frame buffer, so the loop seam would \
             corrupt it",
            channel.period_frames()
        );
    }
    chosen
}

/// The layout entry for a chosen channel; the scene owns both, so a missing
/// id is a programming error, not input.
fn sub(id: u8) -> SubChannelSpec {
    *SUB_CHANNELS
        .iter()
        .find(|s| s.id == id)
        .expect("the chosen channel is in the layout")
}
