//! Signal processing over capture frames. CPU implementations are the
//! correctness oracle; GPU variants (added later, in the app's render world)
//! must match these within tolerance.

pub mod acq;
pub mod fft;
pub mod math;
pub mod measure;
pub mod stats;

pub use acq::peak_advised;
pub use fft::{Spectrum, Window, spectrum};
pub use math::{MathOp, math_trace};
pub use measure::{
    BasicStats, Measurements, basic_stats, estimate_frequency, measure, measure_envelope,
};
pub use stats::StatTrack;
