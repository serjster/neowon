//! Instrument ladders, one home each: the discrete settings a
//! backend advertises in its `Capabilities`.
//!
//! They live in core because a simulated backend must offer exactly what
//! the hardware it stands in for offers, and `neowon-sim` must not depend
//! on a driver crate — a copied ladder drifts, and a sim that accepts a
//! rate the device refuses is a lie the tests cannot catch. The driver
//! crates keep their register maps (prescalers, tuner gain registers);
//! what is shared here is the ladder those maps produce.

/// The distinct sample rates the VDS1022's prescaler ladder produces, S/s.
/// `neowon-vds1022` snaps a requested rate onto this; `neowon-sim`'s scope
/// advertises the same range so the time base behaves identically.
pub const SCOPE_SAMPLE_RATES: [f64; 24] = [
    2.5, 5.0, 12.5, 25.0, 50.0, 125.0, 250.0, 500.0, 1.25e3, 2.5e3, 5e3, 12.5e3, 25e3, 50e3, 125e3,
    250e3, 500e3, 1.25e6, 2.5e6, 5e6, 12.5e6, 25e6, 50e6, 100e6,
];

/// Volts/div the scope offers, ascending. The VDS1022 derives it from its
/// `VOLTBASE_MV` register table (a test there pins the two together); the
/// sim and the app's keyboard fallback read it from here.
pub const SCOPE_VOLTS_DIV: [f64; 10] = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];

/// IQ sample rates offered for an RTL2832 front end, pairs/s (the SDR++
/// list; all exact on the 28.8 MHz crystal — `neowon-sdr` has the test).
pub const RTL_SAMPLE_RATES: [f64; 11] = [
    250e3, 1.024e6, 1.536e6, 1.792e6, 1.92e6, 2.048e6, 2.16e6, 2.4e6, 2.56e6, 2.88e6, 3.2e6,
];

/// The R820T/R828D tuner's discrete gains, tenths of a dB, ascending —
/// the unit the tuner's register tables are written in.
pub const R82XX_GAINS_TDB: [i32; 29] = [
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

/// The same tuner gains in dB, as `SdrCaps::gains_db` carries them.
pub fn r82xx_gains_db() -> Vec<f64> {
    R82XX_GAINS_TDB.iter().map(|&g| g as f64 / 10.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascending(v: &[f64]) -> bool {
        v.windows(2).all(|w| w[0] < w[1])
    }

    #[test]
    fn ladders_are_ascending_and_positive() {
        assert!(ascending(&SCOPE_SAMPLE_RATES));
        assert!(ascending(&SCOPE_VOLTS_DIV));
        assert!(ascending(&RTL_SAMPLE_RATES));
        assert!(R82XX_GAINS_TDB.windows(2).all(|w| w[0] < w[1]));
        assert!(SCOPE_SAMPLE_RATES.iter().all(|&r| r > 0.0));
        assert!(RTL_SAMPLE_RATES.iter().all(|&r| r > 0.0));
    }

    #[test]
    fn gains_convert_to_tenths_exactly() {
        let db = r82xx_gains_db();
        assert_eq!(db.len(), R82XX_GAINS_TDB.len());
        assert_eq!(db.first().copied(), Some(0.0));
        assert_eq!(db.last().copied(), Some(49.6));
        for (&tdb, db) in R82XX_GAINS_TDB.iter().zip(&db) {
            assert_eq!((db * 10.0).round() as i32, tdb);
        }
    }
}
