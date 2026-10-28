//! Summaries of samples, and a bootstrap for telling a real change from noise.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
    /// Median absolute deviation (robust spread).
    pub mad: f64,
    /// Coefficient of variation, stddev / mean (0.05 = 5%).
    pub cv: f64,
}

/// Percentile by linear interpolation on sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = p * (sorted.len() - 1) as f64;
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
}

pub fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    percentile(&v, 0.5)
}

pub fn summarize(values: &[f64]) -> Summary {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return Summary::default();
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = if n > 1 { v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64 } else { 0.0 };
    let p50 = percentile(&v, 0.5);
    let mut dev: Vec<f64> = v.iter().map(|x| (x - p50).abs()).collect();
    dev.sort_by(f64::total_cmp);
    Summary {
        n,
        min: v[0],
        p10: percentile(&v, 0.1),
        p50,
        p90: percentile(&v, 0.9),
        p99: percentile(&v, 0.99),
        max: v[n - 1],
        mean,
        stddev: var.sqrt(),
        mad: percentile(&dev, 0.5),
        cv: if mean.abs() > 0.0 { var.sqrt() / mean.abs() } else { 0.0 },
    }
}

/// Deterministic xorshift for resampling.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The ratio of medians `new / base`, with a 95% bootstrap confidence interval:
/// (low, ratio, high). `None` without enough samples on either side.
pub fn ratio_ci(base: &[f64], new: &[f64]) -> Option<(f64, f64, f64)> {
    if base.len() < 3 || new.len() < 3 {
        return None;
    }
    let (mb, mn) = (median(base), median(new));
    if mb == 0.0 {
        return None;
    }
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut ratios = Vec::with_capacity(2000);
    let (mut rb, mut rn) = (vec![0.0; base.len()], vec![0.0; new.len()]);
    for _ in 0..2000 {
        for x in rb.iter_mut() {
            *x = base[rng.below(base.len())];
        }
        for x in rn.iter_mut() {
            *x = new[rng.below(new.len())];
        }
        let b = median(&rb);
        if b > 0.0 {
            ratios.push(median(&rn) / b);
        }
    }
    ratios.sort_by(f64::total_cmp);
    Some((percentile(&ratios, 0.025), mn / mb, percentile(&ratios, 0.975)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_of_known_values() {
        let s = summarize(&[1.0, 2.0, 3.0, 4.0, 100.0]);
        assert_eq!((s.n, s.min, s.p50, s.max), (5, 1.0, 3.0, 100.0));
        assert_eq!(s.mad, 1.0);
    }

    #[test]
    fn bootstrap_sees_a_real_change_and_not_noise() {
        let base: Vec<f64> = (0..40).map(|i| 100.0 + (i % 5) as f64).collect();
        let same: Vec<f64> = (0..40).map(|i| 100.0 + ((i + 2) % 5) as f64).collect();
        let slower: Vec<f64> = base.iter().map(|x| x * 1.3).collect();
        let (lo, _, hi) = ratio_ci(&base, &same).unwrap();
        assert!(lo <= 1.0 && hi >= 1.0, "{lo} {hi}");
        let (lo, r, _) = ratio_ci(&base, &slower).unwrap();
        assert!(lo > 1.2 && (r - 1.3).abs() < 0.01);
    }
}
