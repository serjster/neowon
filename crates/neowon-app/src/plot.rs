//! The plot surface: the display texture and its sprite, and handing each
//! record to the phosphor pipeline (plus the `NEOWON_SHOT` readback).

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::gpu::{PLOT_H, PLOT_W, Persistence, Phosphor};
use crate::{Link, derived};

/// Startup: the primary camera (egui pinned to it) and the display texture
/// the compose pass writes.
pub(crate) fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut phosphor: ResMut<Phosphor>,
) {
    // Two cameras exist (the viz viewport renders offscreen), so the egui
    // context must be pinned to this one explicitly — bevy_egui's
    // auto-creation grabs whichever camera it sees first.
    commands.spawn((Camera2d, bevy_egui::PrimaryEguiContext));

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
pub struct PlotSprite;

/// One-shot flags live for exactly one frame (extracted at frame end,
/// cleared at the start of the next).
pub(crate) fn clear_one_shot(mut phosphor: ResMut<Phosphor>) {
    phosphor.new_frame = false;
}

/// Headless verification: NEOWON_SHOT=<frames> reads the display texture back
/// after that many records, writes /tmp/neowon-shot.ppm, and exits.
pub(crate) fn readback_hook(mut commands: Commands, link: Res<Link>, phosphor: Res<Phosphor>) {
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
                    for px in row[..(PLOT_W * 4) as usize].as_chunks::<4>().0 {
                        ppm.extend_from_slice(&px[..3]);
                    }
                }
                match neowon_core::atomic_file::write("/tmp/neowon-shot.ppm", &ppm) {
                    Ok(()) => println!("readback: wrote /tmp/neowon-shot.ppm"),
                    Err(e) => eprintln!("readback: could not write shot: {e}"),
                }
                std::process::exit(0);
            });
    }
}

pub(crate) fn update_phosphor(
    time: Res<Time>,
    link: Res<Link>,
    math: Res<derived::MathState>,
    mut phosphor: ResMut<Phosphor>,
    mut last_record_at: Local<f64>,
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
    // Persistence fades one record into the next, so it must advance once
    // per record — not once per rendered frame. Decaying in wall time meant
    // that at slow time bases, where a record can take a second to arrive,
    // the trace faded to black between acquisitions and flashed once a
    // second; it stopped below 200 ms/div only because the instrument rolls
    // there and frames stream. Between records there is nothing new to
    // blend, so the display holds.
    let now = time.elapsed_secs_f64();
    phosphor.decay = if phosphor.new_frame {
        let dt = (now - *last_record_at).clamp(0.0, 10.0) as f32;
        *last_record_at = now;
        match phosphor.persistence {
            Persistence::Off | Persistence::Infinite => 1.0,
            Persistence::Seconds(s) => (-dt / s.max(1e-3)).exp(),
        }
    } else {
        1.0
    };
}
