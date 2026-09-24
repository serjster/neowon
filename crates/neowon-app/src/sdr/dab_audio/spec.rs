use neowon_codec::aac::{AacDecoder, AudioSpecificConfig};
use neowon_dsp::dab::fec::{EepProfile, uep_profile};
use neowon_dsp::dab::{DabStatus, Protection, SubChannel};

use super::super::SdrState;

/// The audio coding of a service, from its `ASCTy` (EN 300 401 clause
/// 8.1.14 / table 33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    /// `ASCTy` 63: DAB+ (HE-AAC v2), TS 102 563.
    DabPlus,
    /// `ASCTy` 0: MPEG-1 Layer II (DAB classic).
    Mp2,
}

impl Coding {
    /// The coding this build decodes, or `None` for one it does not —
    /// refused, never guessed.
    #[must_use]
    pub fn from_ascty(ascty: Option<u8>) -> Option<Self> {
        match ascty {
            Some(0) => Some(Self::Mp2),
            Some(63) => Some(Self::DabPlus),
            _ => None,
        }
    }

    /// The codec backend this build links, for the readout.
    #[must_use]
    pub fn backend(self) -> &'static str {
        match self {
            Self::DabPlus => AacDecoder::BACKEND,
            // `neowon-codec` hides `oxideav-mp2` 0.0.10 behind its own
            // adapter and exposes no `BACKEND` constant for it; the name
            // here is that pinned dependency, not an invention.
            Self::Mp2 => "oxideav-mp2",
        }
    }
}

/// What the worker needs to decode one service's stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamSpec {
    pub coding: Coding,
    /// The MSC sub-channel the bytes arrive from.
    pub sub_channel: u8,
    /// DAB+: `subchannel_index` in 8 kbit/s units (TS 102 563 clause 5.1),
    /// 1..=24. The super-frame decoder validates it.
    pub subchannel_index: u8,
    /// DAB+ only: use this config instead of `for_dabplus(header)`.
    ///
    /// Real streams pass `None` and take the standard-derived config; the
    /// `rf-dab` sim scene's fixture is a 1024-line stream (no open encoder
    /// emits the mandated 960 transform) and passes its encoder's own
    /// config, which is the only way the playback path can be exercised
    /// without hardware. See `tests/fixtures/README.md`.
    pub asc_override: Option<AudioSpecificConfig>,
}

/// Resolve the selected service into a stream this build can decode, or a
/// reason the operator can read. Every fact comes from the FIC; nothing is
/// assumed about the service.
pub fn spec_for(sdr: &SdrState) -> Result<(u16, StreamSpec), String> {
    let sid = sdr
        .dab
        .service
        .ok_or("dab play: no service selected (sdr dab service ...)")?;
    let rx = sdr
        .dab
        .rx
        .as_ref()
        .ok_or("dab play: receiver is off (sdr dab on)")?;
    let status = rx.status();
    if !status.locked {
        return Err("dab play: no ensemble table yet".into());
    }
    let spec = service_spec(&status, sid).map_err(|e| format!("dab play: {e}"))?;
    Ok((sid, spec))
}

/// Whether one service of a locked table can play in this build, and if not
/// why — the question `spec_for` answers for the selection, asked of any
/// service so the dock and `get dab` can say it *before* Play is pressed:
/// a service that can never play is not drawn like one that can.
pub fn service_spec(status: &DabStatus, sid: u16) -> Result<StreamSpec, String> {
    let service = status
        .ensemble
        .services
        .get(&sid)
        .ok_or_else(|| format!("SId {sid} not in the locked table"))?;
    let sub_channel = service
        .sub_channel
        .ok_or_else(|| format!("service {sid:04X} has no audio sub-channel"))?;
    let sub = status
        .ensemble
        .sub_channels
        .get(&sub_channel)
        .ok_or_else(|| format!("sub-channel {sub_channel} is not in the table"))?;
    let coding = Coding::from_ascty(service.ascty).ok_or_else(|| {
        format!(
            "service {sid:04X} is ASCTy {}, which this build does not decode",
            service
                .ascty
                .map_or_else(|| "unknown".into(), |a| a.to_string())
        )
    })?;
    let (subchannel_index, asc_override) = match coding {
        Coding::DabPlus => (
            dabplus_index(sub)?,
            super::super::dab_scene::asc_override(status.ensemble.eid, sid)
                .map(AudioSpecificConfig::parse)
                .transpose()
                .map_err(|e| format!("the scene's ASC override is invalid: {e}"))?,
        ),
        Coding::Mp2 => (0, None),
    };
    Ok(StreamSpec {
        coding,
        sub_channel,
        subchannel_index,
        asc_override,
    })
}

/// The DAB+ `subchannel_index` the FIC's sub-channel resolves to: TS 102 563
/// clause 5.1 makes a super frame `subchannel_index × 110` bytes carried in
/// five 24 ms logical frames, so one logical frame is `24 × index` bytes of
/// information — the same `info_bits` the MSC profile resolves to. The FIC
/// signals size and protection, not the index, so it is derived here exactly
/// as the MSC decoder resolves it, refusing what the standard does not define.
pub(super) fn dabplus_index(sub: &SubChannel) -> Result<u8, String> {
    let info_bits = match sub.protection {
        Protection::Eep { option, level } => {
            let size = sub
                .size_cu
                .ok_or_else(|| "sub-channel size unknown".to_string())?;
            EepProfile::for_size(size, level, option).map(|p| p.info_bits())
        }
        Protection::Uep { table_index } => uep_profile(table_index).map(|p| p.info_bits()),
    }
    .ok_or_else(|| {
        format!(
            "sub-channel {} has an unresolved protection plan ({})",
            sub.id,
            sub.protection.label()
        )
    })?;
    let index = info_bits / 8 / 24;
    if !(1..=24).contains(&index) {
        // Said in the operator's units (kbit/s) first; the codec's index
        // follows for whoever is reading TS 102 563.
        return Err(format!(
            "sub-channel {} carries {} kbit/s, outside DAB+'s 8..=192 kbit/s \
             (subchannel_index {index}, 1..=24)",
            sub.id,
            index * 8
        ));
    }
    Ok(index as u8)
}
