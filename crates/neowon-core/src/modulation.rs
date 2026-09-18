//! Digital modulation constellations shared by the simulator (which
//! transmits them) and the DSP modulation lab (which recovers them), so
//! both agree on every point and every bit label.
//!
//! Every constellation has unit mean energy and Gray labels (neighbours
//! differ in one bit). Points are exact in IEEE arithmetic (0, ±1, ±√½
//! and QAM levels over √10 / √42), so both sides compute identical values
//! on every platform.

use std::f64::consts::FRAC_1_SQRT_2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modulation {
    Bpsk,
    Qpsk,
    Psk8,
    Qam16,
    Qam64,
}

impl Modulation {
    pub const ALL: [Modulation; 5] = [
        Modulation::Bpsk,
        Modulation::Qpsk,
        Modulation::Psk8,
        Modulation::Qam16,
        Modulation::Qam64,
    ];

    pub fn bits_per_symbol(self) -> u32 {
        match self {
            Modulation::Bpsk => 1,
            Modulation::Qpsk => 2,
            Modulation::Psk8 => 3,
            Modulation::Qam16 => 4,
            Modulation::Qam64 => 6,
        }
    }

    pub fn order(self) -> usize {
        1 << self.bits_per_symbol()
    }

    pub fn label(self) -> &'static str {
        match self {
            Modulation::Bpsk => "BPSK",
            Modulation::Qpsk => "QPSK",
            Modulation::Psk8 => "8PSK",
            Modulation::Qam16 => "16QAM",
            Modulation::Qam64 => "64QAM",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.label().eq_ignore_ascii_case(s))
    }

    /// The point for bit label `bits` (low `bits_per_symbol` bits used).
    pub fn point(self, bits: u32) -> (f64, f64) {
        let gray_level = |g: u32, n: u32| -> f64 {
            // Invert the Gray code to the level index, then map index to
            // the odd level 2i - (n - 1).
            let mut i = g;
            let mut s = g >> 1;
            while s != 0 {
                i ^= s;
                s >>= 1;
            }
            (2 * i) as f64 - (n - 1) as f64
        };
        match self {
            Modulation::Bpsk => (if bits & 1 == 0 { 1.0 } else { -1.0 }, 0.0),
            Modulation::Qpsk => (
                if bits & 2 == 0 {
                    FRAC_1_SQRT_2
                } else {
                    -FRAC_1_SQRT_2
                },
                if bits & 1 == 0 {
                    FRAC_1_SQRT_2
                } else {
                    -FRAC_1_SQRT_2
                },
            ),
            Modulation::Psk8 => {
                // Gray label g sits at phase index k with gray(k) = g.
                let k = (0..8u32).find(|&k| k ^ (k >> 1) == bits & 7).unwrap_or(0);
                const R: f64 = FRAC_1_SQRT_2;
                [
                    (1.0, 0.0),
                    (R, R),
                    (0.0, 1.0),
                    (-R, R),
                    (-1.0, 0.0),
                    (-R, -R),
                    (0.0, -1.0),
                    (R, -R),
                ][k as usize]
            }
            Modulation::Qam16 => {
                let s = 1.0 / 10f64.sqrt();
                (
                    gray_level((bits >> 2) & 3, 4) * s,
                    gray_level(bits & 3, 4) * s,
                )
            }
            Modulation::Qam64 => {
                let s = 1.0 / 42f64.sqrt();
                (
                    gray_level((bits >> 3) & 7, 8) * s,
                    gray_level(bits & 7, 8) * s,
                )
            }
        }
    }

    /// All points, indexed by label.
    pub fn points(self) -> Vec<(f64, f64)> {
        (0..self.order() as u32).map(|b| self.point(b)).collect()
    }

    /// Nearest point's label (minimum Euclidean distance).
    pub fn slice(self, (i, q): (f64, f64)) -> u32 {
        (0..self.order() as u32)
            .min_by(|&a, &b| {
                let d = |l: u32| {
                    let (x, y) = self.point(l);
                    (x - i).powi(2) + (y - q).powi(2)
                };
                d(a).total_cmp(&d(b))
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_energy_and_gray_neighbours() {
        for m in Modulation::ALL {
            let pts = m.points();
            let e: f64 = pts.iter().map(|(i, q)| i * i + q * q).sum::<f64>() / pts.len() as f64;
            assert!((e - 1.0).abs() < 1e-12, "{m:?} energy {e}");
            // Every nearest neighbour differs in exactly one bit.
            let dmin = (0..pts.len())
                .flat_map(|a| (0..pts.len()).filter(move |&b| b != a).map(move |b| (a, b)))
                .map(|(a, b)| (pts[a].0 - pts[b].0).hypot(pts[a].1 - pts[b].1))
                .fold(f64::MAX, f64::min);
            for a in 0..pts.len() {
                for b in 0..pts.len() {
                    let d = (pts[a].0 - pts[b].0).hypot(pts[a].1 - pts[b].1);
                    if a != b && d < dmin + 1e-9 {
                        assert_eq!((a ^ b).count_ones(), 1, "{m:?}: {a:b} vs {b:b}");
                    }
                }
            }
            for l in 0..m.order() as u32 {
                assert_eq!(m.slice(m.point(l)), l);
            }
        }
    }
}
