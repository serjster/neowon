//! Touchscreen-style pointer control on the plot, mapped to the mouse the
//! way modern scopes map gestures: drag the trigger-level line to move the
//! level, drag the waveform to move the selected channel's offset
//! (vertical) and the trigger position (horizontal), scroll to step
//! volts/div, shift+scroll to step the sample rate.
//!
//! Priority: measurement-cursor drags (cursors.rs, runs earlier) win; this
//! system stands down while one is active.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::Link;
use crate::cursors::CursorState;
use crate::ui::layout::Layout;
use crate::ui::widgets::{FALLBACK_RATES, FALLBACK_VDIV};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    TriggerLevel,
    TriggerPosition,
    OffsetMarker(usize),
    Waveform,
}

#[derive(Resource, Default)]
pub struct TouchState {
    drag: Option<Drag>,
    last_world: Vec2,
}

fn world_pos(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Vec2> {
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera.single()) else {
        return None;
    };
    let screen = window.cursor_position()?;
    camera.viewport_to_world_2d(cam_tf, screen).ok()
}

fn on_plot(l: &Layout, world: Vec2) -> bool {
    let half = Vec2::new(l.plot.width(), l.plot.height()) / 2.0;
    (world - l.plot_center).abs().cmple(half).all()
}

/// World y of the trigger-level line (same mapping as `draw_trigger`).
pub fn trigger_line_y(l: &Layout, link: &Link) -> f32 {
    let src = link
        .config
        .trigger
        .source
        .min(link.config.channels.len() - 1);
    let ch = &link.config.channels[src];
    let range = ch.volts_div * 10.0 * ch.probe;
    let frac = (link.config.trigger.level / range + ch.offset).clamp(-0.44, 0.44);
    l.frac_to_world_y(frac as f32)
}

fn step_ladder(ladder: &[f64], current: f64, up: bool) -> f64 {
    let mut idx = 0;
    let mut best = f64::MAX;
    for (i, &v) in ladder.iter().enumerate() {
        let d = (v.ln() - current.max(1e-12).ln()).abs();
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

#[allow(clippy::too_many_arguments)]
pub fn plot_pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    egui_wants: Res<EguiWantsInput>,
    layout: Res<Layout>,
    cursors: Res<CursorState>,
    mut touch: ResMut<TouchState>,
    mut link: ResMut<Link>,
) {
    // Measurement cursors (earlier in the chain) own the pointer while
    // dragging; egui owns it over panels.
    if cursors.dragging() {
        touch.drag = None;
        return;
    }
    if touch.drag.is_none() && egui_wants.wants_any_pointer_input() {
        wheel.clear();
        return;
    }
    let Some(world) = world_pos(&windows, &camera) else {
        touch.drag = None;
        return;
    };

    // Scroll: volts/div ladder on the selected channel; shift = sample rate.
    if on_plot(&layout, world) {
        let mut steps = 0.0f32;
        for ev in wheel.read() {
            steps += match ev.unit {
                MouseScrollUnit::Line => ev.y,
                MouseScrollUnit::Pixel => ev.y / 40.0,
            };
        }
        if steps.abs() >= 0.5 {
            let up = steps > 0.0;
            if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
                let ladder = link
                    .caps
                    .as_ref()
                    .map(|c| c.sample_rates.clone())
                    .unwrap_or_else(|| FALLBACK_RATES.to_vec());
                link.config.sample_rate = step_ladder(&ladder, link.config.sample_rate, up);
            } else {
                let ladder = link
                    .caps
                    .as_ref()
                    .map(|c| c.volts_div.clone())
                    .unwrap_or_else(|| FALLBACK_VDIV.to_vec());
                let sel = link.selected.min(1);
                // Scroll up = zoom in = fewer volts per division.
                let v = step_ladder(&ladder, link.config.channels[sel].volts_div, !up);
                link.config.channels[sel].volts_div = v;
            }
            link.dirty = true;
        }
    } else {
        wheel.clear();
    }

    if mouse.just_pressed(MouseButton::Left) && on_plot(&layout, world) {
        let left = layout.plot_center.x - layout.plot.width() / 2.0;
        let top = layout.plot_center.y + layout.plot.height() / 2.0;
        // Hit priority: channel offset markers (left edge) > trigger
        // position marker (top edge) > trigger level line > waveform.
        let mut drag = None;
        if world.x - left < 20.0 {
            for ch in 0..2 {
                let c = link.config.channels[ch];
                if c.enabled {
                    let y = layout.frac_to_world_y(c.offset as f32);
                    if (world.y - y).abs() < 12.0 {
                        drag = Some(Drag::OffsetMarker(ch));
                        link.selected = ch;
                        break;
                    }
                }
            }
        }
        if drag.is_none() && top - world.y < 20.0 {
            let x = left + link.config.position as f32 * layout.plot.width();
            if (world.x - x).abs() < 14.0 {
                drag = Some(Drag::TriggerPosition);
            }
        }
        if drag.is_none() {
            drag = if (world.y - trigger_line_y(&layout, &link)).abs() < 12.0 {
                Some(Drag::TriggerLevel)
            } else {
                Some(Drag::Waveform)
            };
        }
        touch.drag = drag;
        touch.last_world = world;
    }
    if !mouse.pressed(MouseButton::Left) {
        touch.drag = None;
        return;
    }
    let Some(drag) = touch.drag else { return };
    let delta = world - touch.last_world;
    if delta == Vec2::ZERO {
        return;
    }
    touch.last_world = world;

    // One full-scale fraction per 10 divisions of screen (the encoding the
    // config uses; the visible window is 8 of those 10).
    let dfrac = (delta.y / (10.0 * layout.div.y)) as f64;
    match drag {
        Drag::TriggerPosition => {
            let pos =
                (link.config.position + (delta.x / layout.plot.width()) as f64).clamp(0.0, 1.0);
            link.config.position = pos;
            link.dirty = true;
        }
        Drag::OffsetMarker(ch) => {
            let off = (link.config.channels[ch].offset + dfrac).clamp(-0.5, 0.5);
            link.config.channels[ch].offset = off;
            link.dirty = true;
        }
        Drag::TriggerLevel => {
            let src = link.config.trigger.source.min(1);
            let ch = link.config.channels[src];
            let range = ch.volts_div * 10.0 * ch.probe;
            link.config.trigger.level =
                (link.config.trigger.level + dfrac * range).clamp(-range, range);
            link.dirty = true;
        }
        Drag::Waveform => {
            let sel = link.selected.min(1);
            let off = (link.config.channels[sel].offset + dfrac).clamp(-0.5, 0.5);
            link.config.channels[sel].offset = off;
            let pos =
                (link.config.position - (delta.x / layout.plot.width()) as f64).clamp(0.0, 1.0);
            link.config.position = pos;
            link.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_steps_and_clamps() {
        let l = [0.1, 0.2, 0.5, 1.0];
        assert_eq!(step_ladder(&l, 0.2, true), 0.5);
        assert_eq!(step_ladder(&l, 0.2, false), 0.1);
        assert_eq!(step_ladder(&l, 1.0, true), 1.0); // clamps at the top
        assert_eq!(step_ladder(&l, 0.1, false), 0.1); // and the bottom
        // Snaps to nearest rung first (log distance).
        assert_eq!(step_ladder(&l, 0.24, true), 0.5);
    }
}
