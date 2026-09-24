//! What the SDR canvas says about itself: the one-line gesture hint and the
//! tag on the DC notch. The notch's dip sits exactly under the tuned cursor,
//! where a DAB block pick puts the decoded channel, so untagged it looks
//! like a property of the signal.

use bevy_egui::egui;

use super::sdr_view::x_at;
use crate::sdr::{DC_GUARD, SdrState};
use crate::uitree;

/// The full gesture map, shown on hover of `Tuned (MHz)`.
pub const GESTURES: &str = "click: tune\n\
    drag a channel edge: width\n\
    drag: pan (vertical: reference level)\n\
    right-drag: move the hardware window\n\
    scroll: span + sample rate · shift+scroll: pan\n\
    ctrl+scroll: dB range\n\
    double-click: reset the view";

/// The canvas hint, longest first: the first that fits the spectrum's
/// width is drawn, so a narrow window keeps the essentials rather than a
/// clipped sentence.
const HINTS: [&str; 3] = [
    "click tune · drag pan/level · scroll zoom · shift+scroll pan · ctrl+scroll dB · \
     right-drag window · double-click reset · hover Tuned (MHz) for all",
    "click tune · scroll zoom · shift+scroll pan · ctrl+scroll dB · double-click reset",
    "scroll zoom · shift pan · ctrl dB",
];

const HINT_COLOUR: egui::Color32 = egui::Color32::from_rgb(150, 160, 175);

/// The width of the display notch in Hz: `DC_GUARD` bins either side of
/// DC plus the DC bin (`sdr::display::mask_dc`).
#[must_use]
pub fn notch_hz(sdr: &SdrState) -> Option<f64> {
    let s = sdr.spectrum.as_ref()?;
    Some((2 * DC_GUARD + 1) as f64 * s.bin_hz)
}

#[must_use]
pub fn notch_text(sdr: &SdrState) -> String {
    match notch_hz(sdr) {
        // Short: the grid's second column is narrow, and the hover says
        // where the notch sits.
        Some(hz) => format!("{:.1} kHz, display only", hz / 1e3),
        None => "-".into(),
    }
}

pub fn notch_row(ui: &mut egui::Ui, sdr: &SdrState) {
    ui.label("DC notch");
    ui.monospace(notch_text(sdr)).on_hover_text(
        "at the hardware centre (DC): the RTL's zero-IF spike is masked in \
         the trace and waterfall only; the demodulator, detector, DAB \
         receiver and recorder see the raw IQ",
    );
    ui.end_row();
}

/// Paint the gesture hint in the waterfall's bottom-left corner: the
/// spectrum's top line carries the span/peak readout and its bottom the
/// frequency ticks, so the waterfall's corner is the free edge of the
/// canvas.
pub fn hint(ui: &egui::Ui, wf: egui::Rect) {
    let font = egui::FontId::proportional(11.0);
    let room = wf.width() - 12.0;
    let p = ui.painter();
    let Some(galley) = HINTS
        .iter()
        .map(|h| p.layout_no_wrap((*h).to_string(), font.clone(), HINT_COLOUR))
        .find(|g| g.size().x <= room)
    else {
        return;
    };
    let pos = egui::pos2(wf.min.x + 6.0, wf.max.y - 4.0 - galley.size().y);
    let rect = egui::Rect::from_min_size(pos, galley.size());
    let text = galley.text().to_string();
    p.rect_filled(rect.expand(2.0), 2.0, egui::Color32::from_black_alpha(140));
    p.galley(pos, galley, HINT_COLOUR);
    uitree::node(
        ui.ctx(),
        ui.id().with("sdr-gesture-hint"),
        egui::accesskit::Role::Label,
        &format!("gesture hint {text}"),
        rect,
    );
}

/// Tag the hardware centre, where the display notch sits, when it is in
/// view. The line the spectrum draws there (when the cursor is elsewhere)
/// and the dip in the trace are both explained by it.
pub fn dc_notch(ui: &egui::Ui, spec: egui::Rect, sdr: &SdrState) {
    let Some(hz) = notch_hz(sdr) else { return };
    let x = x_at(sdr, spec, sdr.config.centre_hz);
    if !(spec.min.x..=spec.max.x).contains(&x) {
        return;
    }
    let label = "hw centre · DC notch";
    let p = ui.painter();
    let galley = p.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(10.0),
        HINT_COLOUR,
    );
    // Just above the frequency tick labels, clamped inside the spectrum.
    let w = galley.size().x;
    let left = (x - w / 2.0).clamp(spec.min.x + 2.0, spec.max.x - w - 2.0);
    let pos = egui::pos2(left, spec.max.y - 30.0);
    let rect = egui::Rect::from_min_size(pos, galley.size());
    p.rect_filled(rect.expand(1.0), 2.0, egui::Color32::from_black_alpha(140));
    p.galley(pos, galley, HINT_COLOUR);
    uitree::node(
        ui.ctx(),
        ui.id().with("sdr-dc-notch"),
        egui::accesskit::Role::Label,
        &format!("dc notch {:.1} kHz display only", hz / 1e3),
        rect,
    );
}
