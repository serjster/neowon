//! `NEOWON_NO_INPUT`: a test or tooling launch ignores the host's
//! keyboard, mouse, pointer motion, wheel, touch, gestures, IME and file
//! drops, opens its window without taking focus, and on macOS hands
//! activation back to the application that was in front (`macos.rs`).
//!
//! A test's window sits on the operator's desktop. A wheel turned over it
//! zoomed the SDR rate ladder mid-test, and a keystroke meant for another
//! window ran the scope's auto-set in an SDR test. Scripts, `NEOWON_SCRIPT`
//! and the control socket are how a test acts, and none of them travel as
//! host input, so they keep driving everything.
//!
//! **The choke point.** Winit's runner writes every host event into the
//! world as a Bevy message just before the frame's `update`. This drops
//! them at the top of `PreUpdate`, before `InputSystems` folds them into
//! `ButtonInput`/`AccumulatedMouseScroll` and before bevy_egui (which runs
//! after `InputSystems`) turns them into egui events. Every consumer — the
//! shortcut table, the plot gestures, the measurement cursors, every egui
//! widget and the SDR view's wheel and drags — reads one of those, so none
//! is guarded on its own. `Window::cursor_position` is left alone: winit
//! writes it directly, writing it back would warp the operator's pointer,
//! and every handler that reads it acts only on a button or the wheel.
//! AccessKit action requests (a screen reader) are not host input in this
//! sense and still pass.

use bevy::ecs::message::{Message, Messages};
use bevy::input::InputSystems;
use bevy::input::gestures::{DoubleTapGesture, PanGesture, PinchGesture, RotationGesture};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::input::touch::TouchInput;
use bevy::prelude::*;
use bevy::window::{CursorMoved, FileDragAndDrop, Ime};

/// `NEOWON_NO_INPUT` is set (to anything).
pub fn from_env() -> bool {
    std::env::var_os("NEOWON_NO_INPUT").is_some()
}

/// Drops host input every frame when `ignore` is set; a no-op otherwise.
pub struct HostInputPlugin {
    pub ignore: bool,
}

impl Plugin for HostInputPlugin {
    fn build(&self, app: &mut App) {
        if self.ignore {
            app.add_systems(PreUpdate, drop_host_input.before(InputSystems));
            #[cfg(target_os = "macos")]
            macos::build(app);
        }
    }
}

fn drop_host_input(world: &mut World) {
    fn clear<M: Message>(world: &mut World) {
        if let Some(mut m) = world.get_resource_mut::<Messages<M>>() {
            m.clear();
        }
    }
    clear::<KeyboardInput>(world);
    clear::<Ime>(world);
    clear::<MouseButtonInput>(world);
    clear::<MouseWheel>(world);
    clear::<MouseMotion>(world);
    clear::<CursorMoved>(world);
    clear::<TouchInput>(world);
    clear::<PinchGesture>(world);
    clear::<RotationGesture>(world);
    clear::<DoubleTapGesture>(world);
    clear::<PanGesture>(world);
    clear::<FileDragAndDrop>(world);
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;
