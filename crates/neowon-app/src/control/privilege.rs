//! The control socket's authorisation policy: which script actions it will
//! run for a client that has not proved it can read this process's token.
//!
//! **Invariant:** a verb that writes outside the process — any path the
//! request names, any store the app keeps on disk, any network request —
//! or that ends the process, is not reachable from the control socket
//! without the token. Everything that only moves live instrument or UI
//! state stays open, so the operator's `nc 127.0.0.1 7777` loop and every
//! `get …` query still work with no ceremony.
//!
//! The classification is **structural, not a maintained list**: every
//! match below is exhaustive with no `_` arm, so adding a variant to
//! `Action`, `SdrAction`, `RefMapAction` or `CatalogAction` fails to
//! compile until it has been classified. The sites that would otherwise
//! decide this by accident are `script/grammar.rs` (the parse table) and
//! `script/mod.rs` (the runtime); neither can add a verb that reaches the
//! socket without passing through here.
//!
//! This applies to the **control socket only**. `NEOWON_SCRIPT` files, the
//! UI's own injected actions and session replay are already the operator's
//! own process doing what the operator asked, so they run unfiltered.

use crate::catalog::CatalogAction;
use crate::refmap::{LocationSet, RefMapAction};
use crate::script::Action;
use crate::sdr::{IqDumpVerb, SdrAction};

/// What a verb is allowed to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// Moves live instrument/UI state inside this process and nothing else.
    Open,
    /// Names a filesystem path, writes a store the app keeps on disk,
    /// reaches the network, or ends the process.
    Trusted,
}

impl Privilege {
    #[must_use]
    pub fn needs_token(self) -> bool {
        self == Privilege::Trusted
    }
}

impl Action {
    /// The trust this action needs when it arrives over the control socket.
    #[must_use]
    pub fn privilege(&self) -> Privilege {
        use Privilege::{Open, Trusted};
        match self {
            // --- delegated to the sub-grammars ---
            Action::Sdr(a) => a.privilege(),
            Action::RefMap(a) => a.privilege(),
            Action::Catalog(a) => a.privilege(),

            // --- names a path the request chose (write) ---
            Action::Shot { .. }
            | Action::ShotPlot { .. }
            | Action::UiTree(_)
            | Action::Layout(_)
            | Action::Export(..)
            | Action::CapSave(_)
            | Action::SessionSave(_) => Trusted,

            // --- names a path the request chose (read) ---
            // `capload` reads any file into the history ring; `sessionload`
            // reads one and *executes it as a script*, so it is the widest
            // of all; `effect <name>` is joined onto the shader directory
            // unchecked, which a `../` walks out of.
            Action::CapLoad(_) | Action::SessionLoad(_) | Action::Effect(Some(_)) => Trusted,

            // --- ends the process ---
            Action::Quit => Trusted,

            // --- live state only ---
            Action::Effect(None)
            | Action::EffectReload
            | Action::Stimulus(_)
            | Action::Rate(_)
            | Action::Vdiv(..)
            | Action::Enable(..)
            | Action::CouplingSet(..)
            | Action::Probe(..)
            | Action::Offset(..)
            | Action::Trigger { .. }
            | Action::TrigPulse { .. }
            | Action::TrigSlope { .. }
            | Action::TrigVideo { .. }
            | Action::Holdoff(_)
            | Action::AutoSet
            | Action::Force
            | Action::Zoom { .. }
            | Action::HZoom { .. }
            | Action::HView(..)
            | Action::Timebase(_)
            | Action::ZoomWin(_)
            | Action::Deep(_)
            | Action::DeepSpan(_)
            | Action::DeepFollow(_)
            | Action::Decode(_)
            | Action::DecodeLine(..)
            | Action::DecodeBaud(_)
            | Action::Pan(_)
            | Action::Home
            | Action::Acq(_)
            | Action::AutoPeak(_)
            | Action::Mode(_)
            | Action::Persist(_)
            | Action::Gain(_)
            | Action::Crt(_)
            | Action::Select(_)
            | Action::Guides(_)
            | Action::Markers(_)
            | Action::Record(_)
            | Action::RecordClear
            | Action::PaletteSet(_)
            | Action::WindowSize(..)
            | Action::UiScaleSet(_)
            | Action::Scrollback(_)
            | Action::SettingsOpen(_)
            | Action::Math(_)
            | Action::Run(_)
            | Action::Multi(_)
            | Action::PfOut(_)
            | Action::Cursor { .. }
            | Action::Stats(_)
            | Action::StatsReset
            | Action::Fft(_)
            | Action::FftSrc(_)
            | Action::FftWnd(_)
            | Action::Pf(_)
            | Action::PfSrc(_)
            | Action::PfTol(..)
            | Action::PfCapture
            | Action::PfReset
            | Action::Menu(_)
            | Action::MeasWin(_)
            | Action::Dock(_)
            | Action::WindowPos(..)
            | Action::TrigPos(_)
            | Action::HistoryIdx(_)
            | Action::HistoryStep(_)
            | Action::HistoryLive
            | Action::RefSave(_)
            | Action::RefShow(_)
            | Action::RefClear
            | Action::Waterfall(_)
            | Action::Viz(_) => Open,
        }
    }
}

impl SdrAction {
    fn privilege(&self) -> Privilege {
        use Privilege::{Open, Trusted};
        match self {
            // The only SDR verb that names a path.
            SdrAction::IqDump(IqDumpVerb::Start { .. }) => Trusted,
            // Stopping a dump only moves live state (like `sdr run off`),
            // so it stays open — no file is created by it.
            SdrAction::IqDump(IqDumpVerb::Stop)
            | SdrAction::Tune(_)
            | SdrAction::Step(_)
            | SdrAction::Rate(_)
            | SdrAction::Gain(_)
            | SdrAction::Agc(_)
            | SdrAction::Ppm(_)
            | SdrAction::Span(_)
            | SdrAction::Fft(_)
            | SdrAction::Level { .. }
            | SdrAction::Run(_)
            | SdrAction::Seed(_)
            | SdrAction::Detect(_)
            | SdrAction::Survey(_)
            | SdrAction::Analyse(_)
            | SdrAction::Modulation(_)
            | SdrAction::Threshold(_)
            | SdrAction::Pan(_)
            | SdrAction::Centre(_)
            | SdrAction::Follow(_)
            | SdrAction::Width(_)
            | SdrAction::Demod(_)
            | SdrAction::Volume(_)
            | SdrAction::Mute(_)
            | SdrAction::Squelch(_)
            | SdrAction::List(_)
            | SdrAction::Instrument(_)
            | SdrAction::Dab(_) => Open,
        }
    }
}

impl RefMapAction {
    fn privilege(&self) -> Privilege {
        use Privilege::{Open, Trusted};
        match self {
            // `refdb fetch` and `location ip` reach the network; `import`
            // names a path; `clear` deletes the reference store; every
            // `location` verb writes the operator's saved fix
            // (`neowon_refdb::geo::location_path`).
            RefMapAction::Fetch(..)
            | RefMapAction::Import(..)
            | RefMapAction::Clear(_)
            | RefMapAction::Location(
                LocationSet::Coords(..)
                | LocationSet::Locator(_)
                | LocationSet::Ip
                | LocationSet::Clear,
            ) => Trusted,
            // `stations catalog` files a record in the on-disk catalog.
            RefMapAction::CatalogStation(_) => Trusted,
            RefMapAction::Plan(_)
            | RefMapAction::Strip(_)
            | RefMapAction::Mini(_)
            | RefMapAction::Window(_)
            | RefMapAction::Goto(_)
            | RefMapAction::StationsWindow(_)
            | RefMapAction::Overlay(_)
            | RefMapAction::Radius(_)
            | RefMapAction::Find(_)
            | RefMapAction::FilterSource(_)
            | RefMapAction::FilterService(_)
            | RefMapAction::FilterModulation(_)
            | RefMapAction::OnAir(_)
            | RefMapAction::Scope(_)
            | RefMapAction::Sort(_)
            | RefMapAction::TuneStation(_) => Open,
        }
    }
}

impl CatalogAction {
    fn privilege(&self) -> Privilege {
        use Privilege::{Open, Trusted};
        match self {
            // The three that only move the view; everything else edits the
            // catalog store on disk (`~/.neowon/catalog`) or names a path.
            CatalogAction::List(_) | CatalogAction::Window(_) | CatalogAction::Select(_) => Open,
            CatalogAction::Add(_)
            | CatalogAction::Observe
            | CatalogAction::Rename(..)
            | CatalogAction::Delete(..)
            | CatalogAction::Purge(..)
            | CatalogAction::Merge(..)
            | CatalogAction::Tag(..)
            | CatalogAction::Alias(..)
            | CatalogAction::Edit(..)
            | CatalogAction::Bulk(..)
            | CatalogAction::Undo
            | CatalogAction::Pin(..)
            | CatalogAction::Export(_)
            | CatalogAction::Import(_)
            | CatalogAction::Survey(_) => Trusted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Privilege;
    use crate::script::parse;

    /// The privilege of the (single) action one script line parses to.
    fn of(line: &str) -> Privilege {
        let actions = parse(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(
            actions.len(),
            1,
            "{line} parsed to {} actions",
            actions.len()
        );
        actions[0].1.privilege()
    }

    /// Every verb that writes a file, reads one the caller named, reaches
    /// the network or ends the process needs the token.
    #[test]
    fn every_verb_that_leaves_the_process_is_trusted() {
        for line in [
            // writes a path the caller chose
            "shot /tmp/a.png",
            "shot /tmp/a.png 1 2 3 4",
            "shotplot /tmp/a.ppm",
            "uitree /tmp/a.json",
            "layout /tmp/a.json",
            "export csv /tmp/a.csv",
            "capsave /tmp/a.nwc",
            "sessionsave /tmp/a.nws",
            "sdr iqdump /tmp/a.f32 1",
            "catalog export /tmp/a.json",
            // reads a path the caller chose
            "capload /tmp/a.nwc",
            "sessionload /tmp/a.nws",
            "effect ../../../etc/passwd",
            "catalog import /tmp/a.json",
            "refdb import eibi /tmp/a.txt",
            // reaches the network
            "refdb fetch eibi",
            "location ip",
            // writes a store the app keeps on disk
            "location 38.7 -9.1",
            "location IM58",
            "location clear",
            "refdb clear eibi",
            "catalog add 100M thing",
            "catalog delete 1",
            "catalog purge 1,2",
            "catalog undo",
            "catalog observe",
            "stations catalog eibi:1",
            // ends the process
            "quit",
        ] {
            assert_eq!(of(line), Privilege::Trusted, "{line} must need the token");
        }
    }

    /// The live-development loop stays free: tuning, triggering, display,
    /// history and the read-only view verbs need no token.
    #[test]
    fn live_state_verbs_stay_open() {
        for line in [
            "run 1",
            "rate 250000",
            "vdiv 0 0.05",
            "trigger 0 rising 0.1 auto",
            "timebase 0.001",
            "stimulus xy-circle",
            "mode xy",
            "persist inf",
            "fft on",
            "history prev",
            "record 1",
            "recordclear",
            "refsave 0",
            "refclear",
            "dock none",
            "window 1280x800",
            "windowpos 10 20",
            "menu display",
            "settings on",
            "effect off",
            "effectreload",
            "sdr tune 100.3M",
            "sdr dab on",
            "sdr iqdump off",
            "sdr demod nfm",
            "instrument sdr",
            "bandplan uk",
            "stations tune eibi:1",
            "stations find bbc",
            "catalog list x",
            "catalog window on",
            "catalog select 1",
        ] {
            assert_eq!(of(line), Privilege::Open, "{line} must stay open");
        }
    }

    /// A verb that writes must not be open just because its sibling is:
    /// `effect off` clears, `effect <name>` opens a file by name.
    #[test]
    fn the_path_bearing_half_of_a_verb_is_the_trusted_half() {
        assert_eq!(of("effect off"), Privilege::Open);
        assert_eq!(of("effect crt"), Privilege::Trusted);
        assert_eq!(of("sdr iqdump off"), Privilege::Open);
        assert_eq!(of("sdr iqdump /tmp/a.f32 2"), Privilege::Trusted);
    }
}
