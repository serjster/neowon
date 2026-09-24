//! What detection reports: `neowon-core` owns the record,
//! `neowon-dsp` emits it, the catalog wraps it, classifiers annotate it.
//! A survey's per-band coverage follows the same direction, so the scan
//! engine needs no storage crate to describe what it looked at.

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

/// What a survey did to one band; an unscanned or truncated band leaves
/// its signals `unknown`, never `gone`. The scan engine writes it
/// and the catalog stores it (`neowon_catalog::CoverageRecord`).
#[derive(Debug, Clone, PartialEq)]
pub struct BandCoverage {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub scanned: bool,
    pub bins: usize,
    pub threshold_db: f64,
    pub truncated: bool,
    pub peak_cap: usize,
    pub selection_rule: String,
    /// Power of the weakest peak kept when the step was truncated; a
    /// signal below it could have been there unseen.
    pub retained_power_floor_dbfs: f64,
}
