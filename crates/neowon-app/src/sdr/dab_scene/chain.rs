//! The transmitter chain for a sub-channel whose bytes the scene chooses:
//! energy dispersal, the mother code, puncturing and the clause-12 time
//! interleaver, mirroring what the receiver undoes.

use neowon_dsp::dab::CU_BITS;
use neowon_dsp::dab::Protection;
use neowon_dsp::dab::encoder::SubChannelSpec;
use neowon_dsp::dab::fec::{
    EepProfile, UEP_PROFILES, UepProfile, conv_encode, energy_dispersal, puncture_regions,
};
use neowon_dsp::dab::msc::{DEINTERLEAVE_DEPTH, DEINTERLEAVE_MAP, Profile};

/// The coding profile a FIC protection entry resolves to — the same
/// resolution `MscEncoder` performs, refusing a plan the standard does not
/// define rather than guessing.
pub(super) fn profile_of(protection: Protection, size_cu: u16) -> Profile {
    match protection {
        Protection::Eep { option, level } => {
            Profile::Eep(EepProfile::for_size(size_cu, level, option).expect("EEP plan resolves"))
        }
        Protection::Uep { table_index } => {
            let profile: UepProfile = UEP_PROFILES[table_index as usize];
            assert_eq!(profile.size_cu, size_cu, "UEP size must be table 8's");
            Profile::Uep(profile)
        }
    }
}

/// One sub-channel's transmitter chain for a chosen byte source: energy
/// dispersal, the mother code, puncturing, zero padding, and the clause-12
/// time interleaver (table 21).
///
/// This mirrors `neowon_dsp::dab::msc::TimeInterleaver` — which is
/// `pub(crate)` and generates its payloads rather than accepting them — using
/// the public clause-12 map. It exists because the DLS carrier and the two
/// audio programmes must carry *chosen* bytes through the same FEC the
/// receiver undoes. If the encoder ever gains a payload-injection API, this
/// should call it instead.
pub(super) struct ChosenChannel {
    /// Where the sub-channel sits in the CIF, in bits.
    pub(super) start_bit: usize,
    profile: Profile,
    cu_bits: usize,
    /// Bytes one logical frame carries, from the profile.
    chunk: usize,
    /// The source cycles on whole logical frames, so the scene loop seam is
    /// a stream boundary too.
    source: Vec<u8>,
    cursor: usize,
    ring: Vec<u8>,
    frame: u64,
}

impl ChosenChannel {
    pub(super) fn new(sub: &SubChannelSpec, source: Vec<u8>) -> Self {
        let profile = profile_of(sub.protection, sub.size_cu);
        let cu_bits = sub.size_cu as usize * CU_BITS;
        assert!(
            profile.punctured_bits() <= cu_bits,
            "the punctured codeword must fit its allocation"
        );
        // UEP has padding up to the CU boundary (table 15); EEP fills it
        // exactly. The ring write below zero-fills the padding, which is
        // what the encoder mirror transmits and the receiver depunctures.
        let chunk = profile.info_bits() / 8;
        assert!(
            !source.is_empty() && source.len().is_multiple_of(chunk),
            "the source must cycle on a whole logical frame"
        );
        Self {
            start_bit: sub.start_cu as usize * CU_BITS,
            profile,
            cu_bits,
            chunk,
            source,
            cursor: 0,
            ring: vec![0u8; cu_bits * DEINTERLEAVE_DEPTH],
            frame: 0,
        }
    }

    /// Logical frames one full cycle of the source spans.
    pub(super) fn period_frames(&self) -> usize {
        self.source.len() / self.chunk
    }

    /// Encode the next logical frame's payload into the sub-channel's CU
    /// field, in transmission order.
    pub(super) fn advance(&mut self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(self.chunk);
        for _ in 0..self.chunk {
            payload.push(self.source[self.cursor]);
            self.cursor = (self.cursor + 1) % self.source.len();
        }
        let mut info: Vec<u8> = payload
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |bit| (byte >> bit) & 1))
            .collect();
        energy_dispersal(&mut info);
        let mother = conv_encode(&info);
        let punctured = puncture_regions(&mother, &self.profile.regions());
        assert!(punctured.len() <= self.cu_bits);
        let r = self.frame;
        let slot = (r as usize) % DEINTERLEAVE_DEPTH;
        for i in 0..self.cu_bits {
            self.ring[slot * self.cu_bits + i] = punctured.get(i).copied().unwrap_or(0);
        }
        let mut out = vec![0u8; self.cu_bits];
        for i in 0..self.cu_bits {
            let delay = DEINTERLEAVE_MAP[i % DEINTERLEAVE_DEPTH];
            if r >= delay as u64 {
                let source = ((r - delay as u64) as usize) % DEINTERLEAVE_DEPTH;
                out[i] = self.ring[source * self.cu_bits + i];
            }
        }
        self.frame += 1;
        out
    }
}
