//! The band plan on the SDR canvas: the band strip between the spectrum
//! and the waterfall (it shares their frequency axis) and the whole-range
//! minimap across the top (log frequency over everything the tuner
//! reaches, the IQ window as a bracket). Click a band in the strip to tune
//! to its centre, double-click to fit the span to it; click anywhere on the
//! minimap to go there. Every segment is a node in the UI tree.

use bevy_egui::egui::{self, accesskit::Role};
use neowon_backend::SdrCaps;
use neowon_refdb::Band;

use super::sdr_view::{CURSOR, inject};
use crate::refmap::{RefMap, RefMapAction, colour, fmt_range, lanes};
use crate::script::{Action, Script};
use crate::sdr::{SdrAction, SdrState};
use crate::uitree;

pub const STRIP_H: f32 = 22.0;
pub const MINI_H: f32 = 34.0;
const LABEL: egui::Color32 = egui::Color32::from_rgb(235, 238, 245);

fn fill(kind: &str, alpha: f32) -> egui::Color32 {
    colour(kind).gamma_multiply(alpha)
}

/// Draw `text` centred in `r` if it fits, else elided with "…", else not
/// at all.
fn fit_label(p: &egui::Painter, r: egui::Rect, text: &str, size: f32) {
    let font = egui::FontId::proportional(size);
    let fits = |s: &str| {
        p.layout_no_wrap(s.to_string(), font.clone(), LABEL)
            .size()
            .x
            <= r.width() - 6.0
    };
    let mut shown = text.to_string();
    if !fits(&shown) {
        let chars: Vec<char> = text.chars().collect();
        let n = (0..chars.len())
            .rev()
            .find(|&n| fits(&format!("{}…", chars[..n].iter().collect::<String>())));
        match n {
            Some(n) if n >= 3 => shown = format!("{}…", chars[..n].iter().collect::<String>()),
            _ => return,
        }
    }
    p.text(r.center(), egui::Align2::CENTER_CENTER, shown, font, LABEL);
}

/// The band strip over `r`, which spans `lo..hi` Hz linearly.
pub fn strip(ui: &mut egui::Ui, r: egui::Rect, rm: &RefMap, sdr: &SdrState, script: &mut Script) {
    let (lo, hi) = view(sdr);
    let x_of = |hz: f64| r.min.x + ((hz - lo) / (hi - lo)) as f32 * r.width();
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, egui::Color32::from_rgb(18, 20, 26));
    let Some(plan) = rm.plan() else { return };
    let placed = lanes(plan.within(lo, hi));
    let depth = placed.iter().map(|(l, _)| l + 1).max().unwrap_or(1).min(3) as f32;
    let lane_h = r.height() / depth;
    let mut hits: Vec<(egui::Rect, &Band)> = Vec::new();
    for (lane, b) in &placed {
        let lane = (*lane as f32).min(depth - 1.0);
        let seg = egui::Rect::from_min_max(
            egui::pos2(x_of(b.lo_hz).max(r.min.x), r.max.y - (lane + 1.0) * lane_h),
            egui::pos2(x_of(b.hi_hz).min(r.max.x), r.max.y - lane * lane_h),
        );
        let seg = if seg.width() < 2.0 {
            egui::Rect::from_center_size(seg.center(), egui::vec2(2.0, seg.height()))
        } else {
            seg
        };
        p.rect_filled(seg.shrink2(egui::vec2(0.0, 1.0)), 2.0, fill(&b.kind, 0.55));
        p.rect_stroke(
            seg.shrink2(egui::vec2(0.0, 1.0)),
            2.0,
            (1.0, colour(&b.kind)),
            egui::StrokeKind::Inside,
        );
        fit_label(&p, seg, &b.name, (lane_h - 6.0).clamp(9.0, 12.0));
        hits.push((seg, b));
    }
    uitree::node(
        ui.ctx(),
        ui.id().with("band-strip"),
        Role::Group,
        "band strip",
        r,
    );
    for (i, (seg, b)) in hits.iter().enumerate() {
        let label = format!("{} · {} · {}", b.name, fmt_range(b.lo_hz, b.hi_hz), b.kind);
        uitree::node(
            ui.ctx(),
            ui.id().with(("band", i)),
            Role::Button,
            &label,
            *seg,
        );
    }

    let resp = ui.interact(r, ui.id().with("band-strip-hit"), egui::Sense::click());
    let Some(pos) = resp.hover_pos() else { return };
    // The narrowest band under the pointer is the one meant.
    let under = hits
        .iter()
        .filter(|(s, _)| s.x_range().contains(pos.x))
        .min_by(|a, b| (a.1.hi_hz - a.1.lo_hz).total_cmp(&(b.1.hi_hz - b.1.lo_hz)))
        .map(|(_, b)| *b);
    let Some(b) = under else { return };
    let resp = resp.on_hover_text(format!(
        "{}\n{} · {}\nclick: tune to its centre · double-click: fit the span",
        b.name,
        fmt_range(b.lo_hz, b.hi_hz),
        b.kind
    ));
    if resp.double_clicked() {
        script.inject(Action::RefMap(RefMapAction::Goto(b.name.clone())));
    } else if resp.clicked() {
        let c = ((b.lo_hz + b.hi_hz) / 2.0 / 1e3).round() * 1e3;
        inject(script, SdrAction::Tune(c));
    }
}

/// The displayed span, absolute Hz.
fn view(sdr: &SdrState) -> (f64, f64) {
    let half = sdr.span() / 2.0;
    (sdr.view_centre() - half, sdr.view_centre() + half)
}

/// Default tuner range before the instrument's capabilities arrive, Hz.
const FALLBACK_RANGE: (f64, f64) = (500e3, 1.766e9);

/// The tuner's whole range, Hz.
pub fn range(caps: Option<&SdrCaps>) -> (f64, f64) {
    caps.map(|c| c.freq_range_hz).unwrap_or(FALLBACK_RANGE)
}

/// Log-frequency x over `r` for the tuner range.
fn log_x(r: egui::Rect, (lo, hi): (f64, f64), hz: f64) -> f32 {
    let t = (hz.max(lo).ln() - lo.ln()) / (hi.ln() - lo.ln());
    r.min.x + t.clamp(0.0, 1.0) as f32 * r.width()
}

fn log_hz(r: egui::Rect, (lo, hi): (f64, f64), x: f32) -> f64 {
    let t = ((x - r.min.x) / r.width()).clamp(0.0, 1.0) as f64;
    (lo.ln() + t * (hi.ln() - lo.ln())).exp()
}

/// The minimap over `r`: every band of the plan on a log axis across the
/// tuner's range, decade ticks, the IQ window bracket and the tuned tick.
pub fn minimap(
    ui: &mut egui::Ui,
    r: egui::Rect,
    rm: &RefMap,
    sdr: &SdrState,
    caps: Option<&SdrCaps>,
    script: &mut Script,
) {
    let range = range(caps);
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, egui::Color32::from_rgb(18, 20, 26));
    let bar = egui::Rect::from_min_max(r.min + egui::vec2(0.0, 12.0), r.max);
    if let Some(plan) = rm.plan() {
        for (lane, b) in lanes(plan.within(range.0, range.1)) {
            let lane = lane.min(1) as f32;
            let h = bar.height() / 2.0;
            let seg = egui::Rect::from_min_max(
                egui::pos2(log_x(bar, range, b.lo_hz), bar.max.y - (lane + 1.0) * h),
                egui::pos2(
                    log_x(bar, range, b.hi_hz).max(log_x(bar, range, b.lo_hz) + 1.0),
                    bar.max.y - lane * h,
                ),
            );
            p.rect_filled(seg, 0.0, fill(&b.kind, 0.7));
        }
    }
    // Decade ticks: 1, 2, 5 × 10^n.
    let font = egui::FontId::proportional(10.0);
    let mut dec = 10f64.powf(range.0.log10().floor());
    while dec <= range.1 {
        for m in [1.0, 2.0, 5.0] {
            let hz = dec * m;
            if hz < range.0 || hz > range.1 {
                continue;
            }
            let x = log_x(bar, range, hz);
            let major = m == 1.0;
            p.line_segment(
                [
                    egui::pos2(x, bar.min.y),
                    egui::pos2(x, bar.min.y + if major { 6.0 } else { 3.0 }),
                ],
                (1.0, egui::Color32::from_gray(if major { 170 } else { 90 })),
            );
            if major || m == 5.0 {
                p.text(
                    egui::pos2(x + 2.0, r.min.y),
                    egui::Align2::LEFT_TOP,
                    si(hz),
                    font.clone(),
                    egui::Color32::from_gray(if major { 190 } else { 120 }),
                );
            }
        }
        dec *= 10.0;
    }
    // The IQ window as a bracket, never thinner than a grab-able 4 px.
    let half = sdr.config.sample_rate / 2.0;
    let (x0, x1) = (
        log_x(bar, range, sdr.config.centre_hz - half),
        log_x(bar, range, sdr.config.centre_hz + half),
    );
    let win = egui::Rect::from_center_size(
        egui::pos2((x0 + x1) / 2.0, bar.center().y),
        egui::vec2((x1 - x0).max(4.0), bar.height()),
    );
    p.rect_stroke(
        win,
        1.0,
        (1.5, egui::Color32::WHITE),
        egui::StrokeKind::Outside,
    );
    let xt = log_x(bar, range, sdr.tuned_hz);
    p.line_segment(
        [egui::pos2(xt, bar.min.y), egui::pos2(xt, bar.max.y)],
        (2.0, CURSOR),
    );

    uitree::node(
        ui.ctx(),
        ui.id().with("minimap"),
        Role::Group,
        "RF minimap",
        r,
    );
    uitree::node(
        ui.ctx(),
        ui.id().with("minimap-window"),
        Role::Mark,
        "IQ window",
        win,
    );
    let resp = ui.interact(r, ui.id().with("minimap-hit"), egui::Sense::click());
    if let Some(pos) = resp.hover_pos() {
        let hz = log_hz(bar, range, pos.x);
        let names: Vec<String> = rm
            .at(hz)
            .into_iter()
            .map(|b| format!("{} ({})", b.name, fmt_range(b.lo_hz, b.hi_hz)))
            .collect();
        p.line_segment(
            [egui::pos2(pos.x, bar.min.y), egui::pos2(pos.x, bar.max.y)],
            (1.0, egui::Color32::from_white_alpha(90)),
        );
        let resp = resp.on_hover_text(format!(
            "{}\n{}\nclick to tune there",
            crate::ui::sdr_view::fmt_mhz(hz),
            if names.is_empty() {
                "no allocation in this plan".into()
            } else {
                names.join("\n")
            }
        ));
        if resp.clicked() {
            inject(script, SdrAction::Tune(round_hz(hz)));
        }
    }
}

/// A click on a log axis lands on an arbitrary frequency; round it to a
/// step that reads well at that height (1 kHz below 30 MHz, 5 kHz below
/// 300 MHz, 25 kHz above).
pub fn round_hz(hz: f64) -> f64 {
    let step = if hz < 30e6 {
        1e3
    } else if hz < 300e6 {
        5e3
    } else {
        25e3
    };
    (hz / step).round() * step
}

/// "500k", "1M", "20M", "1G".
pub fn si(hz: f64) -> String {
    if hz >= 1e9 {
        format!("{}G", hz / 1e9)
    } else if hz >= 1e6 {
        format!("{}M", hz / 1e6)
    } else {
        format!("{}k", hz / 1e3)
    }
}
