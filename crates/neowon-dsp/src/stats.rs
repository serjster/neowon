//! Running statistics for measurement readouts (Welford's algorithm).

#[derive(Debug, Clone, Copy, Default)]
pub struct StatTrack {
    pub current: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    m2: f64,
    pub count: u64,
}

impl StatTrack {
    pub fn update(&mut self, v: f64) {
        self.current = v;
        if self.count == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.count += 1;
        let delta = v - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (v - self.mean);
    }

    pub fn std_dev(&self) -> f64 {
        if self.count < 2 { 0.0 } else { (self.m2 / (self.count - 1) as f64).sqrt() }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welford() {
        let mut s = StatTrack::default();
        for v in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            s.update(v);
        }
        assert_eq!(s.count, 8);
        assert!((s.mean - 5.0).abs() < 1e-12);
        assert!((s.std_dev() - 2.138089935).abs() < 1e-6);
        assert_eq!(s.min, 2.0);
        assert_eq!(s.max, 9.0);
        assert_eq!(s.current, 9.0);
    }
}
