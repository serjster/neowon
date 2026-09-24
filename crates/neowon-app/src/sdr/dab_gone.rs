//! What an unlocked DAB receiver last lost, and when — the record
//! `DabState::gone` holds so the dock and `get dab` can say "the lock went"
//! instead of a bare "not locked" that reads like a receiver that never
//! found anything.

use neowon_dsp::dab::Ensemble;

/// Why an unlocked receiver has no table, when it had one or had input.
#[derive(Debug, Clone, PartialEq)]
pub struct Gone {
    pub cause: GoneCause,
    /// When it happened, on the clock `last_frame_at` uses (app seconds).
    pub at: f64,
    /// The ensemble that went with it, when a table was held.
    pub ensemble: Option<LostEnsemble>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoneCause {
    /// Frames kept arriving but no clean FIC decoded for the receiver's
    /// expiry window: the signal at this centre stopped being DAB.
    Expired,
    /// No IQ frames at all (`dab::NO_INPUT_TIMEOUT_S`): a stalled link or
    /// a disconnect. Cleared by the next frame that arrives.
    NoInput,
}

impl GoneCause {
    /// The script-facing name (`get dab` → `gone.cause`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            GoneCause::Expired => "expired",
            GoneCause::NoInput => "no_input",
        }
    }
}

/// The identity of an ensemble that is no longer held: enough to say which
/// one went, not a table a path could keep reading as current.
#[derive(Debug, Clone, PartialEq)]
pub struct LostEnsemble {
    pub label: Option<String>,
    pub eid: Option<u16>,
    pub services: usize,
}

impl LostEnsemble {
    #[must_use]
    pub fn of(e: &Ensemble) -> Self {
        Self {
            label: e.label.clone(),
            eid: e.eid,
            services: e.services.len(),
        }
    }

    /// `NEOWON SIM (EId 1046, 5 services)`, for the dock.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} (EId {}, {} services)",
            self.label.as_deref().unwrap_or("<no label>"),
            self.eid.map_or("-".to_string(), |e| format!("{e:04X}")),
            self.services
        )
    }
}
