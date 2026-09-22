//! The `rf-dab` sim scene: a real Mode I ensemble, composed in the app.
//!
//! The sim may not depend on `neowon-dsp` (tier-1 deviation 1 of
//! `docs/tasks/phase10-dab-spec.md`), so this module builds the ensemble with
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

use std::sync::OnceLock;

use neowon_dsp::dab::encoder::{
    EnsembleSpec, FicFrame, MscEncoder, MscSubChannelSpec, ServiceSpec, SubChannelSpec,
};
use neowon_dsp::dab::fec::{
    EepProfile, UEP_PROFILES, UepProfile, conv_encode, crc16, energy_dispersal, puncture_regions,
};
use neowon_dsp::dab::msc::{DEINTERLEAVE_DEPTH, DEINTERLEAVE_MAP, Profile};
use neowon_dsp::dab::{CIFS_PER_FRAME, CU_BITS, Protection, SAMPLE_RATE};
use neowon_sim::sdr::install_scene;
use neowon_sim::{IqBuffer, RfScene};

/// Ensemble identity the scene carries.
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
/// otherwise every loop seam corrupts the first super frames after it
/// (found by an AU-CRC failure once per loop in `sdr_dab_audio`).
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
    // after the seam decode as garbage. This assertion is the check that the
    // invariant holds for every source the scene carries.
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

/// The coding profile a FIC protection entry resolves to — the same
/// resolution `MscEncoder` performs, refusing a plan the standard does not
/// define rather than guessing.
fn profile_of(protection: Protection, size_cu: u16) -> Profile {
    match protection {
        Protection::Eep { option, level } => {
            Profile::Eep(EepProfile::for_size(size_cu, level, option).expect("EEP plan resolves"))
        }
        Protection::Uep { table_index } => {
            let profile: UepProfile = UEP_PROFILES[table_index as usize];
            assert_eq!(profile.size_cu, size_cu, "UEP size must be table 8's");
            Profile::Uep(profile)
        }
    }
}

/// One sub-channel's transmitter chain for a chosen byte source: energy
/// dispersal, the mother code, puncturing, zero padding, and the clause-12
/// time interleaver (table 21).
///
/// This mirrors `neowon_dsp::dab::msc::TimeInterleaver` — which is
/// `pub(crate)` and generates its payloads rather than accepting them — using
/// the public clause-12 map. It exists because the DLS carrier and the two
/// audio programmes must carry *chosen* bytes through the same FEC the
/// receiver undoes. If the encoder ever gains a payload-injection API, this
/// should call it instead.
struct ChosenChannel {
    /// Where the sub-channel sits in the CIF, in bits.
    start_bit: usize,
    profile: Profile,
    cu_bits: usize,
    /// Bytes one logical frame carries, from the profile.
    chunk: usize,
    /// The source cycles on whole logical frames, so the scene loop seam is
    /// a stream boundary too.
    source: Vec<u8>,
    cursor: usize,
    ring: Vec<u8>,
    frame: u64,
}

impl ChosenChannel {
    fn new(sub: &SubChannelSpec, source: Vec<u8>) -> Self {
        let profile = profile_of(sub.protection, sub.size_cu);
        let cu_bits = sub.size_cu as usize * CU_BITS;
        assert!(
            profile.punctured_bits() <= cu_bits,
            "the punctured codeword must fit its allocation"
        );
        // UEP has padding up to the CU boundary (table 15); EEP fills it
        // exactly. The ring write below zero-fills the padding, which is
        // what the encoder mirror transmits and the receiver depunctures.
        let chunk = profile.info_bits() / 8;
        assert!(
            !source.is_empty() && source.len().is_multiple_of(chunk),
            "the source must cycle on a whole logical frame"
        );
        Self {
            start_bit: sub.start_cu as usize * CU_BITS,
            profile,
            cu_bits,
            chunk,
            source,
            cursor: 0,
            ring: vec![0u8; cu_bits * DEINTERLEAVE_DEPTH],
            frame: 0,
        }
    }

    /// Logical frames one full cycle of the source spans.
    fn period_frames(&self) -> usize {
        self.source.len() / self.chunk
    }

    /// Encode the next logical frame's payload into the sub-channel's CU
    /// field, in transmission order.
    fn advance(&mut self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(self.chunk);
        for _ in 0..self.chunk {
            payload.push(self.source[self.cursor]);
            self.cursor = (self.cursor + 1) % self.source.len();
        }
        let mut info: Vec<u8> = payload
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |bit| (byte >> bit) & 1))
            .collect();
        energy_dispersal(&mut info);
        let mother = conv_encode(&info);
        let punctured = puncture_regions(&mother, &self.profile.regions());
        assert!(punctured.len() <= self.cu_bits);
        let r = self.frame;
        let slot = (r as usize) % DEINTERLEAVE_DEPTH;
        for i in 0..self.cu_bits {
            self.ring[slot * self.cu_bits + i] = punctured.get(i).copied().unwrap_or(0);
        }
        let mut out = vec![0u8; self.cu_bits];
        for i in 0..self.cu_bits {
            let delay = DEINTERLEAVE_MAP[i % DEINTERLEAVE_DEPTH];
            if r >= delay as u64 {
                let source = ((r - delay as u64) as usize) % DEINTERLEAVE_DEPTH;
                out[i] = self.ring[source * self.cu_bits + i];
            }
        }
        self.frame += 1;
        out
    }
}

/// A logical frame's payload: the PAD region sits at the **end**, the way
/// DAB MPEG-1 Layer II carries ancillary data (clause 7.4.0); the zero fill
/// before it is never reached by the parser.
fn frame_payload(region: &[u8], size: usize) -> Vec<u8> {
    assert!(
        region.len() <= size,
        "the PAD region must fit a logical frame"
    );
    let mut payload = vec![0u8; size - region.len()];
    payload.extend_from_slice(region);
    payload
}

/// The DLS frame program: one PAD region per logical frame. The toggle
/// alternates per message, as clause 7.4.5.2 defines (a change of message
/// inverts it), so the parser can tell messages apart.
fn dls_payloads(count: usize, size: usize) -> Vec<Vec<u8>> {
    let mut payloads = Vec::with_capacity(count);
    let mut toggle = false;
    let mut text = 0usize;
    while payloads.len() < count {
        for region in message_frames(DLS_TEXTS[text], toggle) {
            payloads.push(frame_payload(&region, size));
        }
        toggle = !toggle;
        text = (text + 1) % DLS_TEXTS.len();
    }
    payloads.truncate(count);
    payloads
}

/// One DLS message as a run of frames. Data groups are at most 16 characters
/// (8 segments per label); the first group is split across two frames so a
/// data group genuinely spans audio frames.
fn message_frames(text: &str, toggle: bool) -> Vec<Vec<u8>> {
    let segments = dls_segments(text);
    let groups: Vec<Vec<u8>> = segments
        .iter()
        .enumerate()
        .map(|(i, segment)| dls_group(toggle, i == 0, i == segments.len() - 1, i as u8, segment))
        .collect();
    let first = &groups[0];
    let split = 12.min(first.len());
    let mut regions = vec![region(&[(APP_DLS_START, &first[..split])])];
    let rest = &first[split..];
    if !rest.is_empty() {
        let mut subfields = vec![(APP_DLS_CONT, rest)];
        if let Some(second) = groups.get(1) {
            subfields.push((APP_DLS_START, second));
        }
        regions.push(region(&subfields));
    } else if let Some(second) = groups.get(1) {
        regions.push(region(&[(APP_DLS_START, second)]));
    }
    for group in groups.iter().skip(2) {
        regions.push(region(&[(APP_DLS_START, group)]));
    }
    regions
}

/// Split a message into at most 16-byte segments. The é is EBU Latin 0x82
/// (charset 0, table 47); the parser's charset decoder returns it as UTF-8.
fn dls_segments(text: &str) -> Vec<Vec<u8>> {
    let bytes: Vec<u8> = text
        .chars()
        .map(|c| if c == 'é' { 0x82 } else { c as u8 })
        .collect();
    bytes.chunks(16).map(<[u8]>::to_vec).collect()
}

/// One DLS data group: the segment header, characters and annex-E CRC.
fn dls_group(toggle: bool, first: bool, last: bool, number: u8, text: &[u8]) -> Vec<u8> {
    assert!(!text.is_empty() && text.len() <= 16);
    let mut group = vec![
        (toggle as u8) << 7 | (first as u8) << 6 | (last as u8) << 5 | (text.len() as u8 - 1),
        if first { 0x00 } else { (number & 0x07) << 4 },
    ];
    group.extend_from_slice(text);
    let crc = crc16(&group);
    group.push((crc >> 8) as u8);
    group.push(crc as u8);
    group
}

/// DLS application type 2: start of a data group (clause 7.4.3).
const APP_DLS_START: u8 = 2;
/// DLS application type 3: continuation of a data group.
const APP_DLS_CONT: u8 = 3;
/// X-PAD sub-field lengths by CI length code (clause 7.4.4.2).
const XPAD_LENGTHS: [usize; 8] = [4, 6, 8, 12, 16, 24, 32, 48];

/// One transmission-order PAD region: the X-PAD bytes reversed (clause
/// 7.4.2), then the two F-PAD bytes — type 0, variable-size indicator, CI
/// flag set.
fn region(subfields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut xpad = Vec::new();
    let mut fields = Vec::new();
    for (app, data) in subfields {
        let len = *XPAD_LENGTHS
            .iter()
            .find(|len| **len >= data.len())
            .expect("sub-field fits a length code");
        let code = XPAD_LENGTHS.iter().position(|l| *l == len).unwrap() as u8;
        xpad.push((code << 5) | app);
        fields.push((len, data.to_vec()));
    }
    xpad.push(0x00); // end marker (clause 7.4.4.2)
    for (len, data) in fields {
        xpad.extend_from_slice(&data);
        xpad.resize(xpad.len() + (len - data.len()), 0x00);
    }
    let mut region: Vec<u8> = xpad.iter().rev().copied().collect();
    region.push(0x20);
    region.push(0x02);
    region
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_codec::dabplus::{SuperframeDecoder, SuperframeHeader};
    use neowon_codec::mp2::Mp2Decoder;
    use neowon_dsp::dab::pad::PadParser;
    use neowon_dsp::dab::receiver::DabReceiver;
    use std::collections::BTreeMap;

    /// The oracle claim, end to end: the composed IQ locks the receiver,
    /// the table names the five services with both protections, the DLS
    /// carrier's labels — the multi-frame one included — come back exactly
    /// as composed, and the two audio programmes' streams reach the tier-3
    /// transports as the committed fixtures (cyclically: the scene loops).
    #[test]
    fn the_ensemble_locks_and_its_programmes_round_trip() {
        let samples = iq_samples();
        let mut rx = DabReceiver::new();
        for chunk in samples.chunks(4096) {
            rx.push_iq(chunk);
        }
        let status = rx.status();
        assert!(status.locked, "the FIC should lock: {status:?}");
        assert_eq!(status.ensemble.eid, Some(EID));
        assert_eq!(status.ensemble.label.as_deref(), Some(ENSEMBLE_LABEL));
        assert_eq!(status.ensemble.services.len(), 5);
        assert!(
            status
                .ensemble
                .sub_channels
                .values()
                .any(|s| matches!(s.protection, Protection::Eep { .. }))
        );
        assert!(
            status
                .ensemble
                .sub_channels
                .values()
                .any(|s| matches!(s.protection, Protection::Uep { .. }))
        );

        let mut parsers: BTreeMap<u8, PadParser> = BTreeMap::new();
        let mut streams: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        let mut seen: Vec<String> = Vec::new();
        for frame in rx.take_msc_frames() {
            streams
                .entry(frame.sub_channel)
                .or_default()
                .extend_from_slice(&frame.bytes);
            let parser = parsers.entry(frame.sub_channel).or_default();
            if let Some(dls) = parser.push_pad_region(&frame.bytes) {
                seen.push(dls);
            }
        }
        for text in DLS_TEXTS {
            assert!(
                seen.iter().any(|s| s == text),
                "{text:?} never decoded: {seen:?}"
            );
        }

        // DAB+ (sub-channel 3): the app's own trial-sync finds the
        // super-frame phase, and the AUs are the fixture's, cyclically.
        let dabplus = streams
            .get(&DABPLUS_SUB_CHANNEL)
            .expect("the DAB+ stream was emitted");
        let mut sync = crate::sdr::dab_audio::DabPlusSync::new(10).expect("index 10");
        let header = SuperframeHeader {
            rfa: false,
            dac_rate: true,
            sbr_flag: true,
            aac_channel_mode: false,
            ps_flag: true,
            mpeg_surround_config: 0,
        };
        let mut recovered: Vec<Vec<u8>> = Vec::new();
        for superframe in sync.push(dabplus) {
            assert!(superframe.firecode_ok);
            assert_eq!(superframe.rs_uncorrectable, 0);
            assert_eq!(superframe.header, Some(header));
            for au in &superframe.aus {
                assert!(au.crc_ok, "AU CRC");
                recovered.push(au.data.clone());
            }
        }
        assert!(
            recovered.len() >= 6,
            "only {} AUs recovered",
            recovered.len()
        );
        let fixture = fixture_access_units();
        let start = fixture
            .iter()
            .position(|au| *au == recovered[0])
            .expect("the first recovered AU is a fixture AU");
        for (i, au) in recovered.iter().enumerate() {
            assert_eq!(*au, fixture[(start + i) % fixture.len()], "AU {i}");
        }

        // MP2 (sub-channel 4): one 384-byte frame per logical frame, so the
        // emitted stream decodes frame by frame with no resync.
        let mp2 = streams
            .get(&MP2_SUB_CHANNEL)
            .expect("the MP2 stream was emitted");
        let mut decoder = Mp2Decoder::new();
        let frames = decoder.decode_all(mp2).expect("MP2 decodes");
        assert!(frames.len() >= 24, "only {} MP2 frames", frames.len());
        assert_eq!(decoder.skipped_frames(), 0, "MP2 needed no resync");
        assert!(
            frames
                .iter()
                .all(|f| f.channels == 2 && f.sample_rate == 48_000)
        );
    }

    /// The fixture's AUs as the transport frames them, for the cyclic match.
    fn fixture_access_units() -> Vec<Vec<u8>> {
        let mut decoder = SuperframeDecoder::new(10).expect("index 10");
        decoder
            .push(DABPLUS_SF)
            .into_iter()
            .flat_map(|sf| sf.aus.into_iter().map(|au| au.data))
            .collect()
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use neowon_codec::mp2::Mp2Decoder;
    use neowon_dsp::dab::receiver::DabReceiver;
    use std::collections::BTreeMap;

    /// The scene loops, so the chosen streams are replayed by the receiver:
    /// every recovered DAB+ AU must be CRC-clean and every MP2 frame must
    /// decode, across several loops. This is the regression for the clause-12
    /// warm-up transient (`WARMUP_FRAMES`) — replaying the zero-history
    /// frames corrupted one super frame per loop, which showed up as an
    /// AU-CRC failure every 1.44 s in playback.
    #[test]
    fn the_looped_scene_recovers_only_clean_aus() {
        let samples = iq_samples();
        let mut rx = DabReceiver::new();
        for _ in 0..3 {
            for chunk in samples.chunks(4096) {
                rx.push_iq(chunk);
            }
        }
        let mut streams: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        for frame in rx.take_msc_frames() {
            streams
                .entry(frame.sub_channel)
                .or_default()
                .extend_from_slice(&frame.bytes);
        }
        let mut sync = crate::sdr::dab_audio::DabPlusSync::new(10).expect("index 10");
        let mut aus = 0usize;
        for superframe in sync.push(streams.get(&DABPLUS_SUB_CHANNEL).expect("sub-channel 3")) {
            assert!(superframe.firecode_ok);
            for au in &superframe.aus {
                aus += 1;
                assert!(au.crc_ok, "AU {aus} of the looped stream failed its CRC");
            }
        }
        assert!(aus >= 60, "only {aus} AUs over three loops");

        let mut decoder = Mp2Decoder::new();
        let frames = decoder
            .decode_all(streams.get(&MP2_SUB_CHANNEL).expect("sub-channel 4"))
            .expect("MP2 decodes");
        assert_eq!(decoder.skipped_frames(), 0, "the looped MP2 needed resync");
        assert!(frames.len() >= 100, "only {} MP2 frames", frames.len());
    }
}
