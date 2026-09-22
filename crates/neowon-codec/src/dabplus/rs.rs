//! Reed–Solomon RS(120,110,t=5) over GF(2^8) — ETSI TS 102 563 V2.1.1
//! clause 6.1.
//!
//! The clause pins three things:
//!
//! * the field: GF(2^8) with α = 2 and primitive polynomial
//!   `P(x) = x^8 + x^4 + x^3 + x^2 + 1` (= 0x11D);
//! * the code: a systematic shortened RS(120,110) derived from
//!   RS(255,245), generator `G(x) = ∏_{i=0}^{9} (x + α^i)`, correcting
//!   `t = 5` random byte errors;
//! * the shortening: 135 zero bytes are placed before the information
//!   at the input of the RS(255,245) encoder and discarded afterwards.
//!   Prepending zeros leaves the remainder of `m(x)·x^10 mod G(x)`
//!   unchanged, so the parity of a 110-byte block is computed directly
//!   (see [`Rs120_110::encode`]).
//!
//! The decoder is the standard algebraic decoder for that code
//! (syndromes → Berlekamp–Massey for the errors-and-erasures locator →
//! Chien search → magnitude solve → syndrome re-check). The clause
//! prescribes the code, not a decoding algorithm.

/// `P(x) = x^8 + x^4 + x^3 + x^2 + 1` — TS 102 563 clause 6.1.
pub const PRIMITIVE_POLY: u16 = 0x011D;

/// Codeword length N — TS 102 563 clause 6.1.
pub const RS_N: usize = 120;
/// Information length K — TS 102 563 clause 6.1.
pub const RS_K: usize = 110;
/// Parity bytes N − K — TS 102 563 clause 6.1.
pub const RS_PARITY: usize = 10;
/// Correctable random byte errors `t = 5` — TS 102 563 clause 6.1.
pub const RS_T: usize = 5;

/// Why an RS(120,110) decode failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsError {
    /// More erasure positions than the 2t = 10 parity bytes can span.
    TooManyErasures(usize),
    /// An erasure index was outside 0..120 or listed twice.
    BadErasure(usize),
    /// The received word is not a correctable codeword.
    Uncorrectable,
}

/// A Reed–Solomon RS(120,110,t=5) codec: the GF(2^8) tables and the
/// generator polynomial are built once at construction.
#[derive(Debug, Clone)]
pub struct Rs120_110 {
    exp: [u8; 256],
    log: [u8; 256],
    /// `generator[j]` is the coefficient of `x^j` in `G(x)`, so
    /// `generator[10] == 1`.
    generator: [u8; RS_PARITY + 1],
}

impl Default for Rs120_110 {
    fn default() -> Self {
        Self::new()
    }
}

impl Rs120_110 {
    /// Build the field tables and generator polynomial.
    #[must_use]
    pub fn new() -> Self {
        let mut exp = [0u8; 256];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        let mut i = 0usize;
        // α = 2, reduced by P(x) whenever it overflows 8 bits (clause 6.1).
        while i < 255 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= PRIMITIVE_POLY;
            }
            i += 1;
        }
        exp[255] = exp[0];

        let mut codec = Self {
            exp,
            log,
            generator: [0u8; RS_PARITY + 1],
        };
        // G(x) = ∏_{i=0}^{9} (x + α^i), monic of degree 10 (clause 6.1).
        let mut g = [0u8; RS_PARITY + 1];
        g[0] = 1;
        for root in 0..RS_PARITY {
            let a = codec.exp[root];
            let mut next = [0u8; RS_PARITY + 1];
            for j in 0..=root {
                next[j + 1] ^= g[j];
                next[j] ^= codec.mul(g[j], a);
            }
            g = next;
        }
        codec.generator = g;
        codec
    }

    /// GF(2^8) multiply.
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[(usize::from(self.log[usize::from(a)])
                + usize::from(self.log[usize::from(b)]))
                % 255]
        }
    }

    /// GF(2^8) divide; `b` must be non-zero.
    fn div(&self, a: u8, b: u8) -> u8 {
        debug_assert!(b != 0);
        if a == 0 {
            0
        } else {
            let d = (255 + usize::from(self.log[usize::from(a)])
                - usize::from(self.log[usize::from(b)]))
                % 255;
            self.exp[d]
        }
    }

    /// GF(2^8) inverse of a non-zero element.
    fn inv(&self, a: u8) -> u8 {
        debug_assert!(a != 0);
        self.exp[255 - usize::from(self.log[usize::from(a)])]
    }

    /// α^p (reduced modulo 255).
    fn pow_alpha(&self, p: usize) -> u8 {
        self.exp[p % 255]
    }

    /// Systematic parity for one 110-byte information block: the 10
    /// coefficients of `m(x)·x^10 mod G(x)` in transmitted order
    /// (`x^9` first — TS 102 563 clause 6.1/6.3).
    #[must_use]
    pub fn encode(&self, data: &[u8; RS_K]) -> [u8; RS_PARITY] {
        // Synthetic division through m(x)·x^10 (the ten appended zeros).
        let mut rem = [0u8; RS_PARITY];
        for &d in data.iter().chain(&[0u8; RS_PARITY]) {
            let top = rem[0];
            rem.copy_within(1..RS_PARITY, 0);
            rem[RS_PARITY - 1] = d;
            if top != 0 {
                for (i, r) in rem.iter_mut().enumerate() {
                    *r ^= self.mul(top, self.generator[RS_PARITY - 1 - i]);
                }
            }
        }
        rem
    }

    /// Correct a received codeword in place.
    ///
    /// `erasures` lists byte positions (0 = first transmitted) known to
    /// be unreliable; the code can absorb up to 10 of them when there
    /// are no errors, or generally any pattern with `2·errors +
    /// erasures <= 10`. Returns the number of byte positions whose
    /// value changed.
    pub fn decode(&self, word: &mut [u8; RS_N], erasures: &[usize]) -> Result<usize, RsError> {
        if erasures.len() > RS_PARITY {
            return Err(RsError::TooManyErasures(erasures.len()));
        }
        let mut seen = [false; RS_N];
        for &e in erasures {
            if e >= RS_N || seen[e] {
                return Err(RsError::BadErasure(e));
            }
            seen[e] = true;
        }

        let syndromes = self.syndromes(word);
        if syndromes.iter().all(|&s| s == 0) {
            return Ok(0);
        }

        let gamma = self.erasure_locator(erasures);
        // Modified syndromes T = Γ(x)·S(x) mod x^10. Only indices
        // ≥ v are pure error-only syndromes (the first v are
        // contaminated by the erasure magnitudes), so Berlekamp–Massey
        // runs on T[v..10] with 2·errors ≤ 10 − v.
        let v = erasures.len();
        let mut modified = [0u8; RS_PARITY];
        for j in v..RS_PARITY {
            let mut acc = 0u8;
            for (k, g) in gamma.iter().enumerate().take(j + 1) {
                acc ^= self.mul(*g, syndromes[j - k]);
            }
            modified[j] = acc;
        }
        let sigma = self.berlekamp_massey(&modified[v..]);

        let lambda = self.multiply(&gamma, &sigma);
        let positions = self.chieh_search(&lambda);
        if positions.len() != lambda.len() - 1 || positions.is_empty() {
            return Err(RsError::Uncorrectable);
        }

        let magnitudes = self.solve_magnitudes(&positions, &syndromes)?;
        let mut changed = 0usize;
        for (&idx, &mag) in positions.iter().zip(&magnitudes) {
            if mag != 0 {
                changed += 1;
            }
            word[idx] ^= mag;
        }
        if self.syndromes(word).iter().any(|&s| s != 0) {
            return Err(RsError::Uncorrectable);
        }
        Ok(changed)
    }

    /// `S_j = r(α^j)` for `j = 0..10`, evaluating the codeword
    /// `Σ word[i]·x^(119−i)` by Horner (MSb/first byte highest degree).
    fn syndromes(&self, word: &[u8; RS_N]) -> [u8; RS_PARITY] {
        let mut s = [0u8; RS_PARITY];
        for (j, out) in s.iter_mut().enumerate() {
            let a = self.exp[j];
            let mut acc = 0u8;
            for &b in word {
                acc = self.mul(acc, a) ^ b;
            }
            *out = acc;
        }
        s
    }

    /// `Γ(x) = ∏ (1 − X·x)` over the erasure locators
    /// `X = α^(119−position)`; `Γ[0] == 1`.
    fn erasure_locator(&self, erasures: &[usize]) -> Vec<u8> {
        let mut gamma = vec![1u8];
        for &pos in erasures {
            let x = self.pow_alpha(RS_N - 1 - pos);
            let mut next = vec![0u8; gamma.len() + 1];
            for (i, &g) in gamma.iter().enumerate() {
                next[i] ^= g;
                next[i + 1] ^= self.mul(g, x);
            }
            gamma = next;
        }
        gamma
    }

    /// Classic Berlekamp–Massey over the (modified) syndrome sequence.
    fn berlekamp_massey(&self, sequence: &[u8]) -> Vec<u8> {
        let mut c = vec![1u8];
        let mut b = vec![1u8];
        let mut l = 0usize;
        let mut shift = 1usize;
        let mut b_scale = 1u8;
        for n in 0..sequence.len() {
            let mut discrepancy = sequence[n];
            for i in 1..=l {
                if i < c.len() {
                    discrepancy ^= self.mul(c[i], sequence[n - i]);
                }
            }
            if discrepancy == 0 {
                shift += 1;
                continue;
            }
            let previous = c.clone();
            let coef = self.div(discrepancy, b_scale);
            if c.len() < b.len() + shift {
                c.resize(b.len() + shift, 0);
            }
            for (i, &bi) in b.iter().enumerate() {
                c[i + shift] ^= self.mul(coef, bi);
            }
            if 2 * l <= n {
                l = n + 1 - l;
                b = previous;
                b_scale = discrepancy;
                shift = 1;
            } else {
                shift += 1;
            }
        }
        c.truncate(l + 1);
        c
    }

    /// Polynomial product over GF(2^8).
    fn multiply(&self, a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; a.len() + b.len() - 1];
        for (i, &ai) in a.iter().enumerate() {
            for (j, &bj) in b.iter().enumerate() {
                out[i + j] ^= self.mul(ai, bj);
            }
        }
        out
    }

    /// Roots of `Λ`: byte positions `i` with `Λ(α^(i−119)) == 0`, found
    /// by evaluating at every valid locator (the code fills 120 of the
    /// field's 255 locators).
    fn chieh_search(&self, lambda: &[u8]) -> Vec<usize> {
        let mut positions = Vec::new();
        for p in 0..RS_N {
            let x_inv = self.pow_alpha(255 - p % 255);
            let mut acc = 0u8;
            for &l in lambda.iter().rev() {
                acc = self.mul(acc, x_inv) ^ l;
            }
            if acc == 0 {
                positions.push(RS_N - 1 - p);
            }
        }
        positions.sort_unstable();
        positions
    }

    /// Solve `S_j = Σ_l e_l·X_l^j` (`j = 0..f`) for the error
    /// magnitudes, by Gauss–Jordan on the Vandermonde system.
    fn solve_magnitudes(
        &self,
        positions: &[usize],
        syndromes: &[u8; RS_PARITY],
    ) -> Result<Vec<u8>, RsError> {
        let f = positions.len();
        let locators: Vec<u8> = positions
            .iter()
            .map(|&pos| self.pow_alpha(RS_N - 1 - pos))
            .collect();
        let mut powers = vec![1u8; f];
        let mut m = vec![vec![0u8; f + 1]; f];
        for (row, row_values) in m.iter_mut().enumerate() {
            row_values[..f].copy_from_slice(&powers);
            row_values[f] = syndromes[row];
            for (power, &x) in powers.iter_mut().zip(&locators) {
                *power = self.mul(*power, x);
            }
        }
        for col in 0..f {
            let pivot = (col..f)
                .find(|&r| m[r][col] != 0)
                .ok_or(RsError::Uncorrectable)?;
            m.swap(col, pivot);
            let scale = self.inv(m[col][col]);
            for value in &mut m[col][col..=f] {
                *value = self.mul(*value, scale);
            }
            let pivot_row = m[col].clone();
            for (r, row) in m.iter_mut().enumerate() {
                if r == col || row[col] == 0 {
                    continue;
                }
                let factor = row[col];
                for (value, pivot) in row[col..=f].iter_mut().zip(&pivot_row[col..=f]) {
                    *value ^= self.mul(factor, *pivot);
                }
            }
        }
        Ok((0..f).map(|i| m[i][f]).collect())
    }
}
