//! MPEG-1 Layer II adapter for DAB classic — `oxideav-mp2` 0.0.10
//! (pure Rust, MIT), hidden behind this module's API.
//!
//! DAB classic carries MPEG-1 Layer II frames directly in the sub-channel
//! byte stream. The programme-associated data rides in the frame's
//! `ancillary_data()` tail: EN 300 401 clause 7.4.0 says the placement of
//! F-PAD/X-PAD depends on the audio coding method, and the F-PAD is
//! defined as "the last two bytes of the DAB audio frame" (EN 300 401
//! clause 3.1). The decoded [`DecodedMp2::ancillary`] is that raw tail;
//! the PAD/DLS parser lives in `neowon-dsp`.
//!
//! DAB classic uses 48 kHz stereo in practice; the adapter reports the
//! rate the frame header carries rather than assuming it.

use crate::Error;

use oxideav_mp2::frame::{Ancillary, FrameDecodeState, decode_frame_with};
use oxideav_mp2::header::{FrameHeader, find_sync};

/// One decoded MP2 frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedMp2 {
    /// Interleaved f32 PCM in `[-1, 1)`, `channels` samples per frame.
    pub pcm: Vec<f32>,
    /// Channel count from the frame header.
    pub channels: usize,
    /// Sample rate in Hz from the frame header (48 kHz for DAB classic).
    pub sample_rate: u32,
    /// The frame's `ancillary_data()` tail, as carried — the DAB PAD
    /// field lives here (F-PAD last; EN 300 401 clauses 3.1 and 7.4).
    pub ancillary: Vec<u8>,
}

/// Stateful MP2 decoder over a chunked byte stream.
///
/// The sub-channel byte stream arrives in arbitrary chunks; the decoder
/// buffers until a whole frame (from `FrameHeader::frame_size_bytes`)
/// is present, then decodes and chains the next frame. The polyphase
/// synthesis filterbank state persists across frames, so feeding the
/// same bytes in different chunkings must produce identical PCM.
#[derive(Debug)]
pub struct Mp2Decoder {
    state: FrameDecodeState,
    buffer: Vec<u8>,
    skipped: usize,
}

impl Default for Mp2Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp2Decoder {
    /// A fresh decoder with zeroed filterbank state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: FrameDecodeState::new(),
            buffer: Vec::new(),
            skipped: 0,
        }
    }

    /// Frames that failed to decode (or whose sync/header was invalid)
    /// since construction.
    #[must_use]
    pub fn skipped_frames(&self) -> usize {
        self.skipped
    }

    /// Push bytes; returns one entry per completed frame, in order.
    ///
    /// An entry is `Err` when a frame that started at a valid syncword
    /// could not be decoded; the decoder resynchronises on the next
    /// syncword and keeps going.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<DecodedMp2, Error>> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let Some(offset) = find_sync(&self.buffer) else {
                // A syncword may straddle the next chunk: keep two bytes.
                let keep = self.buffer.len().min(2);
                let drain = self.buffer.len() - keep;
                self.buffer.drain(..drain);
                break;
            };
            if offset > 0 {
                self.buffer.drain(..offset);
            }
            if self.buffer.len() < 4 {
                break;
            }
            let header = match FrameHeader::parse(&self.buffer) {
                Ok(header) => header,
                Err(error) => {
                    self.buffer.drain(..1);
                    self.skipped += 1;
                    out.push(Err(Error::Decode(format!("MP2 header: {error}"))));
                    continue;
                }
            };
            let size = header.frame_size_bytes();
            if self.buffer.len() < size {
                break;
            }
            let frame: Vec<u8> = self.buffer.drain(..size).collect();
            match decode_frame_with(&frame, &mut self.state) {
                Ok(decoded) => out.push(Ok(convert(
                    &decoded.pcm,
                    header.sample_rate,
                    &decoded.ancillary,
                ))),
                Err(error) => {
                    self.skipped += 1;
                    out.push(Err(Error::Decode(format!("MP2 frame: {error}"))));
                }
            }
        }
        out
    }

    /// Decode a whole buffer, failing on the first bad frame.
    pub fn decode_all(&mut self, bytes: &[u8]) -> Result<Vec<DecodedMp2>, Error> {
        self.push(bytes).into_iter().collect()
    }
}

/// Convert the codec's per-channel `f64` PCM and ancillary tail.
fn convert(pcm: &[Vec<f64>], sample_rate: u32, ancillary: &Ancillary) -> DecodedMp2 {
    let channels = pcm.len();
    let frames = pcm.first().map_or(0, Vec::len);
    let mut interleaved = Vec::with_capacity(channels * frames);
    for i in 0..frames {
        for channel in pcm {
            interleaved.push(channel[i] as f32);
        }
    }
    DecodedMp2 {
        pcm: interleaved,
        channels,
        sample_rate,
        ancillary: ancillary.bytes.clone(),
    }
}
