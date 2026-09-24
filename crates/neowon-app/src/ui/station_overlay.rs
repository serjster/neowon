//! Station overlay on the spectrum: a tick at each known
//! station's frequency and a label `name · WFM [· km]` in up to three
//! staggered rows. Labels that cannot fit become ticks. Returns the hit
//! regions so a click tunes to the station and picks the fitting
//! demodulator. Reference rows only — the operator's catalog has
//! its own display.

use bevy_egui::egui;
use neowon_refdb::{Service, Station};

use super::sdr_view::x_at;
use crate::refmap::RefMap;
use crate::sdr::SdrState;

const ROWS: usize = 3;
const ROW_H: f32 = 13.0;
const TICK: f32 = 14.0;

/// Draw the overlay; returns `(hit rect, station key)` for the pointer.
pub fn draw(
    p: &egui::Painter,
    spec: egui::Rect,
    rm: &RefMap,
    sdr: &SdrState,
) -> Vec<(egui::Rect, String)> {
    if !rm.overlay {
        return Vec::new();
    }
    let font = egui::FontId::proportional(10.0);
    let mut used: Vec<egui::Rect> = Vec::new();
    let mut hits = Vec::new();
    for (s, km) in rm.overlay_stations(sdr) {
        let x = x_at(sdr, spec, s.freq_hz);
        if x < spec.min.x - 1.0 || x > spec.max.x + 1.0 {
            continue;
        }
        let color = colour(s);
        p.line_segment(
            [egui::pos2(x, spec.max.y - TICK), egui::pos2(x, spec.max.y)],
            (1.0, color),
        );
        let label = match km {
            Some(km) => format!("{} · {} · {km:.0} km", s.name, s.modulation.label()),
            None => format!("{} · {}", s.name, s.modulation.label()),
        };
        let galley = p.layout_no_wrap(label, font.clone(), color);
        let (w, h) = (galley.size().x + 6.0, galley.size().y);
        let placed = (0..ROWS).find_map(|row| {
            let y = spec.max.y - TICK - 3.0 - (row as f32 + 1.0) * ROW_H;
            let x0 = (x - w / 2.0).clamp(spec.min.x + 1.0, (spec.max.x - w - 1.0).max(spec.min.x));
            let r = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, h));
            (!used.iter().any(|u| u.intersects(r))).then_some(r)
        });
        match placed {
            Some(r) => {
                p.rect_filled(
                    r,
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(10, 12, 16, 200),
                );
                p.galley(r.min + egui::vec2(3.0, 0.0), galley, color);
                used.push(r);
                hits.push((r, RefMap::key(s)));
            }
            None => {
                // No room for a label: the tick is still clickable.
                hits.push((
                    egui::Rect::from_center_size(
                        egui::pos2(x, spec.max.y - TICK / 2.0),
                        egui::vec2(10.0, TICK + 6.0),
                    ),
                    RefMap::key(s),
                ));
            }
        }
    }
    hits
}

fn colour(s: &Station) -> egui::Color32 {
    match s.service {
        Service::Broadcast => egui::Color32::from_rgb(150, 190, 255),
        Service::Aviation => egui::Color32::from_rgb(120, 220, 150),
        Service::Marine => egui::Color32::from_rgb(120, 220, 230),
        Service::Amateur => egui::Color32::from_rgb(255, 160, 150),
        Service::Utility => egui::Color32::from_rgb(240, 190, 120),
        Service::Other => egui::Color32::from_rgb(190, 195, 205),
    }
}
