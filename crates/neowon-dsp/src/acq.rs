//! Acquisition-mode advice: when the time base is too slow to represent the
//! signal, a scope switches to peak detect so the envelope survives the
//! decimation instead of aliasing into a flat line.
//!
//! Rigol: peak detect means "signal aliasing can be prevented". Keysight: "at
//! slower time/div settings, the maximum and minimum samples in the effective
//! sample period are stored".

/// Samples per cycle below which decimation destroys the waveform.
pub const ALIAS_ENGAGE: f64 = 8.0;
/// Samples per cycle above which plain sampling represents it again. The gap
/// between the two is deliberate: switching peak mode costs a register write
/// that resets the acquisition buffer, so the decision must not chatter.
pub const ALIAS_RELEASE: f64 = 16.0;

/// Should peak detect be engaged?
///
/// `freq` must come from a source that is **independent of the sample rate** —
/// on the VDS1022 that is the hardware frequency meter carried in every frame.
/// Measuring the frequency from the record itself cannot work here: by the time
/// the record aliases, the estimate has aliased with it and reports a low
/// frequency, which is exactly the "no need for peak detect" answer.
///
/// `engaged` is the current state, so the band between the two thresholds
/// latches. `None` leaves the decision unchanged rather than guessing.
pub fn peak_advised(sample_rate: f64, freq: Option<f64>, engaged: bool) -> bool {
    let Some(freq) = freq.filter(|f| *f > 0.0) else {
        return engaged;
    };
    let per_cycle = sample_rate / freq;
    if per_cycle < ALIAS_ENGAGE {
        true
    } else if per_cycle > ALIAS_RELEASE {
        false
    } else {
        engaged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engages_when_the_record_can_no_longer_represent_the_signal() {
        // The reported case: 1 kHz at 1 s/div is 500 S/s = 0.5 samples/cycle.
        assert!(peak_advised(500.0, Some(1000.0), false));
        // Comfortably sampled: 250 kS/s on 1 kHz is 250 samples/cycle.
        assert!(!peak_advised(250e3, Some(1000.0), true));
    }

    #[test]
    fn the_band_between_thresholds_latches() {
        // 12 samples/cycle sits between engage (8) and release (16).
        assert!(peak_advised(12e3, Some(1000.0), true));
        assert!(!peak_advised(12e3, Some(1000.0), false));
    }

    #[test]
    fn thresholds_are_exclusive_at_the_boundaries() {
        assert!(!peak_advised(8e3, Some(1000.0), false)); // exactly 8
        assert!(peak_advised(7.9e3, Some(1000.0), false));
        assert!(peak_advised(16e3, Some(1000.0), true)); // exactly 16
        assert!(!peak_advised(16.1e3, Some(1000.0), true));
    }

    #[test]
    fn no_frequency_leaves_the_decision_alone() {
        assert!(peak_advised(500.0, None, true));
        assert!(!peak_advised(500.0, None, false));
        // A meter reading zero (no edges seen) is not a frequency.
        assert!(!peak_advised(500.0, Some(0.0), false));
    }
}
