//! Measurement cursors: a time pair and an amplitude pair, draggable with
//! the mouse on the plot area.

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::gpu::Phosphor;
use crate::ui::layout::Layout;

/// Record fraction -> visible fraction of the horizontal zoom window.
fn view_frac(pos: f32, hview: (f64, f64)) -> f32 {
    let (center, span) = hview;
    ((pos as f64 - (center - span / 2.0)) / span) as f32
}

/// Visible fraction -> record fraction (inverse of `view_frac`).
fn record_frac(xf: f32, hview: (f64, f64)) -> f32 {
    let (center, span) = hview;
    (xf as f64 * span + (center - span / 2.0)) as f32
}

/// Cursor indices: 0/1 = time (x, fraction 0..1 of the record),
/// 2/3 = amplitude (y, fraction -0.5..0.5 of full scale = +-5 divisions;
/// the visible window is +-4 divisions).
#[derive(Resource)]
pub struct CursorState {
    pub time_on: bool,
    pub amp_on: bool,
    /// On-graph handles (trigger level/position, channel offsets).
    pub markers: bool,
    pub pos: [f32; 4],
    /// Sticky SI bands for the dialog readouts (Δt, 1/Δt, ΔV).
    pub bands: [crate::derived::Band; 3],
    drag: Option<usize>,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            time_on: false,
            amp_on: false,
            markers: true,
            pos: [0.4, 0.6, -0.1, 0.1],
            bands: Default::default(),
            drag: None,
        }
    }
}

impl CursorState {
    /// True while a cursor drag owns the pointer (touch control stands down).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn x_world(&self, l: &Layout, hview: (f64, f64), i: usize) -> f32 {
        l.plot_center.x + (view_frac(self.pos[i], hview) - 0.5) * l.plot.width()
    }

    fn y_world(&self, l: &Layout, i: usize) -> f32 {
        l.frac_to_world_y(self.pos[i])
    }
}

pub fn cursor_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    egui_wants: Res<EguiWantsInput>,
    layout: Res<Layout>,
    phosphor: Res<Phosphor>,
    mut cur: ResMut<CursorState>,
) {
    if cur.drag.is_none() && egui_wants.wants_any_pointer_input() {
        return;
    }
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera.single()) else {
        // A broken query here kills all on-graph dragging — never silent.
        warn_once!("cursor_input: window/camera query failed; drags disabled");
        return;
    };
    let Some(screen) = window.cursor_position() else {
        return;
    };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, screen) else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) {
        let mut best: Option<(usize, f32)> = None;
        let mut consider = |i: usize, d: f32| {
            if d < 12.0 && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        };
        if cur.time_on {
            consider(0, (world.x - cur.x_world(&layout, phosphor.hview, 0)).abs());
            consider(1, (world.x - cur.x_world(&layout, phosphor.hview, 1)).abs());
        }
        if cur.amp_on {
            consider(2, (world.y - cur.y_world(&layout, 2)).abs());
            consider(3, (world.y - cur.y_world(&layout, 3)).abs());
        }
        cur.drag = best.map(|(i, _)| i);
    }
    if mouse.pressed(MouseButton::Left) {
        if let Some(i) = cur.drag {
            if i < 2 {
                let xf = (world.x - layout.plot_center.x) / layout.plot.width() + 0.5;
                cur.pos[i] = record_frac(xf, phosphor.hview).clamp(0.0, 1.0);
            } else {
                cur.pos[i] =
                    ((world.y - layout.plot_center.y) / (10.0 * layout.div.y)).clamp(-0.4, 0.4);
            }
        }
    } else {
        cur.drag = None;
    }
}

pub fn draw_cursors(
    cur: Res<CursorState>,
    layout: Res<Layout>,
    phosphor: Res<Phosphor>,
    mut gizmos: Gizmos,
) {
    let w = layout.plot.width();
    let h = layout.plot.height();
    let o = layout.plot_center;
    let color = Color::srgba(0.4, 1.0, 0.6, 0.7);
    if cur.time_on {
        for i in 0..2 {
            // Cursors outside the zoom window hide at the plot edge.
            let xf = view_frac(cur.pos[i], phosphor.hview);
            if !(0.0..=1.0).contains(&xf) {
                continue;
            }
            let x = cur.x_world(&layout, phosphor.hview, i);
            gizmos.line_2d(
                Vec2::new(x, o.y - h / 2.0),
                Vec2::new(x, o.y + h / 2.0),
                color,
            );
        }
    }
    if cur.amp_on {
        for i in 2..4 {
            let y = cur.y_world(&layout, i);
            gizmos.line_2d(
                Vec2::new(o.x - w / 2.0, y),
                Vec2::new(o.x + w / 2.0, y),
                color,
            );
        }
    }
}
