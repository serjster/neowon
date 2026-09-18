//! What detection reports (D7): `neowon-core` owns the record,
//! `neowon-dsp` emits it, the catalog wraps it, classifiers annotate it.

/// One signal seen in one spectrogram frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalObservation {
    /// Power-weighted centre, Hz (absolute: tuned centre + offset).
    pub centre_hz: f64,
    /// Occupied band: lower and upper edges where the signal crosses the
    /// detection threshold, Hz (absolute).
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Integrated in-band power, dBFS.
    pub power_dbfs: f64,
    /// In-band power over the noise floor integrated across the same
    /// band, dB.
    pub snr_db: f64,
    /// The frame the observation came from, seconds on the capture clock.
    pub t_start: f64,
    pub t_end: f64,
}

impl SignalObservation {
    pub fn bandwidth_hz(&self) -> f64 {
        self.hi_hz - self.lo_hz
    }
}
