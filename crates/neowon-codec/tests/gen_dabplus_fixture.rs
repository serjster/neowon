//! One-off generator for the DAB+-framed HE-AAC v2 fixture used by
//! `tests/aac_960_sbr.rs`. It rewrites files under `tests/fixtures/`;
//! run it deliberately and commit the result:
//!
//! ```sh
//! cargo test -p neowon-codec --features fdk-aac \
//!   --test gen_dabplus_fixture -- --ignored --nocapture
//! ```
//!
//! It encodes `dabplus_heaacv2_ref.s16` (the ffmpeg-generated source
//! tone) with libfdk-aac's own encoder as HE-AAC v2, frames the access
//! units into DAB+ super frames with
//! [`neowon_codec::dabplus::SuperframeEncoder`], and writes the stream
//! plus the encoder's own `AudioSpecificConfig`. libfdk-aac's encoder
//! cannot emit the 960-line transform (its `AACENC_GRANULE_LENGTH`
//! accepts 1024/512/480/256/240/128/120 only), so the fixture carries
//! the 1024-line transform and the README states exactly what that
//! proves and what it does not.
#![cfg(feature = "fdk-aac")]

mod common;

use fdk_aac::enc::{AudioObjectType, BitRate, ChannelMode, Encoder, EncoderParams, Transport};
use neowon_codec::aac::AudioSpecificConfig;
use neowon_codec::dabplus::{SuperframeEncoder, SuperframeHeader};

const REF: &str = "dabplus_heaacv2_ref.s16";
const OUT_STREAM: &str = "dabplus_heaacv2.sf";
const OUT_ASC: &str = "dabplus_heaacv2.asc";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn header() -> SuperframeHeader {
    SuperframeHeader {
        rfa: false,
        dac_rate: true,          // 48 kHz DAC
        sbr_flag: true,          // SBR
        aac_channel_mode: false, // mono core
        ps_flag: true,           // parametric stereo
        mpeg_surround_config: 0,
    }
}

#[test]
#[ignore = "one-off fixture generation; rewrites tests/fixtures"]
fn generate_dabplus_heaacv2_fixture() {
    let source = common::fixture(REF);
    let samples: Vec<i16> = source
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    let encoder = Encoder::new(EncoderParams {
        bit_rate: BitRate::Cbr(64_000),
        sample_rate: 48_000,
        transport: Transport::Raw,
        channels: ChannelMode::Stereo,
        audio_object_type: AudioObjectType::Mpeg4HeAacV2,
    })
    .expect("libfdk-aac encoder");
    let info = encoder.info().expect("encoder info");
    let asc_bytes = info.confBuf[..info.confSize as usize].to_vec();
    let asc = AudioSpecificConfig::parse(&asc_bytes).expect("parse encoder ASC");
    eprintln!(
        "encoder: {info_input} ch, frame length {info_frame}, delay {info_delay}/{info_core}, \
         max out {} bytes",
        info.maxOutBufBytes,
        info_input = info.inputChannels,
        info_frame = info.frameLength,
        info_delay = info.nDelay,
        info_core = info.nDelayCore,
    );
    eprintln!("encoder ASC {asc_bytes:02X?} -> {asc:?}");

    // Feed one encoder frame at a time, collecting raw_data_block() AUs.
    let step = info.frameLength as usize * 2;
    let mut out = vec![0u8; info.maxOutBufBytes.max(8192) as usize];
    let mut aus: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0usize;
    while pos + step <= samples.len() {
        let encoded = encoder
            .encode(&samples[pos..pos + step], &mut out)
            .expect("encode");
        assert!(encoded.input_consumed > 0, "encoder made no progress");
        pos += encoded.input_consumed;
        if encoded.output_size > 0 {
            aus.push(out[..encoded.output_size].to_vec());
        }
    }

    // Complete super frames only: 48 kHz + SBR carries 3 AUs each.
    let header = header();
    assert_eq!(header.num_aus(), 3);
    let complete = aus.len() - aus.len() % header.num_aus();
    aus.truncate(complete);
    eprintln!(
        "AUs: {complete} (max {} bytes, source consumed {pos} of {} samples)",
        aus.iter().map(Vec::len).max().unwrap_or(0),
        samples.len()
    );
    assert!(complete > 0, "not enough encoder output");

    // One super frame must hold its three AUs, their CRCs and the header.
    let fixed = aus
        .chunks(header.num_aus())
        .map(|chunk| header.first_au_start() + chunk.iter().map(|au| au.len() + 2).sum::<usize>())
        .max()
        .expect("complete group");
    let subchannel_index = fixed.div_ceil(110) as u8;
    let encoder = SuperframeEncoder::new(subchannel_index).expect("superframe encoder");
    let mut stream = Vec::new();
    for chunk in aus.chunks(header.num_aus()) {
        let refs: Vec<&[u8]> = chunk.iter().map(Vec::as_slice).collect();
        stream.extend(encoder.encode(&header, &refs).expect("frame AUs"));
    }
    eprintln!(
        "superframes: {} of {} bytes (subchannel_index {subchannel_index})",
        complete / header.num_aus(),
        stream.len()
    );

    std::fs::write(fixture_path(OUT_STREAM), &stream).expect("write stream");
    std::fs::write(fixture_path(OUT_ASC), &asc_bytes).expect("write ASC");
    eprintln!("wrote {OUT_STREAM} and {OUT_ASC}");
}
