//! `Backend` for an RTL-SDR: maps `SdrConfig` onto the in-tree driver and
//! turns the u8 IQ stream into `Complex` × `Stream` frames in full-scale
//! units (|I|, |Q| <= 1).
//!
//! Direct sampling is not a config field: centres below the tuner's range
//! use the HF input (the V3's Q branch) and everything else uses the
//! tuner, so tuning across 24 MHz just works.

use std::time::Duration;

use neowon_backend::{
    Acquisition, Backend, BackendError, Capabilities, InstrumentConfig, SdrCaps, SdrConfig, SdrGain,
};
use neowon_core::{AcqMode, CaptureFrame, ChannelCapture, IqCal, SampleLayout, SharedFrame};

use crate::rtl::{self, DirectSampling, R82XX_GAINS, RtlSdr, Stream, TUNER_MAX_HZ, TUNER_MIN_HZ};

/// I/Q pairs per bulk transfer, hence per frame (256 KiB of u8 pairs).
pub const CHUNK_PAIRS: usize = 128 * 1024;
/// Lowest centre the HF path serves usefully, Hz.
pub const HF_MIN_HZ: f64 = 500e3;

/// Rates offered to the UI (the SDR++ list; all valid for the RTL2832).
pub const SAMPLE_RATES: [f64; 11] = [
    250e3, 1.024e6, 1.536e6, 1.792e6, 1.92e6, 2.048e6, 2.16e6, 2.4e6, 2.56e6, 2.88e6, 3.2e6,
];

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
}

fn err(e: rtl::Error) -> BackendError {
    match e {
        rtl::Error::Invalid(_) | rtl::Error::Pll(_) => BackendError::Transient(e.to_string()),
        _ => BackendError::Fatal(e.to_string()),
    }
}

/// The input a centre frequency needs.
pub fn direct_sampling_for(centre_hz: f64) -> DirectSampling {
    if centre_hz < TUNER_MIN_HZ as f64 {
        DirectSampling::Q
    } else {
        DirectSampling::Off
    }
}

/// u8 offset-binary I,Q to a one-channel complex frame.
pub fn frame_from_u8(bytes: &[u8], seq: u64, t_capture: f64, rate: f64) -> CaptureFrame {
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
}

impl RtlBackend {
    pub fn open(serial: Option<&str>) -> Result<Self, rtl::Error> {
        let sdr = RtlSdr::open(serial)?;
        let caps = Capabilities::Sdr(SdrCaps {
            name: "RTL-SDR".into(),
            serial: sdr.info().serial.clone().unwrap_or_default(),
            tuner: format!("{:?}", sdr.tuner()),
            freq_range_hz: (HF_MIN_HZ, TUNER_MAX_HZ as f64),
            sample_rates: SAMPLE_RATES.to_vec(),
            gains_db: R82XX_GAINS.iter().map(|&g| g as f64 / 10.0).collect(),
            acquisition: Acquisition::Stream { chunk: CHUNK_PAIRS },
        });
        Ok(Self {
            sdr,
            stream: None,
            caps,
            applied: None,
            seq: 0,
            pairs: 0,
            overflows_seen: 0,
        })
    }
}

impl Backend for RtlBackend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &InstrumentConfig) -> Result<(), BackendError> {
        let c = cfg
            .sdr()
            .ok_or_else(|| BackendError::Transient("scope config sent to an SDR backend".into()))?;
        if !(HF_MIN_HZ..=TUNER_MAX_HZ as f64).contains(&c.centre_hz) {
            return Err(BackendError::Transient(format!(
                "centre {} Hz outside {HF_MIN_HZ}..={TUNER_MAX_HZ} Hz",
                c.centre_hz
            )));
        }
        let prev = self.applied.clone();
        let changed = |f: fn(&SdrConfig) -> u64| prev.as_ref().map(f) != Some(f(c));
        if changed(|c| c.sample_rate.to_bits()) {
            self.sdr
                .set_sample_rate(c.sample_rate.round() as u32)
                .map_err(err)?;
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
        }
        if changed(|c| c.agc as u64) {
            self.sdr.set_rtl_agc(c.agc).map_err(err)?;
        }
        if changed(|c| c.ppm.to_bits()) {
            self.sdr.set_ppm(c.ppm.round() as i32).map_err(err)?;
        }
        match (c.running, self.stream.is_some()) {
            (true, false) => {
                self.stream = Some(self.sdr.stream().map_err(err)?);
                self.overflows_seen = 0;
            }
            (false, true) => self.stream = None,
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
        // Dropped chunks are time that passed: advance the clock over them.
        let overflows = stream.overflows();
        self.pairs += (overflows - self.overflows_seen) * CHUNK_PAIRS as u64;
        self.overflows_seen = overflows;
        let rate = self.sdr.sample_rate() as f64;
        let frame = frame_from_u8(&bytes, self.seq, self.pairs as f64 / rate, rate);
        self.seq += 1;
        self.pairs += (bytes.len() / 2) as u64;
        Ok(Some(std::sync::Arc::new(frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_maps_to_full_scale() {
        let f = frame_from_u8(&[0, 255, 127, 128], 3, 0.5, 2.0);
        assert_eq!(f.layout, SampleLayout::Complex);
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
        for r in SAMPLE_RATES {
            let r = r as u32;
            assert!(r > 225_000 && r <= 3_200_000 && !(300_000 < r && r <= 900_000));
            assert_eq!(rtl::resampler(rtl::RTL_XTAL_HZ, r).1, r, "{r}");
        }
    }
}
