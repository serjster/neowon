//! The MSC encoder mirror: the transmitter side of [`crate::dab::msc`].
//!
//! Each sub-channel's logical frame (24 ms at its bit rate) is built from a
//! deterministic payload, then coded in the standard's order — energy
//! dispersal (clause 10.3), the mother convolutional code (clause 11.1.1),
//! puncturing per the sub-channel's EEP or UEP profile (clause 11.3), zero
//! padding for the profiles that need it (table 15), and time interleaving
//! (clause 12) — and placed at its `start_cu` in the CIF.
//!
//! **The payload CRC is an oracle transport, not DAB.** EN 300 401 defines no
//! CRC on an MSC logical frame (DAB+ has its own at the superframe level),
//! so [`MscEncoder`] can append one — annex E's CRC-16 over the payload —
//! and the decoder checks it only when told to. On air the flag is off and
//! `crc_checks` stays zero, which is the honest "not checked".

use std::collections::BTreeMap;

use crate::dab::fec::{EepProfile, UEP_PROFILES, UepProfile, crc16};
use crate::dab::msc::{Profile, TimeInterleaver};
use crate::dab::{CIF_SOFT_BITS, CU_BITS, Protection};

/// One sub-channel for the oracle encoder: where it sits and how it is
/// protected. The payload itself is generated deterministically by
/// [`MscEncoder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MscSubChannelSpec {
    pub id: u8,
    pub start_cu: u16,
    pub size_cu: u16,
    pub protection: Protection,
}

impl MscSubChannelSpec {
    /// Resolve the FIC-facing description into the coding profile, refusing a
    /// plan the standard does not define — the same resolution the decoder
    /// performs, so the two sides cannot disagree silently.
    fn profile(&self) -> Profile {
        match self.protection {
            Protection::Eep { option, level } => {
                let profile =
                    EepProfile::for_size(self.size_cu, level, option).expect("EEP plan resolves");
                assert_eq!(profile.size_cu() as u16, self.size_cu, "EEP size");
                Profile::Eep(profile)
            }
            Protection::Uep { table_index } => {
                let profile: UepProfile = UEP_PROFILES[table_index as usize];
                assert_eq!(
                    profile.size_cu, self.size_cu,
                    "UEP size must be the table-8 size"
                );
                Profile::Uep(profile)
            }
        }
    }
}

/// One CIF's worth of encoded MSC: the transmitted bits for the whole 864 CU
/// field, plus the payload each sub-channel should get back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MscCif {
    /// The CIF's 55 296 transmitted bits (hard decisions).
    pub bits: Vec<u8>,
    /// Full logical-frame payloads, keyed by `SubChId`: the bytes the decoder
    /// must reproduce, CRC included when the oracle transport carries one.
    pub payloads: BTreeMap<u8, Vec<u8>>,
    /// The logical frame index this CIF carries (counted from the encoder's
    /// first call; the receiver's de-interleaver is aligned to the same count
    /// when both start together).
    pub logical_frame: u64,
}

impl MscCif {
    /// Full transmitted bits, for [`super::FicFrame::iq_frame_with_msc`].
    pub fn transmitted_bits(&self) -> &[u8] {
        &self.bits
    }

    /// The CIF as soft bits at full confidence — what [`crate::dab::msc`] sees
    /// on a noiseless channel, which isolates the MSC FEC chain from the OFDM
    /// layer (the same split `FicFrame::ideal_soft_bits` makes).
    pub fn soft_bits(&self) -> Vec<i8> {
        self.bits
            .iter()
            .map(|bit| if *bit == 1 { 100 } else { -100 })
            .collect()
    }
}

struct Plan {
    spec: MscSubChannelSpec,
    profile: Profile,
    interleaver: TimeInterleaver,
}

/// A deterministic MSC encoder/oracle: one plan per sub-channel, one CIF per
/// call, payloads generated from a seeded stream (no wall clock, no
/// `thread_rng`).
pub struct MscEncoder {
    plans: Vec<Plan>,
    payload_crc: bool,
    seed: u64,
    frame: u64,
}

impl MscEncoder {
    /// Build the encoder from the same sub-channel plan the FIC signals, with
    /// an oracle payload CRC per logical frame when `payload_crc` is set.
    pub fn new(specs: Vec<MscSubChannelSpec>, payload_crc: bool, seed: u64) -> Self {
        let plans = specs
            .into_iter()
            .map(|spec| {
                let profile = spec.profile();
                let cu_bits = spec.size_cu as usize * CU_BITS;
                Plan {
                    spec,
                    profile,
                    interleaver: TimeInterleaver::new(cu_bits),
                }
            })
            .collect();
        Self {
            plans,
            payload_crc,
            seed,
            frame: 0,
        }
    }

    /// Encode the next logical frame for every sub-channel into one CIF.
    pub fn next_cif(&mut self) -> MscCif {
        let frame = self.frame;
        let mut bits = vec![0u8; CIF_SOFT_BITS];
        let mut payloads = BTreeMap::new();
        for plan in &mut self.plans {
            let info_bits = plan.profile.info_bits();
            let data_bytes = info_bits / 8 - if self.payload_crc { 2 } else { 0 };
            let mut payload: Vec<u8> = (0..data_bytes)
                .map(|index| payload_byte(self.seed, frame, plan.spec.id, index))
                .collect();
            if self.payload_crc {
                let crc = crc16(&payload);
                payload.push((crc >> 8) as u8);
                payload.push(crc as u8);
            }
            let info: Vec<u8> = payload
                .iter()
                .flat_map(|byte| (0..8).rev().map(move |bit| (byte >> bit) & 1))
                .collect();
            debug_assert_eq!(info.len(), info_bits);
            let coded = plan.interleaver.push(&info, plan.profile);
            let start = plan.spec.start_cu as usize * CU_BITS;
            bits[start..start + coded.len()].copy_from_slice(&coded);
            payloads.insert(plan.spec.id, payload);
        }
        self.frame += 1;
        MscCif {
            bits,
            payloads,
            logical_frame: frame,
        }
    }

    /// Encode `count` CIFs in order (a transmission frame is four).
    pub fn next_cifs(&mut self, count: usize) -> Vec<MscCif> {
        (0..count).map(|_| self.next_cif()).collect()
    }

    /// Forget the interleaver history (a retune, or a fixture boundary).
    pub fn reset(&mut self) {
        for plan in &mut self.plans {
            plan.interleaver.reset();
        }
        self.frame = 0;
    }
}

/// A deterministic payload byte: a splitmix64 mix of the seed, the logical
/// frame, the sub-channel and the byte index. Any value the decoder gets back
/// that differs from this did not come from the encoder.
fn payload_byte(seed: u64, frame: u64, id: u8, index: usize) -> u8 {
    let mut value =
        seed ^ frame.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((id as u64) << 32) ^ index as u64;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (value ^ (value >> 31)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::msc::MscDecoder;
    use crate::dab::{Ensemble, SubChannel};

    fn spec(id: u8, start: u16, size: u16, protection: Protection) -> MscSubChannelSpec {
        MscSubChannelSpec {
            id,
            start_cu: start,
            size_cu: size,
            protection,
        }
    }

    fn plan(size: u16, protection: Protection) -> Vec<SubChannel> {
        vec![SubChannel {
            id: 0,
            start_cu: 0,
            size_cu: Some(size),
            protection,
            bitrate_kbps: None,
        }]
    }

    /// The soft-bit chain round-trips for both protections: after the 16-frame
    /// warm-up the decoder returns exactly the payloads the encoder produced,
    /// with every oracle CRC clean. This is everything between `next_cif` and
    /// `push_cif` — FEC, dispersal, puncturing, padding and clause 12 — and it
    /// passes only if both sides agree bit for bit.
    #[test]
    fn cif_round_trips_through_the_decoder_soft_chain() {
        for (protection, size) in [
            (
                Protection::Eep {
                    option: 0,
                    level: 2,
                },
                96,
            ),
            (Protection::Uep { table_index: 35 }, 96),
        ] {
            let mut ensemble = Ensemble::default();
            for sub in plan(size, protection) {
                ensemble.sub_channels.insert(sub.id, sub);
            }
            let mut demux = MscDecoder::new();
            demux.sync(&ensemble);
            demux.set_payload_crc(true);

            let mut encoder = MscEncoder::new(vec![spec(0, 0, size, protection)], true, 7);
            let mut expected: Vec<Vec<u8>> = Vec::new();
            let mut decoded: Vec<Vec<u8>> = Vec::new();
            for _ in 0..20 {
                let cif = encoder.next_cif();
                expected.push(cif.payloads[&0].clone());
                demux.push_cif(&cif.soft_bits());
                decoded.extend(demux.take_frames().into_iter().map(|f| f.bytes));
            }
            assert_eq!(decoded.len(), 20 - 16, "{protection:?} warm-up");
            for (index, bytes) in decoded.iter().enumerate() {
                assert_eq!(bytes, &expected[index], "{protection:?} frame {index}");
            }
            let status = demux.status();
            assert_eq!(status[&0].crc_failures, 0, "{protection:?}");
            assert_eq!(status[&0].crc_checks, decoded.len() as u64);
        }
    }

    /// Every profile's padding is zero bits appended after the puncturing, so a
    /// CIF holds the plan's size and nothing else.
    #[test]
    fn padding_is_zero_and_sized() {
        let uep = UEP_PROFILES[35];
        assert_eq!(uep.padding, 4);
        let mut encoder = MscEncoder::new(
            vec![spec(1, 0, uep.size_cu, Protection::Uep { table_index: 35 })],
            false,
            1,
        );
        let cif = encoder.next_cif();
        assert_eq!(cif.bits.len(), CIF_SOFT_BITS);
        // Bits after the sub-channel's allocation are untouched.
        let end = uep.size_cu as usize * CU_BITS;
        assert!(cif.bits[end..].iter().all(|b| *b == 0));
    }

    /// Encoding is a pure function of the seed and the frame index.
    #[test]
    fn payloads_are_deterministic() {
        let specs = vec![spec(
            0,
            0,
            96,
            Protection::Eep {
                option: 0,
                level: 2,
            },
        )];
        let a = MscEncoder::new(specs.clone(), true, 5).next_cif();
        let b = MscEncoder::new(specs, true, 5).next_cif();
        assert_eq!(a, b);
        let c = MscEncoder::new(
            vec![spec(
                0,
                0,
                96,
                Protection::Eep {
                    option: 0,
                    level: 2,
                },
            )],
            true,
            6,
        )
        .next_cif();
        assert_ne!(a.payloads, c.payloads);
    }
}
