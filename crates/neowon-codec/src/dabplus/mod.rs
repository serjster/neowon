//! DAB+ audio transport — ETSI TS 102 563 V2.1.1.
//!
//! A DAB+ sub-channel carries **audio super frames** of 120 ms. Each
//! super frame is `subchannel_index × 110` bytes of data ([`110`] bytes
//! per 8 kbit/s of sub-channel) protected by a systematic RS(120,110)
//! code and a byte-wise virtual interleaver, and transmitted in five
//! consecutive DAB logical frames (TS 102 563 clauses 5.1 and 6).
//!
//! ```text
//!  110·s data bytes                 10·s parity bytes
//!  ┌───────────────────────┐       ┌──────────────┐
//!  │ header │ AU0 │ AU1 │ … │  RS   │ P[0] │ … │   each row is one
//!  └───────────────────────┘       └──────────────┘   RS(120,110) word:
//!    C[i][j] = A[i + j·s]  (clause 6.2)                 A[i], A[i+s], …
//! ```
//!
//! The super frame header carries the audio parameters (`dac_rate`,
//! `sbr_flag`, `aac_channel_mode`, `ps_flag`, `mpeg_surround_config`),
//! one 12-bit start offset per AU after the first, and a Fire-code check
//! word over bytes 2..10 (clause 5.2). Every AU ends with the CRC-16 of
//! EN 300 401 annex E. The AudioSpecificConfig the codec adapter needs
//! is derived from those header parameters (clause 7.2); see
//! [`crate::aac::AudioSpecificConfig::for_dabplus`].
//!
//! [`SuperframeDecoder`] consumes sub-channel bytes (in transmission
//! order, starting at a super frame boundary) and emits
//! [`DecodedSuperframe`]s. [`SuperframeEncoder`] is the inverse, used by
//! tests and the sim oracle. The in-band PAD of an AU is extracted by
//! [`extract_pad`] per clause 5.4.3; the PAD field itself is decoded in
//! `neowon-dsp`'s PAD/DLS parser.

mod crc;
mod rs;

pub use crc::{FIRE_CODE_POLY, crc16, fire_code};
pub use rs::{PRIMITIVE_POLY, RS_K, RS_N, RS_PARITY, RS_T, Rs120_110, RsError};

use crate::Error;

/// Data bytes contributed to a super frame per unit of sub-channel
/// index — TS 102 563 clause 5.1: `audio_super_frame_size = s×110`.
pub const SUPERFRAME_DATA_BYTES_PER_INDEX: usize = 110;

/// A super frame is carried in five consecutive DAB logical frames —
/// TS 102 563 clause 5.1.
pub const LOGICAL_FRAMES_PER_SUPERFRAME: usize = 5;

/// `subchannel_index` is 1..=24 (sub-channels of 8..192 kbit/s) —
/// TS 102 563 clause 5.1 note.
pub const MAX_SUBCHANNEL_INDEX: u8 = 24;

/// `element_instance_tag`-carrying data-stream element id in an AAC
/// `raw_data_block()` — ISO/IEC 14496-3 clause 4.4.2.5 (`id_syn_ele`
/// `0b100`).
const ID_DSE: u32 = 0b100;

/// The audio parameters of a super frame header — TS 102 563 clause
/// 5.2, Table 2 (the `rfa` through `mpeg_surround_config` fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperframeHeader {
    /// Reserved for future additions; shall be zero (clause 5.2).
    pub rfa: bool,
    /// DAC sampling rate: `false` = 32 kHz, `true` = 48 kHz
    /// (Table 3).
    pub dac_rate: bool,
    /// SBR used: `false` = no, `true` = yes (Table 4).
    pub sbr_flag: bool,
    /// AAC core coding: `false` = mono, `true` = stereo (Table 5).
    pub aac_channel_mode: bool,
    /// Parametric Stereo used (only with SBR + mono, Table 6).
    pub ps_flag: bool,
    /// MPEG Surround configuration (Table 7; `0` = not used).
    pub mpeg_surround_config: u8,
}

impl SuperframeHeader {
    /// Decode the eight audio-parameter bits (super frame byte 2).
    #[must_use]
    pub fn from_params(byte: u8) -> Self {
        Self {
            rfa: byte & 0x80 != 0,
            dac_rate: byte & 0x40 != 0,
            sbr_flag: byte & 0x20 != 0,
            aac_channel_mode: byte & 0x10 != 0,
            ps_flag: byte & 0x08 != 0,
            mpeg_surround_config: byte & 0x07,
        }
    }

    /// The eight audio-parameter bits as they appear at super frame
    /// byte 2 (MSb first, Table 2).
    #[must_use]
    pub fn to_params_byte(self) -> u8 {
        u8::from(self.rfa) << 7
            | u8::from(self.dac_rate) << 6
            | u8::from(self.sbr_flag) << 5
            | u8::from(self.aac_channel_mode) << 4
            | u8::from(self.ps_flag) << 3
            | (self.mpeg_surround_config & 0x07)
    }

    /// Number of AUs per super frame — clause 5.2, Table 2.
    #[must_use]
    pub fn num_aus(&self) -> usize {
        match (self.dac_rate, self.sbr_flag) {
            (false, true) => 2,
            (true, true) => 3,
            (false, false) => 4,
            (true, false) => 6,
        }
    }

    /// DAC sampling rate in Hz — clause 5.2, Table 3.
    #[must_use]
    pub fn dac_rate_hz(&self) -> u32 {
        if self.dac_rate { 48_000 } else { 32_000 }
    }

    /// AAC core sampling rate in Hz — clause 5.2, Table 4: half the DAC
    /// rate when SBR is used.
    #[must_use]
    pub fn core_rate_hz(&self) -> u32 {
        if self.sbr_flag {
            self.dac_rate_hz() / 2
        } else {
            self.dac_rate_hz()
        }
    }

    /// `au_start[0]`: the first AU always starts immediately after the
    /// header — clause 5.2, Table 8.
    #[must_use]
    pub fn first_au_start(&self) -> usize {
        match self.num_aus() {
            2 => 5,
            3 => 6,
            4 => 8,
            6 => 11,
            _ => unreachable!("num_aus is one of 2, 3, 4, 6"),
        }
    }
}

/// One access unit as carried in a super frame: its bytes, its CRC
/// verdict and the PAD field it carries (clause 5.4.1: PAD in `au[n]`
/// belongs to the audio in `au[n+1]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    /// The `raw_data_block()` bytes, including the PAD-carrying
    /// `data_stream_element()` when present.
    pub data: Vec<u8>,
    /// The 16-bit CRC of clause 5.2 matched the AU bytes.
    pub crc_ok: bool,
    /// The PAD field (F-PAD last, as transmitted); empty when absent.
    pub pad: Vec<u8>,
}

/// A decoded super frame: header verdict, RS outcome and the AUs (if
/// the header was usable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSuperframe {
    /// Audio parameters, present only when the Fire code check passed
    /// (and therefore only when the header bytes themselves are clean).
    pub header: Option<SuperframeHeader>,
    /// `header_firecode` matched (clause 5.2). Error detection only.
    pub firecode_ok: bool,
    /// Byte positions corrected by RS across the `s` codewords.
    pub rs_corrected: usize,
    /// Codewords RS could not correct. A non-zero count means at least
    /// one AU will likely fail its own CRC; use [`AccessUnit::crc_ok`].
    pub rs_uncorrectable: usize,
    /// The AUs, empty when the header was not usable or the start
    /// offsets failed their sanity check.
    pub aus: Vec<AccessUnit>,
}

/// Stateful decoder from DAB+ sub-channel bytes to super frames.
///
/// `subchannel_index` fixes the frame size (`120×s` bytes over five
/// logical frames). The caller feeds bytes in transmission order,
/// starting at a super frame boundary (finding that boundary is the
/// receiver's synchronisation job, TS 102 563 annex C); any chunking is
/// accepted. One RS codec is built once and reused.
#[derive(Debug)]
pub struct SuperframeDecoder {
    subchannel_index: u8,
    rs: Rs120_110,
    buffer: Vec<u8>,
}

impl SuperframeDecoder {
    /// Create a decoder for one sub-channel size.
    pub fn new(subchannel_index: u8) -> Result<Self, Error> {
        if subchannel_index == 0 || subchannel_index > MAX_SUBCHANNEL_INDEX {
            return Err(Error::InvalidSubchannelIndex(subchannel_index));
        }
        Ok(Self {
            subchannel_index,
            rs: Rs120_110::new(),
            buffer: Vec::new(),
        })
    }

    /// The configured sub-channel index.
    #[must_use]
    pub fn subchannel_index(&self) -> u8 {
        self.subchannel_index
    }

    /// Protected super frame size in bytes: `120 × subchannel_index`
    /// (TS 102 563 clauses 5.1 and 6).
    #[must_use]
    pub fn superframe_bytes(&self) -> usize {
        RS_N * usize::from(self.subchannel_index)
    }

    /// Unprotected data size in bytes: `110 × subchannel_index`.
    #[must_use]
    pub fn data_bytes(&self) -> usize {
        SUPERFRAME_DATA_BYTES_PER_INDEX * usize::from(self.subchannel_index)
    }

    /// Bytes contributed by one 24 ms logical frame: `24 × s` (§6.2 /
    /// §6.5 — one logical frame is 24 of the 120 interleaver columns).
    #[must_use]
    pub fn logical_frame_bytes(&self) -> usize {
        self.superframe_bytes() / LOGICAL_FRAMES_PER_SUPERFRAME
    }

    /// Push sub-channel bytes; returns every super frame the buffer now
    /// completes, in order.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<DecodedSuperframe> {
        self.buffer.extend_from_slice(bytes);
        let size = self.superframe_bytes();
        let mut out = Vec::new();
        while self.buffer.len() >= size {
            let raw: Vec<u8> = self.buffer.drain(..size).collect();
            out.push(self.decode_superframe(&raw));
        }
        out
    }

    /// RS-decode `raw` (clauses 6.1–6.5), then parse the header and AUs.
    fn decode_superframe(&self, raw: &[u8]) -> DecodedSuperframe {
        let s = usize::from(self.subchannel_index);
        let capacity = self.data_bytes();
        let mut data = raw[..capacity].to_vec();
        let mut rs_corrected = 0usize;
        let mut rs_uncorrectable = 0usize;

        // Row i of the coding array is the codeword
        // [A[i], A[i+s], …] ++ [P[i][0..10]] (clauses 6.2–6.4).
        for row in 0..s {
            let mut word = [0u8; RS_N];
            for (j, byte) in word.iter_mut().take(RS_K).enumerate() {
                *byte = data[row + j * s];
            }
            for (r, byte) in word.iter_mut().skip(RS_K).enumerate() {
                *byte = raw[capacity + r * s + row];
            }
            match self.rs.decode(&mut word, &[]) {
                Ok(corrected) => {
                    rs_corrected += corrected;
                    for (j, byte) in word.iter().take(RS_K).enumerate() {
                        data[row + j * s] = *byte;
                    }
                }
                Err(_) => rs_uncorrectable += 1,
            }
        }

        // Fire code over bytes 2..10 (clause 5.2, Table 2). The buffer is
        // always at least 110 bytes, so the slice exists.
        let firecode_ok = u16::from_be_bytes([data[0], data[1]]) == fire_code(&data[2..11]);
        let header = firecode_ok.then(|| SuperframeHeader::from_params(data[2]));
        let aus = match header {
            Some(header) => extract_aus(&data, &header).unwrap_or_default(),
            None => Vec::new(),
        };
        DecodedSuperframe {
            header,
            firecode_ok,
            rs_corrected,
            rs_uncorrectable,
            aus,
        }
    }
}

/// Split a corrected super frame's data region into access units.
///
/// Returns `None` when an `au_start` fails the clause-5.2 sanity rules
/// (all offsets between `au_start[0]` and the super frame size, strictly
/// increasing, each region at least the 2 CRC bytes).
fn extract_aus(data: &[u8], header: &SuperframeHeader) -> Option<Vec<AccessUnit>> {
    let num_aus = header.num_aus();
    let size = data.len();
    let mut starts = Vec::with_capacity(num_aus + 1);
    starts.push(header.first_au_start());
    let mut bit = 24;
    for _ in 1..num_aus {
        starts.push(read_bits(data, bit, 12)? as usize);
        bit += 12;
    }
    starts.push(size);

    let mut aus = Vec::with_capacity(num_aus);
    for window in starts.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start + 2 > end || end > size {
            return None;
        }
        let au = &data[start..end - 2];
        let stored = u16::from_be_bytes([data[end - 2], data[end - 1]]);
        aus.push(AccessUnit {
            data: au.to_vec(),
            crc_ok: crc16(au) == stored,
            pad: extract_pad(au).unwrap_or_default(),
        });
    }
    Some(aus)
}

/// Extract the PAD field of an access unit — TS 102 563 clause 5.4.3.
///
/// PAD is stored in a `data_stream_element()` (ISO/IEC 14496-3 clause
/// 4.4.2.5) that must be the first syntactic element of the
/// `raw_data_block()`. Returns `None` when there is no leading DSE or
/// when its length is below the 2-byte F-PAD minimum (clause 5.4.3: an
/// invalid length means "no PAD"). The returned bytes are as carried
/// (the F-PAD is the last two bytes; EN 300 401 clause 7.4.2 reverses
/// the X-PAD byte order on air).
#[must_use]
pub fn extract_pad(au: &[u8]) -> Option<Vec<u8>> {
    if au.len() < 2 {
        return None;
    }
    let mut bit = 0;
    if read_bits(au, bit, 3)? != ID_DSE {
        return None;
    }
    bit += 3;
    bit += 4; // element_instance_tag
    let byte_align = read_bits(au, bit, 1)? == 1;
    bit += 1;
    let mut count = read_bits(au, bit, 8)? as usize;
    bit += 8;
    if count == 255 {
        count += read_bits(au, bit, 8)? as usize;
        bit += 8;
    }
    if byte_align {
        bit = bit.next_multiple_of(8);
    }
    let start = bit / 8;
    let end = start.checked_add(count)?;
    if count < 2 || end > au.len() {
        return None;
    }
    Some(au[start..end].to_vec())
}

/// Inverse of [`SuperframeDecoder`]: builds RS-protected super frames.
///
/// Used by the tests and by the sim oracle. The AUs are packed in order;
/// the last AU is zero-padded to fill `audio_super_frame_size` exactly,
/// which is where the constant-length super frame property (clause 5.1)
/// requires the stuffing to live.
#[derive(Debug)]
pub struct SuperframeEncoder {
    subchannel_index: u8,
    rs: Rs120_110,
}

impl SuperframeEncoder {
    /// Create an encoder for one sub-channel size.
    pub fn new(subchannel_index: u8) -> Result<Self, Error> {
        if subchannel_index == 0 || subchannel_index > MAX_SUBCHANNEL_INDEX {
            return Err(Error::InvalidSubchannelIndex(subchannel_index));
        }
        Ok(Self {
            subchannel_index,
            rs: Rs120_110::new(),
        })
    }

    /// Build the `120×s` protected super frame for `header` and `aus`.
    ///
    /// `aus.len()` must equal `header.num_aus()`; the total framed size
    /// must fit `110×s` bytes.
    pub fn encode(&self, header: &SuperframeHeader, aus: &[&[u8]]) -> Result<Vec<u8>, Error> {
        let s = usize::from(self.subchannel_index);
        let capacity = SUPERFRAME_DATA_BYTES_PER_INDEX * s;
        let num_aus = header.num_aus();
        if aus.len() != num_aus {
            return Err(Error::WrongAuCount {
                expected: num_aus,
                got: aus.len(),
            });
        }

        // Fixed bytes before the last AU: header + every earlier AU and
        // its CRC. The last AU soaks up the rest of the capacity.
        let mut fixed = header.first_au_start();
        for au in aus.iter().take(num_aus - 1) {
            fixed += au.len() + 2;
        }
        let requested = fixed + aus[num_aus - 1].len() + 2;
        if requested > capacity || fixed + 2 > capacity {
            return Err(Error::SuperframeOverflow {
                bytes: requested,
                capacity,
            });
        }
        let last_size = capacity - fixed - 2;

        let mut data = vec![0u8; capacity];
        data[2] = header.to_params_byte();
        let mut bit = 24;
        let mut cursor = header.first_au_start();
        for au in aus.iter().take(num_aus - 1) {
            cursor += au.len() + 2;
            write_bits(&mut data, &mut bit, cursor as u32, 12)?;
        }
        // The trailing 4-bit alignment field (present for every
        // combination except 24 kHz core + SBR, clause 5.2) is zero by
        // definition and the buffer is zero-filled.

        let mut offset = header.first_au_start();
        for (i, au) in aus.iter().enumerate() {
            let size = if i + 1 == num_aus {
                last_size
            } else {
                au.len()
            };
            data[offset..offset + au.len()].copy_from_slice(au);
            let crc = crc16(&data[offset..offset + size]);
            data[offset + size..offset + size + 2].copy_from_slice(&crc.to_be_bytes());
            offset += size + 2;
        }
        let header_firecode = fire_code(&data[2..11]);
        data[..2].copy_from_slice(&header_firecode.to_be_bytes());

        // Parity and the byte-wise virtual interleaver (clauses 6.3–6.5).
        let mut out = vec![0u8; RS_N * s];
        out[..capacity].copy_from_slice(&data);
        for row in 0..s {
            let mut word = [0u8; RS_K];
            for (j, byte) in word.iter_mut().enumerate() {
                *byte = data[row + j * s];
            }
            let parity = self.rs.encode(&word);
            for (r, byte) in parity.iter().enumerate() {
                out[capacity + r * s + row] = *byte;
            }
        }
        Ok(out)
    }
}

/// `count` bits at `bit_offset`, MSb first, as used by the clause 5.2
/// header syntax.
fn read_bits(data: &[u8], bit_offset: usize, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for i in 0..count {
        let bit = bit_offset + i;
        let byte = *data.get(bit / 8)?;
        value = (value << 1) | u32::from((byte >> (7 - (bit % 8))) & 1);
    }
    Some(value)
}

/// Inverse of [`read_bits`] into a zero-filled buffer.
fn write_bits(
    data: &mut [u8],
    bit_offset: &mut usize,
    value: u32,
    count: usize,
) -> Result<(), Error> {
    if *bit_offset + count > data.len() * 8 {
        return Err(Error::InvalidTransport("bit write past end".into()));
    }
    for j in 0..count {
        let v = ((value >> (count - 1 - j)) & 1) as u8;
        let bit = *bit_offset + j;
        data[bit / 8] |= v << (7 - (bit % 8));
    }
    *bit_offset += count;
    Ok(())
}
