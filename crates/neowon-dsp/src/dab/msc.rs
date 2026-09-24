//! The Main Service Channel: CIF soft bits in, sub-channel bytes out.
//!
//! One [`SubChannelDecoder`] per `SubChId`, created and reset from the FIC's
//! FIG 0/1 table. Each consumes one CIF at a time — a CIF is 24 ms and carries
//! exactly one *logical frame* of every sub-channel — and reverses the
//! transmitter's chain, in order (clauses of **ETSI EN 300 401 V2.1.1**):
//!
//! 1. **Capacity-unit extraction.** A CIF is 864 CUs of 64 bits (clause 13);
//!    the sub-channel's fragment is `size_cu` CUs at `start_cu` (clause 6.2.1).
//! 2. **Time de-interleaving** (clause 12, table 21). The delay of bit `i` runs
//!    over 16 logical frames and depends on `i mod 16`:
//!    `D = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]`, so
//!    `c[r][i] = b[r - D[i mod 16]][i]`; the receiver reads a 16-deep ring and
//!    has 16 logical frames of latency before its output is valid.
//! 3. **Energy dispersal** (clause 10.3): the logical frame is descrambled with
//!    the same PRBS as the FIC but started once per logical frame.
//! 4. **Depuncturing** to the mother code, per the sub-channel's EEP profile
//!    (clause 11.3.2) or UEP profile (clause 11.3.1); punctured positions
//!    become erasures.
//! 5. **Terminated soft Viterbi.** Both MSC protections end in the six zero
//!    tail bits of clause 11.1.1, so the survivor is read from state 0 — the
//!    same decoder the FIC uses.
//! 6. **Payload check.** EN 300 401 defines no CRC at this layer; the DAB+
//!    superframe CRC belongs to the audio transport. So the decoder verifies a trailing CRC-16
//!    (annex E) only when the caller declares the payload carries one — the
//!    sim's oracle transport in the tests. On air nothing is checked and
//!    `crc_checks` stays 0, which is the honest "not checked" rather than a
//!    guess.
//!
//! [`MscDecoder`] is the demux: it watches the FIC's sub-channel table, keeps
//! one handler per resolvable sub-channel, and queues the logical frames they
//! emit. A sub-channel whose protection the standard does not define is
//! refused, not guessed.

// Ported from dabradio 0.5.0 (MIT); notice in docs/protocol-dab.md
use std::collections::{BTreeMap, VecDeque};

use super::fec::{EepProfile, UepProfile, conv_encode, crc16, energy_dispersal, viterbi_decode};
use super::{CIF_SOFT_BITS, CIFS_PER_FRAME, CU_BITS, Ensemble, Protection, SubChannel};

/// Time interleaver depth in logical frames (clause 12, table 21).
pub const DEINTERLEAVE_DEPTH: usize = 16;

/// Clause 12, table 21: the delay in logical frames of a bit at position
/// `i mod 16`. The standard writes `r' = r - D`; table 21's rows, read as a
/// list, are exactly this sequence.
pub const DEINTERLEAVE_MAP: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];

/// How many logical frames [`MscDecoder`] queues before dropping the oldest.
/// A caller that never drains must not grow the decoder without bound.
const MAX_QUEUED_FRAMES: usize = 4096;

/// The clause-12 time de-interleaver for one sub-channel's logical frames.
///
/// `push` consumes one received fragment and returns the reconstructed
/// fragment for the logical frame 16 frames back; `None` during the 16-frame
/// warm-up, when the ring has not seen every slot.
pub struct TimeDeinterleaver {
    cu_bits: usize,
    /// 16 logical frames of fragment, slot `r mod 16`.
    ring: Vec<i8>,
    received: u64,
    output: Vec<i8>,
}

impl TimeDeinterleaver {
    pub fn new(cu_bits: usize) -> Self {
        Self {
            cu_bits,
            ring: vec![0i8; cu_bits * DEINTERLEAVE_DEPTH],
            received: 0,
            output: Vec::with_capacity(cu_bits),
        }
    }

    /// Feed one received fragment (`cu_bits` soft bits); the reconstructed
    /// fragment comes out 16 logical frames later.
    pub fn push(&mut self, fragment: &[i8]) -> Option<Vec<i8>> {
        if fragment.len() != self.cu_bits {
            return None;
        }
        let r = self.received;
        let slot = (r as usize) % DEINTERLEAVE_DEPTH;
        self.output.clear();
        for i in 0..self.cu_bits {
            let source = (slot + DEINTERLEAVE_MAP[i % DEINTERLEAVE_DEPTH]) % DEINTERLEAVE_DEPTH;
            self.output.push(self.ring[source * self.cu_bits + i]);
            self.ring[slot * self.cu_bits + i] = fragment[i];
        }
        self.received += 1;
        if r < DEINTERLEAVE_DEPTH as u64 {
            return None;
        }
        Some(std::mem::replace(
            &mut self.output,
            Vec::with_capacity(self.cu_bits),
        ))
    }

    pub fn reset(&mut self) {
        self.ring.fill(0);
        self.received = 0;
        self.output.clear();
    }

    /// Logical frames processed, warm-up included.
    pub fn received(&self) -> u64 {
        self.received
    }
}

/// The protection a sub-channel's logical frames use, resolved from the FIC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Eep(EepProfile),
    Uep(UepProfile),
}

impl Profile {
    /// Information bits per 24 ms logical frame.
    pub fn info_bits(self) -> usize {
        match self {
            Profile::Eep(p) => p.info_bits(),
            Profile::Uep(p) => p.info_bits(),
        }
    }

    /// Mother-code bits per logical frame, tail included.
    pub fn mother_bits(self) -> usize {
        match self {
            Profile::Eep(p) => p.mother_bits(),
            Profile::Uep(p) => p.mother_bits(),
        }
    }

    /// Transmitted (punctured) bits per logical frame, padding excluded.
    pub fn punctured_bits(self) -> usize {
        match self {
            Profile::Eep(p) => p.punctured_bits(),
            Profile::Uep(p) => p.punctured_bits(),
        }
    }

    /// Depuncture one logical frame to the mother code.
    pub fn depuncture(self, soft: &[i8]) -> Vec<i8> {
        match self {
            Profile::Eep(p) => p.depuncture(soft),
            Profile::Uep(p) => p.depuncture(soft),
        }
    }

    /// The puncture regions in transmission order — EEP's two, UEP's four.
    pub fn regions(self) -> Vec<(usize, u8)> {
        match self {
            Profile::Eep(p) => p.regions().to_vec(),
            Profile::Uep(p) => p.regions().to_vec(),
        }
    }
}

/// Per-sub-channel counters, as `get dab` and the dock report them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubChannelStatus {
    /// Logical frames emitted after the de-interleaver's warm-up.
    pub frames: u64,
    /// Payload CRCs checked (0 on air: EN 300 401 has none at this layer).
    pub crc_checks: u64,
    /// Payload CRCs that failed.
    pub crc_failures: u64,
    pub bytes: u64,
}

/// One decoded sub-channel logical frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// The `SubChId` the bytes came from.
    pub sub_channel: u8,
    /// The logical frame's payload: `24 × bit rate / 8` bytes.
    pub bytes: Vec<u8>,
}

/// The decoder for one sub-channel: de-interleave, depuncture, Viterbi,
/// descramble, check.
pub struct SubChannelDecoder {
    id: u8,
    start_cu: u16,
    size_cu: u16,
    profile: Profile,
    payload_crc: bool,
    interleaver: TimeDeinterleaver,
    status: SubChannelStatus,
}

impl SubChannelDecoder {
    /// Build a decoder for a FIC sub-channel entry. Returns `None` when the
    /// protection is not one the standard defines for the signalled size —
    /// refusing beats guessing.
    pub fn new(sub: &SubChannel) -> Option<Self> {
        let size_cu = sub.size_cu?;
        let profile = match sub.protection {
            Protection::Eep { option, level } => {
                Profile::Eep(EepProfile::for_size(size_cu, level, option)?)
            }
            Protection::Uep { table_index } => {
                let profile = super::fec::uep_profile(table_index)?;
                if profile.size_cu != size_cu {
                    return None;
                }
                Profile::Uep(profile)
            }
        };
        Some(Self {
            id: sub.id,
            start_cu: sub.start_cu,
            size_cu,
            profile,
            payload_crc: false,
            interleaver: TimeDeinterleaver::new(size_cu as usize * CU_BITS),
            status: SubChannelStatus::default(),
        })
    }

    /// Check a trailing CRC-16 (annex E) on each logical frame. This is the
    /// sim's oracle transport, not a DAB on-air check; off by default.
    pub fn set_payload_crc(&mut self, on: bool) {
        self.payload_crc = on;
    }

    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn status(&self) -> SubChannelStatus {
        self.status
    }

    /// Feed one CIF (55 296 soft bits, clause 13) and return the logical frame
    /// that completes 16 CIFs back, if any.
    pub fn push_cif(&mut self, cif: &[i8]) -> Option<Vec<u8>> {
        let start = self.start_cu as usize * CU_BITS;
        let size = self.size_cu as usize * CU_BITS;
        if start + size > cif.len() {
            return None;
        }
        let fragment = &cif[start..start + size];
        let deinterleaved = self.interleaver.push(fragment)?;
        let depunctured = self.profile.depuncture(&deinterleaved);
        let (mut bits, _metric) = viterbi_decode(&depunctured);
        if bits.len() != self.profile.info_bits() {
            return None;
        }
        // Clause 10.3: dispersal is undone after the Viterbi decoder, on the
        // logical frame's information bits only.
        energy_dispersal(&mut bits);
        let mut bytes = Vec::with_capacity(bits.len() / 8);
        for chunk in bits.chunks_exact(8) {
            let mut byte = 0u8;
            for bit in chunk {
                byte = (byte << 1) | *bit;
            }
            bytes.push(byte);
        }
        if self.payload_crc && bytes.len() >= 2 {
            let split = bytes.len() - 2;
            let stored = ((bytes[split] as u16) << 8) | bytes[split + 1] as u16;
            self.status.crc_checks += 1;
            if crc16(&bytes[..split]) != stored {
                self.status.crc_failures += 1;
            }
        }
        self.status.frames += 1;
        self.status.bytes += bytes.len() as u64;
        Some(bytes)
    }

    pub fn reset(&mut self) {
        self.interleaver.reset();
        self.status = SubChannelStatus::default();
    }

    /// Restart the de-interleaver's warm-up, keeping the counters.
    pub fn discard(&mut self) {
        self.interleaver.reset();
    }
}

/// The MSC demultiplexer: one handler per sub-channel, driven by the FIC.
#[derive(Default)]
pub struct MscDecoder {
    handlers: BTreeMap<u8, SubChannelDecoder>,
    /// The `(start_cu, size_cu, protection)` each handler was built from, so a
    /// reconfiguration resets the handler rather than mixing plans.
    plans: BTreeMap<u8, (u16, u16, Protection)>,
    frames: VecDeque<DecodedFrame>,
    payload_crc: bool,
    dropped: u64,
}

impl MscDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Turn the oracle payload CRC on or off (rebuilt handlers pick it up).
    pub fn set_payload_crc(&mut self, on: bool) {
        self.payload_crc = on;
        for handler in self.handlers.values_mut() {
            handler.set_payload_crc(on);
        }
    }

    /// Reconcile the handlers with the FIC's table: create one for every
    /// resolvable sub-channel, reset one whose plan changed, drop one whose
    /// id is gone. Returns how many handlers are live.
    pub fn sync(&mut self, ensemble: &Ensemble) -> usize {
        for (id, sub) in &ensemble.sub_channels {
            let plan = (sub.start_cu, sub.size_cu.unwrap_or(0), sub.protection);
            if self.plans.get(id) == Some(&plan) {
                continue;
            }
            self.plans.insert(*id, plan);
            let mut handler = SubChannelDecoder::new(sub);
            if let Some(handler) = handler.as_mut() {
                handler.set_payload_crc(self.payload_crc);
            }
            match handler {
                Some(handler) => {
                    self.handlers.insert(*id, handler);
                }
                None => {
                    self.handlers.remove(id);
                }
            }
        }
        let known: Vec<u8> = self.handlers.keys().copied().collect();
        for id in known {
            if !ensemble.sub_channels.contains_key(&id) {
                self.handlers.remove(&id);
                self.plans.remove(&id);
            }
        }
        self.handlers.len()
    }

    /// Feed one CIF to every handler; decoded logical frames are queued.
    pub fn push_cif(&mut self, cif: &[i8]) {
        for (id, handler) in self.handlers.iter_mut() {
            if let Some(bytes) = handler.push_cif(cif) {
                self.frames.push_back(DecodedFrame {
                    sub_channel: *id,
                    bytes,
                });
            }
        }
        while self.frames.len() > MAX_QUEUED_FRAMES {
            self.frames.pop_front();
            self.dropped += 1;
        }
    }

    /// Feed one transmission frame's four CIFs, in order.
    pub fn push_frame(&mut self, cifs: &[i8]) {
        if cifs.len() < CIFS_PER_FRAME * CIF_SOFT_BITS {
            return;
        }
        for cif in cifs.chunks_exact(CIF_SOFT_BITS) {
            self.push_cif(cif);
        }
    }

    /// Take the queued logical frames, oldest first.
    pub fn take_frames(&mut self) -> Vec<DecodedFrame> {
        self.frames.drain(..).collect()
    }

    /// Per-sub-channel counters, keyed by `SubChId`.
    pub fn status(&self) -> BTreeMap<u8, SubChannelStatus> {
        self.handlers
            .iter()
            .map(|(id, h)| (*id, h.status()))
            .collect()
    }

    /// Frames dropped because the queue was never drained.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped
    }

    /// Forget the interleaver history but keep the counters: for a gap in the
    /// CIF stream. Clause 12's delay line spans 16 logical frames, so a hole
    /// misaligns every later output; discarding restarts the warm-up instead
    /// of emitting mixed frames.
    pub fn discard(&mut self) {
        for handler in self.handlers.values_mut() {
            handler.discard();
        }
    }

    pub fn reset(&mut self) {
        self.handlers.clear();
        self.plans.clear();
        self.frames.clear();
        self.dropped = 0;
    }
}

/// The transmitter side of the interval encoder: encode one sub-channel's
/// logical frame and time-interleave it into a clause-12 stream. Used by the
/// oracle encoder in [`super::encoder`].
pub(crate) struct TimeInterleaver {
    cu_bits: usize,
    ring: Vec<u8>,
    frame: u64,
}

impl TimeInterleaver {
    pub fn new(cu_bits: usize) -> Self {
        Self {
            cu_bits,
            ring: vec![0u8; cu_bits * DEINTERLEAVE_DEPTH],
            frame: 0,
        }
    }

    /// Encode one logical frame of `info` bits with `profile` and delay it per
    /// clause 12. `info` must be the information bits *before* energy
    /// dispersal; the returned vector is the sub-channel's `cu_bits`-bit
    /// contribution to the CIF, padding included.
    pub fn push(&mut self, info: &[u8], profile: Profile) -> Vec<u8> {
        assert_eq!(info.len(), profile.info_bits(), "logical frame size");
        let mut scrambled = info.to_vec();
        energy_dispersal(&mut scrambled);
        let mother = conv_encode(&scrambled);
        let regions = profile.regions();
        let punctured = super::fec::puncture_regions(&mother, &regions);
        assert_eq!(punctured.len(), profile.punctured_bits());
        let cu_bits = self.cu_bits;
        assert!(punctured.len() <= cu_bits, "padding can only pad");
        let r = self.frame;
        let slot = (r as usize) % DEINTERLEAVE_DEPTH;
        // The current frame is stored before reading so that the zero-delay
        // positions see it (clause 12: r' = r for i mod 16 == 0).
        for (i, bit) in punctured
            .iter()
            .copied()
            .chain(std::iter::repeat(0u8))
            .take(cu_bits)
            .enumerate()
        {
            self.ring[slot * cu_bits + i] = bit;
        }
        let mut out = vec![0u8; cu_bits];
        for i in 0..cu_bits {
            let d = DEINTERLEAVE_MAP[i % DEINTERLEAVE_DEPTH];
            if r >= d as u64 {
                let source = (r - d as u64) as usize % DEINTERLEAVE_DEPTH;
                out[i] = self.ring[source * cu_bits + i];
            }
        }
        self.frame += 1;
        out
    }

    pub fn reset(&mut self) {
        self.ring.fill(0);
        self.frame = 0;
    }
}

#[cfg(test)]
mod tests;
