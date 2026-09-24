//! Gizmo overlays over the plot: graticule, trigger level, pass/fail mask,
//! measurement guides, drag markers, zoom band and clip warnings.

use bevy::prelude::*;

use crate::gpu::Phosphor;
// Screen geometry follows the reference scope: 10 horizontal x 8 vertical
// divisions (docs/ui-ux-research.md §6), computed at runtime from the window
// size (`ui::layout::Layout`).
use crate::ui::layout::{H_DIVS, Layout, V_DIVS};
use crate::{Link, cursors, derived, ui, view};

pub(crate) fn draw_graticule(layout: Res<Layout>, mut gizmos: Gizmos) {
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

pub(crate) fn draw_trigger(link: Res<Link>, layout: Res<Layout>, mut gizmos: Gizmos) {
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
pub(crate) fn draw_pf_mask(pf: Res<derived::PfState>, layout: Res<Layout>, mut gizmos: Gizmos) {
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
    let y_at = |raw: f32| layout.frac_to_world_y(raw.clamp(-100.0, 100.0) / 250.0);
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
pub(crate) fn draw_guides(
    meas: Res<derived::MeasureState>,
    math: Res<derived::MathState>,
    menus: Res<ui::MenuState>,
    link: Res<Link>,
    layout: Res<Layout>,
    mut gizmos: Gizmos,
) {
    if !meas.guides || !menus.is_open(ui::Menu::Measure) {
        return;
    }
    let slot = meas.stats_slot;
    let Some(m) = &meas.latest[slot] else { return };
    let scale = match slot {
        2 => math.trace.as_ref().map(|t| (t.cal.scale_i, t.cal.offset_i)),
        s => link
            .latest
            .as_ref()
            .and_then(|f| f.channels.iter().find(|c| c.ch == s))
            .map(|c| (c.cal.scale_i, c.cal.offset_i)),
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
pub(crate) fn draw_markers(
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
    let ty = ui::touch::trigger_line_y(&layout, &link.config);
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

/// Zoom-window band along the plot's top edge: a full-width track for the
/// record with the magnified slice highlighted. Scopes show the zoom region
/// as a box on the main sweep; the app has one grid, so the band is the
/// compact equivalent — without it a zoomed display gives no clue which part
/// of the record is on screen.
pub(crate) fn draw_zoom_band(phosphor: Res<Phosphor>, layout: Res<Layout>, mut gizmos: Gizmos) {
    if !view::zoom_active(&phosphor) {
        return;
    }
    let (w, h) = (layout.plot.width(), layout.plot.height());
    let o = layout.plot_center;
    let (left, top) = (o.x - w / 2.0, o.y + h / 2.0);
    let y = top + 5.0;
    gizmos.line_2d(
        Vec2::new(left, y),
        Vec2::new(left + w, y),
        Color::srgba(0.5, 0.5, 0.55, 0.5),
    );
    let (center, span) = phosphor.hview;
    let (x0, x1) = (
        left + (center - span / 2.0) as f32 * w,
        left + (center + span / 2.0) as f32 * w,
    );
    let col = Color::srgb(0.3, 0.9, 0.6);
    for dy in [-1.0f32, 0.0, 1.0] {
        gizmos.line_2d(Vec2::new(x0, y + dy), Vec2::new(x1, y + dy), col);
    }
    for x in [x0, x1] {
        gizmos.line_2d(Vec2::new(x, y - 4.0), Vec2::new(x, y + 4.0), col);
    }
}

/// Red clip arrows at the plot edge when a channel's samples sit on the ADC
/// rails — the honest companion to the shader's off-screen suppression.
pub(crate) fn draw_clip_warnings(link: Res<Link>, layout: Res<Layout>, mut gizmos: Gizmos) {
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
        for &r in &cap.data {
            top |= r >= 125.0;
            bottom |= r <= -125.0;
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
