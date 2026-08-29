//! Signal processing over capture frames. CPU implementations are the
//! correctness oracle; GPU variants (added later, in the app's render world)
//! must match these within tolerance.

pub mod measure;

pub use measure::{BasicStats, basic_stats, estimate_frequency};
