//! Measurement cursors: a time pair and an amplitude pair, draggable with
//! the mouse on the plot area.

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::gpu::{PLOT_H, PLOT_W};
use crate::PLOT_OFFSET;

/// Cursor indices: 0/1 = time (x, fraction 0..1 of the record),
/// 2/3 = amplitude (y, fraction -0.5..0.5 of full scale).
#[derive(Resource)]
pub struct CursorState {
    pub time_on: bool,
    pub amp_on: bool,
    pub pos: [f32; 4],
    drag: Option<usize>,
}

impl Default for CursorState {
    fn default() -> Self {
        Self { time_on: false, amp_on: false, pos: [0.4, 0.6, -0.1, 0.1], drag: None }
    }
}

impl CursorState {
    fn x_world(&self, i: usize) -> f32 {
        PLOT_OFFSET.x + (self.pos[i] - 0.5) * PLOT_W as f32
    }

    fn y_world(&self, i: usize) -> f32 {
        PLOT_OFFSET.y + self.pos[i] * PLOT_H as f32
    }
}

pub fn cursor_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    egui_wants: Res<EguiWantsInput>,
    mut cur: ResMut<CursorState>,
) {
    if cur.drag.is_none() && egui_wants.wants_any_pointer_input() {
        return;
    }
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera.single()) else {
        return;
    };
    let Some(screen) = window.cursor_position() else { return };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, screen) else { return };

    if mouse.just_pressed(MouseButton::Left) {
        let mut best: Option<(usize, f32)> = None;
        let mut consider = |i: usize, d: f32| {
            if d < 12.0 && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        };
        if cur.time_on {
            consider(0, (world.x - cur.x_world(0)).abs());
            consider(1, (world.x - cur.x_world(1)).abs());
        }
        if cur.amp_on {
            consider(2, (world.y - cur.y_world(2)).abs());
            consider(3, (world.y - cur.y_world(3)).abs());
        }
        cur.drag = best.map(|(i, _)| i);
    }
    if mouse.pressed(MouseButton::Left) {
        if let Some(i) = cur.drag {
            if i < 2 {
                cur.pos[i] =
                    ((world.x - PLOT_OFFSET.x) / PLOT_W as f32 + 0.5).clamp(0.0, 1.0);
            } else {
                cur.pos[i] = ((world.y - PLOT_OFFSET.y) / PLOT_H as f32).clamp(-0.5, 0.5);
            }
        }
    } else {
        cur.drag = None;
    }
}

pub fn draw_cursors(cur: Res<CursorState>, mut gizmos: Gizmos) {
    let w = PLOT_W as f32;
    let h = PLOT_H as f32;
    let color = Color::srgba(0.4, 1.0, 0.6, 0.7);
    if cur.time_on {
        for i in 0..2 {
            let x = cur.x_world(i);
            gizmos.line_2d(
                Vec2::new(x, PLOT_OFFSET.y - h / 2.0),
                Vec2::new(x, PLOT_OFFSET.y + h / 2.0),
                color,
            );
        }
    }
    if cur.amp_on {
        for i in 2..4 {
            let y = cur.y_world(i);
            gizmos.line_2d(
                Vec2::new(PLOT_OFFSET.x - w / 2.0, y),
                Vec2::new(PLOT_OFFSET.x + w / 2.0, y),
                color,
            );
        }
    }
}
