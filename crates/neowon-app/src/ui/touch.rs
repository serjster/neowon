//! Touchscreen-style pointer control on the plot, mapped to the mouse the
//! way modern scopes map gestures: drag the trigger-level line to move the
//! level, drag the waveform to move the selected channel's offset
//! (vertical) and the horizontal zoom window (horizontal), scroll to step
//! volts/div, shift+scroll to zoom the horizontal window at the pointer,
//! and a 2-D wheel's x axis to pan the window. Double-click resets the
//! window. Zoom/pan share the `view` ops with the dock toolbar, keys, and
//! scripts.
//!
//! Priority: measurement-cursor drags (cursors.rs, runs earlier) win; this
//! system stands down while one is active.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use neowon_backend::ScopeConfig;

use crate::Link;
use crate::cursors::CursorState;
use crate::gpu::Phosphor;
use crate::ui::layout::Layout;
use crate::view;

/// Second click within this window counts as a double-click.
const DOUBLE_CLICK_S: f64 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    TriggerLevel,
    TriggerPosition,
    OffsetMarker(usize),
    Waveform,
}

#[derive(Resource, Default)]
pub struct TouchState {
    drag: Option<Drag>,
    last_world: Vec2,
    /// Time of the last press inside the plot (double-click detection).
    last_click: f64,
}

fn world_pos(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
) -> Option<Vec2> {
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera.single()) else {
        // A broken query here kills all plot pointer control — never silent.
        bevy::log::warn_once!("plot pointer: window/camera query failed; gestures disabled");
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
pub fn trigger_line_y(l: &Layout, config: &ScopeConfig) -> f32 {
    let src = config.trigger.source.min(config.channels.len() - 1);
    let ch = &config.channels[src];
    let range = ch.volts_div * 10.0 * ch.probe;
    let frac = (config.trigger.level / range + ch.offset).clamp(-0.44, 0.44);
    l.frac_to_world_y(frac as f32)
}

/// Marker/waveform hit test for a press inside the plot. Priority: channel
/// offset markers (left edge) > trigger position marker (top edge) >
/// trigger level line > waveform (pan).
pub fn hit_drag(layout: &Layout, config: &ScopeConfig, world: Vec2) -> Drag {
    let left = layout.plot_center.x - layout.plot.width() / 2.0;
    let top = layout.plot_center.y + layout.plot.height() / 2.0;
    if world.x - left < 20.0 {
        for ch in 0..2 {
            let c = &config.channels[ch];
            if c.enabled {
                let y = layout.frac_to_world_y(c.offset as f32);
                if (world.y - y).abs() < 12.0 {
                    return Drag::OffsetMarker(ch);
                }
            }
        }
    }
    if top - world.y < 20.0 {
        let x = left + config.position as f32 * layout.plot.width();
        if (world.x - x).abs() < 14.0 {
            return Drag::TriggerPosition;
        }
    }
    if (world.y - trigger_line_y(layout, config)).abs() < 12.0 {
        Drag::TriggerLevel
    } else {
        Drag::Waveform
    }
}

#[allow(clippy::too_many_arguments)]
pub fn plot_pointer(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    egui_wants: Res<EguiWantsInput>,
    layout: Res<Layout>,
    cursors: Res<CursorState>,
    mut touch: ResMut<TouchState>,
    mut link: ResMut<Link>,
    mut phosphor: ResMut<Phosphor>,
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

    // Scroll: volts/div ladder on the selected channel; shift = horizontal
    // zoom of the record window at the pointer; a 2-D wheel's x axis pans
    // the window.
    if on_plot(&layout, world) {
        let mut steps = [0.0f32; 2];
        for ev in wheel.read() {
            let d = match ev.unit {
                MouseScrollUnit::Line => [ev.y, ev.x],
                MouseScrollUnit::Pixel => [ev.y / 40.0, ev.x / 40.0],
            };
            steps[0] += d[0];
            steps[1] += d[1];
        }
        if steps[0].abs() >= 0.5 {
            // Scroll up = zoom in (finer scale).
            let inward = steps[0] > 0.0;
            if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
                let anchor = ((world.x - (layout.plot_center.x - layout.plot.width() / 2.0))
                    / layout.plot.width())
                .clamp(0.0, 1.0);
                let (c, s) = phosphor.hview;
                view::hview_zoom(&mut phosphor, (c - s / 2.0) + anchor as f64 * s, inward);
            } else {
                let sel = link.selected.min(1);
                view::zoom_channel(&mut link, sel, inward);
            }
        }
        if steps[1].abs() >= 0.5 {
            // Content follows the finger: swipe right slides the waveform
            // right (the window moves earlier in the record).
            let d = -(steps[1] as f64) * 0.02 * phosphor.hview.1;
            view::hview_pan(&mut phosphor, d);
        }
    } else {
        wheel.clear();
    }

    if mouse.just_pressed(MouseButton::Left) && on_plot(&layout, world) {
        let now = time.elapsed_secs_f64();
        let drag = hit_drag(&layout, &link.config, world);
        // Double-click on empty plot: reset the horizontal window.
        if drag == Drag::Waveform && now - touch.last_click < DOUBLE_CLICK_S {
            view::hview_home(&mut phosphor);
            touch.drag = None;
            touch.last_click = 0.0;
            return;
        }
        touch.last_click = now;
        if let Drag::OffsetMarker(ch) = drag {
            link.selected = ch;
        }
        touch.drag = Some(drag);
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
            link.dirty = true;
            // Horizontal: pan the zoom window (instant; the trigger-position
            // marker stays the acquisition control). Waveform follows the
            // pointer, so dragging right moves the window earlier.
            let d = -(delta.x / layout.plot.width()) as f64 * phosphor.hview.1;
            view::hview_pan(&mut phosphor, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::compute(1520.0, 820.0)
    }

    #[test]
    fn empty_plot_area_starts_waveform_pan() {
        let l = layout();
        let cfg = crate::view::startup_config();
        assert_eq!(hit_drag(&l, &cfg, l.plot_center), Drag::Waveform);
    }

    #[test]
    fn near_trigger_line_drags_the_level() {
        let l = layout();
        let cfg = crate::view::startup_config();
        let y = trigger_line_y(&l, &cfg);
        let world = Vec2::new(l.plot_center.x, y + 5.0);
        assert_eq!(hit_drag(&l, &cfg, world), Drag::TriggerLevel);
    }

    #[test]
    fn left_edge_at_channel_zero_is_its_offset_marker() {
        let l = layout();
        let mut cfg = crate::view::startup_config();
        cfg.channels[0].enabled = true;
        cfg.channels[1].enabled = true;
        cfg.channels[1].offset = 0.25;
        let left = l.plot_center.x - l.plot.width() / 2.0;
        // CH1 marker sits at offset 0 (plot center in y).
        let world = Vec2::new(left + 5.0, l.frac_to_world_y(0.0));
        assert_eq!(hit_drag(&l, &cfg, world), Drag::OffsetMarker(0));
        // CH2 marker a quarter full-scale up.
        let world = Vec2::new(left + 5.0, l.frac_to_world_y(0.25));
        assert_eq!(hit_drag(&l, &cfg, world), Drag::OffsetMarker(1));
    }

    #[test]
    fn top_edge_at_position_is_the_trigger_marker() {
        let l = layout();
        let cfg = crate::view::startup_config();
        let left = l.plot_center.x - l.plot.width() / 2.0;
        let top = l.plot_center.y + l.plot.height() / 2.0;
        let x = left + cfg.position as f32 * l.plot.width();
        let world = Vec2::new(x, top - 5.0);
        assert_eq!(hit_drag(&l, &cfg, world), Drag::TriggerPosition);
    }
}
