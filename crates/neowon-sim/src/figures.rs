//! XY figure generators. Each figure is a parametric curve (x(u), y(u))
//! normalized to [-1, 1]; the caller supplies u = 2*pi*freq*t and scales by
//! the desired amplitude. u runs continuously (figures with periods longer
//! than 2*pi, like the butterfly, need it).

use std::f64::consts::TAU;

/// A closed or repeating XY figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XyFigure {
    Circle,
    Lissajous { a: u32, b: u32, phase: f64 },
    Rose { k: u32 },
    Heart,
    Butterfly,
}

impl XyFigure {
    /// (x, y) in [-1, 1] at angle `u` (radians, continuous).
    pub fn sample(&self, u: f64) -> (f64, f64) {
        match self {
            XyFigure::Circle => (u.cos(), u.sin()),
            XyFigure::Lissajous { a, b, phase } => {
                ((*a as f64 * u + phase).sin(), (*b as f64 * u).sin())
            }
            XyFigure::Rose { k } => {
                let r = (*k as f64 * u).cos();
                (r * u.cos(), r * u.sin())
            }
            XyFigure::Heart => {
                // Classic parametric heart, scaled by 1/17 to fit the box.
                let x = 16.0 * u.sin().powi(3) / 17.0;
                let y = (13.0 * u.cos()
                    - 5.0 * (2.0 * u).cos()
                    - 2.0 * (3.0 * u).cos()
                    - (4.0 * u).cos())
                    / 17.0;
                (x, y)
            }
            XyFigure::Butterfly => {
                // Temple Fay's butterfly; period 24*pi in u.
                let f = u.cos().exp() - 2.0 * (4.0 * u).cos() + (u / 12.0).sin().powi(5);
                (f * u.sin() / 4.0, f * u.cos() / 4.0)
            }
        }
    }

    /// Angle period after which the figure closes (radians).
    pub fn period(&self) -> f64 {
        match self {
            XyFigure::Circle | XyFigure::Rose { .. } => TAU,
            XyFigure::Lissajous { .. } | XyFigure::Heart => TAU,
            XyFigure::Butterfly => 12.0 * TAU,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_is_unit() {
        for i in 0..64 {
            let u = i as f64 / 64.0 * TAU;
            let (x, y) = XyFigure::Circle.sample(u);
            assert!((x * x + y * y - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn all_figures_stay_in_box() {
        let figs = [
            XyFigure::Circle,
            XyFigure::Lissajous {
                a: 3,
                b: 2,
                phase: 1.0,
            },
            XyFigure::Rose { k: 5 },
            XyFigure::Heart,
            XyFigure::Butterfly,
        ];
        for f in figs {
            let mut u = 0.0;
            while u < f.period() {
                let (x, y) = f.sample(u);
                assert!(
                    x.abs() <= 1.0 + 1e-9 && y.abs() <= 1.0 + 1e-9,
                    "{f:?} at {u}"
                );
                u += 0.001;
            }
        }
    }

    #[test]
    fn lissajous_ratio_counts_lobes() {
        // x completes `a` cycles per 2*pi (phase offsets the crossings from
        // the u=0 boundary).
        let f = XyFigure::Lissajous {
            a: 3,
            b: 2,
            phase: 0.5,
        };
        let mut ups = 0;
        let mut prev = f.sample(0.0).0;
        for i in 1..=10_000 {
            let u = i as f64 / 10_000.0 * TAU;
            let x = f.sample(u).0;
            if prev < 0.0 && x >= 0.0 {
                ups += 1;
            }
            prev = x;
        }
        assert_eq!(ups, 3);
    }
}
