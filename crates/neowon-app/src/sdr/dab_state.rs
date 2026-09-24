//! The DAB-derived state, and the one way it is cleared.
//!
//! Everything here exists *because a DAB signal was decoded*. When the
//! signal it was derived from is gone — the hardware window moved, the
//! stream stopped, the instrument switched, the receiver was cycled — all
//! of it has to go together, or the dock, `get dab` and the playback
//! transport keep naming a service that is not there.
//!
//! [`DabState::reset`] / [`DabState::no_input`]
//! rebuild the whole struct from [`Default`], keeping only the receiver, so
//! a field added below is cleared by construction and cannot be missed.
//!
//! The device-facing half of a clear — flushing the sink so a stopped
//! service does not play on — belongs to whoever owns the audio device, so
//! the methods here report whether a transport was stopped rather than
//! reaching for it. `SdrState::dab_reset` and its siblings are the app's
//! single entry points and join the two.

use std::collections::BTreeMap;

use neowon_dsp::dab::DabReceiver;
use neowon_dsp::dab::pad::PadParser;

pub use super::dab_gone::{Gone, GoneCause, LostEnsemble};

use super::dab_audio::DabAudio;

/// The DAB consumer's state: the receiver, its timing grid, the selection
/// and everything hanging off it.
#[derive(Default)]
pub struct DabState {
    /// The DAB receiver, built while `sdr dab on` is in force.
    /// It is fed the raw IQ frames, not the demodulated channel: DAB wants
    /// the whole 1.536 MHz ensemble, so it is a wideband consumer sitting
    /// beside the demodulator, not a mode of it.
    ///
    /// It survives a clear — reset in place — because it is the thing that
    /// re-acquires; everything below it is derived and goes.
    pub rx: Option<DabReceiver>,
    /// First sample index the next DAB frame should carry: frames whose
    /// timestamps do not continue from it are spliced, not contiguous.
    pub next_sample: Option<i64>,
    /// Wall time of the last complex IQ frame fed to the receiver, or
    /// `None` when none has arrived since it started. A receiver with no
    /// frames is looking at nothing, and past `dab::NO_INPUT_TIMEOUT_S` the
    /// table expires.
    pub last_frame_at: Option<f64>,
    /// PAD/DLS parser per MSC sub-channel, keyed by `SubChId`.
    /// Fed each sub-channel's decoded logical-frame bytes; a retune, reset
    /// or splice clears it rather than mixing partial segments across the
    /// seam.
    pub pad: BTreeMap<u8, PadParser>,
    /// Selected DAB service (`SId`), or `None` (`sdr dab service`).
    pub service: Option<u16>,
    /// DAB audio playback: the decode worker while the selected
    /// service is playing, `None` when the transport is stopped. Dropping
    /// the worker is the stop — its decoders go with it, and its status is
    /// what `get dab` reports.
    pub audio: Option<DabAudio>,
    /// The last `sdr dab play` refusal, shown by the DAB panel. Without it
    /// a refused Play looks like a dead button: no worker exists to report
    /// a reason, so the message otherwise dies in the log (and the operator
    /// running the GUI never sees it). Cleared by a successful Play, Stop,
    /// or any change to the receiver/selection.
    pub play_error: Option<String>,
    /// What the receiver last lost, and when: an unlocked receiver
    /// that once held a table must not read as one that never locked.
    ///
    /// Not signal-derived state in the sense of [`Self::holds_derived`]:
    /// it is the record *that* the signal went, so `no_input` writes it
    /// rather than clearing it. A reset (retune, rate change, instrument
    /// switch, `dab reset`) forgets it with everything else — the hardware
    /// moved, so "the ensemble went away" would describe another window.
    pub gone: Option<Gone>,
}

impl DabState {
    /// True while a receiver is running (`sdr dab on`).
    #[must_use]
    pub fn on(&self) -> bool {
        self.rx.is_some()
    }

    /// True while any signal-derived state is still held. `reset` and
    /// `no_input` must both leave this false: it is the invariant a missed
    /// clear breaks, and the tests assert it at every site that clears.
    #[must_use]
    pub fn holds_derived(&self) -> bool {
        self.next_sample.is_some()
            || self.last_frame_at.is_some()
            || !self.pad.is_empty()
            || self.service.is_some()
            || self.audio.is_some()
            || self.play_error.is_some()
    }

    /// Forget everything derived from a signal that is gone: the lock, the
    /// ensemble table, the timing grid, the selection, the DLS parsers and
    /// playback.
    ///
    /// For a **retune, a rate change or an instrument switch** — the
    /// hardware moved, so the old table describes a different signal and
    /// keeping it would be a stale claim. The deliberate splice path
    /// is `dab::feed`'s `discard_buffer`, which keeps the table across a
    /// short gap; this is not that.
    ///
    /// Returns whether a playback transport was stopped, so the device's
    /// owner can flush the sink.
    pub fn reset(&mut self) -> bool {
        let mut rx = self.rx.take();
        if let Some(rx) = rx.as_mut() {
            rx.reset();
        }
        let played = self.audio.is_some();
        *self = Self {
            rx,
            ..Self::default()
        };
        played
    }

    /// The owner reports that the frame stream stopped (a stopped backend
    /// or a disconnect). Unlike a retune there is no new window to start
    /// from — the receiver keeps its cumulative counters — but the lock,
    /// the table and the timing grid describe a stream that is no longer
    /// there, so they go now instead of waiting for
    /// `dab::NO_INPUT_TIMEOUT_S`.
    ///
    /// Returns whether a playback transport was stopped.
    pub fn no_input(&mut self) -> bool {
        let mut rx = self.rx.take();
        // Which ensemble went with the input — the one held now, or the one
        // an earlier expiry already recorded.
        let ensemble = rx
            .as_ref()
            .filter(|r| r.is_locked())
            .map(|r| LostEnsemble::of(r.ensemble()))
            .or_else(|| self.gone.take().and_then(|g| g.ensemble));
        if let Some(rx) = rx.as_mut() {
            rx.no_input();
        }
        let gone = self.last_frame_at.map(|at| Gone {
            cause: GoneCause::NoInput,
            at,
            ensemble,
        });
        let played = self.audio.is_some();
        *self = Self {
            rx,
            gone,
            ..Self::default()
        };
        played
    }

    /// Drop the selection, its parsers and playback, leaving the receiver
    /// and the timing grid alone: used when the receiver's table expired
    /// under a selection, so no path keeps naming a service the table no
    /// longer holds. The stream itself is still arriving, which is
    /// why the grid survives.
    ///
    /// Returns whether a playback transport was stopped.
    pub fn forget_selection(&mut self) -> bool {
        let played = self.audio.is_some();
        *self = Self {
            rx: self.rx.take(),
            next_sample: self.next_sample,
            last_frame_at: self.last_frame_at,
            gone: self.gone.take(),
            ..Self::default()
        };
        played
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdr::dab_audio::{Coding, StreamSpec};

    /// Every derived field set, and a receiver running.
    fn populated() -> DabState {
        let mut dab = DabState {
            rx: Some(DabReceiver::new()),
            next_sample: Some(4096),
            last_frame_at: Some(12.5),
            service: Some(0x1004),
            play_error: Some("refused".into()),
            audio: Some(DabAudio::start(
                0x1004,
                StreamSpec {
                    coding: Coding::Mp2,
                    sub_channel: 3,
                    subchannel_index: 0,
                    asc_override: None,
                },
                48_000.0,
            )),
            ..Default::default()
        };
        dab.pad.entry(3).or_default();
        assert!(dab.holds_derived(), "the fixture must start dirty");
        dab
    }

    #[test]
    fn reset_clears_every_derived_field_and_keeps_the_receiver() {
        let mut dab = populated();
        assert!(dab.reset(), "a running transport was stopped");
        assert!(!dab.holds_derived(), "reset left derived state behind");
        assert!(dab.on(), "the receiver stays on, just reset");
        assert!(!dab.rx.as_ref().expect("receiver").is_locked());
        // Nothing to stop the second time.
        assert!(!dab.reset());
    }

    #[test]
    fn no_input_clears_every_derived_field_and_keeps_the_receiver() {
        let mut dab = populated();
        assert!(dab.no_input(), "a running transport was stopped");
        assert!(!dab.holds_derived(), "no_input left derived state behind");
        assert!(dab.on());
        // ...and says what happened, at the time the stream stopped:
        // an unlocked receiver with no input is not one still searching.
        let gone = dab.gone.as_ref().expect("no_input records the loss");
        assert_eq!(gone.cause, GoneCause::NoInput);
        assert_eq!(gone.at, 12.5);
    }

    /// An expiry's record survives the selection being dropped and a later
    /// `no_input` (which keeps the ensemble it names); a reset — the
    /// hardware moved — forgets it with everything else.
    #[test]
    fn the_loss_record_survives_the_clears_that_describe_it() {
        let lost = LostEnsemble {
            label: Some("NEOWON SIM".into()),
            eid: Some(0x1046),
            services: 5,
        };
        let mut dab = populated();
        dab.gone = Some(Gone {
            cause: GoneCause::Expired,
            at: 3.0,
            ensemble: Some(lost.clone()),
        });
        dab.forget_selection();
        assert_eq!(dab.gone.as_ref().unwrap().cause, GoneCause::Expired);
        dab.last_frame_at = Some(9.0);
        dab.no_input();
        let gone = dab.gone.clone().unwrap();
        assert_eq!(gone.cause, GoneCause::NoInput);
        assert_eq!(
            gone.ensemble,
            Some(lost),
            "the lost ensemble is still named"
        );
        assert_eq!(
            gone.ensemble.unwrap().describe(),
            "NEOWON SIM (EId 1046, 5 services)"
        );
        dab.reset();
        assert_eq!(dab.gone, None, "a reset describes a new window");
    }

    #[test]
    fn forget_selection_keeps_the_timing_grid() {
        let mut dab = populated();
        assert!(dab.forget_selection());
        assert!(dab.on());
        // The stream is still arriving, so its grid survives; everything
        // the selection hung off does not.
        assert_eq!(dab.next_sample, Some(4096));
        assert_eq!(dab.last_frame_at, Some(12.5));
        assert_eq!(dab.service, None);
        assert!(dab.pad.is_empty());
        assert!(dab.audio.is_none());
        assert_eq!(dab.play_error, None);
    }

    /// A receiver that was never started stays absent through a clear.
    #[test]
    fn clearing_an_off_receiver_leaves_it_off() {
        let mut dab = DabState::default();
        assert!(!dab.reset());
        assert!(!dab.on());
        assert!(!dab.holds_derived());
    }

    /// The sweep, at the level the reset sites actually live: every script
    /// action, applied to a state with every derived field set.
    mod sweep {
        use crate::sdr::SdrState;
        use crate::sdr::actions::{
            DabChannel, DabService, DabVerb, IqDumpVerb, SdrAction, SurveyRequest, apply, variant,
        };

        /// 11C, so the Band III raster verbs have somewhere to step from.
        const CENTRE: f64 = 220_352_000.0;

        /// A state with a receiver running and every derived field set, so
        /// a clear is visible whichever field a site forgets.
        fn dirty() -> SdrState {
            let mut sdr = SdrState {
                launch: crate::launch::Launch::sim(),
                ..Default::default()
            };
            sdr.config.centre_hz = CENTRE;
            sdr.config.sample_rate = 2.048e6;
            sdr.tuned_hz = CENTRE;
            sdr.dab.rx = Some(neowon_dsp::dab::DabReceiver::new());
            sdr.dab.next_sample = Some(4096);
            sdr.dab.last_frame_at = Some(1.5);
            sdr.dab.service = Some(0x1004);
            sdr.dab.play_error = Some("refused".into());
            sdr.dab.pad.entry(3).or_default();
            assert!(sdr.dab.holds_derived(), "the fixture must start dirty");
            sdr
        }

        /// A supervisor on the **simulated** scope — a unit test never
        /// opens a real device (`Launch::sim` above keeps the instrument
        /// switch simulated too).
        fn link() -> crate::Link {
            crate::Link {
                sup: neowon_backend::spawn(|| {
                    Ok(Box::new(neowon_sim::SimBackend::new()) as Box<dyn neowon_backend::Backend>)
                }),
                caps: None,
                status: String::new(),
                latest: None,
                config: neowon_backend::ScopeConfig::default(),
                dirty: false,
                frames_seen: 0,
                multi: neowon_backend::MultiMode::TriggerOut,
                last_frame_at: 0.0,
                arrived: Vec::new(),
                stimulus: String::new(),
                selected: 0,
                last_shot: None,
            }
        }

        /// **The one-owner invariant.** An action that moves the hardware
        /// window, stops the stream, switches instrument or cycles the
        /// receiver must leave *no* DAB-derived state behind: a survivor
        /// keeps naming a service that is not there.
        ///
        /// Every action variant is covered (`variant`, exhaustive), so a
        /// new action cannot be added without deciding which side of the
        /// invariant it is on — which is what a missed reset site looks
        /// like before it ships.
        #[test]
        fn every_action_that_moves_the_window_clears_the_dab_state() {
            // (action, does it clear?) — the answer is a property of the
            // value, not just the variant: a small `Step` stays inside the
            // IQ band and moves nothing, a far `Tune` recentres the
            // hardware.
            let cases: Vec<(SdrAction, bool)> = vec![
                (SdrAction::Tune(100e6), true),
                (SdrAction::Step(1e3), false),
                (SdrAction::Rate(1.024e6), true),
                (SdrAction::Gain(None), false),
                (SdrAction::Agc(true), false),
                (SdrAction::Ppm(2.0), false),
                (SdrAction::Span(200e3), false),
                (SdrAction::Fft(8192), false),
                (
                    SdrAction::Level {
                        ref_db: -10.0,
                        range_db: 80.0,
                    },
                    false,
                ),
                (SdrAction::Run(false), true),
                (SdrAction::Run(true), false),
                (SdrAction::Seed(7), false),
                (SdrAction::Detect(false), false),
                (SdrAction::Survey(None), false),
                (
                    SdrAction::Survey(Some(SurveyRequest {
                        start_hz: 88e6,
                        stop_hz: 108e6,
                        peak_cap: 4,
                        skip: Vec::new(),
                    })),
                    true,
                ),
                (SdrAction::Analyse(true), false),
                (SdrAction::Modulation(None), false),
                (SdrAction::Threshold(9.5), false),
                // `instrument sdr` from the scope: the other instrument
                // owns the signal now.
                (SdrAction::Instrument(true), true),
                (SdrAction::Pan(-1e5), false),
                (SdrAction::Centre(222_064_000.0), true),
                // Follow with the cursor already on the centre moves
                // nothing, so there is nothing to forget.
                (SdrAction::Follow(true), false),
                (SdrAction::Width(None), false),
                (SdrAction::Demod(Some(neowon_dsp::DemodMode::Nfm)), false),
                (SdrAction::Volume(0.5), false),
                (SdrAction::Mute(true), false),
                (SdrAction::Squelch(None), false),
                (SdrAction::List(200.0), false),
                (SdrAction::Dab(DabVerb::On), true),
                (SdrAction::Dab(DabVerb::Off), true),
                (SdrAction::Dab(DabVerb::Reset), true),
                // A selection or a play against a table that never locked
                // is refused, and a refusal changes nothing.
                (SdrAction::Dab(DabVerb::Service(DabService::Sid(1))), false),
                (SdrAction::Dab(DabVerb::Play), false),
                // Stop ends the transport only: the table and the
                // selection are still true, so the dock keeps offering the
                // service the operator just stopped.
                (SdrAction::Dab(DabVerb::Stop), false),
                (SdrAction::Dab(DabVerb::Channel(DabChannel::Next)), true),
                (SdrAction::IqDump(IqDumpVerb::Stop), false),
            ];
            let mut seen = std::collections::BTreeSet::new();
            for (a, clears) in cases {
                seen.insert(variant(&a));
                let mut sdr = dirty();
                let mut link = link();
                let _ = apply(a.clone(), &mut sdr, &mut link);
                assert_eq!(
                    !sdr.dab.holds_derived(),
                    clears,
                    "`{a}` clears the DAB state: expected {clears}"
                );
                // Only `dab on|off` switches the receiver itself; a clear
                // resets it in place rather than dropping it.
                let off = matches!(a, SdrAction::Dab(DabVerb::Off));
                assert_eq!(
                    sdr.dab.on(),
                    !off,
                    "`{a}` changed whether the receiver runs"
                );
                link.sup.shutdown();
            }
            assert_eq!(seen.len(), 28, "an action variant is unclassified");
        }
    }
}
