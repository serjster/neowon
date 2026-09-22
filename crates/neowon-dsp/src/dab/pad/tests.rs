//! Unit tests for the PAD/DLS parser (see `super`).
use super::*;

/// One DLS data group (segment): header, characters, CRC.
fn dls_group(toggle: bool, first: bool, last: bool, number: u8, text: &[u8]) -> Vec<u8> {
    assert!(text.len() <= 16);
    let mut group = vec![0u8; 2];
    group[0] =
        (toggle as u8) << 7 | (first as u8) << 6 | (last as u8) << 5 | (text.len() as u8 - 1);
    group[1] = if first {
        0x00 // charset 0
    } else {
        (number & 0x07) << 4
    };
    group.extend_from_slice(text);
    let crc = crc16(&group);
    group.push((crc >> 8) as u8);
    group.push(crc as u8);
    group
}

/// Assemble a variable-size X-PAD field from `(app_type, data)` sub-fields,
/// padding each to a legal length code. Returns the logical X-PAD. A list
/// of fewer than four indicators is closed with an end marker, as clause
/// 7.4.2.2 requires.
fn variable_xpad(subfields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut xpad = Vec::new();
    let mut chosen = Vec::new();
    for (app, data) in subfields {
        let len = *XPAD_LEN_TABLE
            .iter()
            .find(|len| **len >= data.len())
            .expect("sub-field fits a length code");
        let code = XPAD_LEN_TABLE.iter().position(|l| *l == len).unwrap() as u8;
        chosen.push((*app, len, data.to_vec()));
        xpad.push((code << 5) | app);
    }
    if chosen.len() < 4 {
        xpad.push(0x00);
    }
    for (_, len, data) in chosen {
        xpad.extend_from_slice(&data);
        xpad.resize(xpad.len() + (len - data.len()), 0x00);
    }
    xpad
}

/// Wrap a logical X-PAD field in a transmission-order PAD region: the
/// X-PAD reversed, then the two F-PAD bytes (`X-PAD Ind = 10`, CI flag set
/// unless `ci_flag` is false).
fn pad_region(xpad: &[u8], ci_flag: bool) -> Vec<u8> {
    let mut region: Vec<u8> = xpad.iter().rev().copied().collect();
    region.push(0x20); // F-PAD type 0, variable-size X-PAD indicator
    region.push(if ci_flag { 0x02 } else { 0x00 });
    region
}

/// A single-segment label survives the variable-size X-PAD path, and is
/// not republished unchanged.
#[test]
fn single_segment_round_trips() {
    let group = dls_group(true, true, true, 0, b"HELLO");
    let mut parser = PadParser::new();
    let xpad = variable_xpad(&[(APP_DLS_START, &group)]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&xpad, true)),
        Some("HELLO".to_string())
    );
    assert_eq!(parser.dls(), Some("HELLO"));
    // The same message again is a repeat, not an update.
    assert_eq!(parser.push_pad_region(&pad_region(&xpad, true)), None);
    assert_eq!(parser.dls(), Some("HELLO"));
}

/// The EBU Latin charset reaches DLS: a label with accented characters is
/// exact, not `?`-mangled (charset 0, table 47).
#[test]
fn dls_localises_ebu_latin() {
    // "Café" with 0x82 for é, and a 0x0A line break that becomes a newline.
    let text = [b'C', b'a', b'f', 0x82, 0x0A, b'O', b'l', 0x82];
    let group = dls_group(true, true, true, 0, &text);
    let mut parser = PadParser::new();
    let xpad = variable_xpad(&[(APP_DLS_START, &group)]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&xpad, true)),
        Some("Café\nOlé".to_string())
    );
}

/// Row 12: a multi-segment label where one segment's data group spans two
/// frames. Segment 0 is split 12 + 8 bytes across frames 0 and 1; segment 1
/// arrives in frame 1 and the short segment 2 in frame 2. Nothing is
/// published until the last segment is clean.
#[test]
fn multi_segment_dls_spans_two_frames() {
    // The é is EBU Latin 0x82, not its UTF-8 encoding: charset 0.
    let segments: [Vec<u8>; 3] = [
        b"Now playing: Caf".to_vec(),
        [&[0x82u8][..], b" - Perfect "].concat(),
        b"Day".to_vec(),
    ];
    let text = "Now playing: Café - Perfect Day";
    let groups: Vec<Vec<u8>> = segments
        .iter()
        .enumerate()
        .map(|(i, s)| dls_group(true, i == 0, i == segments.len() - 1, i as u8, s.as_slice()))
        .collect();

    let mut parser = PadParser::new();
    // Frame 0: the start of segment 0 (12 of its 20 bytes).
    let frame0 = variable_xpad(&[(APP_DLS_START, &groups[0][..12])]);
    assert_eq!(parser.push_pad_region(&pad_region(&frame0, true)), None);
    // Frame 1: the rest of segment 0, then all of segment 1.
    let frame1 = variable_xpad(&[
        (APP_DLS_CONT, &groups[0][12..]),
        (APP_DLS_START, &groups[1]),
    ]);
    assert_eq!(parser.push_pad_region(&pad_region(&frame1, true)), None);
    assert_eq!(parser.dls(), None, "no label until the last segment");
    // Frame 2: the last segment, padded inside its sub-field.
    let frame2 = variable_xpad(&[(APP_DLS_START, &groups[2])]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&frame2, true)),
        Some(text.to_string())
    );
    assert_eq!(parser.dls(), Some(text));
}

/// The short X-PAD path (4 bytes per frame, CI only when the group starts
/// or resumes) carries the same 20-byte segment over six frames.
#[test]
fn short_xpad_carries_a_segment_across_frames() {
    let group = dls_group(true, true, true, 0, b"SHORT X-PAD TEXT");
    assert_eq!(group.len(), 20);
    let mut parser = PadParser::new();
    let mut published = None;
    let mut at = 0usize;
    let mut frame = 0;
    while at < group.len() {
        let (xpad, ci_flag) = if frame == 0 {
            // First frame: one CI byte + the first 3 data bytes.
            let mut xpad = vec![0x02u8];
            xpad.extend_from_slice(&group[at..at + 3]);
            at += 3;
            (xpad, true)
        } else {
            // Continuation: up to 4 data bytes, zero-padded to fill the
            // sub-field as clause 7.4.2.1 requires.
            let take = (group.len() - at).min(4);
            let mut xpad = group[at..at + take].to_vec();
            xpad.resize(4, 0x00);
            at += take;
            (xpad, false)
        };
        let mut region: Vec<u8> = xpad.iter().rev().copied().collect();
        region.push(0x10); // F-PAD type 0, short X-PAD
        region.push(if ci_flag { 0x02 } else { 0x00 });
        published = parser.push_pad_region(&region).or(published);
        frame += 1;
    }
    assert!(frame >= 6, "the segment should span frames, took {frame}");
    assert_eq!(published.as_deref(), Some("SHORT X-PAD TEXT"));
}

/// A CRC-damaged group is dropped, never published, and the partial
/// reassembly behind it is discarded.
#[test]
fn a_bad_crc_never_publishes() {
    let mut group = dls_group(true, true, true, 0, b"GOODBYE");
    let last = group.len() - 1;
    group[last] ^= 0xFF;
    let mut parser = PadParser::new();
    let xpad = variable_xpad(&[(APP_DLS_START, &group)]);
    assert_eq!(parser.push_pad_region(&pad_region(&xpad, true)), None);
    assert_eq!(parser.dls(), None);
    assert_eq!(parser.groups_dropped, 1);

    // A clean label afterwards still publishes.
    let good = dls_group(false, true, true, 0, b"HELLO");
    let xpad = variable_xpad(&[(APP_DLS_START, &good)]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&xpad, true)),
        Some("HELLO".to_string())
    );
}

/// A toggle change abandons the previous message's segments rather than
/// gluing two labels together.
#[test]
fn toggle_change_clears_the_partial_message() {
    let mut parser = PadParser::new();
    let first = dls_group(true, true, false, 0, b"OLD MESSAGE PART");
    let xpad = variable_xpad(&[(APP_DLS_START, &first)]);
    assert_eq!(parser.push_pad_region(&pad_region(&xpad, true)), None);
    // A new message (toggle inverted) completes immediately on its own.
    let new = dls_group(false, true, true, 0, b"NEW");
    let xpad = variable_xpad(&[(APP_DLS_START, &new)]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&xpad, true)),
        Some("NEW".to_string())
    );
}

/// Unknown applications are skipped by their declared length and cannot
/// disturb the DLS group beside them (D27).
#[test]
fn unknown_applications_are_skipped_by_length() {
    let group = dls_group(true, true, true, 0, b"MIXED");
    let mut parser = PadParser::new();
    let xpad = variable_xpad(&[(9, b"USER DATA"), (APP_DLS_START, &group), (12, b"MOT!")]);
    assert_eq!(
        parser.push_pad_region(&pad_region(&xpad, true)),
        Some("MIXED".to_string())
    );
}

/// Reserved F-PAD types and a reserved X-PAD indicator parse nothing.
#[test]
fn reserved_fpad_is_refused() {
    let group = dls_group(true, true, true, 0, b"NO");
    let xpad = variable_xpad(&[(APP_DLS_START, &group)]);
    let mut region: Vec<u8> = xpad.iter().rev().copied().collect();
    region.push(0x40); // F-PAD type 1, reserved
    region.push(0x02);
    let mut parser = PadParser::new();
    assert_eq!(parser.push_pad_region(&region), None);
    assert_eq!(parser.dls(), None);
}
