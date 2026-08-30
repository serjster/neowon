//! Asynchronous serial (UART / RS-232 / RS-485).
//!
//! Sampling strategy follows what a good analyser does rather than the
//! textbook: read the bit at its **midpoint**, but also check the level is
//! **steady from 20 % to 80 %** of the bit. A transition inside that window
//! means the configured baud rate does not match the signal, and saying so
//! is far more useful than emitting bytes that happen to fall out of a
//! misaligned grid.

use super::{DecodeError, Digital, Event, EventKind, check_resolution};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub baud: f64,
    /// Data bits per frame, 5..=9.
    pub bits: u8,
    pub parity: Parity,
    /// Stop bits; 2 is accepted but only the first is checked.
    pub stop_bits: u8,
    /// Idle-low signalling (RS-232 after an inverting transceiver).
    pub inverted: bool,
    /// Least-significant bit first, as almost everything does.
    pub lsb_first: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            baud: 115_200.0,
            bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            inverted: false,
            lsb_first: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// Idle gap, in bit times, that separates one packet from the next.
const PACKET_GAP_BITS: f64 = 10.0;

pub fn decode(d: &Digital, cfg: Config) -> Result<Vec<Event>, DecodeError> {
    if !(5..=9).contains(&cfg.bits) {
        return Err(DecodeError::BadParameter("data bits must be 5..=9"));
    }
    let per_bit = check_resolution(d.sample_rate, cfg.baud)?;
    let level = |i: usize| d.level_at(i) != cfg.inverted;

    let mut events = Vec::new();
    let mut i = 0usize;
    let n = d.levels.len();
    let mut last_end: Option<usize> = None;

    while i + 1 < n {
        // A start bit is the falling edge out of idle.
        if !level(i) || level(i + 1) {
            i += 1;
            continue;
        }
        let start = i + 1;
        let frame_bits = 1 + cfg.bits as usize + (cfg.parity != Parity::None) as usize;
        let end = start + ((frame_bits as f64 + cfg.stop_bits as f64) * per_bit) as usize;
        if end >= n {
            break;
        }

        // A long idle before this frame separates packets.
        if let Some(prev) = last_end
            && (start - prev) as f64 > PACKET_GAP_BITS * per_bit
        {
            events.push(Event {
                start: prev,
                end: start,
                kind: EventKind::Marker("idle"),
            });
        }

        // Bit k's centre, and the window it must be steady across.
        let centre = |k: usize| start + ((k as f64 + 0.5) * per_bit) as usize;
        let stable = |k: usize| {
            let lo = start + ((k as f64 + 0.2) * per_bit) as usize;
            let hi = start + ((k as f64 + 0.8) * per_bit) as usize;
            d.steady(lo, hi)
        };

        if level(centre(0)) {
            events.push(Event {
                start,
                end: centre(0),
                kind: EventKind::Error("start bit not low"),
            });
            i = start + 1;
            continue;
        }

        let mut value = 0u64;
        let mut ones = 0u32;
        let mut unstable = false;
        for b in 0..cfg.bits as usize {
            let k = 1 + b;
            unstable |= !stable(k);
            let bit = level(centre(k));
            if bit {
                ones += 1;
                let pos = if cfg.lsb_first {
                    b
                } else {
                    cfg.bits as usize - 1 - b
                };
                value |= 1 << pos;
            }
        }

        let mut kind = EventKind::Word {
            value,
            bits: cfg.bits,
        };
        if unstable {
            // The grid does not line up with the signal: the usual cause is
            // a wrong baud rate, and guessing bytes from it is worthless.
            kind = EventKind::Error("bit unstable mid-cell (baud rate wrong?)");
        } else if cfg.parity != Parity::None {
            let p = level(centre(1 + cfg.bits as usize));
            let want = match cfg.parity {
                Parity::Even => ones % 2 == 1,
                Parity::Odd => ones.is_multiple_of(2),
                Parity::None => unreachable!(),
            };
            if p != want {
                kind = EventKind::Error("parity");
            }
        }
        if !kind.is_error() && !level(centre(frame_bits)) {
            kind = EventKind::Error("stop bit not high (framing)");
        }

        events.push(Event { start, end, kind });
        last_end = Some(end);
        // Resume searching from half a bit *inside* the final stop bit, not
        // from the computed frame end. Samples per bit is rarely an integer,
        // so an end rounded down accumulates lateness across back-to-back
        // frames until it steps over the next start edge — after which the
        // decoder locks onto a data transition and every following byte is
        // wrong. Starting inside the stop bit means the next high-to-low
        // transition is always the real start bit.
        i = start + ((frame_bits as f64 + cfg.stop_bits as f64 - 0.5) * per_bit) as usize;
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

    /// Render bytes as an idle-high UART waveform at `per_bit` samples.
    fn wave(bytes: &[u8], per_bit: usize, idle: usize) -> Vec<i8> {
        let mut bits = vec![true; idle];
        for &b in bytes {
            bits.push(false); // start
            for k in 0..8 {
                bits.push(b & (1 << k) != 0);
            }
            bits.push(true); // stop
            bits.extend(std::iter::repeat_n(true, 2));
        }
        bits.extend(std::iter::repeat_n(true, idle));
        bits.iter()
            .flat_map(|&b| std::iter::repeat_n(if b { 100i8 } else { -100 }, per_bit))
            .collect()
    }

    fn words(events: &[Event]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Word { value, .. } => Some(value),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn decodes_bytes_at_the_configured_baud() {
        let per_bit = 40;
        let rate = 1e6;
        let raw = wave(b"Hi!", per_bit, 20);
        let d = digitize(&raw, rate, Threshold::default()).unwrap();
        let cfg = Config {
            baud: rate / per_bit as f64,
            ..Default::default()
        };
        let ev = decode(&d, cfg).unwrap();
        assert_eq!(words(&ev), vec![b'H' as u64, b'i' as u64, b'!' as u64]);
        assert!(!ev.iter().any(|e| e.kind.is_error()), "{ev:?}");
    }

    #[test]
    fn a_wrong_baud_rate_is_reported_not_decoded() {
        let per_bit = 40;
        let rate = 1e6;
        let raw = wave(b"AAAA", per_bit, 20);
        let d = digitize(&raw, rate, Threshold::default()).unwrap();
        // Claim 1.7x the real baud: the grid drifts through the bits.
        let cfg = Config {
            baud: rate / per_bit as f64 * 1.7,
            ..Default::default()
        };
        let ev = decode(&d, cfg).unwrap();
        assert!(
            ev.iter().any(|e| e.kind.is_error()),
            "a mismatched baud must surface as an error, got {ev:?}"
        );
    }

    #[test]
    fn too_few_samples_per_bit_refuses() {
        let raw = wave(b"x", 4, 8);
        let d = digitize(&raw, 1e6, Threshold::default()).unwrap();
        let cfg = Config {
            baud: 1e6 / 4.0,
            ..Default::default()
        };
        assert!(matches!(
            decode(&d, cfg),
            Err(DecodeError::TooFewSamples { .. })
        ));
    }

    #[test]
    fn events_carry_positions_that_map_back_to_time() {
        let per_bit = 40;
        let rate = 1e6;
        let raw = wave(b"Z", per_bit, 20);
        let d = digitize(&raw, rate, Threshold::default()).unwrap();
        let ev = decode(
            &d,
            Config {
                baud: rate / per_bit as f64,
                ..Default::default()
            },
        )
        .unwrap();
        let w = ev.iter().find(|e| !e.kind.is_error()).unwrap();
        let (t0, t1) = w.seconds(rate);
        assert!(t1 > t0);
        // One frame is 10 bits at 25 kbaud = 400 us.
        assert!((t1 - t0 - 400e-6).abs() < 60e-6, "{t0}..{t1}");
    }

    #[test]
    fn back_to_back_frames_do_not_drift_out_of_sync() {
        // The bug this guards: samples per bit is rarely an integer, so a
        // decoder that resumes from a rounded-down frame end accumulates
        // lateness until it steps over the next start edge. "Hi!" then
        // decoded as "HZ." with no error reported, which is the worst kind
        // of wrong.
        let rate = 250e3;
        let baud = 9600.0; // 26.0417 samples per bit — deliberately ragged
        let per_bit = rate / baud;

        // The bit stream: idle, then frames butted together as a UART sends
        // them, then idle.
        let mut bits = vec![true; 2];
        for &b in b"Hi!" {
            bits.push(false); // start
            for k in 0..8 {
                bits.push(b & (1 << k) != 0);
            }
            bits.push(true); // stop
        }
        bits.extend(std::iter::repeat_n(true, 4));

        // Sample it at a rate that does not divide the bit period.
        let n = (bits.len() as f64 * per_bit) as usize;
        let raw: Vec<i8> = (0..n)
            .map(|i| {
                let k = ((i as f64 / per_bit) as usize).min(bits.len() - 1);
                if bits[k] { 100 } else { -100 }
            })
            .collect();

        let d = digitize(&raw, rate, Threshold::default()).unwrap();
        let ev = decode(
            &d,
            Config {
                baud,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            words(&ev),
            vec![b'H' as u64, b'i' as u64, b'!' as u64],
            "back-to-back frames must stay in sync: {ev:?}"
        );
        assert!(!ev.iter().any(|e| e.kind.is_error()), "{ev:?}");
    }
}
