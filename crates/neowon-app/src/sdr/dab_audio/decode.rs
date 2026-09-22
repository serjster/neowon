//! The per-stream decoder state for one playback stream: the DAB+ super-frame
//! transport with the HE-AAC v2 adapter, or the MPEG-1 Layer II adapter.
//! Split from `mod.rs` along that seam so each file keeps its budget.

use crossbeam_channel::Sender;

use neowon_codec::aac::{AacDecoder, AudioSpecificConfig};
use neowon_codec::mp2::Mp2Decoder;

use super::transport::{DabPlusSync, RateConverter};
use super::{AudioState, AudioStatus, Coding, MAX_MP2_BYTES_WITHOUT_SYNC, Out, StreamSpec, fail};

/// The per-stream decoder state: the DAB+ super-frame transport + HE-AAC v2
/// adapter, or the MPEG-1 Layer II adapter.
pub(super) struct Stream {
    spec: StreamSpec,
    /// DAB+ super-frame sync and transport.
    sync: Option<DabPlusSync>,
    aac: Option<AacDecoder>,
    config: Option<AudioSpecificConfig>,
    mp2: Option<Mp2Decoder>,
    /// MP2 bytes pushed and frames decoded, for the sync-stall error gate.
    mp2_bytes: u64,
    mp2_frames: u64,
}

impl Stream {
    pub(super) fn new(spec: StreamSpec) -> Result<Self, String> {
        let sync = match spec.coding {
            Coding::DabPlus => Some(
                DabPlusSync::new(spec.subchannel_index).map_err(|e| format!("dab play: {e}"))?,
            ),
            Coding::Mp2 => None,
        };
        let mp2 = match spec.coding {
            Coding::Mp2 => Some(Mp2Decoder::new()),
            Coding::DabPlus => None,
        };
        Ok(Self {
            spec,
            sync,
            aac: None,
            config: None,
            mp2,
            mp2_bytes: 0,
            mp2_frames: 0,
        })
    }

    pub(super) fn push(
        &mut self,
        bytes: &[u8],
        converter: &mut RateConverter,
        out: &Sender<Out>,
        status: &mut AudioStatus,
    ) {
        match self.spec.coding {
            Coding::DabPlus => self.push_dabplus(bytes, converter, out, status),
            Coding::Mp2 => self.push_mp2(bytes, converter, out, status),
        }
    }

    fn push_dabplus(
        &mut self,
        bytes: &[u8],
        converter: &mut RateConverter,
        out: &Sender<Out>,
        status: &mut AudioStatus,
    ) {
        let sync = self.sync.as_mut().expect("DAB+ stream has a sync");
        let clean = sync.push(bytes);
        // The sync only hands out Fire-code-clean windows. A stream that is
        // not DAB+ at all is surfaced only once its search has consumed a
        // meaningful amount of bytes with no clean window ever — never on a
        // transient bad stretch, and never on a stream that has decoded once.
        if status.decoded == 0 && sync.shape_mismatch() {
            return fail(
                status,
                format!(
                    "no valid DAB+ super frame in {} windows (Fire code); \
                     the stream is not the signalled {} × 8 kbit/s DAB+ shape",
                    sync.search_windows(),
                    self.spec.subchannel_index
                ),
            );
        }
        for superframe in clean {
            let Some(header) = superframe.header else {
                continue;
            };
            let config = self
                .spec
                .asc_override
                .unwrap_or_else(|| AudioSpecificConfig::for_dabplus(&header));
            if self.aac.is_none() || self.config != Some(config) {
                match AacDecoder::new(config) {
                    Ok(decoder) => {
                        self.aac = Some(decoder);
                        self.config = Some(config);
                    }
                    Err(e) => {
                        return fail(status, format!("{}: {e}", self.spec.coding.backend()));
                    }
                }
            }
            let decoder = self.aac.as_mut().expect("decoder was just built");
            for au in &superframe.aus {
                if !au.crc_ok {
                    tracing::warn!("dab-audio: AU CRC failed on sub {}", self.spec.sub_channel);
                    status.errors += 1;
                    continue;
                }
                match decoder.decode(&au.data) {
                    Ok(decoded) => {
                        if !au.pad.is_empty() {
                            let _ = out.try_send(Out::Pad {
                                sub_channel: self.spec.sub_channel,
                                bytes: au.pad.clone(),
                            });
                        }
                        status.decoded += 1;
                        emit(
                            &decoded.pcm,
                            decoded.channels,
                            decoded.sample_rate,
                            converter,
                            out,
                            status,
                        );
                    }
                    Err(e) => {
                        // The first AU's failure is the stream's verdict
                        // (DAB-G2's surfaced limitation); a later one is a
                        // transient bitstream error.
                        if status.decoded == 0 {
                            return fail(status, format!("{}: {e}", self.spec.coding.backend()));
                        }
                        tracing::warn!("dab-audio: decode error: {e}");
                        status.errors += 1;
                    }
                }
            }
        }
    }

    fn push_mp2(
        &mut self,
        bytes: &[u8],
        converter: &mut RateConverter,
        out: &Sender<Out>,
        status: &mut AudioStatus,
    ) {
        self.mp2_bytes += bytes.len() as u64;
        let frames = self
            .mp2
            .as_mut()
            .expect("MP2 stream has a decoder")
            .push(bytes);
        for frame in frames {
            match frame {
                Ok(decoded) => {
                    if !decoded.ancillary.is_empty() {
                        let _ = out.try_send(Out::Pad {
                            sub_channel: self.spec.sub_channel,
                            bytes: decoded.ancillary.clone(),
                        });
                    }
                    self.mp2_frames += 1;
                    status.decoded += 1;
                    emit(
                        &decoded.pcm,
                        decoded.channels,
                        decoded.sample_rate,
                        converter,
                        out,
                        status,
                    );
                }
                Err(e) => {
                    if status.decoded == 0
                        && self.mp2_frames == 0
                        && self.mp2_bytes >= MAX_MP2_BYTES_WITHOUT_SYNC
                    {
                        return fail(status, format!("{}: {e}", self.spec.coding.backend()));
                    }
                    status.errors += 1;
                }
            }
        }
        if status.decoded == 0
            && self.mp2_frames == 0
            && self.mp2_bytes >= MAX_MP2_BYTES_WITHOUT_SYNC
        {
            fail(
                status,
                format!(
                    "no MPEG-1 Layer II sync in {} bytes; the sub-channel is not an MP2 stream",
                    self.mp2_bytes
                ),
            );
        }
    }
}

/// Downmix to the sink's mono contract, rate-convert, measure and push. A
/// free function because both `push_*` paths hold a decoder borrow while
/// calling it, and it needs nothing from the stream anyway.
fn emit(
    pcm: &[f32],
    channels: usize,
    rate: u32,
    converter: &mut RateConverter,
    out: &Sender<Out>,
    status: &mut AudioStatus,
) {
    if channels == 0 || pcm.is_empty() {
        return;
    }
    converter.set_input(f64::from(rate));
    let mut mono = Vec::with_capacity(pcm.len() / channels);
    for frame in pcm.chunks_exact(channels) {
        mono.push(frame.iter().sum::<f32>() / channels as f32);
    }
    let mut block = Vec::new();
    converter.process(&mono, &mut block);
    if block.is_empty() {
        return;
    }
    status.rate = rate;
    status.channels = channels;
    status.peak = block.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
    status.rms = (block.iter().map(|x| x * x).sum::<f32>() / block.len() as f32).sqrt();
    status.blocks += 1;
    status.state = AudioState::Playing;
    if out.try_send(Out::Pcm(block)).is_err() {
        status.dropped += 1;
    }
}
