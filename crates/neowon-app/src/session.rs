//! Session save/restore. A session file IS a `NEOWON_SCRIPT`: save emits
//! one script action per setting and restore replays them through the
//! script executor — human-readable, dependency-free, and it keeps the
//! "every control is script-reachable" invariant honest (anything the
//! emitter cannot express is a missing script action).
//!
//! Sessions capture instrument + display + analysis state, not window
//! arrangement (dock sections, sizes) — like setup files on real scopes.

use std::fmt::Write;

use neowon_core::{AcqMode, Coupling, PulseCondition, Slope, Sweep, TriggerKind, VideoSync};
use neowon_dsp::{MathOp, Window};

use crate::Link;
use crate::cursors::CursorState;
use crate::derived::{FftState, MathState, MeasureState, PfState};
use crate::gpu::{Palette, Persistence, Phosphor, TraceMode};
use crate::viz::three_d::Viz3dState;
use crate::viz::waterfall::WaterfallState;

fn sweep_name(s: Sweep) -> &'static str {
    match s {
        Sweep::Auto => "auto",
        Sweep::Normal => "normal",
        Sweep::Single => "single",
    }
}

pub(crate) fn condition_words(c: PulseCondition) -> (&'static str, &'static str) {
    use PulseCondition::*;
    match c {
        PositiveGreater => ("pos", "gt"),
        PositiveEqual => ("pos", "eq"),
        PositiveLess => ("pos", "lt"),
        NegativeGreater => ("neg", "gt"),
        NegativeEqual => ("neg", "eq"),
        NegativeLess => ("neg", "lt"),
    }
}

/// Emit the current state as script text.
#[allow(clippy::too_many_arguments)]
pub fn emit(
    ap: &crate::autopeak::AutoPeak,
    deep: &crate::deep::DeepView,
    link: &Link,
    phosphor: &Phosphor,
    math: &MathState,
    meas: &MeasureState,
    fft: &FftState,
    cur: &CursorState,
    pf: &PfState,
    wf: &WaterfallState,
    viz: &Viz3dState,
    fx: &crate::effects::Effects,
) -> String {
    let mut s = String::from("# neowon session\n");
    let w = &mut s;
    let c = &link.config;

    if !link.stimulus.is_empty() {
        let _ = writeln!(w, "stimulus {}", link.stimulus);
    }
    let _ = writeln!(w, "rate {}", c.sample_rate);
    let _ = writeln!(w, "trigpos {}", c.position);
    // The user's selection, never the mode auto peak engaged — otherwise a
    // session saved at a slow time base would restore stuck in Peak.
    let acq = match ap.user_acq {
        AcqMode::Sample => "sample",
        AcqMode::Peak => "peak",
        AcqMode::Average(4) => "avg4",
        AcqMode::Average(64) => "avg64",
        AcqMode::Average(_) => "avg16",
    };
    let _ = writeln!(w, "acq {acq}");
    let _ = writeln!(w, "autopeak {}", if ap.on { "on" } else { "off" });

    for (ch, cc) in c.channels.iter().enumerate().take(2) {
        let _ = writeln!(w, "enable {ch} {}", cc.enabled as u8);
        let _ = writeln!(w, "vdiv {ch} {}", cc.volts_div);
        let coup = match cc.coupling {
            Coupling::Dc => "dc",
            Coupling::Ac => "ac",
            Coupling::Gnd => "gnd",
        };
        let _ = writeln!(w, "coupling {ch} {coup}");
        let _ = writeln!(w, "probe {ch} {}", cc.probe);
        let _ = writeln!(w, "offset {ch} {}", cc.offset);
    }
    let _ = writeln!(w, "select {}", link.selected);

    let t = &c.trigger;
    let sweep = sweep_name(t.sweep);
    match t.kind {
        TriggerKind::Edge { slope } => {
            let slope = match slope {
                Slope::Rising => "rising",
                Slope::Falling => "falling",
            };
            let _ = writeln!(w, "trigger {} {slope} {} {sweep}", t.source, t.level);
        }
        TriggerKind::Pulse { condition, width } => {
            let (pol, cmp) = condition_words(condition);
            // The script grammar takes widths in µs.
            let _ = writeln!(
                w,
                "trigpulse {} {pol} {cmp} {} {sweep}",
                t.source,
                width * 1e6
            );
        }
        TriggerKind::Slope {
            condition,
            width,
            upper,
            lower,
        } => {
            let (pol, cmp) = condition_words(condition);
            let _ = writeln!(
                w,
                "trigslope {} {pol} {cmp} {} {upper} {lower} {sweep}",
                t.source,
                width * 1e6
            );
        }
        TriggerKind::Video { sync, line } => {
            let sync = match sync {
                VideoSync::Line => "line",
                VideoSync::Field => "field",
                VideoSync::OddField => "odd",
                VideoSync::EvenField => "even",
                VideoSync::LineNumber => "linenum",
            };
            let _ = writeln!(w, "trigvideo {sync} {line} {sweep}");
        }
    }
    let _ = writeln!(w, "holdoff {}", t.holdoff);

    let mode = match phosphor.mode {
        TraceMode::Vectors => "vectors",
        TraceMode::Dots => "dots",
        TraceMode::Xy => "xy",
    };
    let _ = writeln!(w, "mode {mode}");
    match phosphor.persistence {
        Persistence::Off => {
            let _ = writeln!(w, "persist off");
        }
        Persistence::Infinite => {
            let _ = writeln!(w, "persist inf");
        }
        Persistence::Seconds(secs) => {
            let _ = writeln!(w, "persist {secs}");
        }
    }
    let _ = writeln!(w, "gain {}", phosphor.gain);
    let _ = writeln!(w, "crt {}", phosphor.crt as u8);
    let palette = match phosphor.palette {
        Palette::Phosphor => "phosphor",
        Palette::Thermal => "thermal",
        Palette::Green => "green",
    };
    let _ = writeln!(w, "palette {palette}");
    let _ = writeln!(w, "hview {} {}", phosphor.hview.0, phosphor.hview.1);
    // After hview on purpose: `deep on` clears the zoom window, so emitting
    // it first would make the session fail to round-trip.
    let _ = writeln!(w, "deepspan {}", deep.span);
    let _ = writeln!(w, "deep {}", if deep.on { "on" } else { "off" });

    let math_op = if !math.enabled {
        "off"
    } else {
        match math.op {
            MathOp::Add => "add",
            MathOp::Sub => "sub",
            MathOp::Mul => "mul",
            MathOp::Div => "div",
            MathOp::Diff => "diff",
            MathOp::Integ => "integ",
        }
    };
    let _ = writeln!(w, "math {math_op}");

    let _ = writeln!(w, "fft {}", if fft.enabled { "on" } else { "off" });
    let _ = writeln!(w, "fftsrc {}", fft.source);
    let wnd = match fft.window {
        Window::Rectangle => "rectangle",
        Window::Hamming => "hamming",
        Window::Hann => "hann",
        Window::Blackman => "blackman",
        Window::Flattop => "flattop",
        Window::Triangular => "triangular",
    };
    let _ = writeln!(w, "fftwnd {wnd}");
    let _ = writeln!(w, "waterfall {}", if wf.on { "on" } else { "off" });
    let _ = writeln!(w, "viz {}", viz.mode.name());
    let _ = writeln!(w, "effect {}", fx.active.as_deref().unwrap_or("off"));

    let _ = writeln!(w, "cursor time {}", if cur.time_on { "on" } else { "off" });
    let _ = writeln!(w, "cursor amp {}", if cur.amp_on { "on" } else { "off" });
    let _ = writeln!(w, "markers {}", cur.markers as u8);
    let _ = writeln!(w, "guides {}", meas.guides as u8);
    let _ = writeln!(w, "stats {}", meas.stats_slot);

    let _ = writeln!(w, "pf {}", if pf.enabled { "on" } else { "off" });
    let _ = writeln!(w, "pfsrc {}", pf.source_slot);
    let _ = writeln!(w, "pftol {} {}", pf.h_div, pf.v_div);

    let _ = writeln!(w, "run {}", c.running as u8);
    s
}
