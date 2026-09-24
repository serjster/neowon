//! The per-stream decoder state for one playback stream: the DAB+ super-frame
//! transport with the HE-AAC v2 adapter, or the MPEG-1 Layer II adapter.

use crossbeam_channel::Sender;

use neowon_codec::aac::{AacDecoder, AudioSpecificConfig};
use neowon_codec::mp2::Mp2Decoder;

use super::transport::{DabPlusSync, RateConverter};
use super::{AudioState, AudioStatus, Coding, MAX_MP2_BYTES_WITHOUT_SYNC, Out, StreamSpec, fail};

pub(super) struct Stream {
    spec: StreamSpec,
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
        feed: &mut SinkFeed,
        out: &Sender<Out>,
        status: &mut AudioStatus,
    ) {
        match self.spec.coding {
            Coding::DabPlus => self.push_dabplus(bytes, feed, out, status),
            Coding::Mp2 => self.push_mp2(bytes, feed, out, status),
        }
    }

    fn push_dabplus(
        &mut self,
        bytes: &[u8],
        feed: &mut SinkFeed,
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
                        feed.emit(
                            &decoded.pcm,
                            decoded.channels,
                            decoded.sample_rate,
                            out,
                            status,
                        );
                    }
                    Err(e) => {
                        // The first AU's failure is the stream's verdict; a
                        // later one is a transient bitstream error.
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
        feed: &mut SinkFeed,
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
                    feed.emit(
                        &decoded.pcm,
                        decoded.channels,
                        decoded.sample_rate,
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

/// Seconds of decoded audio the feed holds back before the first block
/// reaches the device.
///
/// The stream decodes at about real time — one 24 ms logical frame per
/// 24 ms of air — so the feed has no spare throughput to rebuild a cushion
/// it never had. Start pushing with an empty queue and the device callback
/// is racing the decoder from the first sample: one late app frame, or one
/// callback that asks for more than a block, and the output has a hole in
/// it. The only moment a cushion can be built is before playback starts,
/// so it is built there, once.
///
/// It costs latency, nothing else: no audio is dropped and no silence is
/// inserted, and the queue it fills is [`neowon_audio::sink::MAX_QUEUED`]
/// (1 s), which the stream reaches on its own within seconds anyway.
const PRIME_SECONDS: f64 = 0.5;

/// The path from decoded PCM to the sink: downmix to the sink's mono
/// contract, rate-convert, build the start-up cushion, measure and push.
///
/// It owns the cushion because the cushion is a property of the feed, not
/// of one stream: `blocks`, `peak` and `AudioState::Playing` all mean
/// "reached the device", and holding the first blocks here is what keeps
/// that true — the status says `starting` until audio is actually on its
/// way out, so a reader cannot see `playing` over an empty queue.
pub(super) struct SinkFeed {
    converter: RateConverter,
    /// The sink's rate, for the cushion's size in samples.
    out_rate: f64,
    /// Blocks decoded before the cushion was full, in order. Empty and
    /// never refilled once `primed`.
    held: Vec<Vec<f32>>,
    held_samples: usize,
    primed: bool,
}

impl SinkFeed {
    pub(super) fn new(in_rate: f64, out_rate: f64) -> Self {
        Self {
            converter: RateConverter::new(in_rate, out_rate),
            out_rate,
            held: Vec::new(),
            held_samples: 0,
            primed: false,
        }
    }

    /// The output device reported a rate after the worker started.
    pub(super) fn set_output(&mut self, rate: f64) {
        self.converter.set_output(rate);
        if rate > 0.0 {
            self.out_rate = rate;
        }
    }

    /// A splice: the decoders restart, and so does the cushion — the seam
    /// is exactly where the feed has nothing queued behind it.
    pub(super) fn reset(&mut self) {
        self.converter.reset();
        self.held.clear();
        self.held_samples = 0;
        self.primed = false;
    }

    /// Samples of cushion the first push carries.
    pub(super) fn prime_samples(&self) -> usize {
        let rate = if self.out_rate > 0.0 {
            self.out_rate
        } else {
            48_000.0
        };
        (rate * PRIME_SECONDS) as usize
    }

    /// One decoded frame: converted, then either held for the cushion or
    /// pushed. `rate`/`channels` are the decoder's own facts and are
    /// published as soon as they are known; everything that describes the
    /// sink is published only when a block actually goes to it.
    pub(super) fn emit(
        &mut self,
        pcm: &[f32],
        channels: usize,
        rate: u32,
        out: &Sender<Out>,
        status: &mut AudioStatus,
    ) {
        if channels == 0 || pcm.is_empty() {
            return;
        }
        self.converter.set_input(f64::from(rate));
        let mut mono = Vec::with_capacity(pcm.len() / channels);
        for frame in pcm.chunks_exact(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }
        let mut block = Vec::new();
        self.converter.process(&mono, &mut block);
        if block.is_empty() {
            return;
        }
        status.rate = rate;
        status.channels = channels;
        if !self.primed {
            self.held_samples += block.len();
            self.held.push(block);
            if self.held_samples < self.prime_samples() {
                return;
            }
            self.primed = true;
            self.held_samples = 0;
            for block in std::mem::take(&mut self.held) {
                push_block(block, out, status);
            }
            return;
        }
        push_block(block, out, status);
    }
}

/// Hand one block to the sink and record what it was. A free function so
/// the flush above can call it while `self.held` is moved out.
fn push_block(block: Vec<f32>, out: &Sender<Out>, status: &mut AudioStatus) {
    status.peak = block.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
    status.rms = (block.iter().map(|x| x * x).sum::<f32>() / block.len() as f32).sqrt();
    status.blocks += 1;
    status.state = AudioState::Playing;
    if out.try_send(Out::Pcm(block)).is_err() {
        status.dropped += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;

    /// **The invariant playback rests on:** when the status first says
    /// `playing`, the device already has `PRIME_SECONDS` of audio behind
    /// its next sample. Until the cushion is full nothing reaches the sink
    /// and the state stays `starting`; when it is, every held block is
    /// handed over, in order and whole.
    #[test]
    fn nothing_reaches_the_sink_until_the_cushion_is_full() {
        let (tx, rx) = bounded::<Out>(256);
        let mut status = AudioStatus::default();
        let mut feed = SinkFeed::new(48_000.0, 48_000.0);
        let prime = feed.prime_samples();
        assert_eq!(prime, 24_000, "half a second at the sink's rate");

        // One DAB+ AU's worth of stereo at a time (2048 frames).
        let au: Vec<f32> = (0..2048 * 2).map(|i| (i % 7) as f32 / 16.0).collect();
        let mut fed = 0usize;
        while fed + 2048 <= prime {
            feed.emit(&au, 2, 48_000, &tx, &mut status);
            fed += 2048;
            assert_eq!(
                status.state,
                AudioState::Starting,
                "the sink was called playing with {fed} of {prime} samples queued"
            );
            assert_eq!(
                status.blocks, 0,
                "a block reached the sink before the cushion"
            );
            assert!(rx.try_recv().is_err(), "a block reached the sink at {fed}");
            // The decoder's own facts are published straight away.
            assert_eq!(status.rate, 48_000);
            assert_eq!(status.channels, 2);
        }

        // The block that fills the cushion releases all of it at once.
        feed.emit(&au, 2, 48_000, &tx, &mut status);
        fed += 2048;
        assert_eq!(status.state, AudioState::Playing);
        assert!(status.peak > 0.0, "the pushed block is measured");
        let mut queued = 0usize;
        while let Ok(Out::Pcm(block)) = rx.try_recv() {
            queued += block.len();
        }
        assert_eq!(queued, fed, "the cushion reached the sink whole");
        assert!(queued >= prime, "{queued} < {prime}");
        assert_eq!(status.blocks as usize, fed / 2048);

        // Primed: every later block goes straight through.
        feed.emit(&au, 2, 48_000, &tx, &mut status);
        let Ok(Out::Pcm(block)) = rx.try_recv() else {
            panic!("a primed feed pushes every block");
        };
        assert_eq!(block.len(), 2048);
        assert_eq!(status.blocks as usize, fed / 2048 + 1);
    }

    /// A splice rebuilds the cushion: the de-interleavers restart with
    /// nothing queued behind them, which is the one place a gap is honest.
    #[test]
    fn a_reset_rebuilds_the_cushion() {
        let (tx, rx) = bounded::<Out>(256);
        let mut status = AudioStatus::default();
        let mut feed = SinkFeed::new(48_000.0, 48_000.0);
        let au = vec![0.25f32; 2048 * 2];
        for _ in 0..16 {
            feed.emit(&au, 2, 48_000, &tx, &mut status);
        }
        assert_eq!(status.state, AudioState::Playing);
        while rx.try_recv().is_ok() {}

        feed.reset();
        feed.emit(&au, 2, 48_000, &tx, &mut status);
        assert!(
            rx.try_recv().is_err(),
            "the splice pushed into an empty queue"
        );
    }
}
