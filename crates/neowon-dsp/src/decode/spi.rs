//! SPI: a clock, one or two data lines, and an optional chip select.
//!
//! Word length is configurable up to 64 bits because plenty of devices are
//! not byte-oriented (ADCs with 10, 12 or 24-bit frames are common), and
//! forcing them into bytes makes the decode useless.

use super::{DecodeError, Digital, Event, EventKind, MIN_SAMPLES_BIT};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Clock polarity: idle level of SCK.
    pub cpol: bool,
    /// Clock phase: false = sample on the first edge after idle.
    pub cpha: bool,
    /// Bits per word, 1..=64.
    pub bits: u8,
    /// Most-significant bit first, as most devices do.
    pub msb_first: bool,
    /// Chip select is active low.
    pub cs_active_low: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cpol: false,
            cpha: false,
            bits: 8,
            msb_first: true,
            cs_active_low: true,
        }
    }
}

/// Decode one data line against a clock, optionally gated by chip select.
pub fn decode(
    sck: &Digital,
    data: &Digital,
    cs: Option<&Digital>,
    cfg: Config,
) -> Result<Vec<Event>, DecodeError> {
    if !(1..=64).contains(&cfg.bits) {
        return Err(DecodeError::BadParameter("word length must be 1..=64"));
    }
    let n = sck
        .levels
        .len()
        .min(data.levels.len())
        .min(cs.map_or(usize::MAX, |c| c.levels.len()));
    if n < 4 {
        return Err(DecodeError::NoActivity);
    }

    // Sampling edge: with CPHA=0 it is the edge that leaves idle.
    let sample_on_rise = cfg.cpol == cfg.cpha;

    // Resolution: the shortest clock phase in the capture.
    let mut shortest = usize::MAX;
    let mut prev_edge: Option<usize> = None;
    for (i, _) in sck.edges() {
        if i >= n {
            break;
        }
        if let Some(p) = prev_edge {
            shortest = shortest.min(i - p);
        }
        prev_edge = Some(i);
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

    let selected = |i: usize| match cs {
        Some(c) => c.levels[i] != cfg.cs_active_low,
        None => true,
    };

    let mut events = Vec::new();
    let mut value: u64 = 0;
    let mut bits: u32 = 0;
    let mut word_start = 0usize;
    let mut was_selected = selected(0);

    for i in 1..n {
        let now_selected = selected(i);
        if now_selected != was_selected {
            events.push(Event {
                start: i,
                end: i,
                kind: EventKind::Marker(if now_selected { "SELECT" } else { "DESELECT" }),
            });
            // A deselect mid-word means the frame was cut short.
            if !now_selected && bits > 0 {
                events.push(Event {
                    start: word_start,
                    end: i,
                    kind: EventKind::Error("incomplete word at deselect"),
                });
            }
            bits = 0;
            value = 0;
            was_selected = now_selected;
            continue;
        }
        if !now_selected {
            continue;
        }
        let rising = sck.levels[i] && !sck.levels[i - 1];
        let falling = !sck.levels[i] && sck.levels[i - 1];
        if !(if sample_on_rise { rising } else { falling }) {
            continue;
        }
        if bits == 0 {
            word_start = i;
        }
        let bit = data.levels[i] as u64;
        if cfg.msb_first {
            value = (value << 1) | bit;
        } else {
            value |= bit << bits;
        }
        bits += 1;
        if bits == cfg.bits as u32 {
            events.push(Event {
                start: word_start,
                end: i,
                kind: EventKind::Word {
                    value,
                    bits: cfg.bits,
                },
            });
            bits = 0;
            value = 0;
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

    /// Mode-0 SPI: idle low, data set on the falling edge, sampled rising.
    fn frames(words: &[u8], per_bit: usize) -> (Vec<i8>, Vec<i8>, Vec<i8>) {
        let (mut sck, mut mosi, mut cs) = (Vec::new(), Vec::new(), Vec::new());
        let mut push = |c: bool, d: bool, s: bool, n: usize| {
            for _ in 0..n {
                sck.push(if c { 100i8 } else { -100 });
                mosi.push(if d { 100i8 } else { -100 });
                cs.push(if s { 100i8 } else { -100 });
            }
        };
        push(false, false, true, per_bit * 2); // idle, deselected
        for &w in words {
            for k in (0..8).rev() {
                let bit = w & (1 << k) != 0;
                push(false, bit, false, per_bit / 2);
                push(true, bit, false, per_bit / 2);
            }
        }
        push(false, false, true, per_bit * 2);
        (sck, mosi, cs)
    }

    #[test]
    fn decodes_words_between_select_and_deselect() {
        let per_bit = 40;
        let rate = 1e6;
        let (sck, mosi, cs) = frames(&[0xA5, 0x0F], per_bit);
        let d = |v: &[i8]| digitize(v, rate, Threshold::default()).unwrap();
        let ev = decode(&d(&sck), &d(&mosi), Some(&d(&cs)), Config::default()).unwrap();
        let words: Vec<u64> = ev
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Word { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(words, vec![0xA5, 0x0F]);
        assert!(ev.iter().any(|e| e.kind == EventKind::Marker("SELECT")));
        assert!(ev.iter().any(|e| e.kind == EventKind::Marker("DESELECT")));
    }

    #[test]
    fn non_byte_word_lengths_are_supported() {
        // Twelve-bit frames, as an ADC would send.
        let per_bit = 40;
        let rate = 1e6;
        let (sck, mosi, cs) = frames(&[0xAB, 0xCD, 0xEF], per_bit);
        let d = |v: &[i8]| digitize(v, rate, Threshold::default()).unwrap();
        let ev = decode(
            &d(&sck),
            &d(&mosi),
            Some(&d(&cs)),
            Config {
                bits: 12,
                ..Default::default()
            },
        )
        .unwrap();
        let words: Vec<u64> = ev
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Word { value, bits } => {
                    assert_eq!(bits, 12);
                    Some(value)
                }
                _ => None,
            })
            .collect();
        // 24 bits of payload is exactly two 12-bit words.
        assert_eq!(words, vec![0xABC, 0xDEF]);
    }

    #[test]
    fn a_word_cut_short_by_deselect_is_reported() {
        let per_bit = 40;
        let rate = 1e6;
        let (sck, mosi, cs) = frames(&[0xA5], per_bit);
        let d = |v: &[i8]| digitize(v, rate, Threshold::default()).unwrap();
        // Ask for 12 bits when only 8 arrive before deselect.
        let ev = decode(
            &d(&sck),
            &d(&mosi),
            Some(&d(&cs)),
            Config {
                bits: 12,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            ev.iter().any(|e| e.kind.is_error()),
            "a truncated frame must be flagged: {ev:?}"
        );
    }
}
