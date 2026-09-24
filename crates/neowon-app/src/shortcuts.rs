//! Keyboard shortcuts, each homed in the workspace whose controls it drives.
//!
//! **Invariant: a shortcut acts only in its own workspace.** The scope keys
//! edit `Link::config` or send scope commands down the link; while the SDR
//! workspace is up that link is the SDR's, so a stray `A` would send `AutoSet`
//! to a radio and `[`/`]` would edit a scope config nobody can see. Every binding
//! therefore names its [`Home`], and [`dispatch`] fires only the bindings
//! whose home is the active workspace (or [`Home::Global`]). A new key goes
//! in [`SHORTCUTS`] with a home; there is no other place to bind one.
//!
//! | key | home | does |
//! |---|---|---|
//! | Space | scope | run/stop |
//! | Up/Down | scope | CH1 volts/div |
//! | Left/Right | scope | time base (s/div) |
//! | `,` / `.` | scope | trigger level down/up |
//! | S | scope | toggle trigger slope |
//! | N | scope | cycle sweep (auto → normal → single) |
//! | M | scope | cycle acquisition (sample → peak → avg4/16/64) |
//! | C | scope | cycle CH1 coupling (DC → AC → GND) |
//! | `[` / `]` | scope | CH1 vertical offset down/up |
//! | F | scope | force trigger |
//! | A | scope | auto-set |
//! | H | scope | home: default zoom + centre position |
//! | E | scope | cycle persistence |
//! | X | scope | cycle trace mode (vectors → dots → XY) |
//!
//! Shortcuts egui owns are homed by where they are drawn, not by this table:
//! ⌘/Ctrl+1 and +2 (the SCOPE | SDR switch, `ui/menubar.rs`) are global; the
//! Shift/Ctrl wheel modifiers of `ui/sdr_view.rs` exist only in the SDR view,
//! and the Shift modifier of the FFT pane (`ui.rs`) and of the plot wheel
//! (`ui/touch.rs`) only on the scope's plot.

use crate::Link;
use crate::autopeak::AutoPeak;
use crate::gpu::{Persistence, Phosphor, TraceMode};
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use neowon_backend::Command;
use neowon_core::{AcqMode, Coupling, Slope, Sweep, TriggerKind};

/// The workspace a shortcut belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    Scope,
    Sdr,
    Global,
}

impl Home {
    /// The workspace on screen now.
    pub fn active(sdr_active: bool) -> Self {
        if sdr_active { Home::Sdr } else { Home::Scope }
    }

    /// Whether a binding homed here may act while `workspace` is up.
    pub fn acts_in(self, workspace: Home) -> bool {
        self == Home::Global || self == workspace
    }
}

/// What a shortcut may touch.
pub struct Target<'a> {
    pub link: &'a mut Link,
    pub phosphor: &'a mut Phosphor,
    pub autopeak: &'a mut AutoPeak,
}

pub struct Shortcut {
    pub key: KeyCode,
    pub home: Home,
    pub what: &'static str,
    pub run: fn(&mut Target),
}

const fn scope(key: KeyCode, what: &'static str, run: fn(&mut Target)) -> Shortcut {
    Shortcut {
        key,
        home: Home::Scope,
        what,
        run,
    }
}

pub const SHORTCUTS: &[Shortcut] = &[
    scope(KeyCode::Space, "run/stop", |t| {
        t.link.config.running = !t.link.config.running;
        t.link.dirty = true;
    }),
    scope(KeyCode::ArrowUp, "CH1 volts/div up", |t| volts(t, true)),
    scope(KeyCode::ArrowDown, "CH1 volts/div down", |t| {
        volts(t, false)
    }),
    // Horizontal scale, the bench-scope way: right = faster s/div (zoom
    // in), left = slower s/div, all the way down the rate ladder.
    scope(KeyCode::ArrowRight, "time base faster", |t| {
        crate::view::timebase_step(t.link, false)
    }),
    scope(KeyCode::ArrowLeft, "time base slower", |t| {
        crate::view::timebase_step(t.link, true)
    }),
    scope(KeyCode::Period, "trigger level up", |t| trigger(t, 1.0)),
    scope(KeyCode::Comma, "trigger level down", |t| trigger(t, -1.0)),
    scope(KeyCode::KeyS, "toggle trigger slope", |t| {
        if let TriggerKind::Edge { slope } = &mut t.link.config.trigger.kind {
            *slope = match *slope {
                Slope::Rising => Slope::Falling,
                Slope::Falling => Slope::Rising,
            };
            t.link.dirty = true;
        }
    }),
    scope(KeyCode::KeyN, "cycle sweep", |t| {
        let trig = &mut t.link.config.trigger;
        trig.sweep = match trig.sweep {
            Sweep::Auto => Sweep::Normal,
            Sweep::Normal => Sweep::Single,
            Sweep::Single => Sweep::Auto,
        };
        // Arming single (re)starts acquisition.
        t.link.config.running |= trig.sweep == Sweep::Single;
        t.link.dirty = true;
    }),
    scope(KeyCode::KeyM, "cycle acquisition", |t| {
        let next = match t.autopeak.user_acq {
            AcqMode::Sample => AcqMode::Peak,
            AcqMode::Peak => AcqMode::Average(4),
            AcqMode::Average(4) => AcqMode::Average(16),
            AcqMode::Average(16) => AcqMode::Average(64),
            AcqMode::Average(_) => AcqMode::Sample,
        };
        t.autopeak.set_user(next);
        t.link.config.acq = next;
        t.link.dirty = true;
    }),
    scope(KeyCode::KeyC, "cycle CH1 coupling", |t| {
        let ch = &mut t.link.config.channels[0];
        ch.coupling = match ch.coupling {
            Coupling::Dc => Coupling::Ac,
            Coupling::Ac => Coupling::Gnd,
            Coupling::Gnd => Coupling::Dc,
        };
        t.link.dirty = true;
    }),
    scope(KeyCode::BracketRight, "CH1 offset up", |t| offset(t, 0.05)),
    scope(KeyCode::BracketLeft, "CH1 offset down", |t| {
        offset(t, -0.05)
    }),
    scope(KeyCode::KeyF, "force trigger", |t| {
        let _ = t.link.sup.commands.send(Command::ForceTrigger);
    }),
    scope(KeyCode::KeyA, "auto-set", |t| {
        let _ = t.link.sup.commands.send(Command::AutoSet);
    }),
    scope(KeyCode::KeyH, "home", |t| {
        crate::view::home(t.link, t.phosphor)
    }),
    scope(KeyCode::KeyE, "cycle persistence", |t| {
        let ladder = Persistence::LADDER;
        let p = &mut t.phosphor.persistence;
        let i = ladder.iter().position(|l| l == p).unwrap_or(0);
        *p = ladder[(i + 1) % ladder.len()];
    }),
    scope(KeyCode::KeyX, "cycle trace mode", |t| {
        t.phosphor.mode = match t.phosphor.mode {
            TraceMode::Vectors => TraceMode::Dots,
            TraceMode::Dots => TraceMode::Xy,
            TraceMode::Xy => TraceMode::Vectors,
        };
    }),
];

fn volts(t: &mut Target, up: bool) {
    let ladder = t
        .link
        .scope_caps()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| crate::ui::widgets::FALLBACK_VDIV.to_vec());
    let ch = &mut t.link.config.channels[0];
    ch.volts_div = crate::view::step_ladder(&ladder, ch.volts_div, up);
    t.link.dirty = true;
}

/// One division of CH1 per press.
fn trigger(t: &mut Target, sign: f64) {
    t.link.config.trigger.level += sign * t.link.config.channels[0].volts_div;
    t.link.dirty = true;
}

fn offset(t: &mut Target, by: f64) {
    let ch = &mut t.link.config.channels[0];
    ch.offset = (ch.offset + by).clamp(-0.5, 0.5);
    t.link.dirty = true;
}

/// Fire every binding pressed this frame that may act in `workspace`.
pub fn fire(pressed: impl Fn(KeyCode) -> bool, workspace: Home, target: &mut Target) {
    for s in SHORTCUTS {
        if s.home.acts_in(workspace) && pressed(s.key) {
            tracing::debug!(key = ?s.key, what = s.what, "shortcut");
            (s.run)(target);
        }
    }
}

/// The Bevy side: keys go to egui first, then to the active workspace.
pub fn dispatch(
    keys: Res<ButtonInput<KeyCode>>,
    egui_wants: Res<EguiWantsInput>,
    sdr: Res<crate::sdr::SdrState>,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
    mut autopeak: ResMut<AutoPeak>,
) {
    if egui_wants.wants_any_keyboard_input() {
        return;
    }
    let mut target = Target {
        link: &mut link,
        phosphor: &mut phosphor,
        autopeak: &mut autopeak,
    };
    fire(
        |k| keys.just_pressed(k),
        Home::active(sdr.active),
        &mut target,
    );
}

/// Run condition for the scope's own pointer gestures (plot drags, wheel,
/// measurement cursors): the same rule as the keys.
pub fn in_scope(sdr: Res<crate::sdr::SdrState>) -> bool {
    Home::Scope.acts_in(Home::active(sdr.active))
}

#[cfg(test)]
mod tests;
