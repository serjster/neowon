//! Display side of the SDR spectrum: the DC-notch mask and the bin → column
//! mapping the trace and the waterfall both draw from.
//!
//! The RTL2832U's zero-IF front end leaves a DC offset spike at the tuned
//! centre — LO leakage and IQ imbalance. It is a receiver artefact, not a
//! signal, and it is not a measurement: the demodulator, detector, DAB
//! receiver, recorder and the `neowon-dsp` oracle all see the raw IQ or the
//! unmasked spectrum. Only the copy the displays draw is notched.
//!
//! The notch follows the detector's `DC_GUARD` rule — `guard` bins either
//! side of the spectrum's DC bin (`len() / 2`) — so the trace and the signal
//! list agree on where DC is. As a width in Hz that is
//! `(2 * guard + 1) * bin_hz`: with the default four bins, 4.5 kHz at
//! 4096 bins / 2.048 MS/s, 18 kHz at 1024 and 1.125 kHz at 16384. It lives
//! on the spectrum's bin grid, so a panned or zoomed view carries it with
//! the hardware centre, never with the screen.

use neowon_dsp::IqSpectrum;

/// Replace the `guard` bins either side of DC with a straight line between
/// the bins just outside the notch, in dB. The line is display makeup, not
/// an estimate of the signal: the bins it covers hold the receiver's DC
/// offset. Both the trace and the waterfall build their columns from the
/// result, so they can never disagree.
pub fn mask_dc(s: &IqSpectrum, guard: usize) -> IqSpectrum {
    let n = s.len();
    let dc = n / 2;
    let mut out = s.clone();
    if guard == 0 || n < 3 {
        return out;
    }
    let lo = dc.saturating_sub(guard);
    let hi = (dc + guard + 1).min(n);
    let (l, r) = match (lo.checked_sub(1), (hi < n).then_some(hi)) {
        (Some(a), Some(b)) => (s.power_db[a], s.power_db[b]),
        (Some(a), None) => (s.power_db[a], s.power_db[a]),
        (None, Some(b)) => (s.power_db[b], s.power_db[b]),
        (None, None) => return out,
    };
    let span = (hi - lo + 1) as f64;
    for (k, p) in out.power_db[lo..hi].iter_mut().enumerate() {
        *p = l + (r - l) * (k + 1) as f64 / span;
    }
    out
}

/// `cols` display columns across `span` Hz centred `pan` Hz from DC, each the peak of
/// the bins it covers (peak-preserving: a narrow carrier never vanishes
/// between columns).
pub fn columns(s: &IqSpectrum, pan: f64, span: f64, cols: usize) -> Vec<f64> {
    (0..cols)
        .map(|c| {
            let lo = pan + (c as f64 / cols as f64 - 0.5) * span;
            let hi = pan + ((c + 1) as f64 / cols as f64 - 0.5) * span;
            let (a, b) = (s.bin_of(lo), s.bin_of(hi));
            s.power_db[a.min(b)..=a.max(b)]
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect()
}

/// Waterfall intensity of `db`: 0 at `black`, 1 at `white` (the spectrum's
/// reference level); a floor above the reference still gets 20 dB of room.
pub fn wf_level(db: f64, black: f64, white: f64) -> f32 {
    let white = white.max(black + 20.0);
    ((db - black) / (white - black)).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use neowon_dsp::{Window, iq_spectrum};
    use neowon_sim::{IqComponent, IqScene};

    use crate::sdr::{DC_GUARD, WF_UNDER_FLOOR_DB, WF_W};

    use super::*;

    #[test]
    fn columns_keep_a_narrow_carrier() {
        // One 4096-bin spectrum squeezed into 1024 columns: the carrier's
        // bin must survive as its column's peak.
        let scene = IqScene {
            sample_rate: 2.048e6,
            components: vec![IqComponent::Tone {
                offset_hz: 300e3,
                amplitude: 0.5,
                phase: 0.0,
            }],
            noise_rms: 0.01,
        };
        let s = iq_spectrum(&scene.samples(1, 0, 8192), 2.048e6, Window::Hann, 4096).unwrap();
        let cols = columns(&s, 0.0, 2.048e6, WF_W);
        let (c, db) = cols
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!((db + 6.02).abs() < 0.1, "{db}");
        // Column c spans (c/1024 - 0.5) * 2.048 MHz: 300 kHz is column 662.
        assert_eq!(c, 662);
    }

    #[test]
    fn the_waterfall_floor_is_dark() {
        // Floor at -60 dBFS: the noise lands near black, a signal 10 dB
        // up is visibly lit, the reference level is white.
        let black = -60.0 - WF_UNDER_FLOOR_DB;
        assert!(wf_level(-60.0, black, 0.0) < 0.1);
        assert!(wf_level(-50.0, black, 0.0) > 0.2);
        assert_eq!(wf_level(0.0, black, 0.0), 1.0);
        assert_eq!(wf_level(-80.0, black, -90.0), 0.0);
    }

    /// One row of the waterfall must be blind to where the arriving frame
    /// chunk starts. A transfer boundary is not a property of the signal;
    /// were the row's frequency axis to depend on it, a static carrier
    /// would walk across the display row by row — and the diagonal band
    /// would be the display's, not the air's.
    #[test]
    fn a_row_is_blind_to_the_frame_start_phase() {
        // Off-bin tones so a fractional-bin dependence cannot hide.
        let scene = IqScene {
            sample_rate: 2.048e6,
            components: vec![
                IqComponent::Tone {
                    offset_hz: 100_037.0,
                    amplitude: 0.5,
                    phase: 0.0,
                },
                IqComponent::Tone {
                    offset_hz: -250_411.0,
                    amplitude: 0.3,
                    phase: 0.25,
                },
            ],
            noise_rms: 0.0,
        };
        let n = 4096;
        let span = scene.sample_rate;
        let spec = |start| {
            iq_spectrum(
                &scene.samples(1, start, 2 * n),
                scene.sample_rate,
                Window::Hann,
                n,
            )
            .unwrap()
        };
        let a = columns(&spec(0), 0.0, span, WF_W);
        let b = columns(&spec(1234), 0.0, span, WF_W);
        let peak = |v: &[f64]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(c, _)| c)
                .unwrap()
        };
        // Linear power: the two rows are the same signal read from a
        // different sample, so the residual is rounding (see the matching
        // test in `neowon_dsp::iq`), while a per-chunk axis error would move
        // a column by orders more.
        let close = |x: f64, y: f64| {
            let (px, py) = (10f64.powf(x / 10.0), 10f64.powf(y / 10.0));
            (px - py).abs() <= 1e-6 * (px + py) + 1e-12
        };
        for (c, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!(close(*x, *y), "column {c}: {x} dB vs {y} dB");
        }
        assert_eq!(peak(&a), peak(&b), "the carrier moved columns");
    }

    /// The notch is display-only makeup: with a DC spike louder than an
    /// off-centre tone, the DC column comes out at the floor while the
    /// tone's column is exactly what the raw spectrum said.
    #[test]
    fn the_dc_spike_is_notched_out_of_the_display_columns() {
        let n = 4096;
        let rate = 2.048e6;
        let dc = n / 2;
        let mut power_db = vec![-80.0; n];
        power_db[dc - 1..=dc + 1].fill(-20.0);
        power_db[dc + 800] = -30.0; // tone 400 kHz above centre
        let s = IqSpectrum {
            bin_hz: rate / n as f64,
            power_db,
            blocks: 1,
        };

        let tone_col = WF_W / 2 + 200; // +400 kHz of a 2.048 MHz span
        let raw = columns(&s, 0.0, rate, WF_W);
        // The fixture really carries the artefact: without a mask the DC
        // column is the spike and the tone's column is the tone.
        assert_eq!(raw[WF_W / 2], -20.0);
        assert_eq!(raw[tone_col], -30.0);

        let masked = mask_dc(&s, DC_GUARD);
        let cols = columns(&masked, 0.0, rate, WF_W);
        // The DC column and the notch bins either side sit at the floor...
        let notch = &cols[WF_W / 2 - 1..=WF_W / 2 + 1];
        assert!(
            notch.iter().all(|&db| db == -80.0),
            "notch columns {notch:?}"
        );
        // ...while the off-centre tone is untouched, bin for bin.
        assert_eq!(cols[tone_col], -30.0);

        // The notch rides the spectrum's bin grid, not the screen: on a
        // panned, zoomed view it masks the hardware centre where it now
        // lands, and nothing else.
        let (pan, span) = (100e3, rate / 4.0);
        let dc_col = ((0.0 - pan) / span + 0.5) * WF_W as f64;
        assert_eq!(dc_col, 312.0);
        let zoomed = columns(&masked, pan, span, WF_W);
        assert_eq!(zoomed[312], -80.0);
    }

    /// The guard is `DC_GUARD` bins on whatever FFT grid the display runs,
    /// so the notch's width in Hz scales with the bin width — and the tone
    /// and the DC column come out right at both sizes.
    #[test]
    fn the_notch_scales_with_the_fft_size() {
        let rate = 2.048e6;
        for n in [1024usize, 4096] {
            let scene = IqScene {
                sample_rate: rate,
                components: vec![
                    // A constant is the sim's DC offset: the RTL's spike.
                    IqComponent::Tone {
                        offset_hz: 0.0,
                        amplitude: 0.5,
                        phase: 0.0,
                    },
                    IqComponent::Tone {
                        offset_hz: 300e3,
                        amplitude: 0.3,
                        phase: 0.0,
                    },
                ],
                noise_rms: 0.05,
            };
            let raw = iq_spectrum(&scene.samples(1, 0, 4 * n), rate, Window::Hann, n).unwrap();
            let dc = n / 2;
            let tone_bin = dc + (300e3 / (rate / n as f64)).round() as usize;
            assert!(
                raw.power_db[dc] > raw.power_db[tone_bin],
                "fft {n}: the fixture's DC spike must be the strongest"
            );
            let m = mask_dc(&raw, DC_GUARD);
            // Every guard bin was replaced; the bins just outside were not.
            let replaced = (dc - DC_GUARD..=dc + DC_GUARD)
                .filter(|&k| m.power_db[k] != raw.power_db[k])
                .count();
            assert_eq!(replaced, 2 * DC_GUARD + 1, "fft {n}: guard width");
            assert!(
                m.power_db[dc] < raw.power_db[dc] - 20.0,
                "fft {n}: DC spike survived"
            );
            for k in [dc - DC_GUARD - 1, dc + DC_GUARD + 1] {
                assert_eq!(m.power_db[k], raw.power_db[k], "fft {n}: bin {k} moved");
            }
            assert_eq!(m.power_db[tone_bin], raw.power_db[tone_bin]);
            // The notch in Hz is (2·guard + 1) bins: 18 kHz at 1024, 4.5 kHz
            // at 4096. The columns must show the same: tone untouched, DC at
            // its local level, not the spike.
            let notch_hz = (2 * DC_GUARD + 1) as f64 * rate / n as f64;
            assert_eq!(notch_hz, if n == 1024 { 18e3 } else { 4.5e3 });
            let cols = columns(&m, 0.0, rate, WF_W);
            assert_eq!(cols[tone_bin * WF_W / n], raw.power_db[tone_bin]);
            let dc_col = dc * WF_W / n;
            let local = (1..=4)
                .flat_map(|d| [cols[dc_col - d], cols[dc_col + d]])
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                cols[dc_col] <= local + 0.5,
                "fft {n}: DC column {} is above its local level {local}",
                cols[dc_col]
            );
            assert!(
                cols[dc_col] < raw.power_db[dc] - 20.0,
                "fft {n}: DC column still shows the spike"
            );
        }
    }
}
