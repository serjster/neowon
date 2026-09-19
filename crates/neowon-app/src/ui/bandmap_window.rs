//! The RF map window: the whole tunable range laid out like a wall chart of
//! band allocations — one row per decade, log frequency within it,
//! overlapping allocations stacked, a legend of kinds. The IQ window and
//! the tuned frequency are marked. Click tunes there; double-click a band
//! fits the span to it (`bandmap goto`).

use bevy_egui::egui::{self, accesskit::Role};

use super::sdr_bands::{range, round_hz, si};
use super::sdr_view::{CURSOR, fmt_mhz, inject};
use crate::refmap::{RefMap, RefMapAction, colour, fmt_range, lanes};
use crate::script::{Action, Script};
use crate::sdr::{SdrAction, SdrState};
use crate::uitree;

const LANE_H: f32 = 17.0;
const MAX_LANES: usize = 4;

pub fn show(ctx: &egui::Context, rm: &RefMap, sdr: &SdrState, script: &mut Script) {
    if !rm.window {
        return;
    }
    let mut open = true;
    egui::Window::new("RF map")
        .open(&mut open)
        .default_size(egui::vec2(1100.0, 560.0))
        .resizable(true)
        .show(ctx, |ui| {
            uitree::name(ui, "RF map");
            ui.horizontal(|ui| {
                ui.label("Band plan");
                egui::ComboBox::from_id_salt("rfmap-plan")
                    .selected_text(rm.stem())
                    .show_ui(ui, |ui| {
                        for (stem, plan) in &rm.plans {
                            let text = format!("{stem}  ({})", plan.country_name);
                            if ui.selectable_label(rm.stem() == stem, text).clicked() {
                                script.inject(Action::RefMap(RefMapAction::Plan(stem.clone())));
                            }
                        }
                    });
                ui.weak("click: tune · double-click a band: fit the span");
            });
            legend(ui, rm);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| rows(ui, rm, sdr, script));
        });
    if !open {
        script.inject(Action::RefMap(RefMapAction::Window(false)));
    }
}

/// Swatches for the kinds the active plan uses, with counts.
fn legend(ui: &mut egui::Ui, rm: &RefMap) {
    let Some(plan) = rm.plan() else { return };
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for b in &plan.bands {
        match kinds.iter_mut().find(|(k, _)| *k == b.kind) {
            Some((_, n)) => *n += 1,
            None => kinds.push((b.kind.clone(), 1)),
        }
    }
    kinds.sort();
    ui.horizontal_wrapped(|ui| {
        for (kind, n) in kinds {
            let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, colour(&kind));
            ui.label(format!("{kind} ({n})"));
            ui.add_space(6.0);
        }
    });
}

/// One row per decade of the tuner's range.
fn rows(ui: &mut egui::Ui, rm: &RefMap, sdr: &SdrState, script: &mut Script) {
    let tuner = range(sdr);
    let mut dec = 10f64.powf(tuner.0.log10().floor());
    let mut row = 0;
    while dec < tuner.1 {
        let span = (dec, dec * 10.0);
        decade(ui, row, span, tuner, rm, sdr, script);
        dec *= 10.0;
        row += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn decade(
    ui: &mut egui::Ui,
    row: usize,
    (lo, hi): (f64, f64),
    tuner: (f64, f64),
    rm: &RefMap,
    sdr: &SdrState,
    script: &mut Script,
) {
    let placed = rm
        .plan()
        .map(|p| lanes(p.within(lo, hi)))
        .unwrap_or_default();
    let depth = placed
        .iter()
        .map(|(l, _)| l + 1)
        .max()
        .unwrap_or(1)
        .min(MAX_LANES);
    ui.label(egui::RichText::new(format!("{} – {}", si(lo), si(hi))).strong());
    let w = ui.available_width();
    let h = depth as f32 * LANE_H + 16.0;
    let (r, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let bars = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, r.max.y - 16.0));
    let x_of = |hz: f64| {
        let t = (hz.clamp(lo, hi).ln() - lo.ln()) / (hi.ln() - lo.ln());
        bars.min.x + t as f32 * bars.width()
    };
    let hz_at = |x: f32| {
        let t = ((x - bars.min.x) / bars.width()).clamp(0.0, 1.0) as f64;
        (lo.ln() + t * (hi.ln() - lo.ln())).exp()
    };
    let p = ui.painter_at(r);
    p.rect_filled(bars, 2.0, egui::Color32::from_rgb(18, 20, 26));
    // Out of the tuner's reach: hatched grey.
    for (a, b) in [(lo, tuner.0), (tuner.1, hi)] {
        if b > a {
            let dead = egui::Rect::from_min_max(
                egui::pos2(x_of(a), bars.min.y),
                egui::pos2(x_of(b), bars.max.y),
            );
            p.rect_filled(
                dead,
                0.0,
                egui::Color32::from_rgba_unmultiplied(60, 60, 66, 160),
            );
        }
    }
    let mut hits = Vec::new();
    for (i, (lane, b)) in placed.iter().enumerate() {
        let lane = (*lane).min(MAX_LANES - 1) as f32;
        let seg = egui::Rect::from_min_max(
            egui::pos2(x_of(b.lo_hz), bars.max.y - (lane + 1.0) * LANE_H),
            egui::pos2(
                x_of(b.hi_hz).max(x_of(b.lo_hz) + 2.0),
                bars.max.y - lane * LANE_H,
            ),
        )
        .shrink2(egui::vec2(0.0, 1.0));
        p.rect_filled(seg, 2.0, colour(&b.kind).gamma_multiply(0.6));
        p.rect_stroke(seg, 2.0, (1.0, colour(&b.kind)), egui::StrokeKind::Inside);
        let font = egui::FontId::proportional(11.0);
        let galley = p.layout_no_wrap(b.name.clone(), font, egui::Color32::WHITE);
        if galley.size().x + 6.0 <= seg.width() {
            p.galley(
                seg.center() - galley.size() / 2.0,
                galley,
                egui::Color32::WHITE,
            );
        }
        let label = format!("{} · {} · {}", b.name, fmt_range(b.lo_hz, b.hi_hz), b.kind);
        uitree::node(
            ui.ctx(),
            ui.id().with(("rfmap", row, i)),
            Role::Button,
            &label,
            seg,
        );
        hits.push((seg, *b));
    }
    // Ticks: 1, 2, 5 within the decade.
    for m in [1.0, 2.0, 3.0, 5.0, 7.0] {
        let hz = lo * m;
        let x = x_of(hz);
        p.line_segment(
            [egui::pos2(x, bars.max.y), egui::pos2(x, bars.max.y + 4.0)],
            (1.0, egui::Color32::GRAY),
        );
        p.text(
            egui::pos2(x + 2.0, bars.max.y + 2.0),
            egui::Align2::LEFT_TOP,
            si(hz),
            egui::FontId::proportional(10.0),
            egui::Color32::GRAY,
        );
    }
    // The IQ window and the tuned frequency, when this decade holds them.
    let half = sdr.config.sample_rate / 2.0;
    let (w0, w1) = (sdr.config.centre_hz - half, sdr.config.centre_hz + half);
    if w1 > lo && w0 < hi {
        let (x0, x1) = (x_of(w0), x_of(w1));
        let win = egui::Rect::from_center_size(
            egui::pos2((x0 + x1) / 2.0, bars.center().y),
            egui::vec2((x1 - x0).max(4.0), bars.height()),
        );
        p.rect_stroke(
            win,
            1.0,
            (1.5, egui::Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }
    if (lo..hi).contains(&sdr.tuned_hz) {
        let x = x_of(sdr.tuned_hz);
        p.line_segment(
            [egui::pos2(x, bars.min.y), egui::pos2(x, bars.max.y)],
            (2.0, CURSOR),
        );
        uitree::node(
            ui.ctx(),
            ui.id().with(("rfmap-tuned", row)),
            Role::Mark,
            &format!("tuned {}", fmt_mhz(sdr.tuned_hz)),
            egui::Rect::from_center_size(
                egui::pos2(x, bars.center().y),
                egui::vec2(2.0, bars.height()),
            ),
        );
    }

    let Some(pos) = resp.hover_pos() else { return };
    let hz = hz_at(pos.x);
    let under = hits
        .iter()
        .filter(|(s, _)| s.contains(pos))
        .min_by(|a, b| (a.1.hi_hz - a.1.lo_hz).total_cmp(&(b.1.hi_hz - b.1.lo_hz)))
        .map(|(_, b)| *b);
    let names: Vec<String> = rm
        .at(hz)
        .into_iter()
        .map(|b| format!("{} ({})", b.name, fmt_range(b.lo_hz, b.hi_hz)))
        .collect();
    let resp = resp.on_hover_text(format!(
        "{}\n{}",
        fmt_mhz(hz),
        if names.is_empty() {
            "no allocation".into()
        } else {
            names.join("\n")
        }
    ));
    if resp.double_clicked()
        && let Some(b) = under
    {
        script.inject(Action::RefMap(RefMapAction::Goto(b.name.clone())));
    } else if resp.clicked() && (tuner.0..=tuner.1).contains(&hz) {
        inject(script, SdrAction::Tune(round_hz(hz)));
    }
}
