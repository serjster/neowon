//! Floating windows for the waterfall spectrogram and the 3D viewport.

use bevy_egui::egui;

use crate::derived::FftState;
use crate::viz::three_d::{RT_H, RT_W, Viz3d, Viz3dState};
use crate::viz::waterfall::WaterfallState;

pub fn waterfall(
    ctx: &egui::Context,
    tex: egui::TextureId,
    wf: &mut WaterfallState,
    fft: &FftState,
) {
    let mut open = wf.on;
    egui::Window::new("Waterfall")
        .default_width(560.0)
        .default_pos((70.0, 70.0))
        .open(&mut open)
        .show(ctx, |ui| {
            // Horizontal crop follows the spectrum view's zoom.
            let (f0, f1) = fft.view;
            let uv =
                egui::Rect::from_min_max(egui::pos2(f0 as f32, 0.0), egui::pos2(f1 as f32, 1.0));
            let w = ui.available_width().max(64.0);
            let img = egui::Image::new((tex, egui::vec2(w, w * 0.5))).uv(uv);
            ui.add(img);
            ui.label(
                egui::RichText::new(
                    "newest at the bottom · frequency span follows the spectrum zoom",
                )
                .weak()
                .small(),
            );
        });
    wf.on = open;
}

pub fn viz3d(ctx: &egui::Context, tex: egui::TextureId, viz: &mut Viz3dState) {
    let mut open = viz.mode != Viz3d::Off;
    egui::Window::new("3D View")
        .default_width(560.0)
        .default_pos((660.0, 240.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for m in Viz3d::ALL {
                    if m == Viz3d::Off {
                        continue;
                    }
                    if ui.selectable_label(viz.mode == m, m.name()).clicked() {
                        viz.mode = m;
                    }
                }
            });
            let w = ui.available_width().max(64.0);
            let size = egui::vec2(w, w * RT_H as f32 / RT_W as f32);
            let resp = ui.add(egui::Image::new((tex, size)).sense(egui::Sense::click_and_drag()));
            if resp.dragged() {
                let d = resp.drag_delta();
                viz.yaw -= d.x * 0.01;
                viz.pitch = (viz.pitch + d.y * 0.01).clamp(-1.5, 1.5);
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.0 {
                    viz.dist = (viz.dist * (1.0 - scroll * 0.002)).clamp(0.8, 12.0);
                }
            }
            ui.label(
                egui::RichText::new("drag: orbit · scroll: dolly")
                    .weak()
                    .small(),
            );
        });
    if !open {
        viz.mode = Viz3d::Off;
    }
}
