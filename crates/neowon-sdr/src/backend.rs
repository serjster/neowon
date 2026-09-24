//! `Backend` for an RTL-SDR: maps `SdrConfig` onto the in-tree driver and
//! turns the u8 IQ stream into `Complex` × `Stream` frames in full-scale
//! units (|I|, |Q| <= 1).
//!
//! Direct sampling is not a config field: centres below the tuner's range
//! use the HF input (the V3's Q branch) and everything else uses the
//! tuner, so tuning across 24 MHz just works.

use std::time::Duration;

use neowon_backend::{
    Acquisition, Backend, BackendError, Capabilities, InstrumentConfig, SdrCaps, SdrConfig,
    SdrGain, sdr_config,
};
use neowon_core::ladders::{RTL_SAMPLE_RATES, r82xx_gains_db};
use neowon_core::{
    AcqMode, CaptureFrame, ChannelCapture, IqCal, SampleLayout, SharedFrame, stream_chunk_pairs,
};

use crate::rtl::{self, DirectSampling, RtlSdr, Stream, TUNER_MAX_HZ, TUNER_MIN_HZ};

/// Lowest centre the HF path serves usefully, Hz.
pub const HF_MIN_HZ: f64 = 500e3;

pub struct RtlBackend {
    sdr: RtlSdr,
    stream: Option<Stream>,
    caps: Capabilities,
    /// What the device currently runs; `None` until the first apply.
    applied: Option<SdrConfig>,
    seq: u64,
    /// Pairs elapsed since the stream started, including dropped chunks,
    /// so frame timestamps show the gaps.
    pairs: u64,
    overflows_seen: u64,
    /// Set when a setting was changed under a running stream: the next
    /// chunk straddles the change (`docs/protocol-rtlsdr.md`, "the first
    /// chunk after a change straddles it, the second is clean"), so it is
    /// discarded rather than delivered as if it were one setting's samples.
    straddling: bool,
    /// Pairs lost (overflowed or discarded) and not yet reported on a
    /// frame. Cleared by the frame that carries them.
    dropped_pending: u64,
}

fn err(e: rtl::Error) -> BackendError {
    match e {
        rtl::Error::Invalid(_) | rtl::Error::Pll(_) => BackendError::Transient(e.to_string()),
        _ => BackendError::Fatal(e.to_string()),
    }
}

pub fn direct_sampling_for(centre_hz: f64) -> DirectSampling {
    if centre_hz < TUNER_MIN_HZ as f64 {
        DirectSampling::Q
    } else {
        DirectSampling::Off
    }
}

/// u8 offset-binary I,Q to a one-channel complex frame. `dropped_before` is
/// the pairs lost since the previous frame — the gap the consumer needs in
/// order to tell a splice from ordinary arrival jitter.
pub fn frame_from_u8(
    bytes: &[u8],
    seq: u64,
    t_capture: f64,
    rate: f64,
    dropped_before: u64,
) -> CaptureFrame {
    let data: Vec<f32> = bytes.iter().map(|&b| (b as f32 - 127.5) / 127.5).collect();
    let clipped = bytes.iter().any(|&b| b == 0 || b == 255);
    CaptureFrame::new(
        seq,
        Some(t_capture),
        rate,
        AcqMode::Sample,
        Acquisition::Stream {
            chunk: bytes.len() / 2,
        },
        SampleLayout::Complex,
        vec![ChannelCapture {
            ch: 0,
            data,
            cal: IqCal::real(1.0, 0.0),
            clipped,
            freq_meter: None,
        }],
    )
    .expect("a sampled complex stream is a valid frame")
    .with_dropped_before(dropped_before)
}

/// Sample pairs covered by chunks dropped since `seen` was last counted.
/// The chunk size varies with the rate, so this must be the stream's own
/// (a `saturating_sub` guards a recreated stream whose count restarted).
fn dropped_pairs(dropped: u64, seen: u64, chunk_pairs: usize) -> u64 {
    dropped.saturating_sub(seen) * chunk_pairs as u64
}

/// What one arriving chunk does to the stream clock and the drop account.
struct ChunkPlan {
    /// Pairs to advance the sample clock by before this chunk's first sample.
    advance: u64,
    /// `Some(dropped_before)` to deliver the chunk with that gap reported,
    /// `None` to discard it.
    deliver: Option<u64>,
}

/// Decide what to do with a chunk of `pairs` that arrived after `lost` pairs
/// were dropped. Pure, so the policy is testable without a dongle: the
/// straddling chunk after a setting change is discarded and its pairs join
/// the gap the next delivered frame reports, and the gap is only ever
/// reported once.
fn plan_chunk(straddling: &mut bool, pending: &mut u64, lost: u64, pairs: u64) -> ChunkPlan {
    *pending += lost;
    if std::mem::take(straddling) {
        *pending += pairs;
        ChunkPlan {
            advance: lost + pairs,
            deliver: None,
        }
    } else {
        ChunkPlan {
            advance: lost,
            deliver: Some(std::mem::take(pending)),
        }
    }
}

impl RtlBackend {
    pub fn open(serial: Option<&str>) -> Result<Self, rtl::Error> {
        let sdr = RtlSdr::open(serial)?;
        // The chunk follows the sample rate; this is the default config's,
        // and `apply` updates it. A frame's own `acq` is authoritative.
        let caps = Capabilities::Sdr(SdrCaps {
            name: "RTL-SDR".into(),
            serial: sdr.info().serial.clone().unwrap_or_default(),
            tuner: format!("{:?}", sdr.tuner()),
            freq_range_hz: (HF_MIN_HZ, TUNER_MAX_HZ as f64),
            sample_rates: RTL_SAMPLE_RATES.to_vec(),
            gains_db: r82xx_gains_db(),
            acquisition: Acquisition::Stream {
                chunk: stream_chunk_pairs(SdrConfig::default().sample_rate),
            },
        });
        Ok(Self {
            sdr,
            stream: None,
            caps,
            applied: None,
            seq: 0,
            pairs: 0,
            overflows_seen: 0,
            straddling: false,
            dropped_pending: 0,
        })
    }
}

impl Backend for RtlBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &InstrumentConfig) -> Result<(), BackendError> {
        let c = sdr_config(cfg)?;
        if !(HF_MIN_HZ..=TUNER_MAX_HZ as f64).contains(&c.centre_hz) {
            return Err(BackendError::Transient(format!(
                "centre {} Hz outside {HF_MIN_HZ}..={TUNER_MAX_HZ} Hz",
                c.centre_hz
            )));
        }
        let prev = self.applied.clone();
        let changed = |f: fn(&SdrConfig) -> u64| prev.as_ref().map(f) != Some(f(c));
        // Anything that changes what a sample *means* straddles the chunk in
        // flight; `moved` collects them so one place decides.
        let mut moved = false;
        if changed(|c| c.sample_rate.to_bits()) {
            self.sdr
                .set_sample_rate(c.sample_rate.round() as u32)
                .map_err(err)?;
            // The transfer length follows the rate, so a stream in flight
            // has to be replaced: the match below starts a fresh one.
            if self.stream.take().is_some() {
                self.overflows_seen = 0;
            }
        }
        let ds = direct_sampling_for(c.centre_hz);
        let ds_changed = ds != self.sdr.direct_sampling();
        if ds_changed {
            self.sdr.set_direct_sampling(ds).map_err(err)?;
        }
        if ds_changed || changed(|c| c.centre_hz.to_bits()) {
            self.sdr
                .set_center_freq(c.centre_hz.round() as u32)
                .map_err(err)?;
            moved = true;
        }
        if changed(|c| match c.gain {
            SdrGain::Auto => u64::MAX,
            SdrGain::Manual(db) => db.to_bits(),
        }) {
            let g = match c.gain {
                SdrGain::Auto => None,
                SdrGain::Manual(db) => Some((db * 10.0).round() as i32),
            };
            self.sdr.set_gain(g).map_err(err)?;
            moved = true;
        }
        if changed(|c| c.agc as u64) {
            self.sdr.set_rtl_agc(c.agc).map_err(err)?;
            moved = true;
        }
        if changed(|c| c.ppm.to_bits()) {
            self.sdr.set_ppm(c.ppm.round() as i32).map_err(err)?;
            moved = true;
        }
        // Keep the advertised stream chunk in step with the config the
        // stream will actually run at.
        if let Capabilities::Sdr(caps) = &mut self.caps {
            caps.acquisition = Acquisition::Stream {
                chunk: stream_chunk_pairs(c.sample_rate),
            };
        }
        match (c.running, self.stream.is_some()) {
            (true, false) => {
                // A fresh stream's first chunk is taken entirely after the
                // settings, so it does not straddle anything.
                self.stream = Some(self.sdr.stream().map_err(err)?);
                self.overflows_seen = 0;
                self.straddling = false;
            }
            (false, true) => {
                self.stream = None;
                self.straddling = false;
            }
            // A setting changed under a stream in flight: the chunk being
            // filled now is part old setting, part new.
            (true, true) => self.straddling |= moved,
            _ => {}
        }
        self.applied = Some(c.clone());
        Ok(())
    }

    fn poll_frame(&mut self, budget: Duration) -> Result<Option<SharedFrame>, BackendError> {
        let Some(stream) = &self.stream else {
            std::thread::sleep(budget);
            return Ok(None);
        };
        let Some(bytes) = stream.recv_timeout(budget).map_err(err)? else {
            return Ok(None);
        };
        // Dropped chunks are time that passed: advance the clock over them
        // by the chunk the stream really delivers, not a fixed constant, and
        // remember them so the frame that follows can report the gap.
        let overflows = stream.overflows();
        let lost = dropped_pairs(overflows, self.overflows_seen, stream.chunk_pairs());
        self.overflows_seen = overflows;
        let pairs = (bytes.len() / 2) as u64;
        let plan = plan_chunk(&mut self.straddling, &mut self.dropped_pending, lost, pairs);
        self.pairs += plan.advance;
        // A discarded straddling chunk is a hole in the stream like any
        // other: the next frame reports it.
        let Some(dropped_before) = plan.deliver else {
            return Ok(None);
        };
        let rate = self.sdr.sample_rate() as f64;
        let frame = frame_from_u8(
            &bytes,
            self.seq,
            self.pairs as f64 / rate,
            rate,
            dropped_before,
        );
        self.seq += 1;
        self.pairs += pairs;
        Ok(Some(std::sync::Arc::new(frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_maps_to_full_scale() {
        let f = frame_from_u8(&[0, 255, 127, 128], 3, 0.5, 2.0, 0);
        assert_eq!(f.layout(), SampleLayout::Complex);
        assert_eq!(
            f.channels[0].data,
            vec![-1.0, 1.0, -0.5 / 127.5, 0.5 / 127.5]
        );
        assert!(f.channels[0].clipped);
        assert_eq!(f.duration(), 1.0);
        assert_eq!(f.t_capture, Some(0.5));
    }

    #[test]
    fn hf_centres_use_direct_sampling() {
        assert_eq!(direct_sampling_for(9.74e6), DirectSampling::Q);
        assert_eq!(direct_sampling_for(24e6), DirectSampling::Off);
        assert_eq!(direct_sampling_for(100e6), DirectSampling::Off);
    }

    #[test]
    fn offered_rates_are_valid_and_exact() {
        for r in RTL_SAMPLE_RATES {
            let r = r as u32;
            assert!(r > 225_000 && r <= 3_200_000 && !(300_000 < r && r <= 900_000));
            assert_eq!(rtl::resampler(rtl::RTL_XTAL_HZ, r).1, r, "{r}");
        }
    }

    #[test]
    fn dropped_chunks_advance_the_clock_by_the_stream_chunk() {
        // At 250 kS/s the stream chunk is 16 384 pairs: two dropped chunks
        // are 32 768 pairs of time, and the frame timestamps after them must
        // say so.
        let chunk = stream_chunk_pairs(250e3);
        assert_eq!(chunk, 16_384);
        assert_eq!(dropped_pairs(2, 0, chunk), 32_768);
        assert_ne!(dropped_pairs(2, 0, chunk), 2 * 128 * 1024);
        // Only chunks not yet counted advance the clock.
        assert_eq!(dropped_pairs(5, 3, chunk), 2 * 16_384);
        // A recreated stream's counter restarting must not underflow.
        assert_eq!(dropped_pairs(0, 4, chunk), 0);
    }

    #[test]
    fn a_frame_carries_the_pairs_lost_before_it() {
        let f = frame_from_u8(&[0, 255, 127, 128], 3, 0.5, 2.0, 32_768);
        assert_eq!(f.dropped_before(), 32_768);
        assert_eq!(frame_from_u8(&[1, 2], 0, 0.0, 2.0, 0).dropped_before(), 0);
    }

    #[test]
    fn the_straddling_chunk_after_a_setting_change_is_dropped_and_counted() {
        // `docs/protocol-rtlsdr.md`: "the first chunk after a change
        // straddles it, the second is clean. Consumers measuring a setting
        // must drop that chunk."
        let (mut straddling, mut pending) = (false, 0u64);
        // Steady state: nothing lost, nothing to report.
        let p = plan_chunk(&mut straddling, &mut pending, 0, 102_400);
        assert_eq!((p.advance, p.deliver), (0, Some(0)));
        // A setting changed under the stream.
        straddling = true;
        let p = plan_chunk(&mut straddling, &mut pending, 0, 102_400);
        assert_eq!(p.deliver, None, "the straddling chunk must not be served");
        assert_eq!(p.advance, 102_400, "its time still passed");
        // The next chunk is clean, and it reports the hole the discard made.
        let p = plan_chunk(&mut straddling, &mut pending, 0, 102_400);
        assert_eq!(p.deliver, Some(102_400));
        // Reported once, not on every later frame.
        let p = plan_chunk(&mut straddling, &mut pending, 0, 102_400);
        assert_eq!(p.deliver, Some(0));
    }

    #[test]
    fn a_usb_overflow_and_a_discard_add_up_in_one_report() {
        let (mut straddling, mut pending) = (true, 0u64);
        // The straddling chunk arrives after 16 384 pairs were overflowed.
        let p = plan_chunk(&mut straddling, &mut pending, 16_384, 102_400);
        assert_eq!((p.advance, p.deliver), (16_384 + 102_400, None));
        // The next frame reports both, plus anything lost since.
        let p = plan_chunk(&mut straddling, &mut pending, 16_384, 102_400);
        assert_eq!(p.deliver, Some(16_384 + 102_400 + 16_384));
    }
}
