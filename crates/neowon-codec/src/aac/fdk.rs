//! The fallback backend: the `fdk-aac` C binding over libfdk-aac.
//!
//! This is the backend that can decode what DAB+ actually carries —
//! the 960-line transform with SBR and parametric stereo (TS 102 563
//! clause 5.1). Unlike [`super::oxideav`] it accepts that configuration.
//! A configuration or bitstream it refuses is a typed error.
//!
//! One limit is worth stating: libfdk-aac cannot see that an ASC does
//! not describe the AUs it is fed, and conceals (silenced output) rather
//! than erroring when the parser half-succeeds. On air that cannot
//! happen — the ASC is derived from the super frame header (clause 7.2)
//! and the per-AU CRC gates corruption — but a caller pairing a 1024
//! bitstream with a 960 ASC will get silence, not an error. The
//! `tests/aac_fixture.rs` boundary test records this.
//!
//! # One AU per call, and no padding
//!
//! libfdk-aac's `TT_MP4_RAW` transport is a *packet* transport: the
//! bytes of one `aacDecoder_Fill` are exactly one access unit, decoded
//! by one `aacDecoder_DecodeFrame` call, and the next fill discards the
//! leftovers. But the raw parser requires the AU to be bit-exact: any
//! bit the `raw_data_block()` did not consume is a parse error
//! (`unreadBits != 0` in `aacdecoder.cpp`). DAB+ zero-stuffs the last
//! AU of a super frame *inside* its CRC-covered region (TS 102 563
//! clause 5.2), so that stuffing must be removed before the fill — see
//! [`trim_stuffing`].

use super::{AudioSpecificConfig, DecodedAudio, Error, observed_support};

use fdk_aac::dec::{Decoder, Transport};

pub(super) const BACKEND: &str = "fdk-aac";

/// Output scratch for one AU. The largest DAB+ stereo frame is the
/// 1024-line HE-AAC v2 core after 2× SBR upsampling: 2048 samples on
/// two channels; 8192 covers that and the 960-line family with room to
/// spare. A frame that overflows it is reported by libfdk-aac as an
/// error, not truncated.
const PCM_SCRATCH_SAMPLES: usize = 8192;

/// A libfdk-aac raw-transport decoder for one stream.
#[derive(Debug)]
pub(super) struct Backend {
    decoder: Decoder,
}

impl Backend {
    pub(super) fn new(config: &AudioSpecificConfig) -> Result<Self, Error> {
        let mut decoder = Decoder::new(Transport::Raw);
        decoder
            .config_raw(&config.to_bytes())
            .map_err(|error| Error::ConfigRejected {
                backend: BACKEND,
                reason: error.to_string(),
            })?;
        // Parametric stereo is only synthesized when the output is not
        // forced to mono; ask for at least two channels when the stream
        // declares PS or a stereo core. No-op for a stereo core.
        if config.ps_present || config.channel_configuration == 2 {
            decoder
                .set_min_output_channels(2)
                .map_err(|error| Error::ConfigRejected {
                    backend: BACKEND,
                    reason: format!("output-channel request: {error}"),
                })?;
        }
        Ok(Self { decoder })
    }

    pub(super) fn decode(
        &mut self,
        au: &[u8],
        config: &AudioSpecificConfig,
    ) -> Result<DecodedAudio, Error> {
        let au = trim_stuffing(au);
        if au.is_empty() {
            return Err(Error::Decode(
                "access unit carries no audio (all stuffing)".into(),
            ));
        }
        let consumed = self
            .decoder
            .fill(au)
            .map_err(|error| Error::Decode(format!("fdk-aac fill: {error}")))?;
        if consumed != au.len() {
            return Err(Error::Decode(format!(
                "fdk-aac accepted {consumed} of {} access-unit bytes",
                au.len()
            )));
        }
        let mut scratch = vec![0i16; PCM_SCRATCH_SAMPLES];
        self.decoder
            .decode_frame(&mut scratch)
            .map_err(|error| Error::Decode(format!("fdk-aac: {error}")))?;

        let info = self.decoder.stream_info();
        let channels = info.numChannels.max(0) as usize;
        let sample_rate = info.sampleRate.max(0) as u32;
        let channel_samples = info.frameSize.max(0) as usize;
        let samples = channels
            .checked_mul(channel_samples)
            .filter(|&count| count > 0 && count <= scratch.len())
            .ok_or_else(|| {
                Error::Decode(format!(
                    "fdk-aac reported {channels} ch × {channel_samples} samples"
                ))
            })?;
        let pcm = scratch[..samples]
            .iter()
            .map(|&sample| f32::from(sample) / 32768.0)
            .collect();
        Ok(DecodedAudio {
            pcm,
            channels,
            sample_rate,
            sbr_support: observed_support(config, sample_rate, channels),
        })
    }
}

/// Drop trailing zero stuffing from a DAB+ access unit.
///
/// A `raw_data_block()` is self-delimiting and always ends with
/// `ID_END` (three set bits) plus byte alignment, so its final byte is
/// never zero — any trailing `0x00` bytes can only be the clause 5.2
/// stuffing and are invisible to the codec.
fn trim_stuffing(au: &[u8]) -> &[u8] {
    match au.iter().rposition(|&byte| byte != 0) {
        Some(last) => &au[..=last],
        None => &au[..0],
    }
}

#[cfg(test)]
mod tests {
    use super::trim_stuffing;

    #[test]
    fn stuffing_is_trimmed_but_real_zeros_in_the_middle_are_not() {
        assert_eq!(
            trim_stuffing(&[0x21, 0x00, 0x07, 0x00, 0x00]),
            &[0x21, 0, 0x07]
        );
        assert_eq!(trim_stuffing(&[0x21, 0x07]), &[0x21, 0x07]);
        assert_eq!(trim_stuffing(&[0, 0, 0]), &[] as &[u8]);
    }
}
