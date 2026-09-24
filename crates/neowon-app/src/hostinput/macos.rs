//! macOS: a `NEOWON_NO_INPUT` app gives activation back.
//!
//! `focused: false` keeps winit from making the window key, but winit's
//! `applicationDidFinishLaunching` still calls `activateIgnoringOtherApps`
//! (winit 0.30.13 `app_state.rs:137`), and bevy_winit builds the event loop
//! with no hook to turn that off. So the app remembers which application
//! was frontmost when the plugin is built — before `App::run` starts the
//! event loop, hence before winit activates — and whenever it finds itself
//! active during the first [`WATCH`] of `Update`s, it activates that
//! application again.
//!
//! **Timing.** winit activates, then dispatches its init events from the
//! same callback, and bevy_winit runs its first frame from those; so the
//! first `Update` already comes after the activation request (measured: it
//! sees the app active). The window server applies activation
//! asynchronously, so the system checks every frame for [`WATCH`], not
//! once, and hands activation back each time it lands. An app may activate
//! another only while it is itself active (macOS 14's cooperative
//! activation), which is exactly when this acts.
//!
//! **Off the frame.** `activateWithOptions` is a synchronous round trip
//! that waits on this process's main run loop. Called from a system, while
//! the main thread is inside `app.update()`, it stalled for 1.009 s — the
//! frame and the hand-back both — so it runs on a short-lived thread
//! (measured there: 24 ms). At most one is in flight.
//!
//! Safe Rust: every call used here is safe in `objc2-app-kit` 0.3, and
//! `NSRunningApplication` is `Send + Sync`.

use bevy::prelude::*;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long after the first `Update` the app keeps giving activation back.
const WATCH: Duration = Duration::from_secs(10);

/// The application that was frontmost before this one launched. `A` is
/// objc2's retained pointer, inferred rather than named so `objc2` itself
/// need not be a dependency.
#[derive(Resource)]
struct PreviousFront<A>(Option<A>);

pub(super) fn build(app: &mut App) {
    let me = NSRunningApplication::currentApplication();
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .filter(|front| *front != me);
    watch(app, front);
}

fn watch<A>(app: &mut App, front: Option<A>)
where
    A: Deref<Target = NSRunningApplication> + Clone + Send + Sync + 'static,
{
    app.insert_resource(PreviousFront(front))
        .add_systems(Update, give_activation_back::<A>);
}

fn give_activation_back<A>(
    previous: Res<PreviousFront<A>>,
    mut started: Local<Option<Instant>>,
    in_flight: Local<Arc<AtomicBool>>,
) where
    A: Deref<Target = NSRunningApplication> + Clone + Send + Sync + 'static,
{
    let started = *started.get_or_insert_with(Instant::now);
    if started.elapsed() > WATCH
        || in_flight.load(Ordering::Acquire)
        || !NSRunningApplication::currentApplication().isActive()
    {
        return;
    }
    let Some(front) = previous.0.clone().filter(|a| !a.isTerminated()) else {
        return;
    };
    in_flight.store(true, Ordering::Release);
    let done = Arc::clone(&in_flight);
    let spawned = std::thread::Builder::new()
        .name("no-input-yield".into())
        .spawn(move || {
            let ok = front.activateWithOptions(NSApplicationActivationOptions::empty());
            info!(
                "NEOWON_NO_INPUT: activation back to {:?}: {}",
                front.localizedName(),
                if ok { "done" } else { "refused" }
            );
            done.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        in_flight.store(false, Ordering::Release);
    }
}
