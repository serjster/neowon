//! Phase 0: Bevy window with a live simulated trace and a graticule.
//! Rendering is a gizmo polyline for now — the GPU phosphor pipeline
//! replaces it in Phase 4.

use bevy::color::palettes::css;
use bevy::prelude::*;
use neowon_core::CaptureFrame;
use neowon_dsp::{basic_stats, estimate_frequency};
use neowon_sim::SimSource;

/// Screen geometry follows scope convention: 20 horizontal x 10 vertical
/// divisions.
const DIV_PX: f32 = 50.0;
const H_DIVS: i32 = 20;
const V_DIVS: i32 = 10;

#[derive(Resource)]
struct Acquisition {
    source: SimSource,
    latest: Option<CaptureFrame>,
    timer: Timer,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "neowon".into(),
                resolution: [1200, 700].into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Acquisition {
            source: SimSource::default(),
            latest: None,
            timer: Timer::from_seconds(1.0 / 30.0, TimerMode::Repeating),
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (acquire, draw_graticule, draw_trace, update_title).chain())
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn acquire(time: Res<Time>, mut acq: ResMut<Acquisition>) {
    if acq.timer.tick(time.delta()).just_finished() {
        let frame = acq.source.next_frame();
        acq.latest = Some(frame);
    }
}

fn draw_graticule(mut gizmos: Gizmos) {
    let w = H_DIVS as f32 * DIV_PX;
    let h = V_DIVS as f32 * DIV_PX;
    let dim = Color::srgba(0.5, 0.55, 0.6, 0.25);
    let axis = Color::srgba(0.6, 0.65, 0.7, 0.6);
    for i in 0..=H_DIVS {
        let x = -w / 2.0 + i as f32 * DIV_PX;
        let c = if i == H_DIVS / 2 { axis } else { dim };
        gizmos.line_2d(Vec2::new(x, -h / 2.0), Vec2::new(x, h / 2.0), c);
    }
    for i in 0..=V_DIVS {
        let y = -h / 2.0 + i as f32 * DIV_PX;
        let c = if i == V_DIVS / 2 { axis } else { dim };
        gizmos.line_2d(Vec2::new(-w / 2.0, y), Vec2::new(w / 2.0, y), c);
    }
}

fn draw_trace(acq: Res<Acquisition>, mut gizmos: Gizmos) {
    let Some(frame) = &acq.latest else { return };
    let w = H_DIVS as f32 * DIV_PX;
    let h = V_DIVS as f32 * DIV_PX;
    let colors = [css::YELLOW, css::DEEP_SKY_BLUE];
    for cap in &frame.channels {
        let n = cap.raw.len();
        if n < 2 {
            continue;
        }
        let color = colors[cap.ch % colors.len()];
        let points = cap.raw.iter().enumerate().map(move |(i, &r)| {
            Vec2::new(
                -w / 2.0 + i as f32 / (n - 1) as f32 * w,
                r as f32 / 250.0 * h,
            )
        });
        gizmos.linestrip_2d(points, color);
    }
}

fn update_title(acq: Res<Acquisition>, mut windows: Query<&mut Window>) {
    let Some(frame) = &acq.latest else { return };
    let Some(cap) = frame.channels.first() else { return };
    let Ok(mut window) = windows.single_mut() else { return };
    let vpp = basic_stats(cap).map_or(0.0, |s| s.vpp);
    let freq = estimate_frequency(&cap.raw, frame.sample_rate).unwrap_or(0.0);
    window.title = format!(
        "neowon — sim CH1  {:.2} Vpp  {:.1} Hz  frame {}",
        vpp, freq, frame.seq
    );
}
