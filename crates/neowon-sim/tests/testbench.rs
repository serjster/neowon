//! The virtual testbench: signal-verification integration tests that drive the
//! deterministic sim source through `neowon-dsp` and assert real-world
//! correctness of every operation. These are the correctness backbone for the
//! measurement, math, FFT, and trigger paths.
//!
//! Run with: `cargo test -p neowon-sim --test testbench`

use std::time::Duration;

use neowon_backend::{Backend, ChannelConfig, ScopeConfig, TriggerConfig};
use neowon_core::{ChannelCapture, Slope, Sweep, TriggerKind};
use neowon_dsp::{MathOp, Window, basic_stats, estimate_frequency, math_trace, measure, spectrum};
use neowon_sim::{Component, SAMPLES, Scenario, SignalSpec, SimBackend, SimSource};

/// Build a single-channel capture from a spec at `rate`, full-scale `range`.
fn cap(spec: SignalSpec, rate: f64, range: f64) -> ChannelCapture {
    let mut src = SimSource::new(rate, Scenario::PerChannel([spec, SignalSpec::default()]));
    src.set_range(0, range);
    src.next_frame().channels.into_iter().next().unwrap()
}

/// Capture of a named preset's first channel.
fn preset_cap(name: &str, rate: f64, range: f64) -> ChannelCapture {
    cap_from_scenario(Scenario::preset(name).unwrap(), rate, range)
}

fn cap_from_scenario(sc: Scenario, rate: f64, range: f64) -> ChannelCapture {
    let mut src = SimSource::new(rate, sc);
    src.set_range(0, range);
    src.next_frame().channels.into_iter().next().unwrap()
}

fn sine(freq: f64, amp: f64) -> SignalSpec {
    SignalSpec {
        components: vec![Component::Sine {
            freq,
            amp,
            phase: 0.0,
        }],
    }
}

fn index_of_peak(amps: &[f64], skip_around: Option<(usize, usize)>) -> usize {
    let mut best = 1;
    let mut best_v = f64::MIN;
    for (i, &a) in amps.iter().enumerate().skip(1) {
        if let Some((c, r)) = skip_around
            && (i as i64 - c as i64).unsigned_abs() <= r as u64
        {
            continue;
        }
        if a > best_v {
            best_v = a;
            best = i;
        }
    }
    best
}

// --- Amplitudes ----------------------------------------------------------

#[test]
fn sine_amplitudes() {
    let c = cap(sine(1000.0, 1.0), 250e3, 2.5);
    let s = basic_stats(&c).unwrap();
    assert!((s.vpp - 2.0).abs() < 0.04, "vpp {}", s.vpp);
    assert!(
        (s.vrms - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.015,
        "vrms {}",
        s.vrms
    );
}

#[test]
fn triangle_vrms() {
    let spec = SignalSpec {
        components: vec![Component::Triangle {
            freq: 1000.0,
            amp: 1.0,
            phase: 0.0,
        }],
    };
    let c = cap(spec, 250e3, 2.5);
    let s = basic_stats(&c).unwrap();
    let expect = 1.0 / 3f64.sqrt();
    assert!(
        (s.vrms - expect).abs() < 0.02,
        "vrms {} expect {}",
        s.vrms,
        expect
    );
}

#[test]
fn square_vrms() {
    let spec = SignalSpec {
        components: vec![Component::Square {
            freq: 1000.0,
            amp: 1.0,
            duty: 0.5,
            phase: 0.0,
        }],
    };
    let c = cap(spec, 250e3, 2.5);
    let s = basic_stats(&c).unwrap();
    assert!((s.vrms - 1.0).abs() < 0.02, "vrms {}", s.vrms);
}

// --- Frequency accuracy across decades -----------------------------------

#[test]
fn frequency_accuracy_across_decades() {
    for (freq, rate) in [(50.0, 12.5e3), (1000.0, 250e3), (25e3, 2.5e6)] {
        let c = cap(sine(freq, 1.0), rate, 2.5);
        let f = estimate_frequency(&c.raw, rate).expect("freq found");
        let err = (f - freq).abs() / freq;
        assert!(err < 0.002, "{freq} Hz: measured {f} (err {err})");
    }
}

// --- Duty cycle ladder ---------------------------------------------------

#[test]
fn duty_cycle_ladder() {
    for duty in [0.10, 0.25, 0.50, 0.75, 0.90] {
        let spec = SignalSpec {
            components: vec![Component::Square {
                freq: 1000.0,
                amp: 1.0,
                duty,
                phase: 0.0,
            }],
        };
        let c = cap(spec, 250e3, 2.5);
        let m = measure(&c, 250e3).unwrap();
        let d = m.pduty.expect("duty");
        assert!((d - duty).abs() < 0.015, "duty {duty}: measured {d}");
    }
}

// --- Trapezoid rise time ------------------------------------------------

#[test]
fn trapezoid_rise_time() {
    // 200 us linear edges; 10-90% of a linear edge is 0.8 * edge = 160 us.
    let c = preset_cap("trapezoid", 2.5e6, 2.5);
    let m = measure(&c, 2.5e6).unwrap();
    let rise = m.rise.expect("rise");
    assert!((rise - 160e-6).abs() < 16e-6, "rise {rise}");
}

// --- FFT: two-tone -------------------------------------------------------

#[test]
fn fft_two_tone() {
    // 409600 S/s over 4096 points => 10 ms window, 100 Hz/bin; both tones
    // land exactly on bins (10 and 35) so there is no leakage.
    let rate = 409_600.0;
    let c = preset_cap("two-tone", rate, 5.0);
    let s = spectrum(&c.raw, c.volts_per_lsb, rate, Window::Hann, SAMPLES).unwrap();
    assert!((s.bin_hz - 100.0).abs() < 1e-6, "bin {}", s.bin_hz);

    let p1 = index_of_peak(&s.amplitude, None);
    let f1 = p1 as f64 * s.bin_hz;
    assert!((f1 - 1000.0).abs() <= 2.0 * s.bin_hz, "first peak at {f1}");

    let p2 = index_of_peak(&s.amplitude, Some((p1, 3)));
    let f2 = p2 as f64 * s.bin_hz;
    assert!((f2 - 3500.0).abs() <= 2.0 * s.bin_hz, "second peak at {f2}");

    // Amplitude ratio 1.0/0.4 within 1 dB.
    let (a1, a2) = (s.amplitude[p1], s.amplitude[p2]);
    let db = 20.0 * (a1 / a2).log10();
    let expect = 20.0 * (1.0f64 / 0.4).log10();
    assert!((db - expect).abs() < 1.0, "ratio {db} dB expect {expect}");
}

// --- FFT: AM sidebands ---------------------------------------------------

#[test]
fn fft_am_sidebands() {
    // Same 100 Hz/bin grid: carrier at bin 100, sidebands at bins 95/105.
    let rate = 409_600.0;
    let c = preset_cap("am", rate, 5.0);
    let s = spectrum(&c.raw, c.volts_per_lsb, rate, Window::Hann, SAMPLES).unwrap();
    let pc = index_of_peak(&s.amplitude, None);
    let fc = pc as f64 * s.bin_hz;
    assert!((fc - 10_000.0).abs() <= 2.0 * s.bin_hz, "carrier at {fc}");
    let a_c = s.amplitude[pc];

    // Each sideband is depth/2 = 0.25 of the carrier, within 1.5 dB.
    for side_f in [9500.0, 10500.0] {
        let center = (side_f / s.bin_hz).round() as usize;
        let lo = center.saturating_sub(2);
        let hi = (center + 3).min(s.amplitude.len());
        let a = s.amplitude[lo..hi].iter().cloned().fold(0.0f64, f64::max);
        let db = 20.0 * (a / a_c).log10();
        let expect = 20.0 * 0.25f64.log10();
        assert!(
            (db - expect).abs() < 1.5,
            "{side_f} Hz sideband {db} dB expect {expect}"
        );
    }
}

// --- Math: derivative ----------------------------------------------------

#[test]
fn math_derivative_of_sine() {
    // d/dt of A sin(2*pi*f*t) has vpp = 2*A*2*pi*f. Rate ~30x the tone keeps
    // the central-difference gain error (~0.7%) and quantization noise small.
    let (f, a, rate) = (1000.0, 0.9, 30e3);
    let c = cap(sine(f, a), rate, 2.0);
    let (m, _fs) = math_trace(&c, None, MathOp::Diff, rate, None);
    let s = basic_stats(&m).unwrap();
    let expect = 2.0 * a * std::f64::consts::TAU * f;
    let err = (s.vpp - expect).abs() / expect;
    assert!(
        err < 0.05,
        "d/dt vpp {} expect {} (err {err})",
        s.vpp,
        expect
    );
}

// --- Math: integral ------------------------------------------------------

#[test]
fn math_integral_of_square_is_triangle() {
    // Integrating a +-A square of period T gives a triangle of vpp A*T/2.
    let (f, a, rate) = (1000.0, 1.0, 250e3);
    let spec = SignalSpec {
        components: vec![Component::Square {
            freq: f,
            amp: a,
            duty: 0.5,
            phase: 0.0,
        }],
    };
    let c = cap(spec, rate, 2.5);
    let (m, _fs) = math_trace(&c, None, MathOp::Integ, rate, None);
    let s = basic_stats(&m).unwrap();
    let expect = a * (1.0 / f) / 2.0;
    let err = (s.vpp - expect).abs() / expect;
    assert!(
        err < 0.05,
        "integral vpp {} expect {} (err {err})",
        s.vpp,
        expect
    );
}

// --- Chirp ---------------------------------------------------------------

#[test]
fn chirp_frequency_increases() {
    let c = preset_cap("chirp", 250e3, 2.5);
    let half = SAMPLES / 2;
    let f1 = estimate_frequency(&c.raw[..half], 250e3).expect("first half");
    let f2 = estimate_frequency(&c.raw[half..], 250e3).expect("second half");
    assert!(f2 > 2.0 * f1, "first {f1}, second {f2}");
}

// --- XY figures ----------------------------------------------------------

#[test]
fn lissajous_frequency_ratio() {
    let mut src = SimSource::new(250e3, Scenario::preset("xy-lissajous-3-2").unwrap());
    src.set_enabled(1, true);
    src.set_range(0, 5.0);
    src.set_range(1, 5.0);
    let frame = src.next_frame();
    assert_eq!(frame.channels.len(), 2, "XY needs both channels enabled");
    let f1 = estimate_frequency(&frame.channels[0].raw, 250e3).expect("ch1");
    let f2 = estimate_frequency(&frame.channels[1].raw, 250e3).expect("ch2");
    let ratio = f1 / f2;
    assert!((ratio - 1.5).abs() < 0.03, "ratio {ratio}");
}

#[test]
fn all_xy_presets_render_in_range() {
    for name in [
        "xy-circle",
        "xy-lissajous-3-2",
        "xy-rose-5",
        "xy-heart",
        "xy-butterfly",
    ] {
        let mut src = SimSource::new(250e3, Scenario::preset(name).unwrap());
        src.set_enabled(1, true);
        src.set_range(0, 5.0);
        src.set_range(1, 5.0);
        let frame = src.next_frame();
        assert_eq!(frame.channels.len(), 2, "{name}");
        for ch in &frame.channels {
            let s = basic_stats(ch).unwrap();
            assert!(s.vpp > 0.5, "{name}: vpp {} (figure collapsed?)", s.vpp);
            assert!(!ch.clipped, "{name}: clipped");
        }
    }
}

// --- Trigger alignment ---------------------------------------------------

fn armed_backend(stim: &str, level: f64, sweep: Sweep) -> SimBackend {
    let mut b = SimBackend::new();
    assert!(b.set_stimulus(stim).unwrap());
    let cfg = ScopeConfig {
        sample_rate: 250e3,
        channels: vec![
            ChannelConfig {
                enabled: true,
                volts_div: 1.0,
                ..Default::default()
            },
            ChannelConfig::default(),
        ],
        trigger: TriggerConfig {
            source: 0,
            kind: TriggerKind::Edge {
                slope: Slope::Rising,
            },
            level,
            sweep,
            holdoff: 100e-9,
        },
        position: 0.5,
        ..Default::default()
    };
    b.apply(&cfg).unwrap();
    b
}

#[test]
fn normal_sweep_aligns_five_frames() {
    let mut b = armed_backend("sine-1k", 0.0, Sweep::Normal);
    for n in 0..5 {
        let frame = b
            .poll_frame(Duration::from_millis(500))
            .unwrap()
            .expect("triggered");
        let raw = &frame.channels[0].raw;
        // Crossing placed at index 2500; 0 V = 0 counts on a 10 V range.
        assert!(
            raw[2500].unsigned_abs() <= 4,
            "frame {n}: raw[2500]={}",
            raw[2500]
        );
        assert!(raw[2510] > raw[2490], "frame {n}: not rising");
    }
}

#[test]
fn normal_sweep_starves_on_impossible_level() {
    let mut b = armed_backend("sine-1k", 10.0, Sweep::Normal);
    for _ in 0..5 {
        assert!(b.poll_frame(Duration::from_millis(500)).unwrap().is_none());
    }
}

// --- Determinism ---------------------------------------------------------

#[test]
fn identical_sources_produce_identical_frames() {
    for name in ["probe-comp", "two-tone", "xy-circle", "noise"] {
        let mut a = SimSource::new(250e3, Scenario::preset(name).unwrap());
        let mut b = SimSource::new(250e3, Scenario::preset(name).unwrap());
        a.set_range(0, 5.0);
        b.set_range(0, 5.0);
        let fa = a.next_frame();
        let fb = b.next_frame();
        assert_eq!(fa.channels.len(), fb.channels.len(), "{name}");
        for (ca, cb) in fa.channels.iter().zip(&fb.channels) {
            assert_eq!(ca.raw, cb.raw, "{name} ch{}", ca.ch);
        }
    }
}
