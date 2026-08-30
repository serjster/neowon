//! Signal processing over capture frames. CPU implementations are the
//! correctness oracle; GPU variants (added later, in the app's render world)
//! must match these within tolerance.

pub mod acq;
pub mod decode;
pub mod fft;
pub mod math;
pub mod measure;
pub mod stats;
pub mod timeline;

pub use acq::peak_advised;
pub use decode::{DecodeError, Digital, Event, EventKind, Threshold, digitize};
pub use fft::{Spectrum, Window, spectrum};
pub use math::{MathOp, math_trace};
pub use measure::{
    BasicStats, Measurements, basic_stats, estimate_frequency, measure, measure_envelope,
};
pub use stats::StatTrack;
pub use timeline::{NO_DATA, Reduced, Segment, Tiles, reduce, summarize};
