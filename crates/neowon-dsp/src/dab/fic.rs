//! The Fast Information Channel: soft bits in, a checked ensemble table out.
//!
//! One transmission frame's FIC is three OFDM symbols of `2K` QPSK bits, which
//! clause 14.4.1.1 de-multiplexes into four codewords of 2304 bits — one per
//! CIF, three FIBs each. Each codeword is depunctured, Viterbi-decoded, energy
//! dispersed, and split into three FIBs whose CRC decides whether any of it is
//! believed (clause 5.2.1).
//!
//! **Lock policy (D27).** The table is published only when the FIC is decoding
//! reliably, and `locked` additionally requires an ensemble identity — a
//! CRC-clean FIC with no `EId` is "receiving something", not "locked to a
//! station". Service labels arrive on their own schedule (once per second), so
//! the table grows from empty to complete; a short input gap does not clear it,
//! and nothing is invented to fill one. When the signal is gone, though — the
//! receiver stops accepting frames, or its owner reports no input at all — the
//! table **expires**: an unlocked receiver reporting no services is the honest
//! state, and a stale table is the lie this rule exists to prevent.

use std::collections::VecDeque;

use super::fec::{depuncture_fic, energy_dispersal, viterbi_decode};
use super::fib::{fib_crc_ok, walk_figs};
use super::{DabStatus, Ensemble, FIB_BYTES, FIC_SOFT_BITS, FIC_SUBBLOCK_BITS};

/// How many frames the lock decision looks back over.
pub const LOCK_WINDOW_FRAMES: usize = 8;
/// Fraction of CRC-clean FIBs required for `locked`.
pub const LOCK_CRC_RATE: f64 = 0.5;

/// The FIC decoder's state: counters, the lock window, and the ensemble table.
#[derive(Debug, Clone, Default)]
pub struct FicState {
    ensemble: Ensemble,
    fib_crc_ok: u64,
    fib_total: u64,
    frames: u64,
    /// `(crc-ok, attempted)` per frame, newest last.
    window: VecDeque<(u16, u16)>,
}

impl FicState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The ensemble table as it stands (empty until something was decoded).
    pub fn ensemble(&self) -> &Ensemble {
        &self.ensemble
    }

    /// Discard the lock and the table — for a retune, or an operator reset.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Drop the lock and the ensemble after the signal has been undecodable
    /// for long enough that publishing them would be a stale claim (D27).
    ///
    /// The window is emptied rather than filled with misses, so the window
    /// rate reports "no window" until `LOCK_WINDOW_FRAMES` new frames arrive;
    /// the cumulative FIB counters are history and stay. A **short** input
    /// gap is different: `DabReceiver::discard_buffer` keeps the table
    /// deliberately, because that is the behaviour that made air decoding
    /// work. This is for "the signal is gone".
    pub fn expire(&mut self) {
        self.window.clear();
        self.ensemble = Ensemble::default();
    }

    /// Feed one transmission frame's FIC soft bits (9216 of them, symbol
    /// order). Returns how many FIBs passed their CRC.
    ///
    /// A short or malformed buffer is ignored rather than panicked on: input
    /// frames are allowed to be ragged.
    pub fn process_frame(&mut self, fic_soft: &[i8]) -> usize {
        if fic_soft.len() < FIC_SOFT_BITS {
            return 0;
        }
        self.frames += 1;
        let mut ok_this_frame = 0usize;
        let mut total_this_frame = 0usize;

        for sub in 0..(FIC_SOFT_BITS / FIC_SUBBLOCK_BITS) {
            let start = sub * FIC_SUBBLOCK_BITS;
            let block = &fic_soft[start..start + FIC_SUBBLOCK_BITS];
            let bits = decode_codeword(block);
            // Three FIBs of 256 bits, MSb first within each byte.
            for fib_index in 0..3 {
                let base = fib_index * 256;
                let mut fib = [0u8; FIB_BYTES];
                for (byte_index, byte) in fib.iter_mut().enumerate() {
                    let mut value = 0u8;
                    for bit in 0..8 {
                        value = (value << 1) | bits[base + byte_index * 8 + bit];
                    }
                    *byte = value;
                }
                total_this_frame += 1;
                if fib_crc_ok(&fib) {
                    ok_this_frame += 1;
                    walk_figs(&fib, &mut self.ensemble);
                }
            }
        }

        self.fib_crc_ok += ok_this_frame as u64;
        self.fib_total += total_this_frame as u64;
        self.window
            .push_back((ok_this_frame as u16, total_this_frame as u16));
        while self.window.len() > LOCK_WINDOW_FRAMES {
            self.window.pop_front();
        }
        ok_this_frame
    }

    /// Is the FIC decoding well enough to be believed (D27)?
    fn crc_rate_ok(&self) -> bool {
        let (ok, total) = self.window.iter().fold((0u32, 0u32), |(o, t), (fo, ft)| {
            (o + *fo as u32, t + *ft as u32)
        });
        if total == 0 {
            return false;
        }
        // A complete window is required: two lucky FIBs are not a lock.
        if self.window.len() < LOCK_WINDOW_FRAMES {
            return false;
        }
        f64::from(ok) / f64::from(total) >= LOCK_CRC_RATE
    }

    /// Is the FIC decoding well enough to be believed *and* identifying an
    /// ensemble (D27)?
    pub fn is_locked(&self) -> bool {
        self.crc_rate_ok() && self.ensemble.eid.is_some()
    }

    /// The current state of the receiver, for the UI, `get dab`, and MCP.
    pub fn status(&self) -> DabStatus {
        let locked = self.is_locked();
        DabStatus {
            locked,
            fib_crc_ok: self.fib_crc_ok,
            fib_total: self.fib_total,
            frames: self.frames,
            // Unlocked means unknown: publishing a partial table would invite
            // the operator to trust a guess.
            ensemble: if locked {
                self.ensemble.clone()
            } else {
                Ensemble::default()
            },
            // The receiver fills the MSC counters; the FIC alone has none.
            msc: std::collections::BTreeMap::new(),
        }
    }
}

/// One 2304-bit codeword → 768 information bits: depuncture, decode, disperse.
///
/// The order is the standard's, not a choice: energy dispersal is applied to
/// the FIC *before* coding (clause 10.2), so the receiver undoes it after the
/// Viterbi decoder.
pub fn decode_codeword(soft: &[i8]) -> Vec<u8> {
    let depunctured = depuncture_fic(soft);
    let (mut bits, _metric) = viterbi_decode(&depunctured);
    bits.truncate(super::fec::FIC_DATA_BITS);
    energy_dispersal(&mut bits);
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::FIBS_PER_FRAME;
    use crate::dab::encoder::{EnsembleSpec, FicFrame, ServiceSpec};

    fn spec<'a>(ensemble_label: &'a str, services: Vec<ServiceSpec<'a>>) -> EnsembleSpec<'a> {
        EnsembleSpec {
            eid: 0xF044,
            label: ensemble_label,
            services,
            sub_channels: Vec::new(),
        }
    }

    /// A FIC frame that survives the whole chain comes back with its ensemble,
    /// services and labels intact.
    #[test]
    fn a_frame_round_trips_through_the_fic_decoder() {
        let spec = spec(
            "METROPOLITAIN 2",
            vec![
                ServiceSpec {
                    sid: 0x1001,
                    label: "FRANCE INTER",
                    sub_channel: 0,
                    ascty: 63,
                },
                ServiceSpec {
                    sid: 0x1002,
                    label: "FRANCE MUSIQUE",
                    sub_channel: 1,
                    ascty: 63,
                },
            ],
        );
        let soft = FicFrame::new(&spec).ideal_soft_bits();
        let mut state = FicState::new();
        // One frame in = the lock window is not yet full, so the table stays
        // unpublished even though every FIB checks out.
        assert_eq!(state.process_frame(&soft), FIBS_PER_FRAME);
        assert!(!state.status().locked);
        assert!(state.status().ensemble.services.is_empty());

        for _ in 1..LOCK_WINDOW_FRAMES {
            state.process_frame(&soft);
        }
        let status = state.status();
        assert!(status.locked, "should lock after a full window");
        assert_eq!(
            status.fib_crc_ok,
            (FIBS_PER_FRAME * LOCK_WINDOW_FRAMES) as u64
        );
        assert_eq!(
            status.fib_total,
            (FIBS_PER_FRAME * LOCK_WINDOW_FRAMES) as u64
        );
        assert_eq!(status.fib_crc_rate(), Some(1.0));
        let ensemble = &status.ensemble;
        assert_eq!(ensemble.eid, Some(0xF044));
        assert_eq!(ensemble.label.as_deref(), Some("METROPOLITAIN 2"));
        assert_eq!(ensemble.services.len(), 2);
        assert_eq!(
            ensemble.services[&0x1001].label.as_deref(),
            Some("FRANCE INTER")
        );
        assert_eq!(
            ensemble.services[&0x1002].label.as_deref(),
            Some("FRANCE MUSIQUE")
        );
        assert_eq!(ensemble.services[&0x1002].sub_channel, Some(1));
    }

    /// Corrupted input must not lock, and must not invent an ensemble.
    #[test]
    fn garbage_never_locks() {
        let mut state = FicState::new();
        let mut rng: u32 = 0x1234_5678;
        for _ in 0..(LOCK_WINDOW_FRAMES + 2) {
            let noise: Vec<i8> = (0..FIC_SOFT_BITS)
                .map(|_| {
                    rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((rng >> 24) as i8) / 2
                })
                .collect();
            state.process_frame(&noise);
        }
        let status = state.status();
        assert!(!status.locked);
        assert_eq!(status.ensemble, Ensemble::default());
        assert!(status.fib_crc_ok < status.fib_total);
    }

    /// A reset forgets the lock and the table.
    #[test]
    fn reset_clears_the_lock() {
        let spec = spec(
            "TEST",
            vec![ServiceSpec {
                sid: 0x1001,
                label: "SERVICE",
                sub_channel: 0,
                ascty: 63,
            }],
        );
        let soft = FicFrame::new(&spec).ideal_soft_bits();
        let mut state = FicState::new();
        for _ in 0..LOCK_WINDOW_FRAMES {
            state.process_frame(&soft);
        }
        assert!(state.status().locked);
        state.reset();
        let status = state.status();
        assert!(!status.locked);
        assert_eq!(status.frames, 0);
        assert_eq!(status.fib_crc_ok, 0);
        assert!(status.ensemble.services.is_empty());
    }

    /// An expiry (the signal was gone for seconds) drops the lock and the
    /// table but keeps the cumulative FIB counters: those are history, the
    /// table is the claim (D27). New frames rebuild the table.
    #[test]
    fn expire_drops_the_table_and_keeps_the_history() {
        let spec = spec(
            "TEST",
            vec![ServiceSpec {
                sid: 0x1001,
                label: "SERVICE",
                sub_channel: 0,
                ascty: 63,
            }],
        );
        let soft = FicFrame::new(&spec).ideal_soft_bits();
        let mut state = FicState::new();
        for _ in 0..LOCK_WINDOW_FRAMES {
            state.process_frame(&soft);
        }
        assert!(state.status().locked);
        let frames = state.status().frames;

        state.expire();
        let status = state.status();
        assert!(!status.locked, "expired lock still reported as locked");
        assert_eq!(status.ensemble, Ensemble::default());
        assert_eq!(status.frames, frames, "history is kept");
        assert_eq!(
            status.fib_total,
            (FIBS_PER_FRAME * LOCK_WINDOW_FRAMES) as u64
        );

        // The window is empty, so the lock needs a full new window.
        state.process_frame(&soft);
        assert!(!state.status().locked);
        for _ in 1..LOCK_WINDOW_FRAMES {
            state.process_frame(&soft);
        }
        let status = state.status();
        assert!(status.locked, "new frames should re-lock");
        assert_eq!(status.ensemble.eid, Some(0xF044));
        assert!(status.ensemble.services.contains_key(&0x1001));
    }
}
