//! Synthetic host input, written as the messages winit's runner writes, into
//! a headless app running the real shortcut handler. Deterministic: no
//! window, no OS event queue, no operator.

use super::*;
use bevy::input::ButtonState;
use bevy::input::keyboard::Key;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::touch::TouchPhase;
use neowon_backend::Backend;

/// Messages still readable after `InputSystems` — where bevy_egui reads
/// them (`EguiPreUpdateSet::ProcessInput` is ordered after it).
#[derive(Resource, Default)]
struct AfterInput {
    keys: usize,
    wheel: usize,
    buttons: usize,
    moved: usize,
}

fn count_after_input(
    mut seen: ResMut<AfterInput>,
    mut keys: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut moved: MessageReader<CursorMoved>,
) {
    seen.keys += keys.read().count();
    seen.wheel += wheel.read().count();
    seen.buttons += buttons.read().count();
    seen.moved += moved.read().count();
}

fn app(ignore: bool) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::input::InputPlugin,
        HostInputPlugin { ignore },
    ));
    app.add_message::<CursorMoved>();
    app.init_resource::<AfterInput>();
    app.add_systems(PreUpdate, count_after_input.after(InputSystems));
    let sup = neowon_backend::spawn(|| -> Result<Box<dyn Backend>, String> {
        Err("no instrument in unit tests".into())
    });
    app.insert_resource(crate::Link {
        sup,
        caps: None,
        status: String::new(),
        latest: None,
        config: crate::view::startup_config(),
        dirty: false,
        frames_seen: 0,
        multi: neowon_backend::MultiMode::TriggerOut,
        last_frame_at: 0.0,
        arrived: Vec::new(),
        stimulus: String::new(),
        selected: 0,
        last_shot: None,
    })
    .insert_resource(crate::gpu::Phosphor::default())
    .init_resource::<crate::autopeak::AutoPeak>()
    .init_resource::<crate::sdr::SdrState>() // the scope workspace
    .init_resource::<bevy_egui::input::EguiWantsInput>()
    .add_systems(Update, crate::shortcuts::dispatch);
    app.update();
    app
}

/// One frame of host input: Space, a wheel notch, a left press, a move.
fn host_input(app: &mut App) {
    let window = Entity::PLACEHOLDER;
    let w = app.world_mut();
    w.write_message(KeyboardInput {
        key_code: KeyCode::Space,
        logical_key: Key::Space,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
    w.write_message(MouseWheel {
        unit: MouseScrollUnit::Line,
        x: 0.0,
        y: 3.0,
        window,
        phase: TouchPhase::Moved,
    });
    w.write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Pressed,
        window,
    });
    w.write_message(CursorMoved {
        window,
        position: Vec2::new(100.0, 100.0),
        delta: None,
    });
    app.update();
}

struct Outcome {
    running_toggled: bool,
    left_pressed: bool,
    scroll: Vec2,
    after_input: (usize, usize, usize, usize),
}

fn run(ignore: bool) -> Outcome {
    let mut app = app(ignore);
    let was = app.world().resource::<crate::Link>().config.running;
    host_input(&mut app);
    let w = app.world();
    let s = w.resource::<AfterInput>();
    Outcome {
        running_toggled: w.resource::<crate::Link>().config.running != was,
        left_pressed: w
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        scroll: w.resource::<AccumulatedMouseScroll>().delta,
        after_input: (s.keys, s.wheel, s.buttons, s.moved),
    }
}

#[test]
fn host_input_reaches_the_app_without_the_variable() {
    let o = run(false);
    assert!(o.running_toggled, "Space did not reach the shortcut table");
    assert!(o.left_pressed);
    assert_eq!(o.scroll, Vec2::new(0.0, 3.0));
    assert_eq!(o.after_input, (1, 1, 1, 1));
}

#[test]
fn no_input_drops_every_host_event_before_anything_reads_it() {
    let o = run(true);
    assert!(!o.running_toggled, "Space reached the shortcut table");
    assert!(!o.left_pressed, "a host click reached ButtonInput");
    assert_eq!(o.scroll, Vec2::ZERO, "a host wheel reached the scroll");
    assert_eq!(
        o.after_input,
        (0, 0, 0, 0),
        "egui would have seen host input"
    );
}
