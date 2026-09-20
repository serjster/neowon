//! DAB (Eureka-147) decoding — Phase 10.15, tier 1: the Fast Information
//! Channel.
//!
//! This is the part of DAB that answers *what is on the air* rather than
//! guessing it: an ensemble announces its identity and its service list in the
//! FIC, error-protected and CRC-checked ([`detect`] is the classifier guessing;
//! this module is the standard telling us).
//!
//! Scope of tier 1: an ensemble and service table, no audio, no MSC. Tier 2
//! (sub-channel bytes, DLS text) and tier 3 (audio codecs) are separate work in
//! `docs/tasks/phase10-dab-spec.md`; nothing here anticipates them.
//!
//! Every constant below cites its clause of **ETSI EN 300 401 V2.1.1
//! (2017-01)**. The normative tables live in [`tables`], machine-transcribed
//! from the standard (see that module). Anything the standard does not define
//! is not invented here: see [`Protection`] for what tier 1 honestly does not
//! know.
//!
//! Portions of this module follow the MIT-licensed reference `dabradio` 0.5.0
//! (`xoolive/desperado`) for algorithm structure; the notice is recorded in
//! `docs/protocol-dab.md`.

pub mod encoder;
pub mod fec;
pub mod fib;
pub mod fic;
pub mod fig;
pub mod ofdm;
pub mod receiver;
pub mod tables;

use std::collections::BTreeMap;

pub use fec::{
    FIC_DATA_BITS, FIC_INFO_BITS, FIC_MOTHER_BITS, FIC_TRANSMITTED_BITS, conv_encode,
    depuncture_fic, energy_dispersal, prbs_bits, puncture_fic, viterbi_decode,
};
pub use fib::{fib_crc_ok, walk_figs};
pub use fic::{FicState, LOCK_CRC_RATE, LOCK_WINDOW_FRAMES};
pub use ofdm::{Fft2048, carrier_bin, demap_soft, interleaver, prs_phase, prs_reference, qpsk};
pub use receiver::{DabReceiver, PRS_METRIC_MIN};

// ---------------------------------------------------------------------------
// Mode I parameters — ETSI EN 300 401 V2.1.1 (2017-01), table 22.
// ---------------------------------------------------------------------------

/// Sample rate the whole mode is defined at, in samples/second (`T = 1/2 048 000 s`).
pub const SAMPLE_RATE: f64 = 2_048_000.0;
/// Useful symbol duration `Tu`, in samples (table 22).
pub const T_U: usize = 2048;
/// Guard interval `Delta`, in samples (table 22).
pub const T_G: usize = 504;
/// Total symbol duration `Ts = Tu + Delta`, in samples (table 22).
pub const T_S: usize = T_U + T_G;
/// Null symbol duration `TNULL`, in samples (table 22).
pub const T_NULL: usize = 2656;
/// Transmission frame duration `TF`, in samples (table 22): 96 ms.
pub const FRAME_SAMPLES: usize = 196_608;
/// OFDM symbols per transmission frame, `L` (table 22): 1 phase reference + 75 data.
pub const SYMBOLS_PER_FRAME: usize = 76;
/// Active carriers, `K` (table 22).
pub const CARRIERS: usize = 1536;
/// Carrier spacing, in Hz (clause 14.2).
pub const CARRIER_SPACING_HZ: f64 = 1000.0;
/// OFDM symbols carrying the FIC per frame (clause 14.4.1.1: symbols l = 2, 3, 4).
pub const FIC_SYMBOLS: usize = 3;
/// QPSK bits carried by one FIC symbol: `2K` (clause 14.5).
pub const FIC_BITS_PER_SYMBOL: usize = 2 * CARRIERS;
/// Soft bits in one transmission frame's FIC: `3 * 2K` (clause 14.4.1.1).
pub const FIC_SOFT_BITS: usize = FIC_SYMBOLS * FIC_BITS_PER_SYMBOL;
/// One convolutionally encoded FIB group as transmitted: 2304 bits
/// (clause 11.2.1; four of them make one frame's FIC).
pub const FIC_SUBBLOCK_BITS: usize = 2304;
/// FIBs per transmission frame (clause 5.1: 4 CIFs x 3 FIBs).
pub const FIBS_PER_FRAME: usize = 12;
/// Bytes in a FIB: 30 data bytes + 2 CRC bytes (clause 5.2.1, figure 6).
pub const FIB_BYTES: usize = 32;
/// Data bytes in a FIB, excluding the CRC (clause 5.2.1).
pub const FIB_DATA_BYTES: usize = 30;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// How a sub-channel is protected.
///
/// Tier 1 reports what the FIC actually says. The short (UEP) form carries a
/// table *index* only — the size, level and bit rate live in the standard's
/// table 8, which is not transcribed yet, so they are reported as `None`
/// rather than guessed (D27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Short-form / UEP entry: the index into the standard's table 8.
    Uep { table_index: u8 },
    /// Long-form / EEP entry: option (0 = A, 1 = B) and the signalled level
    /// (stored raw, 0..=3; displayed as 1..=4).
    Eep { option: u8, level: u8 },
}

impl Protection {
    /// Human-readable label for the UI and scripts.
    pub fn label(self) -> String {
        match self {
            Protection::Uep { table_index } => format!("UEP index {table_index}"),
            Protection::Eep { option, level } => {
                let option = if option == 0 { 'A' } else { 'B' };
                format!("EEP {}-{option}", level + 1)
            }
        }
    }
}

/// One sub-channel of the multiplex, from FIG 0/1 (clause 6.2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SubChannel {
    /// `SubChId`, 6 bits.
    pub id: u8,
    /// Start address in capacity units (10 bits).
    pub start_cu: u16,
    /// Size in capacity units; known exactly only for EEP (long form).
    pub size_cu: Option<u16>,
    /// Protection, as signalled.
    pub protection: Protection,
    /// Bit rate in kbit/s. One capacity unit is 64 bits per 24 ms, so this is
    /// exact whenever `size_cu` is known.
    pub bitrate_kbps: Option<f64>,
}

/// A service carried by the ensemble, from FIG 0/2 and its FIG 1 label.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Service {
    /// Service identifier (`SId`), 16 bits for programme services.
    pub sid: u16,
    /// Service label (FIG 1/0 or 1/1), once seen.
    pub label: Option<String>,
    /// Primary audio sub-channel, when the service has one.
    pub sub_channel: Option<u8>,
    /// Audio service type (`ASCTy`), when the primary component is audio.
    pub ascty: Option<u8>,
    /// True when the service has at least one stream audio component.
    pub has_audio: bool,
}

impl Service {
    /// Audio coding label from `ASCTy` (clause 8.1.14 / table 33 assignments).
    ///
    /// Only the two values that matter for a receiver are named; anything else
    /// is reported as its number rather than guessed.
    pub fn coding_label(&self) -> String {
        match self.ascty {
            Some(0) => "MPEG-1 Layer II".to_string(),
            Some(63) => "DAB+ (HE-AAC v2)".to_string(),
            Some(other) => format!("ASCTy {other}"),
            None => "unknown".to_string(),
        }
    }
}

/// The ensemble as the FIC describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ensemble {
    /// Ensemble identifier (`EId`), 16 bits.
    pub eid: Option<u16>,
    /// Ensemble label (FIG 1/0 referenced by the ensemble's EId).
    pub label: Option<String>,
    /// Services, keyed by `SId`.
    pub services: BTreeMap<u16, Service>,
    /// Sub-channels, keyed by `SubChId`.
    pub sub_channels: BTreeMap<u8, SubChannel>,
    /// Data services seen (`P/D = 1`). Tier 1 counts them without tabling them,
    /// so a readout can say they exist without implying they were decoded.
    pub data_services: usize,
}

impl Ensemble {
    /// One line per service, for the dock readout and the JSON surface.
    ///
    /// A service with no label yet is listed by identifier rather than hidden,
    /// so "found but not yet named" is visible instead of looking like absent.
    pub fn service_lines(&self) -> Vec<String> {
        self.services
            .values()
            .map(|s| {
                let label = s.label.as_deref().unwrap_or("<no label yet>");
                let bitrate = s
                    .sub_channel
                    .and_then(|id| self.sub_channels.get(&id))
                    .map(|sc| match sc.bitrate_kbps {
                        Some(kbps) => format!("{kbps:.0} kbit/s"),
                        None => sc.protection.label(),
                    })
                    .unwrap_or_else(|| "-".to_string());
                format!("{:04X}  {label:<20} {bitrate}", s.sid)
            })
            .collect()
    }
}

/// Everything tier 1 can say about the signal it is looking at.
#[derive(Debug, Clone, PartialEq)]
pub struct DabStatus {
    /// True only when the FIC is decoding reliably *and* identifies an ensemble.
    pub locked: bool,
    /// FIBs whose CRC checked out, and how many were attempted.
    pub fib_crc_ok: u64,
    pub fib_total: u64,
    /// Transmission frames seen.
    pub frames: u64,
    /// The ensemble table. Empty until `locked`.
    pub ensemble: Ensemble,
}

impl DabStatus {
    /// Fraction of FIBs whose CRC passed, or `None` before any was attempted.
    pub fn fib_crc_rate(&self) -> Option<f64> {
        (self.fib_total > 0).then(|| self.fib_crc_ok as f64 / self.fib_total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mode constants are mutually consistent: a frame is the null symbol
    /// plus `L` symbols, and it lasts 96 ms at 2.048 MS/s (table 22).
    #[test]
    fn mode_i_frame_arithmetic_holds() {
        assert_eq!(T_S, 2552);
        assert_eq!(T_NULL + SYMBOLS_PER_FRAME * T_S, FRAME_SAMPLES);
        let frame_ms = FRAME_SAMPLES as f64 / SAMPLE_RATE * 1e3;
        assert!((frame_ms - 96.0).abs() < 1e-9, "{frame_ms} ms");
        // 1.536 MHz of carriers inside a 2.048 MHz window.
        assert_eq!(CARRIERS as f64 * CARRIER_SPACING_HZ, 1_536_000.0);
    }

    /// The FIC sizes follow from the carriers: 3 symbols x 2 bits x K, four
    /// 2304-bit codewords, twelve FIBs (clauses 11.2.1, 14.4.1.1).
    #[test]
    fn fic_sizes_are_consistent() {
        assert_eq!(FIC_BITS_PER_SYMBOL, 3072);
        assert_eq!(FIC_SOFT_BITS, 9216);
        assert_eq!(FIC_SOFT_BITS, 4 * FIC_SUBBLOCK_BITS);
        assert_eq!(FIBS_PER_FRAME, 4 * 3);
    }

    #[test]
    fn bitrate_is_exact_for_a_known_size() {
        // 1 CU = 64 bits per 24 ms.
        let size_cu = 96;
        let expected = size_cu as f64 * 64.0 / 0.024 / 1000.0;
        assert!((expected - 256.0).abs() < 1e-9);
    }
}
