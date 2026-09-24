use bevy::prelude::*;

use crate::plot::PlotSprite;
use crate::ui::layout::Layout;
use crate::{Link, autostate, ui};

/// Fit the app to the display it opened on. A 4K panel the OS does not scale
/// leaves 12 pt text 12 physical pixels tall, so the UI scale comes from the
/// monitor unless `NEOWON_UI_SCALE` overrides it; the window grows to match
/// so the chrome still leaves a usable grid. `NEOWON_WINDOW` pins the size
/// for layout tests and wins over the fit. The saved state sits
/// between the two: env > saved > auto-fit, for the scale and the size
/// alike; a saved position is used only while it is on a monitor.
pub(crate) fn fit_display(
    monitors: Query<&bevy::window::Monitor>,
    mut windows: Query<&mut Window>,
    mut scale: ResMut<ui::UiScale>,
    state: Res<autostate::AutoState>,
) {
    let saved = state.geometry();
    let env_scale = std::env::var("NEOWON_UI_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    let Ok(mut window) = windows.single_mut() else {
        if let Some(s) = env_scale.or(saved.scale) {
            scale.0 = s.clamp(ui::layout::UI_SCALE_RANGE.0, ui::layout::UI_SCALE_RANGE.1);
        }
        return;
    };
    let monitor = monitors.iter().next();
    let auto = monitor
        .map(|m| ui::layout::auto_scale(m.physical_height, m.scale_factor as f32))
        .unwrap_or(1.0);
    scale.0 = env_scale
        .or(saved.scale)
        .unwrap_or(auto)
        .clamp(ui::layout::UI_SCALE_RANGE.0, ui::layout::UI_SCALE_RANGE.1);

    window.resize_constraints.min_width = ui::layout::MIN_W * scale.0;
    window.resize_constraints.min_height = ui::layout::MIN_H * scale.0;
    if std::env::var_os("NEOWON_WINDOW").is_some() {
        return;
    }
    if let Some(p) = saved.pos
        && monitors.iter().any(|m| {
            let (lo, size) = (
                m.physical_position,
                IVec2::new(m.physical_width as i32, m.physical_height as i32),
            );
            // The title bar must land on a monitor, or the window cannot be
            // grabbed back (a monitor unplugged since the save).
            p.cmpge(lo).all() && p.cmplt(lo + size - IVec2::splat(40)).all()
        })
    {
        window.position = WindowPosition::At(p);
    }
    if let Some(m) = monitor {
        // The saved size, else ~70% of the monitor; never below the scaled
        // minimum nor beyond the monitor.
        let (mw, mh) = (
            m.physical_width as f32 / m.scale_factor as f32,
            m.physical_height as f32 / m.scale_factor as f32,
        );
        let (w, h) = saved.size.unwrap_or((mw * 0.7, mh * 0.7));
        let w = w.max(ui::layout::MIN_W * scale.0).min(mw - 40.0);
        let h = h.max(ui::layout::MIN_H * scale.0).min(mh - 80.0);
        window.resolution.set(w, h);
    } else if let Some((w, h)) = saved.size {
        window.resolution.set(w, h);
    }
}

pub(crate) fn sync_layout(
    windows: Query<&Window>,
    scale: Res<ui::UiScale>,
    mut layout: ResMut<Layout>,
    mut sprite: Query<(&mut Sprite, &mut Transform), With<PlotSprite>>,
) {
    let Ok(window) = windows.single() else { return };
    let next = Layout::compute(window.width(), window.height(), scale.0);
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

pub(crate) fn update_title(link: Res<Link>, mut windows: Query<&mut Window>) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    // On-screen chrome carries the state; the OS title stays quiet.
    let title = format!("neowon — {}", link.status);
    if window.title != title {
        window.title = title;
    }
}
