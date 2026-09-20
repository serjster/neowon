//! Signal processing over capture frames. CPU implementations are the
//! correctness oracle; GPU variants (added later, in the app's render world)
//! must match these within tolerance.

pub mod acq;
pub mod classify;
pub mod dab;
pub mod decode;
pub mod demod;
pub mod detect;
pub mod fft;
pub mod iq;
pub mod math;
pub mod measure;
pub mod modlab;
pub mod modmeas;
pub mod stats;
pub mod timeline;

pub use acq::peak_advised;
pub use decode::{DecodeError, Digital, Event, EventKind, Threshold, digitize};
pub use demod::{DemodMode, Receiver, ReceiverConfig};
pub use detect::{DetectConfig, Track, Tracker, TrackerConfig, detect, detect_spectrum};
pub use fft::{Spectrum, Window, spectrum};
pub use iq::{IqSpectrum, iq_spectrum};
pub use math::{MathOp, math_trace};
pub use measure::{
    BasicStats, Measurements, basic_stats, estimate_frequency, measure, measure_envelope,
};
pub use stats::StatTrack;
pub use timeline::{NO_DATA, Reduced, Segment, Tiles, reduce, summarize};
