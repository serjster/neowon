use std::fmt;

/// Everything that can go wrong in `neowon-codec`.
///
/// [`Error::SbrUnsupportedFrameFamily`] is a first-class variant on
/// purpose: the default `oxideav-aac` 0.1.7 SBR back end is defined for
/// the 1024-line core frame only, while DAB+ mandates the 960-line
/// transform (TS 102 563 clause 5.1). A DAB+ HE-AAC v2 access unit
/// therefore surfaces here rather than being silently mis-decoded —
/// the adapter must make this limitation visible. The
/// `fdk-aac` feature replaces that backend with libfdk-aac, which does
/// accept 960 + SBR and so never returns this variant.
///
/// [`Error::ConfigRejected`] is its counterpart for that backend: a
/// configuration libfdk-aac refuses (unknown AOT, unsupported SBR
/// shape) is reported with the backend's own reason, never ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `subchannel_index` is outside 1..=24 (TS 102 563 clause 5.1 note).
    InvalidSubchannelIndex(u8),
    /// The supplied `AudioSpecificConfig` is malformed or uses a shape
    /// this adapter does not configure (core AOT other than AAC-LC).
    InvalidAsc(String),
    /// A superframe or access unit did not match TS 102 563's layout.
    InvalidTransport(String),
    /// The AU list does not match the header's `num_aus` (clause 5.2).
    WrongAuCount { expected: usize, got: usize },
    /// The framed data exceeds `audio_super_frame_size` (clause 5.1).
    SuperframeOverflow { bytes: usize, capacity: usize },
    /// The stream combines SBR with a 960-line core frame, which the
    /// default `oxideav-aac` back end cannot decode. Not a transient
    /// error: this is the DAB+ HE-AAC v2 limitation. Never returned by
    /// the `fdk-aac` backend, which decodes that combination.
    SbrUnsupportedFrameFamily,
    /// The selected codec backend refused the stream configuration
    /// (`AacDecoder::BACKEND` names it; `reason` is the backend's own
    /// message, e.g. libfdk-aac's `AAC_DEC_UNSUPPORTED_AOT`).
    ConfigRejected {
        backend: &'static str,
        reason: String,
    },
    /// The underlying codec rejected the bitstream (message only: the
    /// codec crate is hidden behind this adapter's API).
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSubchannelIndex(i) => {
                write!(f, "subchannel_index {i} outside 1..=24")
            }
            Self::InvalidAsc(m) => write!(f, "invalid AudioSpecificConfig: {m}"),
            Self::InvalidTransport(m) => write!(f, "invalid DAB+ transport: {m}"),
            Self::WrongAuCount { expected, got } => {
                write!(f, "expected {expected} AUs, got {got}")
            }
            Self::SuperframeOverflow { bytes, capacity } => {
                write!(f, "superframe needs {bytes} bytes, capacity is {capacity}")
            }
            Self::SbrUnsupportedFrameFamily => write!(
                f,
                "SBR with a 960-line core frame is not decodable by the pinned \
                 oxideav-aac 0.1.7 (DAB+ mandates 960)"
            ),
            Self::ConfigRejected { backend, reason } => {
                write!(f, "{backend} rejected the stream configuration: {reason}")
            }
            Self::Decode(m) => write!(f, "codec decode failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}
