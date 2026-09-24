//! The DAB receiver: interleaved IQ at 2.048 MS/s in, an ensemble table out.
//!
//! This is what makes the FIC decoder reachable from a real signal. It is a
//! stateful object fed whatever block sizes the caller has, holding a sample
//! buffer across calls, so a caller can stream a capture in chunks and read
//! [`DabReceiver::status`] whenever it likes.
//!
//! ## How it locks
//!
//! 1. **Find the null symbol.** The transmitter is silent for `T_NULL` samples
//!    at the start of every 96 ms frame (clause 14.3.1), so the frame start is
//!    the deepest sustained power dip — found on a coarse stride with an
//!    integral of the sample power, not by scanning every offset.
//! 2. **Confirm it is DAB, not a dip in the noise.** The phase reference symbol
//!    that must follow the null is a known 1536-carrier sequence, so the
//!    normalized correlation between the received spectrum and
//!    [`prs_reference`] is ~1 for DAB and ~`1/sqrt(1536)` for noise. A frame
//!    below [`PRS_METRIC_MIN`] is rejected and *nothing* is decoded from it —
//!    this is what keeps the receiver from naming an ensemble that is not there
//!    (the spec's false-lock criterion).
//! 3. **Remove the frequency offset.** The dongle's clock error rotates the
//!    differential product by `2·pi·df·Ts` per symbol, which breaks DQPSK, so
//!    the cyclic prefix of the PRS measures `df` and the frame's symbols are
//!    de-rotated on a *single* continuous time base (restarting the phasor per
//!    symbol would re-introduce exactly the rotation it is meant to remove).
//! 4. **Demap every data symbol.** The 76-symbol frame carries 3 FIC symbols
//!    and 72 MSC symbols; all 75 data symbols are demapped with the same
//!    timing and carrier state, in one differential chain (the first against
//!    the PRS, each later one against its predecessor). The FIC's 9216 soft
//!    bits go to [`FicState`]; the MSC's 221 184 go to [`MscDecoder`] as four
//!    CIFs of 55 296, which extracts and decodes each sub-channel.
//!
//! Timing needs no fine search: the PRS correlation is insensitive to where
//! inside the guard the window starts, and the resulting phase ramp is the same
//! for every symbol, so it cancels in the differential product — both facts are
//! properties of the modulation (clause 14.7), not of this implementation.

use rustfft::num_complex::Complex32;

use super::fic::FicState;
use super::msc::{DecodedFrame, MscDecoder};
use super::ofdm::{Fft2048, demap_soft};
use super::{
    DabStatus, Ensemble, FIC_BITS_PER_SYMBOL, FIC_SOFT_BITS, FRAME_SAMPLES, MSC_BITS_PER_SYMBOL,
    MSC_SOFT_BITS, MSC_SYMBOLS, T_G, T_NULL, T_S, T_U,
};

mod sync;

/// Lowest normalized PRS correlation accepted as a frame. DAB scores ~1, noise
/// scores ~`1/sqrt(K)` (0.03 in practice): the gate exists to reject noise,
/// not to demand a clean signal — the FIB CRC and the sub-channel FEC decide
/// what reaches the table, and a marginal ensemble (measured ~12 dB
/// peak-to-floor on air) scores 0.3–0.5 on most frames. A high gate bought no
/// table honesty and cost half the stream.
pub const PRS_METRIC_MIN: f32 = 0.35;

/// On the predicted frame grid, a PRS score below this means the clock is
/// wrong (a shifted frame lands in payload and scores ≈ noise, ~0.03), not
/// faded: give it up and re-search. Fades score 0.1–0.4 and must be kept as
/// soft misses, or the MSC chain loses the continuity it needs.
const CLOCK_LOST_METRIC: f32 = 0.06;

/// Consecutive soft misses on the predicted grid before it is given up.
/// Fades keep the MSC chain alive; only a run this long (~6 s at Mode I)
/// of sub-noise scores means the grid is genuinely lost — the app's splice
/// detection resets it immediately for real gaps.
const CLOCK_STRIKES: u32 = 64;

/// Consecutive frames that decode no accepted FIC before the published table
/// is discarded. A splice shorter than the lock window is deliberately
/// survived — keeping the table across a USB drop is what made air decoding
/// usable — but a receiver that has attempted this many frames without
/// accepting one is not listening to an ensemble, and publishing the old
/// table would name a station that is not on the air. 48 frames ≈ 4.6 s at
/// Mode I; on air acceptance is about one attempt in three, so
/// a fading-but-decodable ensemble cannot reach it. The owner's own no-input
/// case is shorter still ([`DabReceiver::no_input`]).
pub const TABLE_EXPIRY_FRAMES: u32 = 48;

/// A streaming DAB Mode I receiver.
pub struct DabReceiver {
    fft: Fft2048,
    pending: Vec<Complex32>,
    /// Scratch: power prefix sums for the null search.
    prefix: Vec<f32>,
    /// Scratch: one symbol's spectrum, and the PRS spectrum to demap against.
    spectrum: Vec<Complex32>,
    prs_spectrum: Vec<Complex32>,
    /// Scratch: the three FIC symbols' spectra.
    fic_spectra: Vec<Complex32>,
    /// Scratch: the frame's soft bits.
    soft: Vec<i8>,
    fic: FicState,
    /// The 72 MSC symbols of the frame as one symbol-major block of soft bits:
    /// 4 CIFs of 55 296 bits, in CIF order.
    msc_soft: Vec<i8>,
    /// The last MSC symbol demapped, the differential reference for the next.
    prev_msc_spectrum: Vec<Complex32>,
    /// The MSC demultiplexer: one decoder per FIC sub-channel.
    msc: MscDecoder,
    /// Carrier frequency offset estimated on the last accepted frame, Hz.
    pub freq_offset_hz: f64,
    /// Normalized PRS correlation of the last **accepted** frame — the score
    /// that justified decoding it.
    prs_metric: f32,
    /// The same score for the last attempt, accepted or not. Kept apart
    /// because a rejected attempt's score says nothing about the signal, and
    /// reporting it as the signal's quality would be false: on air this read ~0.03 on a receiver decoding 98.9% of its FIBs.
    last_attempt_metric: f32,
    /// Frames decoded, and frames rejected for a failed PRS check.
    pub frames_decoded: u64,
    pub frames_rejected: u64,
    /// Where the next frame starts in `pending`, once a frame has been
    /// accepted. Re-searching the null symbol every frame is what starved the
    /// MSC chain on air: the open-loop search lands on the true grid only a
    /// few percent of the time, and the receiver discarded the clause-12
    /// delay line on every miss, so it could never complete the 16 frames of
    /// continuity a sub-channel needs. `None` until the first accepted frame
    /// and after the grid is given up.
    next_frame: Option<usize>,
    /// Consecutive soft misses at the predicted start.
    clock_misses: u32,
    /// Consecutive attempts that accepted no FIC, whichever path rejected
    /// them. At [`TABLE_EXPIRY_FRAMES`] the published table expires; an
    /// accepted frame starts the count again.
    frames_since_accept: u32,
    /// Test-only counterfactual switch: with `NEOWON_DAB_NO_PREDICTION` set in
    /// the environment, the frame grid is never used, so every frame re-derives
    /// its start from the null-symbol power dip. The capture harnesses use it
    /// to show the MSC chain starving while the FIC stays clean. Unset (the
    /// production path) changes nothing.
    disable_prediction: bool,
}

impl DabReceiver {
    pub fn new() -> Self {
        Self {
            fft: Fft2048::new(),
            pending: Vec::new(),
            prefix: Vec::new(),
            spectrum: vec![Complex32::new(0.0, 0.0); T_U],
            prs_spectrum: vec![Complex32::new(0.0, 0.0); T_U],
            fic_spectra: vec![Complex32::new(0.0, 0.0); 3 * T_U],
            soft: vec![0i8; FIC_SOFT_BITS],
            fic: FicState::new(),
            msc_soft: vec![0i8; MSC_SOFT_BITS],
            prev_msc_spectrum: vec![Complex32::new(0.0, 0.0); T_U],
            msc: MscDecoder::new(),
            freq_offset_hz: 0.0,
            prs_metric: 0.0,
            last_attempt_metric: 0.0,
            frames_decoded: 0,
            frames_rejected: 0,
            next_frame: None,
            clock_misses: 0,
            frames_since_accept: 0,
            disable_prediction: std::env::var_os("NEOWON_DAB_NO_PREDICTION").is_some(),
        }
    }

    /// Feed interleaved I, Q samples in full-scale units (as
    /// `SampleLayout::Complex` frames carry them). Returns how many transmission
    /// frames were decoded by this call.
    pub fn push_iq(&mut self, interleaved: &[f32]) -> usize {
        self.pending.extend(
            interleaved
                .chunks_exact(2)
                .map(|pair| Complex32::new(pair[0], pair[1])),
        );

        let mut decoded = 0;
        while self.pending.len() >= FRAME_SAMPLES + T_NULL {
            // With the frame grid known, decode at the predicted start; the
            // open-loop null search is for acquisition and re-lock only. The
            // test-only switch disables the grid entirely (see the field).
            let predicted = if self.disable_prediction {
                None
            } else {
                self.next_frame
                    .filter(|&at| at + FRAME_SAMPLES + T_NULL <= self.pending.len())
            };
            let frame_start = predicted.unwrap_or_else(|| self.find_null_symbol());
            if frame_start + FRAME_SAMPLES > self.pending.len() {
                break;
            }
            // The dip only says roughly where the frame is; the PRS peak says
            // exactly where it is, and the predicted grid inherits that
            // accuracy frame to frame.
            let (frame_start, _) = self.refine_frame_start(frame_start);
            // Track the carrier offset on every attempt, not only accepted
            // frames — but damped. A marginal frame's guard estimate is
            // noisy, and a tracker that follows it walks off the true carrier
            // (measured live: the offset wandered to +100 Hz while the PRS at
            // the grid fell from 0.5 to 0.1). Implausible jumps are ignored
            // and real moves are taken a quarter at a time, and only where
            // the grid looks like a frame at all.
            let coarse_metric = self.extract_prs_spectrum(frame_start, self.freq_offset_hz);
            if coarse_metric >= CLOCK_LOST_METRIC
                && let Some(offset) = self.estimate_frequency_offset(frame_start)
            {
                let delta = offset - self.freq_offset_hz;
                if delta.abs() < 250.0 {
                    self.freq_offset_hz += 0.25 * delta;
                }
            }
            let attempt_metric = self.extract_prs_spectrum(frame_start, self.freq_offset_hz);
            self.last_attempt_metric = attempt_metric;
            let accepted = attempt_metric >= PRS_METRIC_MIN;
            if accepted {
                self.frames_since_accept = 0;
            } else {
                // A table nobody can re-derive is not a table. Count
                // every unaccepted attempt — open-loop searches, noise on the
                // predicted grid, fades — and expire once the run is long
                // enough that "the signal is gone" is the only reading left.
                self.frames_since_accept = self.frames_since_accept.saturating_add(1);
                if self.frames_since_accept == TABLE_EXPIRY_FRAMES {
                    self.fic.expire();
                }
            }
            if !accepted {
                self.frames_rejected += 1;
                // On the predicted grid a weak PRS is a fade, not a lost
                // frame: keep decoding so the MSC chain holds the clause-12
                // continuity it needs, but keep the frame out of the FIC (its
                // CRC gate stays honest). A score that is merely noise means
                // the clock is wrong — give it up at once. Off the grid there
                // is nothing to keep.
                let clock_lost = attempt_metric < CLOCK_LOST_METRIC;
                if predicted.is_none() || clock_lost {
                    self.msc.discard();
                    self.next_frame = None;
                    self.clock_misses = 0;
                    let consume = (frame_start + FRAME_SAMPLES / 2).min(self.pending.len());
                    self.pending.drain(..consume);
                    continue;
                }
                self.clock_misses += 1;
                if self.clock_misses >= CLOCK_STRIKES {
                    self.msc.discard();
                    self.next_frame = None;
                    self.clock_misses = 0;
                }
            } else {
                self.clock_misses = 0;
            }

            let offset_hz = self.freq_offset_hz;
            if accepted {
                self.prs_metric = attempt_metric;
            }

            for symbol in 0..3usize {
                let start = frame_start + T_NULL + (symbol + 1) * T_S + T_G;
                let base = (start - frame_start) as f64;
                let mut window = std::mem::take(&mut self.spectrum);
                self.corrected_window(start, base, offset_hz, &mut window);
                self.fft.forward(&mut window);
                let at = symbol * T_U;
                self.fic_spectra[at..at + T_U].copy_from_slice(&window);
                let mut soft = std::mem::take(&mut self.soft);
                {
                    let (bits, _) = soft.split_at_mut((symbol + 1) * FIC_BITS_PER_SYMBOL);
                    let slice = &mut bits[symbol * FIC_BITS_PER_SYMBOL..];
                    let reference = if symbol == 0 {
                        &self.prs_spectrum
                    } else {
                        let at = (symbol - 1) * T_U;
                        &self.fic_spectra[at..at + T_U]
                    };
                    demap_soft(&window, reference, slice);
                }
                self.soft = soft;
                self.spectrum = window;
            }

            // The 72 MSC symbols continue the same differential chain: the
            // first MSC symbol is referenced to the last FIC symbol.
            self.prev_msc_spectrum
                .copy_from_slice(&self.fic_spectra[2 * T_U..3 * T_U]);
            for symbol in 0..MSC_SYMBOLS {
                let data_symbol = symbol + super::FIC_SYMBOLS;
                let start = frame_start + T_NULL + (data_symbol + 1) * T_S + T_G;
                let base = (start - frame_start) as f64;
                let mut window = std::mem::take(&mut self.spectrum);
                self.corrected_window(start, base, offset_hz, &mut window);
                self.fft.forward(&mut window);
                let at = symbol * MSC_BITS_PER_SYMBOL;
                let mut reference = std::mem::take(&mut self.prev_msc_spectrum);
                demap_soft(
                    &window,
                    &reference,
                    &mut self.msc_soft[at..at + MSC_BITS_PER_SYMBOL],
                );
                reference.copy_from_slice(&window);
                self.prev_msc_spectrum = reference;
                self.spectrum = window;
            }

            if accepted {
                self.fic.process_frame(&self.soft);
            }
            // The demux follows the FIC's sub-channel table: handlers appear
            // and reset with it, and only CRC-clean FIGs reach the table.
            self.msc.sync(self.fic.ensemble());
            self.msc.push_frame(&self.msc_soft);
            self.frames_decoded += 1;
            decoded += 1;

            // Draining a whole frame leaves the next frame's start at the head
            // of `pending`.
            self.next_frame = Some(0);
            let consume = (frame_start + FRAME_SAMPLES).min(self.pending.len());
            self.pending.drain(..consume);
        }
        decoded
    }

    /// The ensemble table as the FIC decoder sees it (empty until locked).
    pub fn status(&self) -> DabStatus {
        let mut status = self.fic.status();
        status.msc = self.msc.status();
        status
    }

    /// The ensemble, locked or not — for callers that show raw progress.
    pub fn ensemble(&self) -> &Ensemble {
        self.fic.ensemble()
    }

    /// Take the MSC logical frames decoded since the last call, oldest first.
    ///
    /// Each frame is one sub-channel's 24 ms payload (`24 × bit rate / 8`
    /// bytes).
    pub fn take_msc_frames(&mut self) -> Vec<DecodedFrame> {
        self.msc.take_frames()
    }

    /// Check a trailing CRC-16 (annex E) on each MSC logical frame.
    ///
    /// This is the **sim's oracle transport, not DAB**: EN 300 401 has no CRC
    /// at this layer, so a real receiver never turns it on, and the counters
    /// stay at zero (not checked, not guessed). The encoder mirror appends the
    /// CRC when the fixture asks for it.
    pub fn enable_msc_payload_crc(&mut self) {
        self.msc.set_payload_crc(true);
    }

    /// Decoded MSC logical frames thrown away because the consumer never
    /// drained the queue. Coverage lost, so it is reported rather than left
    /// as a number only the decoder can see.
    pub fn msc_dropped(&self) -> u64 {
        self.msc.dropped_frames()
    }

    /// Samples buffered but not yet consumed by a frame attempt. Exposed so
    /// an owner (and its tests) can see that a [`Self::discard_buffer`]
    /// really happened, instead of assuming it.
    pub fn buffered(&self) -> usize {
        self.pending.len()
    }

    /// Drop the buffered samples, keeping the lock window and the table.
    ///
    /// For a caller that has detected a **gap or an overlap** in its frame
    /// stream: spliced samples are worse than missing ones, because the null
    /// symbol stops being the unique power dip and the sync wanders.
    pub fn discard_buffer(&mut self) {
        self.pending.clear();
        self.msc.discard();
        self.next_frame = None;
        self.clock_misses = 0;
    }

    /// The owner reports that **no samples have arrived** for long enough
    /// that this is not a splice (a stopped backend, a retune, an instrument
    /// switch). With no input the receiver cannot count missed frames, so the
    /// owner says so: the lock, the table and the timing grid are all claims
    /// about a stream that is not there any more. The deliberate short-gap
    /// path is [`Self::discard_buffer`], which keeps the table.
    pub fn no_input(&mut self) {
        self.fic.expire();
        self.pending.clear();
        self.msc.discard();
        self.next_frame = None;
        self.clock_misses = 0;
        self.frames_since_accept = 0;
    }

    /// Cheap lock check for a per-frame consumer: true when the FIC is
    /// decoding reliably and names an ensemble. [`Self::status`] clones the
    /// table; this does not.
    pub fn is_locked(&self) -> bool {
        self.fic.is_locked()
    }

    /// Forget the lock, the table and the buffered samples.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.fic.reset();
        self.msc.reset();
        self.freq_offset_hz = 0.0;
        self.prs_metric = 0.0;
        self.last_attempt_metric = 0.0;
        self.frames_decoded = 0;
        self.frames_rejected = 0;
        self.next_frame = None;
        self.clock_misses = 0;
        self.frames_since_accept = 0;
    }

    /// The accepted frame's PRS correlation: the score behind the table.
    pub fn prs_metric(&self) -> f32 {
        self.prs_metric
    }

    /// The last attempt's PRS correlation, whether or not it was believed.
    pub fn last_attempt_metric(&self) -> f32 {
        self.last_attempt_metric
    }
}

impl Default for DabReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
