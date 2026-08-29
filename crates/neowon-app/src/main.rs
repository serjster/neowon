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

mod cursors;
mod derived;
mod gpu;
mod script;
mod ui;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use gpu::{PLOT_H, PLOT_W, Persistence, Phosphor, PhosphorPlugin, TraceMode};
use neowon_backend::{Backend, Capabilities, Command, Event, MultiMode, ScopeConfig, Supervisor};
use neowon_core::{AcqMode, Coupling, SharedFrame, Slope, Sweep, TriggerKind};
/// Screen geometry follows the reference scope: 10 horizontal x 8
/// vertical divisions (docs/ui-ux-research.md §6), computed at runtime
/// from the window size (`ui::layout::Layout`).
use ui::layout::{H_DIVS, Layout, V_DIVS};

#[derive(Resource)]
pub struct Link {
    pub sup: Supervisor,
    pub caps: Option<Capabilities>,
    pub status: String,
    pub latest: Option<SharedFrame>,
    pub config: ScopeConfig,
    pub dirty: bool,
    pub frames_seen: u64,
    /// Last selected MULTI port function.
    pub multi: MultiMode,
    /// Elapsed time when the last frame arrived — the WAIT indicator in the
    /// menu bar compares against it (starved Normal/Single trigger).
    pub last_frame_at: f64,
    /// Name of the active stimulus (generating backends only).
    pub stimulus: String,
    /// The channel pointer gestures and scroll steps act on.
    pub selected: usize,
}

fn main() {
    // Logging is owned by Bevy's LogPlugin (honors RUST_LOG).
    let use_sim = std::env::args().any(|a| a == "--sim");
    let sup = if use_sim {
        neowon_backend::spawn(|| Ok(Box::new(neowon_sim::SimBackend::new()) as Box<dyn Backend>))
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
                resize_constraints: WindowResizeConstraints {
                    min_width: ui::layout::MIN_W,
                    min_height: ui::layout::MIN_H,
                    ..default()
                },
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(PhosphorPlugin)
        .insert_resource(Link {
            sup,
            caps: None,
            status: "connecting…".into(),
            latest: None,
            config,
            dirty: false,
            frames_seen: 0,
            multi: MultiMode::TriggerOut,
            last_frame_at: 0.0,
            stimulus: "probe-comp".into(),
            selected: 0,
        })
        .init_resource::<Phosphor>()
        .init_resource::<Layout>()
        .init_resource::<ui::touch::TouchState>()
        .init_resource::<ui::MenuState>()
        .init_resource::<derived::MathState>()
        .insert_resource(derived::MeasureState {
            guides: true,
            ..Default::default()
        })
        .init_resource::<derived::FftState>()
        .init_resource::<derived::PfState>()
        .init_resource::<cursors::CursorState>()
        .insert_resource(script::load_from_env())
        .add_systems(Startup, setup)
        .add_systems(EguiPrimaryContextPass, ui::panel)
        .add_systems(
            Update,
            (
                sync_layout,
                clear_one_shot,
                ingest,
                input,
                phosphor_input,
                cursors::cursor_input,
                ui::touch::plot_pointer,
                script::run_script,
                flush,
                derived::compute_derived,
                update_phosphor,
                readback_hook,
                draw_graticule,
                draw_trigger,
                draw_pf_mask,
                draw_guides,
                draw_markers,
                draw_clip_warnings,
                cursors::draw_cursors,
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
        Extent3d {
            width: PLOT_W,
            height: PLOT_H,
            depth_or_array_layers: 1,
        },
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
    commands.spawn((Sprite::from_image(handle.clone()), PlotSprite));
    phosphor.display_image = handle;
}

/// Marker for the plot sprite; `sync_layout` sizes and places it.
#[derive(Component)]
struct PlotSprite;

/// Recompute the layout when the window size changes and keep the plot
/// sprite stretched over the plot region.
fn sync_layout(
    windows: Query<&Window>,
    mut layout: ResMut<Layout>,
    mut sprite: Query<(&mut Sprite, &mut Transform), With<PlotSprite>>,
) {
    let Ok(window) = windows.single() else { return };
    let next = Layout::compute(window.width(), window.height());
    if *layout != next {
        *layout = next;
    }
    if let Ok((mut sprite, mut tf)) = sprite.single_mut() {
        let size = Some(bevy::math::Vec2::new(
            layout.plot.width(),
            layout.plot.height(),
        ));
        if sprite.custom_size != size {
            sprite.custom_size = size;
        }
        let pos = layout.plot_center.extend(0.0);
        if tf.translation != pos {
            tf.translation = pos;
        }
    }
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

fn update_phosphor(
    time: Res<Time>,
    link: Res<Link>,
    math: Res<derived::MathState>,
    mut phosphor: ResMut<Phosphor>,
) {
    if let Some(frame) = &link.latest
        && phosphor.frame.as_ref().map(|f| f.seq) != Some(frame.seq)
    {
        // Append the math trace (slot 2) when present.
        phosphor.frame = Some(match &math.trace {
            Some(m) => {
                let mut f = (**frame).clone();
                f.channels.push(m.clone());
                std::sync::Arc::new(f)
            }
            None => frame.clone(),
        });
        phosphor.new_frame = true;
    }
    phosphor.decay = match phosphor.persistence {
        Persistence::Off | Persistence::Infinite => 1.0,
        Persistence::Seconds(s) => (-time.delta_secs() / s.max(1e-3)).exp(),
    };
}

fn ingest(time: Res<Time>, mut link: ResMut<Link>) {
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
                link.last_frame_at = time.elapsed_secs_f64();
                if link.frames_seen == 1 || link.frames_seen.is_multiple_of(500) {
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
    let idx = if up {
        (idx + 1).min(ladder.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    ladder[idx]
}

fn input(keys: Res<ButtonInput<KeyCode>>, egui_wants: Res<EguiWantsInput>, mut link: ResMut<Link>) {
    if egui_wants.wants_any_keyboard_input() {
        return;
    }
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
    if keys.just_pressed(KeyCode::KeyS)
        && let TriggerKind::Edge { slope } = &mut link.config.trigger.kind
    {
        *slope = match *slope {
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

fn phosphor_input(
    keys: Res<ButtonInput<KeyCode>>,
    egui_wants: Res<EguiWantsInput>,
    mut phosphor: ResMut<Phosphor>,
) {
    if egui_wants.wants_any_keyboard_input() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        let ladder = Persistence::LADDER;
        let i = ladder
            .iter()
            .position(|p| *p == phosphor.persistence)
            .unwrap_or(0);
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

fn draw_graticule(layout: Res<Layout>, mut gizmos: Gizmos) {
    let w = layout.plot.width();
    let h = layout.plot.height();
    let o = layout.plot_center;
    let dim = Color::srgba(0.5, 0.55, 0.6, 0.25);
    let axis = Color::srgba(0.6, 0.65, 0.7, 0.6);
    for i in 0..=H_DIVS {
        let x = o.x - w / 2.0 + i as f32 * layout.div.x;
        let c = if i == H_DIVS / 2 { axis } else { dim };
        gizmos.line_2d(Vec2::new(x, o.y - h / 2.0), Vec2::new(x, o.y + h / 2.0), c);
    }
    for i in 0..=V_DIVS {
        let y = o.y - h / 2.0 + i as f32 * layout.div.y;
        let c = if i == V_DIVS / 2 { axis } else { dim };
        gizmos.line_2d(Vec2::new(o.x - w / 2.0, y), Vec2::new(o.x + w / 2.0, y), c);
    }
}

fn draw_trigger(link: Res<Link>, layout: Res<Layout>, mut gizmos: Gizmos) {
    let w = layout.plot.width();
    let o = layout.plot_center;
    let src = link
        .config
        .trigger
        .source
        .min(link.config.channels.len() - 1);
    let ch = &link.config.channels[src];
    let range = ch.volts_div * 10.0 * ch.probe;
    // Fraction of full (10 div) range; the display window is +-4 div.
    let frac = (link.config.trigger.level / range + ch.offset).clamp(-0.44, 0.44);
    let y = layout.frac_to_world_y(frac as f32);
    gizmos.line_2d(
        Vec2::new(o.x - w / 2.0, y),
        Vec2::new(o.x + w / 2.0, y),
        Color::srgba(1.0, 0.5, 0.2, 0.5),
    );
}

/// The pass/fail envelope as two dim-green polylines (lo and hi bounds).
fn draw_pf_mask(pf: Res<derived::PfState>, layout: Res<Layout>, mut gizmos: Gizmos) {
    let Some(mask) = &pf.mask else { return };
    if !pf.enabled || mask.lo.is_empty() {
        return;
    }
    let w = layout.plot.width();
    let o = layout.plot_center;
    let n = mask.lo.len();
    // Decimate so the gizmo stays cheap on a 5000-sample record.
    let step = (n / 500).max(1);
    let x_at = |i: usize| o.x - w / 2.0 + i as f32 / (n - 1).max(1) as f32 * w;
    // The display window is +-100 counts (+-4 div); pin beyond that.
    let y_at = |raw: i8| layout.frac_to_world_y(raw.clamp(-100, 100) as f32 / 250.0);
    let color = Color::srgba(0.2, 0.6, 0.3, 0.6);
    for bounds in [&mask.lo, &mask.hi] {
        let points: Vec<Vec2> = (0..n)
            .step_by(step)
            .map(|i| Vec2::new(x_at(i), y_at(bounds[i])))
            .collect();
        gizmos.linestrip_2d(points, color);
    }
}

/// Measurement guides: dashed levels at Vtop/Vbase/Vavg and the 10%/90%
/// rise-time thresholds of the stats trace, drawn while the Measure dialog
/// is open (toggleable there).
fn draw_guides(
    meas: Res<derived::MeasureState>,
    math: Res<derived::MathState>,
    menus: Res<ui::MenuState>,
    link: Res<Link>,
    layout: Res<Layout>,
    mut gizmos: Gizmos,
) {
    if !meas.guides || menus.open != Some(ui::Menu::Measure) {
        return;
    }
    let slot = meas.stats_slot;
    let Some(m) = &meas.latest[slot] else { return };
    let scale = match slot {
        2 => math.trace.as_ref().map(|t| (t.volts_per_lsb, t.zero_volts)),
        s => link
            .latest
            .as_ref()
            .and_then(|f| f.channels.iter().find(|c| c.ch == s))
            .map(|c| (c.volts_per_lsb, c.zero_volts)),
    };
    let Some((lsb, zero)) = scale else { return };
    let base = match slot {
        0 => Color::srgb(1.0, 0.85, 0.1),
        1 => Color::srgb(0.2, 0.75, 1.0),
        _ => Color::srgb(1.0, 0.35, 0.85),
    };
    let w = layout.plot.width();
    let o = layout.plot_center;
    let lines = [
        (m.vtop, 0.55),
        (m.vbase, 0.55),
        (m.vavg, 0.4),
        (m.vbase + 0.1 * (m.vtop - m.vbase), 0.25),
        (m.vbase + 0.9 * (m.vtop - m.vbase), 0.25),
    ];
    for (v, alpha) in lines {
        let frac = (((v - zero) / lsb) / 250.0) as f32;
        if frac.abs() > 0.4 {
            continue; // outside the visible +-4-division window
        }
        let y = layout.frac_to_world_y(frac);
        let color = base.with_alpha(alpha);
        // Dashed: 6 px on / 6 px off.
        let mut x = o.x - w / 2.0;
        while x < o.x + w / 2.0 {
            let x2 = (x + 6.0).min(o.x + w / 2.0);
            gizmos.line_2d(Vec2::new(x, y), Vec2::new(x2, y), color);
            x += 12.0;
        }
    }
}

/// On-graph handles: trigger-level arrow at the right edge, trigger-position
/// arrow at the top edge, per-channel offset arrows at the left edge — all
/// draggable (ui::touch), all hidden by the Markers toggle.
fn draw_markers(
    link: Res<Link>,
    cur: Res<cursors::CursorState>,
    layout: Res<Layout>,
    mut gizmos: Gizmos,
) {
    if !cur.markers {
        return;
    }
    let w = layout.plot.width();
    let h = layout.plot.height();
    let o = layout.plot_center;
    let (left, right, top) = (o.x - w / 2.0, o.x + w / 2.0, o.y + h / 2.0);

    // Left-pointing arrowhead at the right edge: trigger level.
    let ty = ui::touch::trigger_line_y(&layout, &link);
    let tcol = Color::srgb(1.0, 0.55, 0.25);
    for d in 0..6 {
        let f = d as f32;
        gizmos.line_2d(
            Vec2::new(right - f, ty - (6.0 - f)),
            Vec2::new(right - f, ty + (6.0 - f)),
            tcol,
        );
    }

    // Down-pointing arrowhead at the top edge: trigger position.
    let tx = left + link.config.position as f32 * w;
    for d in 0..6 {
        let f = d as f32;
        gizmos.line_2d(
            Vec2::new(tx - (6.0 - f), top - f),
            Vec2::new(tx + (6.0 - f), top - f),
            tcol,
        );
    }

    // Right-pointing arrowheads at the left edge: channel zero offsets.
    for ch in 0..2 {
        let c = link.config.channels[ch];
        if !c.enabled {
            continue;
        }
        let y = layout.frac_to_world_y(c.offset as f32);
        let col = if ch == 0 {
            Color::srgb(1.0, 0.85, 0.1)
        } else {
            Color::srgb(0.2, 0.75, 1.0)
        };
        for d in 0..6 {
            let f = d as f32;
            gizmos.line_2d(
                Vec2::new(left + f, y - (6.0 - f)),
                Vec2::new(left + f, y + (6.0 - f)),
                col,
            );
        }
    }
}

fn update_title(link: Res<Link>, mut windows: Query<&mut Window>) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    // On-screen chrome carries the state; the OS title stays quiet.
    let title = format!("neowon — {}", link.status);
    if window.title != title {
        window.title = title;
    }
}

/// Red clip arrows at the plot edge when a channel's samples sit on the ADC
/// rails — the honest companion to the shader's off-screen suppression.
fn draw_clip_warnings(link: Res<Link>, layout: Res<Layout>, mut gizmos: Gizmos) {
    let Some(frame) = &link.latest else { return };
    let w = layout.plot.width();
    let h = layout.plot.height();
    let o = layout.plot_center;
    let red = Color::srgb(1.0, 0.25, 0.2);
    for cap in &frame.channels {
        if !cap.clipped {
            continue;
        }
        let (mut top, mut bottom) = (false, false);
        for &r in &cap.raw {
            top |= r >= 125;
            bottom |= r <= -125;
        }
        let x = o.x + w / 2.0 - 26.0 - cap.ch as f32 * 22.0;
        let mut arrow = |y: f32, dir: f32| {
            gizmos.line_2d(Vec2::new(x, y), Vec2::new(x, y + 12.0 * dir), red);
            gizmos.line_2d(
                Vec2::new(x, y + 12.0 * dir),
                Vec2::new(x - 4.0, y + 7.0 * dir),
                red,
            );
            gizmos.line_2d(
                Vec2::new(x, y + 12.0 * dir),
                Vec2::new(x + 4.0, y + 7.0 * dir),
                red,
            );
        };
        if top {
            arrow(o.y + h / 2.0 - 16.0, 1.0);
        }
        if bottom {
            arrow(o.y - h / 2.0 + 16.0, -1.0);
        }
    }
}
