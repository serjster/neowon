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

/// Acquisition mode. `Peak` captures min/max pairs (odd = max, even = min on
/// the VDS1022); `Average` is a host-side running average over N records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcqMode {
    Sample,
    Peak,
    Average(u8),
}

/// Pulse/slope trigger condition: polarity of the excursion and how its
/// width compares to the configured width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PulseCondition {
    PositiveGreater,
    PositiveEqual,
    PositiveLess,
    NegativeGreater,
    NegativeEqual,
    NegativeLess,
}

impl PulseCondition {
    pub const ALL: [PulseCondition; 6] = [
        PulseCondition::PositiveGreater,
        PulseCondition::PositiveEqual,
        PulseCondition::PositiveLess,
        PulseCondition::NegativeGreater,
        PulseCondition::NegativeEqual,
        PulseCondition::NegativeLess,
    ];

    /// Hardware condition code (trigger word bits 5-7): bit 2 = polarity,
    /// bits 0-1 = comparator. Negative codes are 4/5/6, NOT 3/4/5 —
    /// hardware-verified; the Python reference's 3/4/5 makes negative
    /// conditions starve.
    pub fn code(self) -> u16 {
        match self {
            PulseCondition::PositiveGreater => 0,
            PulseCondition::PositiveEqual => 1,
            PulseCondition::PositiveLess => 2,
            PulseCondition::NegativeGreater => 4,
            PulseCondition::NegativeEqual => 5,
            PulseCondition::NegativeLess => 6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PulseCondition::PositiveGreater => "+ >",
            PulseCondition::PositiveEqual => "+ =",
            PulseCondition::PositiveLess => "+ <",
            PulseCondition::NegativeGreater => "− >",
            PulseCondition::NegativeEqual => "− =",
            PulseCondition::NegativeLess => "− <",
        }
    }
}

/// Video trigger sync mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSync {
    Line,
    Field,
    OddField,
    EvenField,
    /// Trigger on a specific line number.
    LineNumber,
}

impl VideoSync {
    pub const ALL: [VideoSync; 5] = [
        VideoSync::Line,
        VideoSync::Field,
        VideoSync::OddField,
        VideoSync::EvenField,
        VideoSync::LineNumber,
    ];

    /// Hardware sync code (trigger word bits 10-12).
    pub fn code(self) -> u16 {
        match self {
            VideoSync::Line => 0,
            VideoSync::Field => 1,
            VideoSync::OddField => 2,
            VideoSync::EvenField => 3,
            VideoSync::LineNumber => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoSync::Line => "Line",
            VideoSync::Field => "Field",
            VideoSync::OddField => "Odd field",
            VideoSync::EvenField => "Even field",
            VideoSync::LineNumber => "Line #",
        }
    }
}

/// What the trigger hardware is armed on. Widths are seconds; slope
/// `upper`/`lower` thresholds and pulse/edge levels are volts at the probe
/// tip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TriggerKind {
    Edge {
        slope: Slope,
    },
    Pulse {
        condition: PulseCondition,
        width: f64,
    },
    Slope {
        condition: PulseCondition,
        width: f64,
        /// Upper threshold of the slope window (upper > lower).
        upper: f64,
        lower: f64,
    },
    Video {
        sync: VideoSync,
        /// Line number; only meaningful with `sync = LineNumber`.
        line: u16,
    },
}

impl TriggerKind {
    pub fn label(&self) -> &'static str {
        match self {
            TriggerKind::Edge { .. } => "Edge",
            TriggerKind::Pulse { .. } => "Pulse",
            TriggerKind::Slope { .. } => "Slope",
            TriggerKind::Video { .. } => "Video",
        }
    }
}
