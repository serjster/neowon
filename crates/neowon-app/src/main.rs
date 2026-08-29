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
//!   N           cycle sweep (auto -> normal -> single)
//!   M           cycle acquisition (sample -> peak -> avg4 -> avg16 -> avg64)
//!   C           cycle CH1 coupling (DC -> AC -> GND)
//!   [/]         CH1 vertical offset down/up
//!   F           force trigger
//!   A           auto-set
//!   E           cycle persistence (off -> 0.2s -> 1s -> 5s -> infinite)
//!   X           cycle trace mode (vectors -> dots -> XY)

mod gpu;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use gpu::{Persistence, Phosphor, PhosphorPlugin, TraceMode, PLOT_H, PLOT_W};

use neowon_backend::{Backend, Capabilities, Command, Event, ScopeConfig, Supervisor};
use neowon_core::{AcqMode, Coupling, SharedFrame, Slope, Sweep};
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
        .add_plugins(PhosphorPlugin)
        .insert_resource(Link {
            sup,
            caps: None,
            status: "connecting…".into(),
            latest: None,
            config,
            dirty: false,
            frames_seen: 0,
        })
        .init_resource::<Phosphor>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                clear_one_shot,
                ingest,
                input,
                phosphor_input,
                flush,
                update_phosphor,
                readback_hook,
                draw_graticule,
                draw_trigger,
                update_title,
            )
                .chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut phosphor: ResMut<Phosphor>,
) {
    commands.spawn(Camera2d);

    // Display texture: written by the compose pass, shown via this sprite.
    let mut image = Image::new(
        Extent3d { width: PLOT_W, height: PLOT_H, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0; (PLOT_W * PLOT_H * 4) as usize],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::STORAGE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.spawn(Sprite::from_image(handle.clone()));
    phosphor.display_image = handle;
}

/// One-shot flags live for exactly one frame (extracted at frame end,
/// cleared at the start of the next).
fn clear_one_shot(mut phosphor: ResMut<Phosphor>) {
    phosphor.new_frame = false;
}

/// Headless verification: NEOWON_SHOT=<frames> reads the display texture back
/// after that many records, writes /tmp/neowon-shot.ppm, and exits.
fn readback_hook(mut commands: Commands, link: Res<Link>, phosphor: Res<Phosphor>) {
    let Ok(shot) = std::env::var("NEOWON_SHOT").map(|v| v.parse::<u64>().unwrap_or(0)) else {
        return;
    };
    if link.frames_seen == shot && phosphor.new_frame {
        commands
            .spawn(bevy::render::gpu_readback::Readback::texture(
                phosphor.display_image.clone(),
            ))
            .observe(|event: On<bevy::render::gpu_readback::ReadbackComplete>| {
                let rgba = &event.data;
                // Readback rows are 256-byte aligned; strip the padding.
                let stride = rgba.len() / PLOT_H as usize;
                let mut ppm = format!("P6\n{PLOT_W} {PLOT_H}\n255\n").into_bytes();
                for row in rgba.chunks_exact(stride) {
                    for px in row[..(PLOT_W * 4) as usize].chunks_exact(4) {
                        ppm.extend_from_slice(&px[..3]);
                    }
                }
                match std::fs::write("/tmp/neowon-shot.ppm", &ppm) {
                    Ok(()) => println!("readback: wrote /tmp/neowon-shot.ppm"),
                    Err(e) => eprintln!("readback: could not write shot: {e}"),
                }
                std::process::exit(0);
            });
    }
}

fn update_phosphor(time: Res<Time>, link: Res<Link>, mut phosphor: ResMut<Phosphor>) {
    if let Some(frame) = &link.latest
        && phosphor.frame.as_ref().map(|f| f.seq) != Some(frame.seq)
    {
        phosphor.frame = Some(frame.clone());
        phosphor.new_frame = true;
    }
    phosphor.decay = match phosphor.persistence {
        Persistence::Off | Persistence::Infinite => 1.0,
        Persistence::Seconds(s) => (-time.delta_secs() / s.max(1e-3)).exp(),
    };
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
            Event::ConfigUpdated(cfg) => {
                link.config = cfg;
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
    if keys.just_pressed(KeyCode::KeyN) {
        link.config.trigger.sweep = match link.config.trigger.sweep {
            Sweep::Auto => Sweep::Normal,
            Sweep::Normal => Sweep::Single,
            Sweep::Single => Sweep::Auto,
        };
        // Arming single (re)starts acquisition.
        if link.config.trigger.sweep == Sweep::Single {
            link.config.running = true;
        }
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        link.config.acq = match link.config.acq {
            AcqMode::Sample => AcqMode::Peak,
            AcqMode::Peak => AcqMode::Average(4),
            AcqMode::Average(4) => AcqMode::Average(16),
            AcqMode::Average(16) => AcqMode::Average(64),
            AcqMode::Average(_) => AcqMode::Sample,
        };
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::KeyC) {
        link.config.channels[0].coupling = match link.config.channels[0].coupling {
            Coupling::Dc => Coupling::Ac,
            Coupling::Ac => Coupling::Gnd,
            Coupling::Gnd => Coupling::Dc,
        };
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        let o = (link.config.channels[0].offset + 0.05).clamp(-0.5, 0.5);
        link.config.channels[0].offset = o;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        let o = (link.config.channels[0].offset - 0.05).clamp(-0.5, 0.5);
        link.config.channels[0].offset = o;
        link.dirty = true;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        let _ = link.sup.commands.send(Command::ForceTrigger);
    }
    if keys.just_pressed(KeyCode::KeyA) {
        let _ = link.sup.commands.send(Command::AutoSet);
    }
}

fn phosphor_input(keys: Res<ButtonInput<KeyCode>>, mut phosphor: ResMut<Phosphor>) {
    if keys.just_pressed(KeyCode::KeyE) {
        let ladder = Persistence::LADDER;
        let i = ladder.iter().position(|p| *p == phosphor.persistence).unwrap_or(0);
        phosphor.persistence = ladder[(i + 1) % ladder.len()];
    }
    if keys.just_pressed(KeyCode::KeyX) {
        phosphor.mode = match phosphor.mode {
            TraceMode::Vectors => TraceMode::Dots,
            TraceMode::Dots => TraceMode::Xy,
            TraceMode::Xy => TraceMode::Vectors,
        };
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

fn format_rate(r: f64) -> String {
    if r >= 1e6 {
        format!("{} MS/s", r / 1e6)
    } else if r >= 1e3 {
        format!("{} kS/s", r / 1e3)
    } else {
        format!("{r} S/s")
    }
}

fn update_title(link: Res<Link>, phosphor: Res<Phosphor>, mut windows: Query<&mut Window>) {
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
    let sweep = match link.config.trigger.sweep {
        Sweep::Auto => "auto",
        Sweep::Normal => "norm",
        Sweep::Single => "single",
    };
    let acq = match link.config.acq {
        AcqMode::Sample => String::new(),
        AcqMode::Peak => "  peak".into(),
        AcqMode::Average(n) => format!("  avg{n}"),
    };
    let coupling = match ch.coupling {
        Coupling::Dc => "DC",
        Coupling::Ac => "AC",
        Coupling::Gnd => "GND",
    };
    let mode = match phosphor.mode {
        TraceMode::Vectors => "",
        TraceMode::Dots => "  dots",
        TraceMode::Xy => "  XY",
    };
    window.title = format!(
        "neowon — {}  [{run}]  {} V/div {coupling}  {}  trig {:+.2} V {sweep}{acq}  P:{}{mode}{meas}",
        link.status,
        ch.volts_div,
        format_rate(link.config.sample_rate),
        link.config.trigger.level,
        phosphor.persistence.label(),
    );
}
