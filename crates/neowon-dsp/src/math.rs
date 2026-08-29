//! Math channels: derived traces computed in volts, re-quantized to the
//! shared i8 screen encoding so every consumer (renderer, measurements,
//! cursors, FFT) treats them like any other channel.

use neowon_core::ChannelCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
    /// d/dt of the first operand.
    Diff,
    /// Running integral of the first operand.
    Integ,
}

impl MathOp {
    pub const ALL: [MathOp; 6] = [
        MathOp::Add,
        MathOp::Sub,
        MathOp::Mul,
        MathOp::Div,
        MathOp::Diff,
        MathOp::Integ,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            MathOp::Add => "A + B",
            MathOp::Sub => "A − B",
            MathOp::Mul => "A × B",
            MathOp::Div => "A ÷ B",
            MathOp::Diff => "d/dt A",
            MathOp::Integ => "∫ A dt",
        }
    }

    pub fn needs_b(&self) -> bool {
        matches!(self, MathOp::Add | MathOp::Sub | MathOp::Mul | MathOp::Div)
    }

    /// Result unit for display: V, V·V, V/s, V·s...
    pub fn unit(&self) -> &'static str {
        match self {
            MathOp::Add | MathOp::Sub | MathOp::Div => "V",
            MathOp::Mul => "V²",
            MathOp::Diff => "V/s",
            MathOp::Integ => "V·s",
        }
    }
}

/// Compute the math trace in result units (volts etc.).
fn compute(a: &ChannelCapture, b: Option<&ChannelCapture>, op: MathOp, rate: f64) -> Vec<f64> {
    let n = a.raw.len();
    let av = |i: usize| a.volts_at(i);
    match op {
        MathOp::Add | MathOp::Sub | MathOp::Mul | MathOp::Div => {
            let Some(b) = b else { return vec![0.0; n] };
            let n = n.min(b.raw.len());
            (0..n)
                .map(|i| {
                    let (x, y) = (av(i), b.volts_at(i));
                    match op {
                        MathOp::Add => x + y,
                        MathOp::Sub => x - y,
                        MathOp::Mul => x * y,
                        MathOp::Div => {
                            if y.abs() < 1e-9 {
                                0.0
                            } else {
                                x / y
                            }
                        }
                        _ => unreachable!(),
                    }
                })
                .collect()
        }
        MathOp::Diff => {
            // Central difference, forward/backward at the ends.
            (0..n)
                .map(|i| match i {
                    0 => (av(1) - av(0)) * rate,
                    i if i == n - 1 => (av(i) - av(i - 1)) * rate,
                    i => (av(i + 1) - av(i - 1)) * rate * 0.5,
                })
                .collect()
        }
        MathOp::Integ => {
            let dt = 1.0 / rate;
            let mut acc = 0.0;
            (0..n)
                .map(|i| {
                    acc += av(i) * dt;
                    acc
                })
                .collect()
        }
    }
}

/// Build a math `ChannelCapture` (ch = 2). `full_scale` is the value spanned
/// by the 10 vertical divisions; pass `None` to auto-scale to the data.
/// Returns the capture and the full scale actually used.
pub fn math_trace(
    a: &ChannelCapture,
    b: Option<&ChannelCapture>,
    op: MathOp,
    sample_rate: f64,
    full_scale: Option<f64>,
) -> (ChannelCapture, f64) {
    let values = compute(a, b, op, sample_rate);
    let fs = full_scale.unwrap_or_else(|| {
        let peak = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        // Snap to a 1-2-5 ladder covering the peak with ~20% headroom.
        let target = (peak * 2.4).max(1e-6);
        let decade = 10f64.powf(target.log10().floor());
        [1.0, 2.0, 5.0, 10.0]
            .iter()
            .map(|m| m * decade)
            .find(|&fs| fs >= target)
            .unwrap_or(10.0 * decade)
    });
    let lsb = fs / 250.0;
    let mut clipped = false;
    let raw = values
        .iter()
        .map(|&v| {
            let q = (v / lsb).round();
            let c = q.clamp(-125.0, 125.0);
            if c != q {
                clipped = true;
            }
            c as i8
        })
        .collect();
    (
        ChannelCapture {
            ch: 2,
            raw,
            volts_per_lsb: lsb,
            zero_volts: 0.0,
            clipped,
            freq_meter: None,
        },
        fs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(raw: Vec<i8>, lsb: f64) -> ChannelCapture {
        ChannelCapture {
            ch: 0,
            raw,
            volts_per_lsb: lsb,
            zero_volts: 0.0,
            clipped: false,
            freq_meter: None,
        }
    }

    #[test]
    fn add_and_autoscale() {
        let a = cap(vec![50; 100], 0.01); // 0.5 V
        let b = cap(vec![100; 100], 0.01); // 1.0 V
        let (m, fs) = math_trace(&a, Some(&b), MathOp::Add, 1000.0, None);
        // 1.5 V on the chosen scale.
        let v = m.volts_at(0);
        assert!((v - 1.5).abs() < fs / 100.0, "got {v}, fs {fs}");
        assert!(!m.clipped);
        assert_eq!(m.ch, 2);
    }

    #[test]
    fn diff_of_ramp_is_constant() {
        // Ramp 1 LSB/sample at 0.01 V/LSB and 1 kS/s -> 10 V/s.
        let a = cap((0..100).map(|i| i as i8).collect(), 0.01);
        let (m, _fs) = math_trace(&a, None, MathOp::Diff, 1000.0, Some(50.0));
        let mid = m.volts_at(50);
        assert!((mid - 10.0).abs() < 0.5, "d/dt {mid}");
    }

    #[test]
    fn integral_of_constant_is_ramp() {
        let a = cap(vec![100; 1000], 0.01); // 1 V constant, 1 kS/s
        let (m, _) = math_trace(&a, None, MathOp::Integ, 1000.0, Some(4.0));
        let end = m.volts_at(999);
        assert!((end - 1.0).abs() < 0.05, "integral end {end}");
    }
}
