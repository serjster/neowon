use std::sync::Arc;

use crate::{AcqMode, Acquisition};

/// Sample domain of a frame's channel data (D1b: the one layout home is the
/// frame, so every consumer can read it without guessing from channel
/// shapes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleLayout {
    /// One real scalar per sample.
    Real,
    /// Interleaved I, Q pairs: `data[2k]` = I, `data[2k+1]` = Q.
    Complex,
}

/// Per-component calibration: `volts = value * scale + offset`.
///
/// For a [`SampleLayout::Real`] frame the Q fields mirror I, so a real frame
/// can be treated uniformly. `scale_i` is volts per ADC count for scope
/// backends (i8 wire encoding converted at frame construction).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IqCal {
    pub scale_i: f64,
    pub scale_q: f64,
    pub offset_i: f64,
    pub offset_q: f64,
}

impl IqCal {
    /// Real calibration: the Q component mirrors I.
    pub fn real(scale: f64, offset: f64) -> Self {
        Self {
            scale_i: scale,
            scale_q: scale,
            offset_i: offset,
            offset_q: offset,
        }
    }

    pub fn volts_i(&self, v: f32) -> f64 {
        v as f64 * self.scale_i + self.offset_i
    }

    pub fn volts_q(&self, v: f32) -> f64 {
        v as f64 * self.scale_q + self.offset_q
    }
}

/// Why a frame could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// `Complex` data only makes sense for sampled streams (D6): a record,
    /// or a peak/average acquisition, cannot be complex.
    ComplexRecordRejected,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::ComplexRecordRejected => {
                write!(f, "complex samples require sampled acquisition")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// One acquisition record: a set of simultaneously captured channel traces.
///
/// Frames are immutable once produced and shared by `Arc` between the
/// acquisition thread, DSP consumers, and the renderer.
#[derive(Debug, Clone)]
pub struct CaptureFrame {
    /// Monotonic sequence number assigned by the producing backend.
    pub seq: u64,
    /// When this record's **first sample** was taken, in seconds on a
    /// monotonic per-session clock. `None` when the producer cannot say.
    ///
    /// This is what lets consumers place records on a real time axis
    /// instead of assuming they are contiguous — they generally are not.
    /// Accuracy is producer-defined: a streaming source knows it exactly,
    /// while a triggered instrument that is polled over USB can only report
    /// arrival minus record duration, which is biased late by up to one poll
    /// interval.
    pub t_capture: Option<f64>,
    /// Actual sample rate of this record. For `Complex` frames this is I/Q
    /// **pairs** per second; Real frames are samples/second.
    pub sample_rate: f64,
    /// How the samples were produced (affects interpretation: peak-detect
    /// records are min/max pairs).
    pub acq: AcqMode,
    /// Sample domain of every channel in this frame.
    pub layout: SampleLayout,
    pub channels: Vec<ChannelCapture>,
}

pub type SharedFrame = Arc<CaptureFrame>;

impl CaptureFrame {
    /// Construct a frame, rejecting the invalid cells (D6): `Complex` is
    /// only valid on a `Stream` delivered in `AcqMode::Sample`. `delivery` is
    /// the producing instrument's `Acquisition`; it is checked, not stored.
    /// Direct struct-literal construction remains possible but skips this
    /// check.
    pub fn new(
        seq: u64,
        t_capture: Option<f64>,
        sample_rate: f64,
        acq: AcqMode,
        delivery: Acquisition,
        layout: SampleLayout,
        channels: Vec<ChannelCapture>,
    ) -> Result<Self, FrameError> {
        if layout == SampleLayout::Complex
            && (!matches!(acq, AcqMode::Sample) || !delivery.is_stream())
        {
            return Err(FrameError::ComplexRecordRejected);
        }
        Ok(Self {
            seq,
            t_capture,
            sample_rate,
            acq,
            layout,
            channels,
        })
    }

    /// Seconds of signal in this record. A complex frame's `data` holds
    /// interleaved I/Q pairs, so its unit count is half its scalar length.
    pub fn duration(&self) -> f64 {
        let n = self
            .channels
            .first()
            .map_or(0, |c| c.unit_count(self.layout));
        n as f64 / self.sample_rate.max(1e-12)
    }

    /// Start time on the session clock, falling back to a contiguous
    /// estimate from `seq` when the producer could not stamp one. Loaders of
    /// stored captures use the fallback, so a file with no timestamps still
    /// lands on a sensible axis — it just cannot show real gaps.
    pub fn t_start(&self) -> f64 {
        self.t_capture
            .unwrap_or_else(|| self.seq as f64 * self.duration())
    }
}

/// A single channel's samples within a frame.
///
/// `data` carries raw sample values: a real scope backend stores its ADC
/// counts here (the i8 ±125 convention survives as the **wire encoding**,
/// converted to `f32` counts at frame construction), and `cal` recovers
/// volts. A complex channel stores interleaved I, Q, each calibrated by
/// `cal.scale_i`/`offset_i` and `cal.scale_q`/`offset_q`.
#[derive(Debug, Clone)]
pub struct ChannelCapture {
    /// Zero-based channel index.
    pub ch: usize,
    /// Real: `n` scalars. Complex: `2n` interleaved I, Q values.
    pub data: Vec<f32>,
    pub cal: IqCal,
    /// True if any sample sits at the ADC rails (|value| >= 125).
    pub clipped: bool,
    /// Hardware frequency-meter reading, when the backend provides one.
    pub freq_meter: Option<f64>,
}

impl ChannelCapture {
    /// Number of units (samples, or I/Q pairs) the channel holds.
    pub fn unit_count(&self, layout: SampleLayout) -> usize {
        match layout {
            SampleLayout::Real => self.data.len(),
            SampleLayout::Complex => self.data.len() / 2,
        }
    }

    /// Volts of real sample `i` (the I component for a complex channel — use
    /// [`ChannelCapture::iq_at`] for complex data).
    pub fn volts_at(&self, i: usize) -> f64 {
        self.cal.volts_i(self.data[i])
    }

    pub fn iter_volts(&self) -> impl Iterator<Item = f64> + '_ {
        self.data.iter().map(move |&v| self.cal.volts_i(v))
    }

    /// The I/Q pair at pair index `i`.
    pub fn iq_at(&self, i: usize) -> (f32, f32) {
        (self.data[2 * i], self.data[2 * i + 1])
    }

    pub fn iter_iq(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.data.chunks_exact(2).map(|p| (p[0], p[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: Acquisition = Acquisition::Stream { chunk: 1 };

    fn real_cap(data: Vec<f32>) -> ChannelCapture {
        ChannelCapture {
            ch: 0,
            data,
            cal: IqCal::real(0.01, -0.5),
            clipped: false,
            freq_meter: None,
        }
    }

    #[test]
    fn complex_requires_sampled_stream() {
        assert_eq!(
            CaptureFrame::new(
                1,
                None,
                1.0,
                AcqMode::Peak,
                STREAM,
                SampleLayout::Complex,
                vec![real_cap(vec![1.0, 2.0])],
            )
            .unwrap_err(),
            FrameError::ComplexRecordRejected
        );
        assert_eq!(
            CaptureFrame::new(
                1,
                None,
                1.0,
                AcqMode::Average(4),
                STREAM,
                SampleLayout::Complex,
                vec![real_cap(vec![1.0, 2.0])],
            )
            .unwrap_err(),
            FrameError::ComplexRecordRejected
        );
        assert!(
            CaptureFrame::new(
                1,
                None,
                1.0,
                AcqMode::Sample,
                STREAM,
                SampleLayout::Complex,
                vec![real_cap(vec![1.0, 2.0])],
            )
            .is_ok()
        );
        assert_eq!(
            CaptureFrame::new(
                1,
                None,
                1.0,
                AcqMode::Sample,
                Acquisition::Record { samples: 1 },
                SampleLayout::Complex,
                vec![real_cap(vec![1.0, 2.0])],
            )
            .unwrap_err(),
            FrameError::ComplexRecordRejected
        );
    }

    #[test]
    fn duration_counts_pairs_for_complex() {
        let cap = real_cap(vec![0.0; 20]);
        let real = CaptureFrame::new(
            1,
            None,
            10.0,
            AcqMode::Sample,
            STREAM,
            SampleLayout::Real,
            vec![cap.clone()],
        )
        .unwrap();
        assert!((real.duration() - 2.0).abs() < 1e-12);
        let complex = CaptureFrame::new(
            1,
            None,
            10.0,
            AcqMode::Sample,
            STREAM,
            SampleLayout::Complex,
            vec![cap],
        )
        .unwrap();
        assert!((complex.duration() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn complex_components_calibrate_separately() {
        let cap = ChannelCapture {
            ch: 0,
            data: vec![10.0, -10.0],
            cal: IqCal {
                scale_i: 0.5,
                scale_q: 2.0,
                offset_i: 1.0,
                offset_q: -1.0,
            },
            clipped: false,
            freq_meter: None,
        };
        assert_eq!(cap.iq_at(0), (10.0, -10.0));
        assert!((cap.cal.volts_i(10.0) - 6.0).abs() < 1e-12);
        assert!((cap.cal.volts_q(-10.0) + 21.0).abs() < 1e-12);
        assert_eq!(cap.iter_iq().collect::<Vec<_>>(), vec![(10.0, -10.0)]);
    }
}
