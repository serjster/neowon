//! The default pure-Rust backend: `oxideav-aac` 0.1.7 (MIT).
//!
//! Its SBR back end is defined for the 1024-line core family only, so a
//! DAB+ 960 + SBR configuration is rejected with
//! [`Error::SbrUnsupportedFrameFamily`] as soon as a bitstream carrying
//! SBR is decoded. That is the surfaced limitation that made libfdk-aac
//! the `fdk-aac` feature.

use super::{AudioSpecificConfig, DecodedAudio, Error, observed_support};

use oxideav_aac::decode::StreamDecoder;
use oxideav_aac::swb_offset::FrameFamily;

pub(super) const BACKEND: &str = "oxideav-aac";

#[derive(Debug)]
pub(super) struct Backend {
    inner: StreamDecoder,
}

impl Backend {
    pub(super) fn new(config: &AudioSpecificConfig) -> Result<Self, Error> {
        let mut inner = StreamDecoder::new();
        inner.set_frame_family(FrameFamily::from_aot_and_flag(
            config.aot,
            config.frame_length_960,
        ));
        Ok(Self { inner })
    }

    pub(super) fn decode(
        &mut self,
        au: &[u8],
        config: &AudioSpecificConfig,
    ) -> Result<DecodedAudio, Error> {
        let frame = self
            .inner
            .decode_raw_data_block(
                config.aot,
                config.sampling_frequency_index,
                config.sample_rate,
                config.channel_configuration,
                1,
                au,
            )
            .map_err(map_decode_error)?;
        let pcm = frame.pcm.iter().map(|&s| f32::from(s) / 32768.0).collect();
        let sbr_support = observed_support(config, frame.sample_rate, frame.channels);
        Ok(DecodedAudio {
            pcm,
            channels: frame.channels,
            sample_rate: frame.sample_rate,
            sbr_support,
        })
    }
}

fn map_decode_error(error: oxideav_aac::Error) -> Error {
    if matches!(error, oxideav_aac::Error::SbrUnsupportedFrameFamily) {
        return Error::SbrUnsupportedFrameFamily;
    }
    Error::Decode(error.to_string())
}
