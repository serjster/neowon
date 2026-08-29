//! Phase 2: live oscilloscope view driven by the backend supervisor.
//! Defaults to the VDS1022 (with hot-replug recovery); `--sim` runs the
//! simulated source. Rendering is still a gizmo polyline — the GPU phosphor
//! pipeline replaces it in Phase 4.
//!
//! Keys:
//!   Space       run/stop
//!   Up/Down     CH1 volts/div
//!   Left/Right  sample rate
//!   ,/.         trigger level down/up
//!   S           toggle trigger slope

use bevy::color::palettes::css;
use bevy::prelude::*;

use neowon_backend::{Backend, Capabilities, Event, ScopeConfig, Supervisor};
use neowon_core::{SharedFrame, Slope};
use neowon_dsp::{basic_stats, estimate_frequency};

/// Screen geometry follows scope convention: 20 horizontal x 10 vertical
/// divisions.
const DIV_PX: f32 = 50.0;
const H_DIVS: i32 = 20;
const V_DIVS: i32 = 10;

#[derive(Resource)]
struct Link {
    sup: Supervisor,
    caps: Option<Capabilities>,
    status: String,
    latest: Option<SharedFrame>,
    config: ScopeConfig,
    dirty: bool,
    frames_seen: u64,
}

fn main() {
    // Logging is owned by Bevy's LogPlugin (honors RUST_LOG).
    let use_sim = std::env::args().any(|a| a == "--sim");
    let sup = if use_sim {
        neowon_backend::spawn(|| {
            Ok(Box::new(neowon_sim::SimBackend::new()) as Box<dyn Backend>)
        })
    } else {
        neowon_backend::spawn(neowon_vds1022::backend::factory(None))
    };

    // Defaults matched to the 1 kHz probe-comp signal through a x10 probe.
    let config = ScopeConfig {
        channels: {
            let mut chs = ScopeConfig::default().channels;
            chs[0].volts_div = 0.2;
            chs
        },
        trigger: neowon_backend::TriggerConfig {
            level: 0.25,
            ..Default::default()
        },
        ..Default::default()
    };
    sup.apply(config.clone());

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "neowon".into(),
                resolution: [1200, 700].into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(Link {
            sup,
            caps: None,
            status: "connecting…".into(),
            latest: None,
            config,
            dirty: false,
            frames_seen: 0,
        })
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (ingest, input, flush, draw_graticule, draw_trigger, draw_trace, update_title)
                .chain(),
        )
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn ingest(mut link: ResMut<Link>) {
    while let Ok(event) = link.sup.events.try_recv() {
        match event {
            Event::Connected(caps) => {
                link.status = format!("{} {}", caps.name, caps.serial);
                link.caps = Some(caps);
            }
            Event::Disconnected(e) => {
                link.status = format!("disconnected: {e}");
                link.caps = None;
            }
            Event::Frame(f) => {
                link.frames_seen += 1;
                if link.frames_seen == 1 || link.frames_seen % 500 == 0 {
                    tracing::info!(frames = link.frames_seen, "acquiring");
                }
                link.latest = Some(f);
            }
            Event::Error(e) => link.status = format!("error: {e}"),
        }
    }
}

fn step(ladder: &[f64], current: f64, up: bool) -> f64 {
    // Find nearest, then move one rung.
    let mut idx = 0;
    let mut best = f64::MAX;
    for (i, &v) in ladder.iter().enumerate() {
        let d = (v.ln() - current.max(1e-9).ln()).abs();
        if d < best {
            best = d;
            idx = i;
        }
    }
    let idx = if up { (idx + 1).min(ladder.len() - 1) } else { idx.saturating_sub(1) };
    ladder[idx]
}

fn input(keys: Res<ButtonInput<KeyCode>>, mut link: ResMut<Link>) {
    let volts_ladder = link
        .caps
        .as_ref()
        .map(|c| c.volts_div.clone())
        .unwrap_or_else(|| vec![0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0]);
    let rate_ladder = link
        .caps
        .as_ref()
        .map(|c| c.sample_rates.clone())
        .unwrap_or_else(|| vec![2.5e3, 25e3, 250e3, 2.5e6, 25e6, 100e6]);

    if keys.just_pressed(KeyCode::Space) {
        link.config.running = !link.config.running;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        let v = step(&volts_ladder, link.config.channels[0].volts_div, true);
        link.config.channels[0].volts_div = v;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        let v = step(&volts_ladder, link.config.channels[0].volts_div, false);
        link.config.channels[0].volts_div = v;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        link.config.sample_rate = step(&rate_ladder, link.config.sample_rate, true);
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        link.config.sample_rate = step(&rate_ladder, link.config.sample_rate, false);
        link.dirty = true;
    }
    let trig_step = link.config.channels[0].volts_div; // one div per press
    if keys.just_pressed(KeyCode::Period) {
        link.config.trigger.level += trig_step;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::Comma) {
        link.config.trigger.level -= trig_step;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::KeyS) {
        link.config.trigger.slope = match link.config.trigger.slope {
            Slope::Rising => Slope::Falling,
            Slope::Falling => Slope::Rising,
        };
        link.dirty = true;
    }
}

fn flush(mut link: ResMut<Link>) {
    if link.dirty {
        link.dirty = false;
        let cfg = link.config.clone();
        link.sup.apply(cfg);
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

fn draw_trigger(link: Res<Link>, mut gizmos: Gizmos) {
    let w = H_DIVS as f32 * DIV_PX;
    let h = V_DIVS as f32 * DIV_PX;
    let src = link.config.trigger.source.min(link.config.channels.len() - 1);
    let ch = &link.config.channels[src];
    let range = ch.volts_div * 10.0 * ch.probe;
    let frac = (link.config.trigger.level / range + ch.offset).clamp(-0.55, 0.55);
    let y = frac as f32 * h;
    gizmos.line_2d(
        Vec2::new(-w / 2.0, y),
        Vec2::new(w / 2.0, y),
        Color::srgba(1.0, 0.5, 0.2, 0.5),
    );
}

fn draw_trace(link: Res<Link>, mut gizmos: Gizmos) {
    let Some(frame) = &link.latest else { return };
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

fn format_rate(r: f64) -> String {
    if r >= 1e6 {
        format!("{} MS/s", r / 1e6)
    } else if r >= 1e3 {
        format!("{} kS/s", r / 1e3)
    } else {
        format!("{r} S/s")
    }
}

fn update_title(link: Res<Link>, mut windows: Query<&mut Window>) {
    let Ok(mut window) = windows.single_mut() else { return };
    let run = if link.config.running { "RUN" } else { "STOP" };
    let ch = &link.config.channels[0];
    let mut meas = String::new();
    if let Some(frame) = &link.latest
        && let Some(cap) = frame.channels.first()
    {
        let vpp = basic_stats(cap).map_or(0.0, |s| s.vpp);
        let freq = estimate_frequency(&cap.raw, frame.sample_rate).unwrap_or(0.0);
        meas = format!("  |  CH1 {vpp:.3} Vpp  {freq:.1} Hz  [{}]", link.frames_seen);
    }
    window.title = format!(
        "neowon — {}  [{run}]  {} V/div  {}  trig {:+.2} V{meas}",
        link.status,
        ch.volts_div,
        format_rate(link.config.sample_rate),
        link.config.trigger.level,
    );
}
