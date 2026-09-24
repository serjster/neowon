//! The wheel gestures' decision logic: the sample-rate rung a zoomed span
//! calls for, the pan step, and the action list one gesture injects. Kept
//! out of the view so a unit test can drive exactly what the pointer
//! handler injects — the egui pointer/scroll path cannot be event-injected
//! by the current windowed rig.

use neowon_backend::SdrCaps;

use super::{SdrAction, SdrState};

/// An egui mouse-wheel line (one notch), in points: the native default of
/// `InputOptions::line_scroll_speed`. The view converts
/// `smooth_scroll_delta` with it, so one notch is one unit here.
pub const WHEEL_LINE_POINTS: f64 = 40.0;
/// Fraction of the visible span one shift+wheel notch pans.
pub const PAN_FRACTION: f64 = 0.1;
/// The captured band covers at least this many visible spans: the zoom
/// margin, `rate >= RATE_MARGIN * span`.
pub const RATE_MARGIN: f64 = 2.0;
/// Zoom-in floor as a fraction of the current rate; the ladder keeps it
/// reachable at every rung.
pub const MIN_SPAN_FRACTION: f64 = 1.0 / 256.0;

/// The rung of `rates` (ascending) that carries a visible `span_hz`: the
/// smallest rung with `r >= RATE_MARGIN * span`, never below the ladder's
/// smallest or above its largest. `None` when the instrument publishes no
/// rungs — the rate is then not ours to choose.
pub fn rate_for_span(rates: &[f64], span_hz: f64) -> Option<f64> {
    let smallest = *rates.first()?;
    let largest = *rates.last()?;
    debug_assert!(
        rates.windows(2).all(|w| w[0] <= w[1]),
        "rate ladder must ascend"
    );
    Some(
        rates
            .iter()
            .copied()
            .find(|&r| r >= span_hz * RATE_MARGIN)
            .unwrap_or(largest)
            .max(smallest),
    )
}

/// One plain-wheel zoom step (`zf` is the log2 span factor; negative zooms
/// in), anchored so the frequency under the pointer stays put. Returns the
/// actions in apply order — `Rate`, `Span`, `Pan`, and `Centre` when the
/// new band would drop the tuned channel — so the rate rung is chosen for
/// the new span before the pan clamps against it.
///
/// The rung follows the zoom; DAB pins the rate at 2.048 MS/s (the wheel
/// then zooms the display span only, and the DAB dock says so). When the
/// shrunken band can no longer hold the tuned frequency, the hardware
/// recentres on it, so the channel the operator monitors survives
/// the step.
pub fn zoom_actions(sdr: &SdrState, caps: Option<&SdrCaps>, t: f64, zf: f64) -> Vec<SdrAction> {
    let rate = sdr.config.sample_rate;
    let rates = caps.map(|c| c.sample_rates.as_slice()).unwrap_or_default();
    let pinned = sdr.dab.on();
    // DAB's ensemble is defined at exactly 2.048 MS/s: the display cannot
    // zoom past that band while the receiver is on.
    let max_span = if pinned {
        rate
    } else {
        rates.last().copied().unwrap_or(rate)
    };
    let span = (sdr.span() * 2f64.powf(zf)).clamp(rate * MIN_SPAN_FRACTION, max_span);
    // The frequency under the pointer, and the view centre that keeps it
    // there at the new span.
    let hz = sdr.view_centre() + t * sdr.span();
    let centre = hz - t * span;
    let rung = rate_for_span(rates, span).unwrap_or(rate);
    let new_rate = if pinned {
        rate
    } else if zf < 0.0 {
        // Zooming in never raises the rate: the initial full-span view has
        // no margin, so the rung alone would jump to the largest rate the
        // moment the operator zoomed in, capturing *more* band.
        rung.min(rate)
    } else {
        // Zooming out never lowers it: the band only grows with the view.
        rung.max(rate)
    };
    let mut out = Vec::new();
    if new_rate != rate {
        out.push(SdrAction::Rate(new_rate));
    }
    out.push(SdrAction::Span(if span >= new_rate { 0.0 } else { span }));
    out.push(SdrAction::Pan(centre - sdr.config.centre_hz));
    // The rate step must not drop the channel the operator monitors.
    // Only a window that held it before the step is recentred on it — a
    // tuned frequency an explicit `sdr centre` already left outside the
    // band is the operator's own move, not the zoom's to undo.
    let holds = |r: f64| (sdr.tuned_hz - sdr.config.centre_hz).abs() <= r * super::TUNE_REACH;
    if holds(rate) && !holds(new_rate) {
        out.push(SdrAction::Centre(sdr.tuned_hz));
    }
    out
}

/// Hz the view centre moves for `lines` wheel notches, positive = up the
/// spectrum (the content follows the scroll).
pub fn pan_step(span_hz: f64, lines: f64) -> f64 {
    lines * PAN_FRACTION * span_hz
}

/// One shift+wheel (or 2-D wheel x-axis) pan step: a display pan while the
/// view can stay inside the IQ band (the left-drag semantics, `sdr pan`),
/// else the hardware window moves with `sdr centre` — the right-drag code
/// path — so the scroll keeps going past the band edge instead of
/// stopping.
pub fn pan_actions(sdr: &SdrState, lines: f64) -> Vec<SdrAction> {
    let want = sdr.view_centre() + pan_step(sdr.span(), lines);
    let room = (sdr.config.sample_rate - sdr.span()).max(0.0) / 2.0;
    let pan = (want - sdr.config.centre_hz).clamp(-room, room);
    if pan == want - sdr.config.centre_hz {
        vec![SdrAction::Pan(pan)]
    } else {
        vec![SdrAction::Centre(want)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_backend::{Acquisition, SdrCaps};

    /// The dongle's ladder, from its one home — the same list the
    /// sim and the driver advertise.
    use neowon_core::ladders::RTL_SAMPLE_RATES as LADDER;
    const CENTRE: f64 = 100e6;

    fn caps(rates: Vec<f64>) -> SdrCaps {
        SdrCaps {
            name: "test".into(),
            serial: "test".into(),
            tuner: "sim".into(),
            freq_range_hz: (500e3, 1.766e9),
            sample_rates: rates,
            gains_db: vec![],
            acquisition: Acquisition::Stream { chunk: 1024 },
        }
    }

    fn state(rate: f64, span_hz: f64) -> SdrState {
        let mut s = SdrState::default();
        s.config.centre_hz = CENTRE;
        s.config.sample_rate = rate;
        s.tuned_hz = CENTRE;
        s.span_hz = span_hz;
        s
    }

    #[test]
    fn the_ladder_picks_the_smallest_covering_rung_with_margin() {
        // rate >= 2 x span, smallest first.
        assert_eq!(rate_for_span(&LADDER, 500e3), Some(1.024e6));
        assert_eq!(rate_for_span(&LADDER, 512e3), Some(1.024e6));
        assert_eq!(rate_for_span(&LADDER, 768e3), Some(1.536e6));
        assert_eq!(rate_for_span(&LADDER, 1.024e6), Some(2.048e6));
        assert_eq!(rate_for_span(&LADDER, 1.05e6), Some(2.16e6));
        assert_eq!(rate_for_span(&LADDER, 1.6e6), Some(3.2e6));
        // Clamped at the ends.
        assert_eq!(rate_for_span(&LADDER, 0.0), Some(250e3));
        assert_eq!(rate_for_span(&LADDER, 2.048e6), Some(3.2e6));
        // No ladder, no choice.
        assert_eq!(rate_for_span(&[], 1e6), None);
    }

    #[test]
    fn zooming_in_steps_the_rate_down() {
        let s = state(2.048e6, 0.0);
        // Half the span: 1.024 MS/s still covers it at the 2x margin, so
        // the rate holds at the next step (512 kHz -> 1.024 MS/s).
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -1.0),
            vec![SdrAction::Span(1.024e6), SdrAction::Pan(0.0)]
        );
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -2.0),
            vec![
                SdrAction::Rate(1.024e6),
                SdrAction::Span(512e3),
                SdrAction::Pan(0.0)
            ]
        );
        // A rate that does not change is not injected.
        let s = state(1.024e6, 512e3);
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -0.5),
            vec![
                SdrAction::Span(512e3 * 2f64.powf(-0.5)),
                SdrAction::Pan(0.0)
            ]
        );
        // The floor holds when the rung is the smallest.
        let s = state(250e3, 1000.0);
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -1.0),
            vec![SdrAction::Span(250e3 / 256.0), SdrAction::Pan(0.0)]
        );
    }

    #[test]
    fn a_zoom_in_never_raises_the_rate() {
        // The initial full-span view has no 2x margin; the rung alone
        // would jump to 3.2 MS/s the moment the operator zoomed in,
        // capturing *more* band than before.
        let s = state(2.048e6, 0.0);
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -1.0);
        assert!(!a.iter().any(|x| matches!(x, SdrAction::Rate(_))), "{a:?}");
    }

    #[test]
    fn a_zoom_out_never_lowers_the_rate() {
        // 3.2 MS/s is wider than a 1.2 MHz span needs, but zooming out
        // (1.2 -> 1.43 MHz) must not shrink the captured band.
        let s = state(3.2e6, 1.2e6);
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, 0.25);
        assert!(!a.iter().any(|x| matches!(x, SdrAction::Rate(_))), "{a:?}");
    }

    #[test]
    fn zooming_out_steps_the_rate_up() {
        let s = state(250e3, 200e3);
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, 2.0),
            vec![
                SdrAction::Rate(1.792e6),
                SdrAction::Span(800e3),
                SdrAction::Pan(0.0)
            ]
        );
        // At the widest rung the full-span encoding takes over.
        let s = state(2.88e6, 1.6e6);
        assert_eq!(
            zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, 1.0),
            vec![
                SdrAction::Rate(3.2e6),
                SdrAction::Span(0.0),
                SdrAction::Pan(0.0)
            ]
        );
    }

    #[test]
    fn a_zoom_keeps_the_frequency_under_the_pointer() {
        let s = state(2.048e6, 0.0);
        // Pointer a quarter to the right of centre: 100.512 MHz.
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.25, -2.0);
        assert_eq!(a[0], SdrAction::Rate(1.024e6));
        assert_eq!(a[1], SdrAction::Span(512e3));
        // The view centre moves so 100.512 MHz stays under the pointer:
        // 100.512 MHz - 0.25 x 512 kHz = 100.384 MHz -> pan 384 kHz.
        assert_eq!(a[2], SdrAction::Pan(384e3));
    }

    #[test]
    fn dab_pins_the_rate_and_the_span_still_zooms() {
        let mut s = state(2.048e6, 500e3);
        s.dab.rx = Some(neowon_dsp::dab::DabReceiver::new());
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -2.0);
        assert!(
            !a.iter().any(|x| matches!(x, SdrAction::Rate(_))),
            "DAB pins the rate: {a:?}"
        );
        assert_eq!(a[0], SdrAction::Span(125e3));
        // Zooming out cannot leave the ensemble band either.
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, 4.0);
        assert_eq!(a[0], SdrAction::Span(0.0));
    }

    #[test]
    fn a_shrunk_band_recentres_the_hardware_on_the_tuned_frequency() {
        // Tuned 900 kHz off centre: inside the 2.048 MS/s band's 45% reach
        // (921.6 kHz), outside the 1.024 MS/s reach (460.8 kHz).
        let mut s = state(2.048e6, 0.0);
        s.tuned_hz = CENTRE + 900e3;
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -2.0);
        assert_eq!(a.last(), Some(&SdrAction::Centre(CENTRE + 900e3)));
        // A channel the new band still holds stays where it is.
        let mut s = state(2.048e6, 0.0);
        s.tuned_hz = CENTRE + 400e3;
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -2.0);
        assert!(!a.iter().any(|x| matches!(x, SdrAction::Centre(_))));
        // One an explicit window move already left outside is not yanked
        // back by a zoom.
        let mut s = state(2.048e6, 0.0);
        s.tuned_hz = CENTRE + 1.2e6;
        let a = zoom_actions(&s, Some(&caps(LADDER.to_vec())), 0.0, -2.0);
        assert!(!a.iter().any(|x| matches!(x, SdrAction::Centre(_))));
    }

    #[test]
    fn pan_steps_are_a_tenth_of_the_span_per_notch() {
        assert_eq!(pan_step(500e3, 1.0), 50e3);
        assert_eq!(pan_step(500e3, -1.0), -50e3);
        assert_eq!(pan_step(500e3, 2.5), 125e3);
    }

    #[test]
    fn a_pan_inside_the_band_stays_a_display_pan() {
        let s = state(2.048e6, 500e3);
        assert_eq!(pan_actions(&s, 1.0), vec![SdrAction::Pan(50e3)]);
        assert_eq!(pan_actions(&s, -2.0), vec![SdrAction::Pan(-100e3)]);
    }

    #[test]
    fn a_pan_past_the_band_edge_moves_the_hardware() {
        // The view sits at the band edge (room = (2.048 - 0.5)/2 = 774 kHz).
        let mut s = state(2.048e6, 500e3);
        s.pan_hz = 774e3;
        assert_eq!(
            pan_actions(&s, 1.0),
            vec![SdrAction::Centre(CENTRE + 824e3)]
        );
        // At full span there is no display room at all: every notch moves
        // the hardware window.
        let s = state(2.048e6, 0.0);
        assert_eq!(
            pan_actions(&s, 1.0),
            vec![SdrAction::Centre(CENTRE + 204.8e3)]
        );
    }
}
