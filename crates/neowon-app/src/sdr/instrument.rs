//! `instrument scope|sdr`: switch instrument at run time. The old supervisor
//! is dropped first (its thread shuts the backend down and releases the
//! device), then the launch family's other instrument is spawned and sent
//! the config the UI already holds for it, so each mode resumes where it
//! was left.

use neowon_backend::{InstrumentConfig, ScopeCaps, SdrCaps};

use super::SdrState;
use crate::Link;

impl Link {
    /// The scope's capabilities, when a scope is what is connected.
    ///
    /// The link carries one `Option<Capabilities>` and the variant is the
    /// instrument; these two readers are its halves, so nothing
    /// downstream keeps a second `Option` in step by hand.
    pub fn scope_caps(&self) -> Option<&ScopeCaps> {
        self.caps.as_ref()?.scope()
    }

    /// The SDR's capabilities, when an SDR is what is connected.
    pub fn sdr_caps(&self) -> Option<&SdrCaps> {
        self.caps.as_ref()?.sdr()
    }
}

pub fn switch(to_sdr: bool, sdr: &mut SdrState, link: &mut Link) -> Result<(), String> {
    if to_sdr == sdr.active {
        return Ok(());
    }
    // One device claim at a time: release before claiming the other.
    link.sup.shutdown();
    link.sup = sdr.launch.supervisor(to_sdr);
    link.status = "connecting…".into();
    link.caps = None;
    link.latest = None;
    sdr.latest = None;
    sdr.frames_seen = 0;
    sdr.spectrum = None;
    sdr.survey = None;
    sdr.analysis = None;
    sdr.classification = None;
    sdr.tracker.clear();
    // The other instrument owns the signal now: a DAB table from the old
    // stream would be a stale claim when this mode comes back.
    sdr.dab_reset();
    sdr.active = to_sdr;
    if to_sdr {
        link.sup.apply(InstrumentConfig::Sdr(sdr.config.clone()));
        link.stimulus = neowon_sim::RfScene::PRESETS[0].into();
    } else {
        link.sup.apply(link.config.clone());
        link.stimulus = "probe-comp".into();
    }
    Ok(())
}
