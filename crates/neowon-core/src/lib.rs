//! Engine-free core types shared by every neowon crate: capture frames,
//! channel/trigger vocabulary. No I/O, no device specifics, no GPU.

pub mod frame;

pub use frame::{CaptureFrame, ChannelCapture, SharedFrame};

/// Input coupling. Hardware encodings are backend-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coupling {
    Ac,
    Dc,
    Gnd,
}

/// Trigger edge slope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slope {
    Rising,
    Falling,
}

/// Trigger sweep mode. `Auto` acquires even without a trigger event,
/// `Normal` waits for one, `Single` arms once and stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sweep {
    Auto,
    Normal,
    Single,
}
