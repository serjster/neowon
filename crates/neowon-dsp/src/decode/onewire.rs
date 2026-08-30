//! Dallas/Maxim 1-Wire.
//!
//! Everything is carried by how long the master holds the line low, so this
//! decoder measures pulse widths rather than sampling a clock. Standard
//! speed timings (the overwhelmingly common case):
//!
//! - reset pulse: master low ≥ 480 µs, then a presence pulse from the slave
//! - write 1 / read slot: low 1–15 µs
//! - write 0: low 60–120 µs
//!
//! Bits are least-significant first.

use super::{DecodeError, Digital, Event, EventKind};

/// Timing thresholds in seconds, standard speed.
const RESET_LOW: f64 = 400e-6;
const ZERO_LOW: f64 = 30e-6;
/// A low longer than this is not a data slot at all.
const SLOT_MAX: f64 = 200e-6;

pub fn decode(d: &Digital) -> Result<Vec<Event>, DecodeError> {
    if d.sample_rate <= 0.0 {
        return Err(DecodeError::BadParameter("sample rate"));
    }
    // The shortest thing to resolve is a "1" slot, ~1-15 us. Insist on
    // enough samples to tell it from a "0".
    let per_us = d.sample_rate / 1e6;
    if per_us * 15.0 < super::MIN_SAMPLES_BIT {
        return Err(DecodeError::TooFewSamples {
            have: (per_us * 15.0) as u32,
            need: super::MIN_SAMPLES_BIT as u32,
        });
    }

    let mut events = Vec::new();
    let mut bits: u32 = 0;
    let mut value: u64 = 0;
    let mut byte_start = 0usize;
    let mut fall: Option<usize> = None;

    for (i, level) in d.edges() {
        if level {
            // Rising edge ends a low pulse; its width says what it was.
            let Some(start) = fall.take() else { continue };
            let width = (i - start) as f64 / d.sample_rate;
            if width >= RESET_LOW {
                if bits > 0 {
                    events.push(Event {
                        start: byte_start,
                        end: i,
                        kind: EventKind::Error("reset mid-byte"),
                    });
                }
                events.push(Event {
                    start,
                    end: i,
                    kind: EventKind::Marker("RESET"),
                });
                bits = 0;
                value = 0;
                continue;
            }
            if width > SLOT_MAX {
                events.push(Event {
                    start,
                    end: i,
                    kind: EventKind::Error("low pulse too long for a slot"),
                });
                bits = 0;
                value = 0;
                continue;
            }
            if bits == 0 {
                byte_start = start;
            }
            // Short low = 1, long low = 0.
            if width < ZERO_LOW {
                value |= 1 << bits;
            }
            bits += 1;
            if bits == 8 {
                events.push(Event {
                    start: byte_start,
                    end: i,
                    kind: EventKind::Word { value, bits: 8 },
                });
                bits = 0;
                value = 0;
            }
        } else {
            fall = Some(i);
        }
    }

    if events.is_empty() {
        return Err(DecodeError::NoActivity);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::super::digitize::{Threshold, digitize};
    use super::*;

    struct Builder {
        raw: Vec<i8>,
        rate: f64,
    }

    impl Builder {
        fn new(rate: f64) -> Self {
            Self {
                raw: vec![100; (rate * 20e-6) as usize],
                rate,
            }
        }
        fn low(&mut self, us: f64) -> &mut Self {
            let n = (self.rate * us * 1e-6) as usize;
            self.raw.extend(std::iter::repeat_n(-100i8, n));
            self
        }
        fn high(&mut self, us: f64) -> &mut Self {
            let n = (self.rate * us * 1e-6) as usize;
            self.raw.extend(std::iter::repeat_n(100i8, n));
            self
        }
        fn reset(&mut self) -> &mut Self {
            self.low(500.0).high(70.0)
        }
        fn byte(&mut self, b: u8) -> &mut Self {
            for k in 0..8 {
                if b & (1 << k) != 0 {
                    self.low(6.0).high(60.0);
                } else {
                    self.low(70.0).high(10.0);
                }
            }
            self
        }
    }

    #[test]
    fn decodes_a_reset_and_the_rom_command() {
        let rate = 10e6; // 10 samples per microsecond
        let mut b = Builder::new(rate);
        b.reset().byte(0xCC).byte(0x44); // SKIP ROM, CONVERT T
        let d = digitize(&b.raw, rate, Threshold::default()).unwrap();
        let ev = decode(&d).unwrap();
        assert!(ev.iter().any(|e| e.kind == EventKind::Marker("RESET")));
        let words: Vec<u64> = ev
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Word { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(words, vec![0xCC, 0x44]);
    }

    #[test]
    fn a_capture_too_slow_to_separate_slots_refuses() {
        // 100 kS/s: a 15 us slot is 1.5 samples.
        let rate = 100e3;
        let mut b = Builder::new(rate);
        b.reset().byte(0xCC);
        let d = digitize(&b.raw, rate, Threshold::default()).unwrap();
        assert!(matches!(decode(&d), Err(DecodeError::TooFewSamples { .. })));
    }
}
