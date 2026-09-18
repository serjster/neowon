//! Higher-order cumulants of complex samples (M3). `C_pq` has `p` factors
//! of which `q` are conjugated; each is the joint cumulant, the sum over
//! set partitions of the factors of (−1)^(k−1)(k−1)! times the product of
//! the blocks' moments, normalised by C21^(p/2). Singleton blocks vanish
//! because the mean is removed first.
//!
//! Published normalised values (Swami & Sadler 2000), for reference. C40
//! and C41 rotate with the constellation (C40 by 4θ): the table's QPSK has
//! its points on the axes, but the shared Gray QPSK sits at 45°, where
//! C40 = −1. C42 and C63 do not depend on orientation.
//!
//! | | C20 | C40 | C41 | C42 | C63 |
//! |---|---|---|---|---|---|
//! | BPSK | 1 | −2 | −2 | −2 | 16 |
//! | QPSK | 0 | 1 | 0 | −1 | 4 |
//! | 8PSK | 0 | 0 | 0 | −1 | 4 |
//! | 16QAM | 0 | −0.68 | 0 | −0.68 | 2.08 |
//! | 64QAM | 0 | −0.619 | 0 | −0.619 | 1.797 |

use rustfft::num_complex::Complex64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cumulants {
    pub c20: Complex64,
    pub c21: f64,
    pub c40: Complex64,
    pub c41: Complex64,
    pub c42: f64,
    pub c63: f64,
}

/// Every set partition of `0..n` (as block lists).
fn partitions(n: usize) -> Vec<Vec<Vec<usize>>> {
    let mut out = vec![vec![]];
    for i in 0..n {
        let mut next = Vec::new();
        for p in &out {
            for b in 0..p.len() {
                let mut q: Vec<Vec<usize>> = p.clone();
                q[b].push(i);
                next.push(q);
            }
            let mut q = p.clone();
            q.push(vec![i]);
            next.push(q);
        }
        out = next;
    }
    out
}

/// Joint cumulant of `p` factors, the last `q` conjugated, from the moment
/// table `m[a][b] = E[x^a conj(x)^b]`.
fn cumulant(m: &[[Complex64; 7]; 7], p: usize, q: usize) -> Complex64 {
    let mut sum = Complex64::new(0.0, 0.0);
    for part in partitions(p) {
        if part.iter().any(|b| b.len() == 1) {
            continue; // zero mean
        }
        let k = part.len() as i32;
        let coef = (-1f64).powi(k - 1) * (1..k).map(f64::from).product::<f64>();
        let prod = part.iter().fold(Complex64::new(1.0, 0.0), |acc, b| {
            let conj = b.iter().filter(|&&i| i >= p - q).count();
            acc * m[b.len() - conj][conj]
        });
        sum += prod * coef;
    }
    sum
}

/// Normalised cumulants of `x` (interleaved I, Q), mean removed.
pub fn cumulants(iq: &[f32]) -> Option<Cumulants> {
    let n = iq.len() / 2;
    if n < 8 {
        return None;
    }
    let mean = iq.chunks_exact(2).fold(Complex64::new(0.0, 0.0), |a, p| {
        a + Complex64::new(p[0] as f64, p[1] as f64)
    }) / n as f64;
    let mut m = [[Complex64::new(0.0, 0.0); 7]; 7];
    for p in iq.chunks_exact(2) {
        let x = Complex64::new(p[0] as f64, p[1] as f64) - mean;
        let xc = x.conj();
        let mut pa = Complex64::new(1.0, 0.0);
        for (a, row) in m.iter_mut().enumerate() {
            let mut pb = pa;
            for cell in row.iter_mut().take(7 - a) {
                *cell += pb;
                pb *= xc;
            }
            pa *= x;
        }
    }
    for row in m.iter_mut() {
        for cell in row.iter_mut() {
            *cell /= n as f64;
        }
    }
    let c21 = m[1][1].re;
    if c21 <= 0.0 {
        return None;
    }
    let norm = |c: Complex64, p: i32| c / c21.powf(p as f64 / 2.0);
    Some(Cumulants {
        c20: norm(cumulant(&m, 2, 0), 2),
        c21: 1.0,
        c40: norm(cumulant(&m, 4, 0), 4),
        c41: norm(cumulant(&m, 4, 1), 4),
        c42: norm(cumulant(&m, 4, 2), 4).re,
        c63: norm(cumulant(&m, 6, 3), 6).re,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use neowon_core::Modulation;

    fn all_points(m: Modulation, reps: usize) -> Vec<f32> {
        m.points()
            .iter()
            .cycle()
            .take(m.order() * reps)
            .flat_map(|&(i, q)| [i as f32, q as f32])
            .collect()
    }

    #[test]
    fn partitions_of_six_number_203() {
        assert_eq!(partitions(6).len(), 203); // the Bell number B6
    }

    #[test]
    fn ideal_constellations_give_the_published_values() {
        let cases = [
            (Modulation::Bpsk, 1.0, -2.0, -2.0, 16.0),
            // At 45°: C40 = -1 (the table's +1 is the axis-aligned QPSK).
            (Modulation::Qpsk, 0.0, -1.0, -1.0, 4.0),
            (Modulation::Psk8, 0.0, 0.0, -1.0, 4.0),
            (Modulation::Qam16, 0.0, -0.68, -0.68, 2.08),
            (Modulation::Qam64, 0.0, -0.619, -0.619, 1.797),
        ];
        for (m, c20, c40, c42, c63) in cases {
            let c = cumulants(&all_points(m, 4)).unwrap();
            assert!((c.c20.norm() - c20).abs() < 1e-6, "{m:?} C20 {:?}", c.c20);
            assert!(
                (c.c40.re - c40).abs() < 1e-3 && c.c40.im.abs() < 1e-6,
                "{m:?} C40 {:?}",
                c.c40
            );
            assert!((c.c42 - c42).abs() < 1e-3, "{m:?} C42 {}", c.c42);
            assert!((c.c63 - c63).abs() < 1e-3, "{m:?} C63 {}", c.c63);
        }
    }
}
