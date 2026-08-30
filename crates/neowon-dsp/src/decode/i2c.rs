//! I²C: clocked, two wires, framing carried by SDA transitions while SCL
//! is high.
//!
//! Unlike UART there is no bit rate to guess — the clock is on the wire —
//! so the resolution guard uses the clock the capture actually shows.

use super::{DecodeError, Digital, Event, EventKind, MIN_SAMPLES_BIT};

/// Decode SDA against SCL. Both must be digitized from the same capture, so
/// their sample indices line up.
pub fn decode(scl: &Digital, sda: &Digital) -> Result<Vec<Event>, DecodeError> {
    let n = scl.levels.len().min(sda.levels.len());
    if n < 4 {
        return Err(DecodeError::NoActivity);
    }
    // The observed clock period sets the resolution check: the shortest
    // high phase is the tightest thing we have to resolve.
    let mut shortest = usize::MAX;
    let mut last_rise = None;
    for (i, level) in scl.edges() {
        if i >= n {
            break;
        }
        if level {
            last_rise = Some(i);
        } else if let Some(r) = last_rise {
            shortest = shortest.min(i - r);
        }
    }
    if shortest == usize::MAX {
        return Err(DecodeError::NoActivity);
    }
    if (shortest as f64) < MIN_SAMPLES_BIT / 2.0 {
        return Err(DecodeError::TooFewSamples {
            have: shortest as u32 * 2,
            need: MIN_SAMPLES_BIT as u32,
        });
    }

    let mut events = Vec::new();
    let mut bits: u32 = 0;
    let mut value: u16 = 0;
    let mut byte_start = 0usize;
    let mut in_frame = false;

    for i in 1..n {
        let scl_high = scl.levels[i];
        let scl_rise = scl_high && !scl.levels[i - 1];
        let sda_now = sda.levels[i];
        let sda_changed = sda_now != sda.levels[i - 1];

        // START / STOP: SDA moves while SCL is held high.
        if scl_high && !scl_rise && sda_changed {
            events.push(Event {
                start: i,
                end: i,
                kind: EventKind::Marker(if sda_now { "STOP" } else { "START" }),
            });
            in_frame = !sda_now;
            bits = 0;
            value = 0;
            byte_start = i;
            continue;
        }

        // Data is sampled on the rising clock edge.
        if scl_rise && in_frame {
            if bits == 0 {
                byte_start = i;
            }
            if bits < 8 {
                value = (value << 1) | sda_now as u16;
                bits += 1;
            } else {
                // The ninth clock is the acknowledge, driven low by the
                // receiver.
                events.push(Event {
                    start: byte_start,
                    end: i,
                    kind: EventKind::Word {
                        value: value as u64,
                        bits: 8,
                    },
                });
                events.push(Event {
                    start: i,
                    end: i,
                    kind: EventKind::Ack(!sda_now),
                });
                bits = 0;
                value = 0;
            }
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

    /// Build SCL/SDA waveforms for a write transaction.
    fn transaction(addr: u8, data: &[u8], per_bit: usize) -> (Vec<i8>, Vec<i8>) {
        let (mut scl, mut sda) = (Vec::new(), Vec::new());
        let mut push = |c: bool, d: bool, n: usize| {
            for _ in 0..n {
                scl.push(if c { 100i8 } else { -100 });
                sda.push(if d { 100i8 } else { -100 });
            }
        };
        // Idle high, then START: SDA falls while SCL is high.
        push(true, true, per_bit * 2);
        push(true, false, per_bit);
        let byte = |b: u8, ack: bool, push: &mut dyn FnMut(bool, bool, usize)| {
            for k in (0..8).rev() {
                let bit = b & (1 << k) != 0;
                push(false, bit, per_bit / 2);
                push(true, bit, per_bit / 2);
            }
            push(false, !ack, per_bit / 2);
            push(true, !ack, per_bit / 2);
        };
        byte(addr, true, &mut push);
        for &b in data {
            byte(b, true, &mut push);
        }
        // STOP: SDA rises while SCL is high.
        push(false, false, per_bit / 2);
        push(true, false, per_bit / 2);
        push(true, true, per_bit * 2);
        (scl, sda)
    }

    #[test]
    fn decodes_address_data_and_acks() {
        let per_bit = 40;
        let (scl, sda) = transaction(0x50, &[0xAB, 0x3C], per_bit);
        let rate = 1e6;
        let d_scl = digitize(&scl, rate, Threshold::default()).unwrap();
        let d_sda = digitize(&sda, rate, Threshold::default()).unwrap();
        let ev = decode(&d_scl, &d_sda).unwrap();

        let words: Vec<u64> = ev
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Word { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(words, vec![0x50, 0xAB, 0x3C]);
        assert!(ev.iter().any(|e| e.kind == EventKind::Marker("START")));
        assert!(ev.iter().any(|e| e.kind == EventKind::Marker("STOP")));
        assert!(
            ev.iter().filter(|e| e.kind == EventKind::Ack(true)).count() >= 3,
            "every byte was acked: {ev:?}"
        );
    }

    #[test]
    fn a_clock_too_fast_for_the_capture_refuses() {
        let (scl, sda) = transaction(0x50, &[0x01], 6);
        let rate = 1e6;
        let d_scl = digitize(&scl, rate, Threshold::default()).unwrap();
        let d_sda = digitize(&sda, rate, Threshold::default()).unwrap();
        assert!(matches!(
            decode(&d_scl, &d_sda),
            Err(DecodeError::TooFewSamples { .. })
        ));
    }
}
