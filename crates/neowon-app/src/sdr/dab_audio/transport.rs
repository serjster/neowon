//! Stream-shaping helpers for playback: the DAB+ super-frame sync search
//! (`SuperframeDecoder` assumes a boundary the caller knows; an app starting
//! mid-stream has none) and the sink rate converter. Split from `mod.rs`
//! along that seam so each file keeps its budget.

use neowon_codec::dabplus::{DecodedSuperframe, SuperframeDecoder};

use super::RESYNC_AFTER_BAD;

/// How many consecutive clean windows an alignment needs before it is
/// trusted — TS 102 563 annex C's search: one clean window can be a chance
/// Fire-code hit on garbage (2^-16 per window, and the search tries every
/// phase), two in a row cannot.
const CONFIRM_WINDOWS: u32 = 2;

/// Super-frame lengths of bytes a search may consume without one Fire-clean
/// window before the stream is declared not to be the signalled DAB+ shape.
/// Twenty super frames is 2.4 s of stream at every sub-channel index, and no
/// working receiver emits that much of a real DAB+ stream with zero valid
/// headers; a shorter budget condemned streams whose receiver was still
/// acquiring (the `14 windows` verdict on air).
const SHAPE_VERDICT_SUPERFRAMES: u64 = 20;

/// DAB+ super-frame synchronisation at the only granularity this transport
/// has: the MSC hands over whole logical frames, so a super-frame boundary
/// is at 0, 1, 2, 3 or 4 logical frames into the pending buffer. Each phase
/// is trial-decoded until the header Fire code passes (TS 102 563 annex C's
/// search; `SuperframeDecoder` itself starts from a boundary the caller is
/// assumed to know, which an app starting mid-stream does not).
///
/// Two consecutive clean windows are needed before the phase is trusted, so
/// a single random RS "correction" cannot lock it. Once aligned, a bad
/// window is a channel error and is consumed in place; `RESYNC_AFTER_BAD`
/// consecutive failures restart the search from the next logical frame.
///
/// **The search is not evidence about the stream.** Its failed windows and
/// consumed bytes are counted apart from the aligned failures, and the
/// "not the signalled shape" verdict rests on them alone: a stream that has
/// ever aligned is a DAB+ stream, whatever happens later, and the verdict is
/// disabled for good (a transient fade or a receiver re-acquisition must not
/// kill playback for the rest of the session).
#[derive(Debug)]
pub(crate) struct DabPlusSync {
    decoder: SuperframeDecoder,
    pending: Vec<u8>,
    phase: usize,
    /// A clean window has been seen and the confirmation is in progress.
    aligned: bool,
    /// Clean windows since the search last restarted (annex C confirmation).
    confirming: u32,
    bad: u32,
    /// A stream that has ever aligned is DAB+; this never clears.
    ever_aligned: bool,
    /// Windows that failed while aligned: channel errors, not shape evidence.
    bad_windows: u64,
    /// Windows that failed the Fire code while searching, and the bytes the
    /// search has consumed. The shape verdict reads these.
    search_windows: u64,
    search_bytes: u64,
}

impl DabPlusSync {
    pub(crate) fn new(index: u8) -> Result<Self, neowon_codec::Error> {
        Ok(Self {
            decoder: SuperframeDecoder::new(index)?,
            pending: Vec::new(),
            phase: 0,
            aligned: false,
            confirming: 0,
            bad: 0,
            ever_aligned: false,
            bad_windows: 0,
            search_windows: 0,
            search_bytes: 0,
        })
    }

    /// Windows that failed the Fire code while aligned: channel errors.
    /// Only the tests read this apart from the search windows, because the
    /// app's contract is the shape verdict and the decoded-superframe stream.
    #[cfg(test)]
    pub(crate) fn bad_windows(&self) -> u64 {
        self.bad_windows
    }

    /// Windows the search has tried without a clean one, and the bytes it
    /// consumed doing so. Only meaningful while nothing has decoded yet.
    pub(crate) fn search_windows(&self) -> u64 {
        self.search_windows
    }

    /// The stream is not the signalled DAB+ shape: the search has consumed
    /// twenty super-frame lengths of bytes without ever seeing one Fire-clean
    /// window. A stream that has ever aligned can never be declared this way.
    pub(crate) fn shape_mismatch(&self) -> bool {
        !self.ever_aligned
            && self.search_bytes
                >= SHAPE_VERDICT_SUPERFRAMES * self.decoder.superframe_bytes() as u64
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<DecodedSuperframe> {
        self.pending.extend_from_slice(bytes);
        let superframe_bytes = self.decoder.superframe_bytes();
        let logical_bytes = self.decoder.logical_frame_bytes();
        let mut out = Vec::new();
        loop {
            // While a candidate alignment is being confirmed the window is
            // taken where the clean one ended; there is nothing to search.
            let searching = !self.aligned && self.confirming == 0;
            let phase = if searching { self.phase } else { 0 };
            if self.pending.len() < phase + superframe_bytes {
                break;
            }
            let window = self.pending[phase..phase + superframe_bytes].to_vec();
            let Some(decoded) = self.decoder.push(&window).pop() else {
                break;
            };
            if decoded.firecode_ok {
                self.pending.drain(..phase + superframe_bytes);
                if self.aligned {
                    self.bad = 0;
                } else {
                    self.confirming += 1;
                    if self.confirming >= CONFIRM_WINDOWS {
                        self.aligned = true;
                        self.confirming = 0;
                        self.ever_aligned = true;
                    }
                    self.phase = 0;
                }
                out.push(decoded);
            } else if self.aligned || self.confirming > 0 {
                // A bad window on a trusted alignment is a channel error and
                // is consumed in place; a run of them means the alignment is
                // gone. A failed confirmation is not shape evidence either.
                self.pending.drain(..superframe_bytes);
                if self.aligned {
                    self.bad += 1;
                    self.bad_windows += 1;
                    if self.bad >= RESYNC_AFTER_BAD {
                        self.aligned = false;
                        self.bad = 0;
                        self.phase = logical_bytes;
                    }
                } else {
                    self.confirming = 0;
                    self.phase = logical_bytes;
                }
            } else if self.phase + logical_bytes < superframe_bytes {
                self.search_windows += 1;
                self.phase += logical_bytes;
            } else {
                // No phase matched this window: it was garbage; step one
                // logical frame and search again.
                self.search_windows += 1;
                self.search_bytes += logical_bytes as u64;
                self.pending.drain(..logical_bytes);
                self.phase = 0;
            }
        }
        out
    }
}

/// Streaming rate conversion for the sink. Identity unless the device rate
/// differs from the stream's; then linear interpolation (see the module
/// docs for why 10.10's windowed-sinc resampler is not used here).
pub(super) struct RateConverter {
    in_rate: f64,
    out_rate: f64,
    step: f64,
    next: f64,
    hist: Vec<f32>,
}

impl RateConverter {
    pub(super) fn new(in_rate: f64, out_rate: f64) -> Self {
        let mut converter = Self {
            in_rate: 0.0,
            out_rate: 0.0,
            step: 1.0,
            next: 0.0,
            hist: Vec::new(),
        };
        converter.set_input(in_rate);
        converter.set_output(out_rate);
        converter
    }

    pub(super) fn set_input(&mut self, rate: f64) {
        if rate > 0.0 && (rate - self.in_rate).abs() > 0.5 {
            self.in_rate = rate;
            self.reset();
            self.step = self.ratio();
        }
    }

    pub(super) fn set_output(&mut self, rate: f64) {
        if rate > 0.0 && (rate - self.out_rate).abs() > 0.5 {
            self.out_rate = rate;
            self.reset();
            self.step = self.ratio();
        }
    }

    fn ratio(&self) -> f64 {
        if self.in_rate > 0.0 && self.out_rate > 0.0 {
            self.in_rate / self.out_rate
        } else {
            1.0
        }
    }

    pub(super) fn reset(&mut self) {
        self.next = 0.0;
        self.hist.clear();
    }

    pub(super) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if (self.step - 1.0).abs() < 1e-12 {
            out.extend_from_slice(input);
            return;
        }
        self.hist.extend_from_slice(input);
        loop {
            let base = self.next as usize;
            if base + 1 >= self.hist.len() {
                break;
            }
            let frac = (self.next - base as f64) as f32;
            out.push(self.hist[base] + (self.hist[base + 1] - self.hist[base]) * frac);
            self.next += self.step;
        }
        let keep = self.next as usize;
        if keep > 0 {
            self.hist.drain(..keep);
            self.next -= keep as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DAB+ sync finds the super-frame phase with a logical frame of
    /// junk in front, and only then: every Fire-code-clean window it emits
    /// is a real fixture super frame, in order.
    #[test]
    fn dabplus_sync_finds_the_phase_in_front_of_a_logical_frame_of_junk() {
        let fixture: &[u8] = include_bytes!("../../../tests/fixtures/dabplus_heaacv2.sf");
        let mut sync = DabPlusSync::new(10).expect("index 10");
        let mut bytes = vec![0xA5u8; 240];
        bytes.extend_from_slice(fixture);
        let decoded = sync.push(&bytes);
        assert!(
            decoded.len() >= 2,
            "sync should find the boundary: {decoded:?}"
        );
        assert!(decoded.iter().all(|sf| sf.firecode_ok));
        // The first clean super frame is the fixture's first: the junk was
        // discarded at the phase that matched.
        let first = &decoded[0];
        assert_eq!(first.aus.len(), 3);
        assert!(first.aus.iter().all(|au| au.crc_ok));
        let fixture_aus = {
            let mut d = SuperframeDecoder::new(10).expect("index 10");
            d.push(fixture).remove(0).aus
        };
        assert_eq!(first.aus, fixture_aus);
    }

    #[test]
    fn dabplus_sync_rejects_a_bad_subchannel_index_typed() {
        let error = DabPlusSync::new(32).expect_err("32 is outside 1..=24");
        assert_eq!(error, neowon_codec::Error::InvalidSubchannelIndex(32));
    }

    /// The shape verdict is evidence, not impatience: junk that fills several
    /// super frames before the real stream starts is a ragged start, not a
    /// verdict, and must not condemn the stream — the defect behind the air
    /// session's "no valid DAB+ super frame in 14 windows" (12 windows of
    /// search, which a receiver still acquiring produces on a real stream).
    #[test]
    fn a_ragged_start_is_not_a_shape_verdict() {
        let fixture: &[u8] = include_bytes!("../../../tests/fixtures/dabplus_heaacv2.sf");
        let index = 10u8;
        let sf = 120 * index as usize;
        // Six super frames of junk in front: enough to have tripped the old
        // 12-window gate several times over, far short of the 20-super-frame
        // shape budget.
        let mut bytes = vec![0xA5u8; 6 * sf];
        bytes.extend_from_slice(fixture);
        let mut sync = DabPlusSync::new(index).expect("index 10");
        let decoded = sync.push(&bytes);
        assert!(sync.search_windows() >= 12, "the old gate would have fired");
        assert!(!sync.shape_mismatch(), "a ragged start is not a verdict");
        assert_eq!(decoded.len(), 3, "the fixture holds three super frames");
        assert!(decoded.iter().all(|sf| sf.firecode_ok));
        assert!(decoded.iter().flat_map(|sf| &sf.aus).all(|au| au.crc_ok));
    }

    /// The verdict does exist: a stream that never produces one Fire-clean
    /// window is not DAB+, and a stream that has aligned once can never be
    /// declared one by a later bad stretch.
    #[test]
    fn the_shape_verdict_needs_never_aligned_and_a_full_budget() {
        let index = 10u8;
        let sf = 120 * index as usize;
        let mut sync = DabPlusSync::new(index).expect("index 10");
        let junk = vec![0x5Au8; 8 * sf];
        assert!(sync.push(&junk).is_empty());
        assert!(sync.search_windows() > 12);
        assert!(
            !sync.shape_mismatch(),
            "eight super frames is not a verdict"
        );
        let _ = sync.push(&vec![0x5Au8; 14 * sf]);
        assert!(sync.shape_mismatch(), "twenty-two super frames is");

        let fixture: &[u8] = include_bytes!("../../../tests/fixtures/dabplus_heaacv2.sf");
        let mut aligned = DabPlusSync::new(index).expect("index 10");
        assert_eq!(aligned.push(fixture).len(), 3);
        aligned.push(&vec![0x5Au8; 40 * sf]);
        assert!(!aligned.shape_mismatch(), "a stream that aligned is DAB+");
        assert!(
            aligned.bad_windows() > 0,
            "the bad stretch is a channel error"
        );
    }

    /// Offline harness for the on-air transport failure: the real receiver
    /// and this transport, driven from a recorded IQ capture. Not a CI test —
    /// it needs `NEOWON_IQ_CAPTURE` and 15 s of 2.048 MS/s samples:
    ///
    /// ```text
    /// NEOWON_IQ_CAPTURE=tmp-inspiration/dab-11c.f32 \
    ///   cargo test -p neowon-app --release --bin neowon-app \
    ///   -- --ignored air_capture --nocapture
    /// ```
    ///
    /// `NEOWON_IQ_HOLE=<n>` drops one 32 k-sample chunk every `n` to emulate
    /// the USB drops a live session sees (0 = contiguous).
    #[test]
    #[ignore = "requires NEOWON_IQ_CAPTURE (a real air capture); offline only"]
    fn air_capture_through_the_real_transport() {
        use neowon_dsp::dab::DabReceiver;
        use std::collections::BTreeMap;

        let Some(path) = std::env::var_os("NEOWON_IQ_CAPTURE") else {
            eprintln!("set NEOWON_IQ_CAPTURE to run this harness");
            return;
        };
        let hole: usize = std::env::var("NEOWON_IQ_HOLE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let bytes = std::fs::read(path.to_string_lossy().to_string()).expect("capture");
        let iq: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|w| f32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();

        let mut receiver = DabReceiver::new();
        let mut transports: BTreeMap<u8, DabPlusSync> = BTreeMap::new();
        let mut clean: BTreeMap<u8, (u64, u64)> = BTreeMap::new(); // superframes, AU CRCs
        let mut chunks = 0usize;
        for block in iq.chunks(65_536) {
            chunks += 1;
            if hole != 0 && chunks.is_multiple_of(hole) {
                // A dropped chunk is the live splice the app's `feed` sees:
                // the grid is given up and re-searched.
                receiver.discard_buffer();
                continue;
            }
            receiver.push_iq(block);
            for (id, sub) in &receiver.ensemble().sub_channels {
                if transports.contains_key(id) {
                    continue;
                }
                if let Ok(index) = super::super::dabplus_index(sub)
                    && let Ok(sync) = DabPlusSync::new(index)
                {
                    transports.insert(*id, sync);
                }
            }
            for frame in receiver.take_msc_frames() {
                let Some(sync) = transports.get_mut(&frame.sub_channel) else {
                    continue;
                };
                for sf in sync.push(&frame.bytes) {
                    let entry = clean.entry(frame.sub_channel).or_default();
                    entry.0 += 1;
                    entry.1 += sf.aus.iter().filter(|au| au.crc_ok).count() as u64;
                    if entry.0 == 1 {
                        eprintln!(
                            "subCh {}: first clean superframe, header {:?}",
                            frame.sub_channel, sf.header
                        );
                    }
                }
            }
        }
        eprintln!(
            "receiver: decoded {} rejected {} (holes every {hole})",
            receiver.frames_decoded, receiver.frames_rejected,
        );
        let mut total_ok = 0u64;
        let mut mismatches = 0usize;
        for (id, sync) in &transports {
            let (sf, aus) = clean.get(id).copied().unwrap_or((0, 0));
            total_ok += aus;
            mismatches += usize::from(sync.shape_mismatch());
            let status = receiver.status().msc.get(id).copied().unwrap_or_default();
            eprintln!(
                "subCh {id}: {} logical frames, {} clean superframes, {aus} AU CRCs ok, \
                 {} search windows, shape {}",
                status.frames,
                sf,
                sync.search_windows(),
                if sync.shape_mismatch() {
                    "MISMATCH"
                } else {
                    "ok"
                }
            );
        }
        eprintln!(
            "total AU CRCs ok: {total_ok}, sub-channels declared a shape mismatch: {mismatches}"
        );
        assert!(total_ok > 0, "no sub-channel produced a CRC-clean AU");
        assert_eq!(
            mismatches, 0,
            "the transport must not declare a real DAB+ ensemble not-DAB+"
        );
    }

    /// 48 kHz → 44.1 kHz keeps a 1 kHz tone's frequency and produces the
    /// right number of samples; the identity ratio is untouched.
    #[test]
    fn rate_converter_keeps_a_tone() {
        let n = 48_000usize;
        let input: Vec<f32> = (0..n)
            .map(|k| (std::f32::consts::TAU * 1000.0 * k as f32 / 48_000.0).sin())
            .collect();
        let mut converter = RateConverter::new(48_000.0, 44_100.0);
        let mut out = Vec::new();
        converter.process(&input, &mut out);
        assert!(
            (out.len() as i64 - 44_100).abs() < 4,
            "{} samples out",
            out.len()
        );
        // Zero crossings over the middle half give the frequency back.
        let mid = &out[out.len() / 4..out.len() * 3 / 4];
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let hz = crossings as f64 / 2.0 / (mid.len() as f64 / 44_100.0);
        assert!((hz - 1000.0).abs() < 20.0, "{hz} Hz");

        let mut identity = RateConverter::new(48_000.0, 48_000.0);
        let mut out = Vec::new();
        identity.process(&[0.5, -0.5], &mut out);
        assert_eq!(out, vec![0.5, -0.5]);
    }
}
