//! Programme Associated Data: F-PAD, X-PAD, and the dynamic label (DLS).
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **7.4.1** the F-PAD: two bytes at the end of each audio frame's PAD
//!   region. Byte L-1 carries the F-PAD type (bits 7-6), the X-PAD indicator
//!   (bits 5-4: none / short / variable) and the Byte L indicator; Byte L
//!   carries six bits of data, the CI flag (bit 1) and the `Z` bit.
//! - **7.4.2** the X-PAD field, whose bytes are **reversed before
//!   transmission**, so a receiver reverses them back; the contents
//!   indicators come first in logical order, then the data sub-fields.
//! - **7.4.3/7.4.4** application types and contents indicators: short X-PAD is
//!   4 bytes (one 1-byte CI + 3 data, or 4 data continuing the previous
//!   application); variable-size X-PAD carries up to four CI bytes, each with a
//!   length code (4, 6, 8, 12, 16, 24, 32 or 48 bytes) and an application type,
//!   terminated by application type 0.
//! - **7.4.5.0** the data-group CRC: annex E's CRC-16, all-ones init,
//!   complemented. Every DLS data group carries one; a group that fails it is
//!   dropped, never published.
//! - **7.4.5.2** the dynamic label: up to 8 segments of up to 16 characters,
//!   each segment one X-PAD data group with `T | First | Last | C | Length`,
//!   `Field 2` (charset for the first segment, `Rfa | SegNum` otherwise) and
//!   `Field 3`, then the characters and the CRC. Application type 2 starts a
//!   data group, type 3 continues one; a group may span several audio frames.
//!
//! The 2-byte F-PAD + X-PAD layout is the same for DAB's MPEG-1 Layer II audio
//! (the PAD region at the end of each MPEG frame) and DAB+ (the in-band PAD at
//! the start of each AAC access unit). Tier 2 does not own either transport —
//! it takes the PAD region, transmission order and F-PAD last, via
//! [`PadParser::push_pad_region`] — so the same parser serves both when tier 3
//! wires them up.
//!
//! **Honesty (D27).** Unknown applications are skipped by their declared length
//! and never parsed; a DLS string is published only when every segment from the
//! first to the last reassembled cleanly and each data group's CRC passed. A
//! partial or corrupt label is never shown.
//!
//! Portions of this module follow the MIT-licensed reference `dabradio` 0.5.0
//! (`src/pad/mod.rs`) for the DLS segment structure and the reassembly rules;
//! the notice is recorded in `docs/protocol-dab.md`.

// Ported from dabradio 0.5.0 (MIT); notice in docs/protocol-dab.md
use std::collections::BTreeMap;

use super::charset;
use super::fec::crc16;

/// X-PAD sub-field length by CI length code (clause 7.4.4.2).
const XPAD_LEN_TABLE: [usize; 8] = [4, 6, 8, 12, 16, 24, 32, 48];
/// Application type 2: DLS segment, start of data group (clause 7.4.3).
const APP_DLS_START: u8 = 2;
/// Application type 3: DLS segment, continuation of data group.
const APP_DLS_CONT: u8 = 3;
/// The dynamic label carries at most 8 segments (clause 7.4.5.2).
const MAX_DLS_SEGMENTS: u8 = 8;
/// The dynamic label is at most 128 characters (clause 7.4.5.2).
const MAX_DLS_CHARS: usize = 128;

/// One contents indicator: an application type and the sub-field length it
/// announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentsIndicator {
    app_type: u8,
    len: usize,
}

/// One reassembled DLS segment.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DlsSegment {
    toggle: bool,
    first: bool,
    last: bool,
    /// Segment number: 0 for the first segment, otherwise the standard's
    /// `SegNum` (sequence number minus one).
    number: u8,
    /// Character set, carried by the first segment only.
    charset: u8,
    chars: Vec<u8>,
}

impl DlsSegment {
    fn key(&self) -> u8 {
        if self.first { 0 } else { self.number }
    }
}

/// The PAD/DLS parser state.
#[derive(Debug, Default)]
pub struct PadParser {
    /// The DLS data group being accumulated (header + characters + CRC, at
    /// most one segment's worth).
    group: Vec<u8>,
    group_active: bool,
    segments: BTreeMap<u8, DlsSegment>,
    toggle: Option<bool>,
    dls: Option<String>,
    /// The last application type seen, for CI-less continuation sub-fields.
    last_app: Option<ContentsIndicator>,
    /// Counters for the honest failures: CRC-rejected groups and DLS commands
    /// this tier does not interpret.
    pub groups_dropped: u64,
    pub commands_ignored: u64,
}

/// What one X-PAD field turned out to be.
enum XpadShape {
    /// Contents indicators followed by that many data sub-fields; `data_at` is
    /// where the sub-fields start (the CI list includes its end marker).
    Data {
        indicators: Vec<ContentsIndicator>,
        data_at: usize,
    },
    /// No CI flag: every byte continues the last application seen.
    Continuation,
    /// An end marker: the X-PAD field carries no data (clause 7.4.3).
    Nothing,
}

impl PadParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one audio frame's PAD region in **transmission order**: the X-PAD
    /// bytes (as transmitted, i.e. reversed) followed by the two F-PAD bytes.
    /// Returns the DLS string the frame completed, if any.
    ///
    /// The two placements this serves (clause 7.4.0): the trailing PAD bytes of
    /// a DAB MPEG-1 Layer II frame, and the data bytes of the DAB+ in-band PAD
    /// element.
    pub fn push_pad_region(&mut self, region: &[u8]) -> Option<String> {
        if region.len() < 2 {
            return None;
        }
        let split = region.len() - 2;
        let fpad = [region[split], region[split + 1]];
        // Clause 7.4.2: the byte order within the X-PAD field is reversed
        // before transmission.
        let xpad: Vec<u8> = region[..split].iter().rev().copied().collect();
        self.push_xpad(&xpad, fpad)
    }

    /// Feed a logical-order X-PAD field and the two F-PAD bytes.
    pub fn push_xpad(&mut self, xpad: &[u8], fpad: [u8; 2]) -> Option<String> {
        let fpad_type = fpad[0] >> 6;
        let xpad_indicator = (fpad[0] >> 4) & 0x03;
        let ci_flag = fpad[1] & 0x02 != 0;
        // F-PAD types 1..3 are reserved (clause 7.4.1); X-PAD indicator 0 means
        // no X-PAD; 3 is reserved. Nothing here is guessed.
        if fpad_type != 0 || xpad_indicator == 0 || xpad_indicator == 3 {
            return None;
        }
        let shape = self.parse_indicators(xpad, xpad_indicator, ci_flag);
        let (indicators, data_at) = match shape {
            XpadShape::Nothing => return None,
            XpadShape::Continuation => {
                // A CI-less field continues the previous application; that is
                // only meaningful if it was a DLS continuation.
                let last = self.last_app?;
                if last.app_type != APP_DLS_CONT {
                    return None;
                }
                self.continue_group(xpad);
                return self.extract_groups();
            }
            XpadShape::Data {
                indicators,
                data_at,
            } => (indicators, data_at),
        };
        let mut offset = data_at;
        let mut completed = None;
        for indicator in &indicators {
            if offset + indicator.len > xpad.len() {
                // The frame is malformed; do not consume a truncated sub-field.
                return completed;
            }
            let data = &xpad[offset..offset + indicator.len];
            offset += indicator.len;
            match indicator.app_type {
                APP_DLS_START => {
                    self.start_group(data);
                    completed = self.extract_groups().or(completed);
                    // A start is followed by continuations of the same group.
                    self.last_app = Some(ContentsIndicator {
                        app_type: APP_DLS_CONT,
                        len: indicator.len,
                    });
                }
                APP_DLS_CONT => {
                    self.continue_group(data);
                    completed = self.extract_groups().or(completed);
                    self.last_app = Some(*indicator);
                }
                // Application type 1 (data group length indicator for MOT),
                // 4..=11 and 16..=30 (user-defined) and 12..=15 (MOT) are
                // legal but not this tier's; skipped by length (D27).
                _ => {}
            }
        }
        completed
    }

    /// The last complete, CRC-clean DLS string, if one was published.
    pub fn dls(&self) -> Option<&str> {
        self.dls.as_deref()
    }

    /// Forget the label and every partial segment (a retune, or a reset).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Classify the field and resolve its contents indicators.
    fn parse_indicators(&self, xpad: &[u8], xpad_indicator: u8, ci_flag: bool) -> XpadShape {
        if xpad_indicator == 0b01 {
            // Short X-PAD: exactly four bytes (clause 7.4.2.1).
            if xpad.len() < 4 {
                return XpadShape::Nothing;
            }
            if !ci_flag {
                return XpadShape::Continuation;
            }
            let app_type = xpad[0] & 0x1F;
            if app_type == 0 {
                return XpadShape::Nothing;
            }
            XpadShape::Data {
                indicators: vec![ContentsIndicator { app_type, len: 3 }],
                data_at: 1,
            }
        } else {
            // Variable-size X-PAD: up to four CIs, terminated by type 0
            // (clauses 7.4.2.2, 7.4.4.2).
            if !ci_flag {
                return XpadShape::Continuation;
            }
            let mut indicators = Vec::new();
            let mut at = 0usize;
            let mut terminated = false;
            while at < 4 && at < xpad.len() {
                let byte = xpad[at];
                let app_type = byte & 0x1F;
                if app_type == 0 {
                    terminated = true;
                    break;
                }
                let len = XPAD_LEN_TABLE[(byte >> 5) as usize];
                indicators.push(ContentsIndicator { app_type, len });
                at += 1;
            }
            if indicators.is_empty() {
                XpadShape::Nothing
            } else {
                XpadShape::Data {
                    indicators,
                    // A list shorter than four bytes carries an end marker
                    // (clause 7.4.2.2), which the data starts after.
                    data_at: at + usize::from(terminated),
                }
            }
        }
    }

    /// Start a new DLS data group: the previous partial group is abandoned
    /// (the standard's segmentation allows interruption by other
    /// applications, and a new start means the old one is not being resumed).
    fn start_group(&mut self, data: &[u8]) {
        self.group.clear();
        self.group.extend_from_slice(data);
        self.group_active = true;
        self.check_group_size();
    }

    /// Append to the active DLS data group. A continuation with no active
    /// group cannot be placed and is dropped (it would be a guess).
    fn continue_group(&mut self, data: &[u8]) {
        if !self.group_active {
            return;
        }
        self.group.extend_from_slice(data);
        self.check_group_size();
    }

    /// A segment's data group is at most 2 + 16 + 2 bytes, but a sub-field can
    /// carry it plus zero padding (up to 48 bytes), so the accumulator is
    /// bounded a little above one group. `extract_groups` drains complete
    /// groups and clears zero padding; anything larger than this means the
    /// segmentation is not understood and is dropped.
    fn check_group_size(&mut self) {
        if self.group.len() > 68 {
            self.drop_group();
            self.groups_dropped += 1;
        }
    }

    fn drop_group(&mut self) {
        self.group.clear();
        self.group_active = false;
    }

    /// Extract every complete DLS data group from the accumulator, newest
    /// first. A group with a bad CRC drops the accumulator and the partial
    /// label (D27).
    fn extract_groups(&mut self) -> Option<String> {
        loop {
            if !self.group_active || self.group.len() < 2 {
                return None;
            }
            let first = self.group[0];
            let command = first & 0x10 != 0;
            if command {
                // Commands are their own data group; only the clear-display
                // command (1) is defined in EN 300 401, as exactly four bytes.
                if self.group.len() < 4 {
                    return None;
                }
                let stored = ((self.group[2] as u16) << 8) | self.group[3] as u16;
                if crc16(&self.group[..2]) != stored {
                    self.drop_group();
                    self.groups_dropped += 1;
                    return None;
                }
                let command = first & 0x0F;
                self.group.drain(..4);
                if command == 1 {
                    // Clear display: drop the label and every partial segment.
                    self.segments.clear();
                    self.toggle = None;
                    self.dls = None;
                } else {
                    // DL Plus has a length defined outside EN 300 401 and is
                    // not interpreted; the accumulator cannot be safely
                    // re-split, so it is dropped rather than guessed.
                    self.drop_group();
                    self.commands_ignored += 1;
                }
                if self.group.iter().all(|b| *b == 0) {
                    self.drop_group();
                }
                continue;
            }

            let field_len = (first & 0x0F) as usize + 1;
            let total = 2 + field_len + 2;
            if self.group.len() < total {
                return None;
            }
            let stored = ((self.group[total - 2] as u16) << 8) | self.group[total - 1] as u16;
            if crc16(&self.group[..total - 2]) != stored {
                self.drop_group();
                self.groups_dropped += 1;
                // A bad group invalidates the partial label it belongs to.
                self.segments.clear();
                self.toggle = None;
                return None;
            }
            let segment = self.parse_segment(&self.group[..total]);
            self.group.drain(..total);
            if self.group.iter().all(|b| *b == 0) {
                // Zero padding filling the rest of the sub-field.
                self.drop_group();
            }
            if let Some(text) = self.add_segment(segment) {
                return Some(text);
            }
        }
    }

    /// Parse one CRC-clean data group into a segment (clause 7.4.5.2).
    fn parse_segment(&self, group: &[u8]) -> DlsSegment {
        let first_byte = group[0];
        let second = group[1];
        let first = first_byte & 0x40 != 0;
        let number = if first { 0 } else { (second >> 4) & 0x07 };
        let charset = if first { (second >> 4) & 0x0F } else { 0 };
        DlsSegment {
            toggle: first_byte & 0x80 != 0,
            first,
            last: first_byte & 0x20 != 0,
            number,
            charset,
            chars: group[2..group.len() - 2].to_vec(),
        }
    }

    /// Add a segment to the reassembler and return the label when every
    /// segment from the first to the last is present.
    fn add_segment(&mut self, segment: DlsSegment) -> Option<String> {
        if segment.number >= MAX_DLS_SEGMENTS {
            // Beyond the standard's 8 segments: not a label we can assemble.
            self.segments.clear();
            self.toggle = None;
            return None;
        }
        if !segment.first && segment.number == 0 {
            // SegNum 0 is reserved by clause 7.4.5.2; only the First flag may
            // claim the first segment.
            return None;
        }
        if self.toggle != Some(segment.toggle) {
            // Clause 7.4.5.2: the toggle inverts when the message changes, so
            // cached segments belong to the previous message.
            self.segments.clear();
            self.toggle = Some(segment.toggle);
        }
        let key = segment.key();
        self.segments.entry(key).or_insert(segment);

        let last_key = self.segments.values().find(|s| s.last).map(|s| s.key())?;
        for key in 0..=last_key {
            if !self.segments.contains_key(&key) {
                return None;
            }
        }
        let charset = self.segments[&0].charset;
        let mut raw = Vec::new();
        for key in 0..=last_key {
            raw.extend_from_slice(&self.segments[&key].chars);
        }
        if raw.len() > MAX_DLS_CHARS {
            self.segments.clear();
            self.toggle = None;
            return None;
        }
        let text = charset::decode_dls(&raw, charset);
        if self.dls.as_deref() == Some(text.as_str()) {
            // A repeated label is not an update (the toggle bit says so too).
            return None;
        }
        self.dls = Some(text.clone());
        Some(text)
    }
}

#[cfg(test)]
mod tests;
