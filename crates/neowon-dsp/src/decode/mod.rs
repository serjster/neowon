//! Protocol decoding: analogue samples in, annotated protocol events out.
//!
//! Two layers, kept apart on purpose. **Digitizing** turns a captured
//! waveform into logic levels — most decode failures are really
//! digitizing failures, so it is a separate, testable step with its own
//! choices. **Transport decoders** (UART, I²C, SPI, 1-Wire) turn logic
//! levels into bytes and framing errors.
//!
//! Nothing here knows about an instrument: a decoder takes samples and a
//! sample rate. That makes them the most portable thing in the workspace —
//! they work identically on a scope record, a sound card stream, or a file.
//!
//! **Refusing beats guessing.** Below `MIN_SAMPLES_BIT` samples per bit the
//! edges cannot be located well enough to trust the result, and the decoder
//! returns an error saying so rather than emitting plausible bytes. A
//! decoder that quietly produces wrong data is worse than one that declines.

pub mod digitize;
pub mod i2c;
pub mod onewire;
pub mod spi;
pub mod uart;

pub use digitize::{Digital, Threshold, digitize};

/// Samples per bit below which decoding is refused. Edges are located by
/// interpolation between samples, so a handful of samples per bit puts the
/// bit boundaries inside the error bars.
pub const MIN_SAMPLES_BIT: f64 = 12.0;

/// One decoded item, positioned in sample space so the UI can draw it over
/// the trace and a table can sort it.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// First and last sample this spans, inclusive of the first.
    pub start: usize,
    pub end: usize,
    pub kind: EventKind,
}

impl Event {
    pub fn seconds(&self, sample_rate: f64) -> (f64, f64) {
        (
            self.start as f64 / sample_rate,
            self.end as f64 / sample_rate,
        )
    }
}

/// What was decoded. Deliberately shared across protocols where the meaning
/// is the same, so a UI can render "a byte" once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// Frame/transaction boundary, named by the protocol (START, STOP,
    /// RESET, SELECT…).
    Marker(&'static str),
    /// A decoded data word and how wide it is in bits.
    Word { value: u64, bits: u8 },
    /// An acknowledgement bit, where the protocol has one.
    Ack(bool),
    /// The decode failed here, with the reason a human needs.
    Error(&'static str),
}

impl EventKind {
    pub fn is_error(&self) -> bool {
        matches!(self, EventKind::Error(_))
    }
}

/// Why a decode could not be attempted at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer than `MIN_SAMPLES_BIT` samples per bit.
    TooFewSamples { have: u32, need: u32 },
    /// The capture holds no transitions to work with.
    NoActivity,
    /// A parameter makes no sense (zero baud, a bad word length…).
    BadParameter(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::TooFewSamples { have, need } => write!(
                f,
                "resolution too low to decode reliably: {have} samples per bit, need {need}. \
                 Use a faster time base."
            ),
            DecodeError::NoActivity => write!(f, "no transitions in the capture"),
            DecodeError::BadParameter(p) => write!(f, "bad parameter: {p}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Guard every decoder shares: refuse rather than emit garbage.
pub fn check_resolution(sample_rate: f64, bit_rate: f64) -> Result<f64, DecodeError> {
    if bit_rate <= 0.0 || sample_rate <= 0.0 {
        return Err(DecodeError::BadParameter("bit rate"));
    }
    let per_bit = sample_rate / bit_rate;
    if per_bit < MIN_SAMPLES_BIT {
        return Err(DecodeError::TooFewSamples {
            have: per_bit as u32,
            need: MIN_SAMPLES_BIT as u32,
        });
    }
    Ok(per_bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_guard_refuses_rather_than_guesses() {
        // 115200 baud at 1 MS/s is 8.7 samples/bit — under the floor.
        assert_eq!(
            check_resolution(1e6, 115_200.0),
            Err(DecodeError::TooFewSamples { have: 8, need: 12 })
        );
        // The same baud at 2.5 MS/s is fine.
        assert!(check_resolution(2.5e6, 115_200.0).unwrap() > 20.0);
        assert_eq!(
            check_resolution(1e6, 0.0),
            Err(DecodeError::BadParameter("bit rate"))
        );
    }

    #[test]
    fn the_refusal_says_what_to_do_about_it() {
        let e = check_resolution(1e6, 115_200.0).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("samples per bit"), "{msg}");
        assert!(msg.contains("faster time base"), "{msg}");
    }
}
