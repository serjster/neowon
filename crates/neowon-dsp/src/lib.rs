//! Signal processing over capture frames. CPU implementations are the
//! correctness oracle; GPU variants (added later, in the app's render world)
//! must match these within tolerance.

pub mod fft;
pub mod math;
pub mod measure;
pub mod stats;

pub use fft::{spectrum, Spectrum, Window};
pub use math::{math_trace, MathOp};
pub use measure::{basic_stats, estimate_frequency, measure, BasicStats, Measurements};
pub use stats::StatTrack;
