//! App entry point: builds the Bevy app from the launch flags and wires every
//! plugin, resource and system in schedule order. `--sim` runs the simulated
//! source; the instrument a flag selects is `launch.rs`.
//!
//! Where the systems live: the instrument link (`Link`, event ingest, config
//! flush) is `link.rs`; the plot texture and phosphor hand-off `plot.rs`; the
//! OS window fit, layout sync and title `window.rs`; the gizmo overlays
//! `overlays.rs`.
//!
//! Keys: `shortcuts.rs`, each homed in the workspace it drives. Pointer
//! gestures on the plot: `ui/touch.rs` and `cursors.rs`. `NEOWON_NO_INPUT`
//! (tests, tooling) ignores all host input: `hostinput.rs`.

mod autopeak;
mod autostate;
mod catalog;
mod control;
mod cursors;
mod decode;
mod deep;
mod derived;
mod effects;
mod gpu;
mod hostinput;
mod launch;
mod link;
mod overlays;
mod plot;
mod record;
mod refmap;
mod refs;
mod script;
mod sdr;
mod session;
mod shortcuts;
mod ui;
mod uitree;
mod view;
mod viz;
mod window;

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use gpu::{Persistence, Phosphor, PhosphorPlugin, TraceMode};
use ui::layout::Layout;

/// The app's central resource; defined in `link.rs`, re-exported here so
/// every module keeps naming it `crate::Link`.
pub use link::Link;
use overlays::{
    draw_clip_warnings, draw_graticule, draw_guides, draw_markers, draw_pf_mask, draw_trigger,
    draw_zoom_band,
};
use plot::{clear_one_shot, readback_hook, setup, update_phosphor};
use window::{fit_display, sync_layout, update_title};

fn main() {
    // Logging is owned by Bevy's LogPlugin (honors RUST_LOG). Flags and
    // the instrument they select: `launch.rs`.
    let launch = launch::Launch::from_args();
    let (sup, config) = launch.start();
    let demo = launch.demo;
    let no_input = hostinput::from_env();

    // NEOWON_WINDOW=WxH overrides the initial size (layout tests).
    let (win_w, win_h) = std::env::var("NEOWON_WINDOW")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1520u32, 820u32));

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "neowon".into(),
                resolution: [win_w, win_h].into(),
                // Fixed position so ROI screenshots map 1:1 to screen space.
                position: WindowPosition::At(IVec2::new(40, 40)),
                focused: !no_input, // a test window must not take focus
                resize_constraints: WindowResizeConstraints {
                    min_width: ui::layout::MIN_W,
                    min_height: ui::layout::MIN_H,
                    ..default()
                },
                ..default()
            }),
            ..default()
        }))
        // A scope keeps sweeping while you look away: render continuously
        // even unfocused (the default reactive-low-power mode stalls the
        // frame loop on throttled compositors — and every automated test).
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .add_plugins(EguiPlugin::default())
        .add_plugins(hostinput::HostInputPlugin { ignore: no_input })
        .add_plugins(PhosphorPlugin)
        .add_plugins(effects::EffectsPlugin)
        .insert_resource(Link::new(sup, config))
        .insert_resource({
            let mut p = Phosphor::default();
            if demo {
                p.mode = TraceMode::Xy;
                p.persistence = Persistence::Off;
                p.palette = gpu::Palette::Green;
                p.gain = 1.1;
            }
            p
        })
        .init_resource::<Layout>()
        .init_resource::<ui::layout::UiRects>()
        .init_resource::<ui::UiScale>()
        .init_resource::<autopeak::AutoPeak>()
        .init_resource::<deep::DeepView>()
        .init_resource::<decode::DecodeState>()
        .init_resource::<ui::settings::Settings>()
        .init_resource::<ui::touch::TouchState>()
        .init_resource::<ui::MenuState>()
        .init_resource::<derived::MathState>()
        .insert_resource(derived::MeasureState {
            guides: true,
            show_slot: [true; derived::SLOTS],
            ..Default::default()
        })
        .init_resource::<derived::FftState>()
        .init_resource::<derived::PfState>()
        .init_resource::<cursors::CursorState>()
        .init_resource::<record::Recorder>()
        .init_resource::<record::History>()
        .init_resource::<script::shot::WindowShots>()
        .init_resource::<refs::RefState>()
        .init_gizmo_group::<viz::three_d::VizGizmos>()
        .init_resource::<effects::Effects>()
        .init_resource::<viz::waterfall::WaterfallState>()
        .init_resource::<viz::three_d::Viz3dState>()
        .insert_resource(sdr::SdrState::new(launch))
        .insert_resource(catalog::CatalogState::open_from_env())
        .insert_resource(script::load_from_env())
        .insert_resource(autostate::AutoState::from_env())
        .insert_resource(uitree::UiTree::from_env())
        .insert_resource(refmap::RefMap::load())
        .insert_resource(control::start_from_env())
        .add_systems(
            PreStartup,
            |mut egui: ResMut<bevy_egui::EguiGlobalSettings>| {
                egui.auto_create_primary_context = false;
            },
        )
        .add_systems(
            Startup,
            (
                setup,
                viz::waterfall::setup,
                viz::three_d::setup,
                fit_display,
            ),
        )
        // Last: it must see the frame's AppExit to save before quitting.
        .add_systems(Last, autostate::tick)
        .add_systems(PreUpdate, uitree::enable)
        .add_systems(
            PostUpdate,
            uitree::capture.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
        )
        .add_systems(
            EguiPrimaryContextPass,
            (
                ui::panel,
                ui::sdr_view::show,
                ui::catalog_window::show,
                ui::stations_window::show,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                // Split only because Bevy caps how many systems one tuple
                // can chain; the order across both halves is what matters.
                (
                    (
                        sync_layout,
                        clear_one_shot,
                        link::ingest,
                        sdr::update,
                        record::record_frames,
                        shortcuts::dispatch,
                        cursors::cursor_input.run_if(shortcuts::in_scope),
                        ui::touch::plot_pointer.run_if(shortcuts::in_scope),
                        control::poll,
                        script::run_script,
                        refmap::tick,
                    )
                        .chain(),
                    (
                        // Before `flush`: the rule writes `config.acq`, and
                        // `flush` is what sends it to the instrument.
                        autopeak::update,
                        script::shot::tick,
                        sdr::flush,
                        link::flush,
                        derived::compute_derived,
                        decode::run,
                        deep::build,
                        viz::waterfall::update,
                        viz::three_d::update,
                        update_phosphor,
                    )
                        .chain(),
                )
                    .chain(),
                (
                    readback_hook,
                    draw_graticule,
                    draw_trigger,
                    draw_pf_mask,
                    draw_guides,
                    draw_markers,
                    draw_zoom_band,
                    decode::draw,
                    draw_clip_warnings,
                    cursors::draw_cursors,
                    update_title,
                )
                    .chain(),
            )
                .chain(),
        )
        .run();
}
