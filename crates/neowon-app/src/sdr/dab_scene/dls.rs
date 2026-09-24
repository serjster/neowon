//! The scene's DLS programme: the song/artist labels as X-PAD data groups,
//! one PAD region per logical frame, at the end of each frame's payload.

use neowon_dsp::dab::fec::crc16;

use super::DLS_TEXTS;

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
pub(super) fn dls_payloads(count: usize, size: usize) -> Vec<Vec<u8>> {
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
