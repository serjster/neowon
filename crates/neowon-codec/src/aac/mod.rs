//! HE-AAC v2 adapter for DAB+.
//!
//! # What the adapter takes
//!
//! One DAB+ access unit (a `raw_data_block()`, TS 102 563 clause 5.2)
//! plus the [`AudioSpecificConfig`] for the stream. DAB+ does not carry
//! the ASC on the wire: the super frame header signals the audio
//! parameters and the receiver derives the config (TS 102 563 clause
//! 7.2), so [`AudioSpecificConfig::for_dabplus`] is the bridge.
//!
//! # The two backends
//!
//! The default build decodes with the pure-Rust MIT `oxideav-aac`
//! 0.1.7. It cannot decode SBR on the 960-line transform that DAB+
//! mandates (TS 102 563 clause 5.1) — its SBR back end is defined for
//! the 1024-line core only — so a real DAB+ AU reaches
//! [`Error::SbrUnsupportedFrameFamily`] there. The `fdk-aac` feature
//! compiles in libfdk-aac instead (the `fdk-aac` C binding, itself MIT,
//! wrapping Fraunhofer's BSD-based libfdk-aac; no patent grant), which
//! does handle 960 + SBR + PS. [`AacDecoder::BACKEND`] names the
//! compiled-in backend.
//!
//! Whichever backend is linked, the adapter surfaces what it did rather
//! than assuming: [`AacDecoder::sbr_support`] reports what the config
//! declares, [`DecodedAudio::sbr_support`] what the decode actually
//! produced, and a configuration or bitstream the backend refuses is a
//! typed [`Error`]. (The one thing a backend cannot see is an ASC that
//! does not describe its AUs; the `fdk` module documents libfdk-aac's
//! concealment in that case.)

use crate::Error;
use crate::dabplus::SuperframeHeader;

use oxideav_aac::asc::AudioSpecificConfig as OxideAsc;
use oxideav_aac::asc::FrameLength as OxideFrameLength;
use oxideav_aac::latm::{AudioSyncStream, LayerConfig};

#[cfg(not(feature = "fdk-aac"))]
mod oxideav;
#[cfg(not(feature = "fdk-aac"))]
use oxideav as backend;

#[cfg(feature = "fdk-aac")]
mod fdk;
#[cfg(feature = "fdk-aac")]
use fdk as backend;

/// Sampling-frequency indices of ISO/IEC 14496-3 Table 1.18 that DAB+
/// can use (clause 5.1: 16/24/32/48 kHz cores).
fn rate_index(rate: u32) -> Option<u8> {
    match rate {
        48_000 => Some(3),
        32_000 => Some(5),
        24_000 => Some(6),
        16_000 => Some(8),
        _ => None,
    }
}

/// The AudioSpecificConfig (ISO/IEC 14496-3 clause 1.6.2.1) as this
/// crate cares about it.
///
/// Built either from DAB+ super frame parameters
/// ([`Self::for_dabplus`]) or parsed from ASC bytes
/// ([`Self::parse`]); the latter runs the pinned `oxideav-aac` parser,
/// the former is written here and round-tripped through that parser in
/// the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// Outer `audioObjectType` as transmitted: `2` (AAC-LC), `5`
    /// (SBR) or `29` (PS) for DAB+ (ISO/IEC 14496-3 Table 1.15).
    pub outer_aot: u8,
    /// Inner/effective object type; `2` for DAB+.
    pub aot: u8,
    /// Core `samplingFrequencyIndex` (Table 1.18).
    pub sampling_frequency_index: u8,
    /// Resolved core sample rate in Hz.
    pub sample_rate: u32,
    /// `channelConfiguration` (Table 1.19): `1` = mono, `2` = stereo.
    pub channel_configuration: u8,
    /// `frameLengthFlag == 1` — the 960-line transform DAB+ mandates
    /// (TS 102 563 clause 5.1).
    pub frame_length_960: bool,
    /// SBR signalled by the config (explicit wrapper or trailing probe).
    /// In-band SBR can still appear without this flag.
    pub sbr_present: bool,
    /// PS signalled by the config (outer AOT 29 or the 0x548 probe).
    pub ps_present: bool,
    /// SBR output rate when signalled.
    pub extension_sample_rate: Option<u32>,
}

impl AudioSpecificConfig {
    /// The config a DAB+ super frame header implies (TS 102 563 clause
    /// 7.2 + clause 5.2 Tables 3–6).
    ///
    /// `ps_flag` implies SBR (Table 6 restricts PS to `sbr_flag == 1 &&
    /// aac_channel_mode == 0`), so it is normalised here.
    #[must_use]
    pub fn for_dabplus(header: &SuperframeHeader) -> Self {
        let dac_rate = header.dac_rate_hz();
        let core_rate = header.core_rate_hz();
        let sbr = header.sbr_flag || header.ps_flag;
        let ps = header.ps_flag;
        Self {
            outer_aot: if ps {
                29
            } else if sbr {
                5
            } else {
                2
            },
            aot: 2,
            sampling_frequency_index: rate_index(core_rate)
                .expect("DAB+ core rates are 16/24/32/48 kHz"),
            sample_rate: core_rate,
            channel_configuration: if header.aac_channel_mode { 2 } else { 1 },
            // TS 102 563 clause 5.1: the transform is 960 for every
            // DAB+ configuration.
            frame_length_960: true,
            sbr_present: sbr,
            ps_present: ps,
            extension_sample_rate: sbr.then_some(dac_rate),
        }
    }

    /// Parse ASC bytes (ISO/IEC 14496-3 clause 1.6.2.1) through the
    /// pinned codec's parser.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let (asc, _) = OxideAsc::parse(bytes).map_err(|e| Error::InvalidAsc(format!("{e}")))?;
        Ok(Self::from_oxide(&asc))
    }

    /// The bit-exact ASC for this configuration.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = BitWriter::default();
        writer.write(u32::from(self.outer_aot), 5);
        writer.write(u32::from(self.sampling_frequency_index), 4);
        writer.write(u32::from(self.channel_configuration), 4);
        if self.outer_aot == 5 || self.outer_aot == 29 {
            let extension_index = self
                .extension_sample_rate
                .and_then(rate_index)
                .expect("SBR config has a DAB+ extension rate");
            writer.write(u32::from(extension_index), 4);
            writer.write(u32::from(self.aot), 5);
        }
        writer.write_bit(self.frame_length_960);
        writer.write_bit(false); // dependsOnCoreCoder
        writer.write_bit(false); // extensionFlag
        writer.finish()
    }

    #[must_use]
    pub fn core_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Decoder output rate in Hz: the SBR extension rate when present,
    /// else the core rate.
    #[must_use]
    pub fn output_sample_rate(&self) -> u32 {
        self.extension_sample_rate.unwrap_or(self.sample_rate)
    }

    fn from_oxide(asc: &OxideAsc) -> Self {
        Self {
            outer_aot: asc.outer_aot,
            aot: asc.aot,
            sampling_frequency_index: asc.sampling_frequency_index,
            sample_rate: asc.sample_rate,
            channel_configuration: asc.channel_configuration,
            frame_length_960: matches!(asc.ga_body.frame_length, OxideFrameLength::Long960),
            sbr_present: asc.sbr_present,
            ps_present: asc.ps_present,
            extension_sample_rate: asc.extension_sample_rate,
        }
    }
}

/// What a stream's decode does with SBR/PS — surfaced, never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbrSupport {
    /// No SBR: output rate equals the core rate.
    None,
    /// SBR without PS (HE-AAC v1-like): output rate is doubled.
    Sbr,
    /// SBR with parametric stereo (HE-AAC v2): mono core, stereo out.
    Ps,
}

/// One decoded access unit.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    /// Interleaved f32 PCM in `[-1, 1)`, `channels` samples per frame.
    pub pcm: Vec<f32>,
    pub channels: usize,
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// What the decode actually did about SBR/PS (observed, not
    /// configured).
    pub sbr_support: SbrSupport,
}

/// Stateful HE-AAC v2 decoder configured by one
/// [`AudioSpecificConfig`].
///
/// Feed access units in stream order; the decoder carries the AAC
/// overlap, SBR and PS state across them. Which codec implementation
/// does the work is a compile-time feature (`fdk-aac`); the API is the
/// same either way, and [`Self::BACKEND`] names it.
#[derive(Debug)]
pub struct AacDecoder {
    config: AudioSpecificConfig,
    inner: backend::Backend,
}

impl AacDecoder {
    /// Which codec backend this build links: `"oxideav-aac"` (default,
    /// pure Rust) or `"fdk-aac"` (the `fdk-aac` feature, C libfdk-aac).
    pub const BACKEND: &'static str = backend::BACKEND;

    /// A configuration the backend cannot honour is a typed error:
    /// `oxideav-aac` rejects SBR on the 960-line family with
    /// [`Error::SbrUnsupportedFrameFamily`], libfdk-aac reports its own
    /// reason as [`Error::ConfigRejected`].
    pub fn new(config: AudioSpecificConfig) -> Result<Self, Error> {
        validate(&config)?;
        Ok(Self {
            inner: backend::Backend::new(&config)?,
            config,
        })
    }

    #[must_use]
    pub fn config(&self) -> &AudioSpecificConfig {
        &self.config
    }

    /// What the configuration declares about SBR/PS.
    #[must_use]
    pub fn sbr_support(&self) -> SbrSupport {
        if self.config.ps_present {
            SbrSupport::Ps
        } else if self.config.sbr_present {
            SbrSupport::Sbr
        } else {
            SbrSupport::None
        }
    }

    /// Decode one access unit (`raw_data_block()` bytes).
    ///
    /// In-band SBR/PS extension payloads upgrade the output even when
    /// the config only signals the core, so the reported
    /// [`DecodedAudio::sbr_support`] is derived from the actual result.
    pub fn decode(&mut self, au: &[u8]) -> Result<DecodedAudio, Error> {
        if au.is_empty() {
            return Err(Error::InvalidTransport("empty access unit".into()));
        }
        self.inner.decode(au, &self.config)
    }
}

/// Reject a configuration before any backend sees it.
///
/// The public fields allow hand-built configs that [`AudioSpecificConfig::to_bytes`]
/// cannot serialise (an explicit-frequency escape index, an SBR
/// extension rate outside DAB+'s 16/24/32/48 kHz set) or that no DAB+
/// stream can use. Those are typed errors here rather than a panic or a
/// silently mangled ASC.
fn validate(config: &AudioSpecificConfig) -> Result<(), Error> {
    if config.aot != 2 {
        return Err(Error::InvalidAsc(format!(
            "core AOT {} is not AAC-LC",
            config.aot
        )));
    }
    if rate_index(config.sample_rate) != Some(config.sampling_frequency_index) {
        return Err(Error::InvalidAsc(format!(
            "sample rate {} does not match sampling_frequency_index {}",
            config.sample_rate, config.sampling_frequency_index
        )));
    }
    if config.outer_aot == 5 || config.outer_aot == 29 {
        match config.extension_sample_rate {
            Some(rate) if rate_index(rate).is_some() => {}
            _ => {
                return Err(Error::InvalidAsc(format!(
                    "SBR/PS config needs a DAB+ extension rate, got {:?}",
                    config.extension_sample_rate
                )));
            }
        }
    }
    Ok(())
}

/// LOAS/LATM demux result: the stream's config and its access units.
#[derive(Debug, Clone, PartialEq)]
pub struct LoasStream {
    /// Config recovered from the `StreamMuxConfig` (the LATM driver
    /// installs this ASC; the adapter hides that choice).
    pub config: AudioSpecificConfig,
    /// One `raw_data_block()` per payload, in order.
    pub access_units: Vec<Vec<u8>>,
}

/// Split a LOAS (`.latm`) stream into the config and the access units.
///
/// This is the fixture-facing entry point; DAB+ itself never carries
/// LOAS (the AUs travel in the super frame), so the adapter's normal
/// path is [`AacDecoder::decode`].
pub fn demux_loas(bytes: &[u8]) -> Result<LoasStream, Error> {
    let mut stream = AudioSyncStream::new(bytes);
    let mut config: Option<AudioSpecificConfig> = None;
    let mut access_units = Vec::new();
    while let Some(frame) = stream
        .next_frame()
        .map_err(|e| Error::Decode(format!("LOAS: {e}")))?
    {
        let layer: &LayerConfig = frame
            .element
            .config
            .layers
            .first()
            .ok_or_else(|| Error::Decode("LOAS: StreamMuxConfig has no layer".into()))?;
        let asc = AudioSpecificConfig::from_oxide(&layer.effective_asc);
        if config.is_none() {
            config = Some(asc);
        }
        for payload in &frame.element.payloads {
            access_units.push(payload.data.clone());
        }
    }
    Ok(LoasStream {
        config: config.ok_or_else(|| Error::Decode("LOAS: no frames".into()))?,
        access_units,
    })
}

/// SBR/PS support actually observed in a decoded frame.
fn observed_support(
    config: &AudioSpecificConfig,
    out_rate: u32,
    out_channels: usize,
) -> SbrSupport {
    let stereo_from_mono = out_channels == 2 && config.channel_configuration == 1;
    if config.ps_present || stereo_from_mono {
        SbrSupport::Ps
    } else if config.sbr_present || out_rate >= config.sample_rate.saturating_mul(2) {
        SbrSupport::Sbr
    } else {
        SbrSupport::None
    }
}

/// Minimal MSb-first bit writer for the ASC (the config is a few bytes).
#[derive(Debug, Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    fn write(&mut self, value: u32, count: usize) {
        for j in 0..count {
            self.write_bit((value >> (count - 1 - j)) & 1 == 1);
        }
    }

    fn write_bit(&mut self, value: bool) {
        if self.bit.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if value {
            let byte = self.bytes.len() - 1;
            self.bytes[byte] |= 1 << (7 - (self.bit % 8));
        }
        self.bit += 1;
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
