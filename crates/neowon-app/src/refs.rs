//! Reference waveforms: frozen copies of a channel's trace drawn as dim
//! ghost polylines over the plot, in raw-count space (the shape exactly as
//! captured, like real budget scopes — it does not rescale with V/div).

use bevy::prelude::*;
use bevy_egui::egui;
use neowon_core::ChannelCapture;

use crate::ui::layout::{Layout, Roi};

/// Ghost hues: dimmed versions of the channel colors.
const REF_COLORS: [egui::Color32; 2] = [
    egui::Color32::from_rgba_premultiplied(120, 104, 16, 160),
    egui::Color32::from_rgba_premultiplied(24, 90, 120, 160),
];

#[derive(Resource, Default)]
pub struct RefState {
    pub traces: [Option<ChannelCapture>; 2],
    pub show: bool,
}

impl RefState {
    /// Freeze channel `ch` of the given frame as its reference.
    pub fn capture(&mut self, frame: &neowon_core::CaptureFrame, ch: usize) {
        if ch < 2
            && let Some(cap) = frame.channels.iter().find(|c| c.ch == ch)
        {
            self.traces[ch] = Some(cap.clone());
            self.show = true;
        }
    }

    pub fn clear(&mut self) {
        self.traces = [None, None];
    }
}

/// Draw the ghost traces over the plot. The visible vertical window is
/// ±100 counts (±4 of the 10 divisions), matching the raster shader.
pub fn overlay(ctx: &egui::Context, l: &Layout, refs: &RefState) {
    if !refs.show || refs.traces.iter().all(Option::is_none) {
        return;
    }
    let rect = Roi::Plot.rect(l);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::new("ref-ghosts"),
    ));
    for (ch, trace) in refs.traces.iter().enumerate() {
        let Some(t) = trace else { continue };
        if t.raw.len() < 2 {
            continue;
        }
        // At most ~1k points: min/max-preserving would be nicer, but a
        // stride is fine for a ghost.
        let step = (t.raw.len() / 1024).max(1);
        let pts: Vec<egui::Pos2> = t
            .raw
            .iter()
            .step_by(step)
            .enumerate()
            .map(|(i, &r)| {
                let x = rect.left() + (i * step) as f32 / (t.raw.len() - 1) as f32 * rect.width();
                let y = rect.center().y - r as f32 / 200.0 * rect.height();
                egui::pos2(x, y.clamp(rect.top(), rect.bottom()))
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.0, REF_COLORS[ch]),
        ));
    }
}
