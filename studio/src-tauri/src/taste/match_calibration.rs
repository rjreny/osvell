//! C2 displayed Match %: monotonic calibration of Content Fit_v1 → taste-match score.
//!
//! Not claimed like-probability unless a future calibration proves it.
//! Higher raw fit must never produce a lower displayed Match.

use serde::{Deserialize, Serialize};

/// Provisional curve until live eligibility calibration freezes empirical bins.
/// Linear map into a constrained recommendation range; replaced after holdout fit.
const PROVISIONAL_MIN: u8 = 52;
const PROVISIONAL_MAX: u8 = 88;

/// Frozen piecewise (fit_01 → match %). From live C1 holdout isotonic bins
/// (support≈336). Higher raw fit never lowers Match.
pub const MATCH_CURVE_V1: &[(f32, u8)] = &[
    (0.00, 52),
    (0.40, 58),
    (0.475, 65),
    (0.575, 67),
    (0.625, 74),
    (0.675, 91),
    (0.75, 93),
    (1.00, 95),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCalibrationReport {
    pub bins: Vec<MatchBin>,
    pub curve: Vec<(f32, u8)>,
    pub display_min: u8,
    pub display_max: u8,
    pub support: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchBin {
    pub fit_lo: f32,
    pub fit_hi: f32,
    pub center: f32,
    pub mean_outcome: f32,
    pub n: usize,
    pub isotonic_outcome: f32,
}

/// Map rating bucket → taste-match outcome in 0..=1 (not a probability claim).
pub fn bucket_outcome(bucket: &str) -> f32 {
    match bucket {
        "loved" => 1.0,
        "liked" => 0.75,
        "meh" => 0.45,
        "disliked" => 0.15,
        _ => 0.45,
    }
}

/// Piecewise-linear interpolation over a non-decreasing curve.
pub fn fit_to_match_percent(raw_fit_01: f32) -> u8 {
    let fit = raw_fit_01.clamp(0.0, 1.0);
    let curve = MATCH_CURVE_V1;
    if curve.is_empty() {
        return provisional_match(fit);
    }
    if fit <= curve[0].0 {
        return curve[0].1;
    }
    for w in curve.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if fit <= x1 {
            if (x1 - x0).abs() < 1e-9 {
                return y1;
            }
            let t = (fit - x0) / (x1 - x0);
            let y = y0 as f32 + t * (y1 as f32 - y0 as f32);
            return y.round().clamp(0.0, 100.0) as u8;
        }
    }
    curve.last().map(|(_, y)| *y).unwrap_or(provisional_match(fit))
}

fn provisional_match(fit: f32) -> u8 {
    let span = (PROVISIONAL_MAX - PROVISIONAL_MIN) as f32;
    (PROVISIONAL_MIN as f32 + fit * span)
        .round()
        .clamp(0.0, 100.0) as u8
}

/// Bucket pairs and apply pool-adjacent-violators (isotonic) so outcomes are non-decreasing in fit.
pub fn fit_isotonic_bins(pairs: &[(f32, f32)], bin_width: f32) -> MatchCalibrationReport {
    let width = bin_width.max(0.01);
    let n_bins = ((1.0 / width).ceil() as usize).max(1);
    let mut sums = vec![0.0f32; n_bins];
    let mut counts = vec![0usize; n_bins];
    for &(fit, outcome) in pairs {
        let f = fit.clamp(0.0, 1.0);
        let mut idx = (f / width).floor() as usize;
        if idx >= n_bins {
            idx = n_bins - 1;
        }
        sums[idx] += outcome.clamp(0.0, 1.0);
        counts[idx] += 1;
    }

    let mut bins: Vec<MatchBin> = Vec::new();
    let mut raw_means: Vec<(usize, f32, f32, usize)> = Vec::new(); // idx, center, mean, n
    for i in 0..n_bins {
        if counts[i] == 0 {
            continue;
        }
        let lo = i as f32 * width;
        let hi = ((i + 1) as f32 * width).min(1.0);
        let center = 0.5 * (lo + hi);
        let mean = sums[i] / counts[i] as f32;
        raw_means.push((i, center, mean, counts[i]));
        bins.push(MatchBin {
            fit_lo: lo,
            fit_hi: hi,
            center,
            mean_outcome: mean,
            n: counts[i],
            isotonic_outcome: mean,
        });
    }

    // PAVA on occupied bins only.
    let means: Vec<f32> = raw_means.iter().map(|r| r.2).collect();
    let weights: Vec<f32> = raw_means.iter().map(|r| r.3 as f32).collect();
    let iso = pool_adjacent_violators(&means, &weights);
    for (bin, &v) in bins.iter_mut().zip(iso.iter()) {
        bin.isotonic_outcome = v;
    }

    let mut curve: Vec<(f32, u8)> = bins
        .iter()
        .map(|b| {
            let pct = (40.0 + b.isotonic_outcome * 55.0)
                .round()
                .clamp(35.0, 95.0) as u8;
            (b.center, pct)
        })
        .collect();
    enforce_curve_monotonic(&mut curve);

    let (display_min, display_max) = if curve.is_empty() {
        (PROVISIONAL_MIN, PROVISIONAL_MAX)
    } else {
        let min = curve.iter().map(|(_, y)| *y).min().unwrap_or(PROVISIONAL_MIN);
        let max = curve.iter().map(|(_, y)| *y).max().unwrap_or(PROVISIONAL_MAX);
        (min, max)
    };

    MatchCalibrationReport {
        support: pairs.len(),
        bins,
        curve,
        display_min,
        display_max,
    }
}

fn pool_adjacent_violators(values: &[f32], weights: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }
    // Block: (sum_w * mean, weight, count)
    let mut blocks: Vec<(f32, f32, usize)> = Vec::new();
    for (&v, &w) in values.iter().zip(weights.iter()) {
        let w = w.max(1e-6);
        blocks.push((v * w, w, 1));
        while blocks.len() >= 2 {
            let n = blocks.len();
            let (s1, w1, c1) = blocks[n - 2];
            let (s2, w2, c2) = blocks[n - 1];
            let m1 = s1 / w1;
            let m2 = s2 / w2;
            if m1 <= m2 + 1e-9 {
                break;
            }
            blocks.pop();
            blocks.pop();
            blocks.push((s1 + s2, w1 + w2, c1 + c2));
        }
    }
    let mut out = Vec::with_capacity(values.len());
    for (sum, w, count) in blocks {
        let mean = sum / w;
        for _ in 0..count {
            out.push(mean);
        }
    }
    out
}

fn enforce_curve_monotonic(curve: &mut [(f32, u8)]) {
    if curve.is_empty() {
        return;
    }
    curve.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut last = curve[0].1;
    for entry in curve.iter_mut().skip(1) {
        if entry.1 < last {
            entry.1 = last;
        }
        last = entry.1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_curve_is_monotonic() {
        let mut prev = 0u8;
        for i in 0..=100 {
            let fit = i as f32 / 100.0;
            let m = fit_to_match_percent(fit);
            assert!(
                m >= prev,
                "fit {fit:.2} match {m} < prev {prev}"
            );
            prev = m;
        }
    }

    #[test]
    fn isotonic_enforces_non_decreasing_outcomes() {
        // Deliberately noisy: higher bin has lower raw mean.
        let pairs = vec![
            (0.30, 0.9),
            (0.32, 0.85),
            (0.50, 0.2),
            (0.52, 0.25),
            (0.70, 0.8),
            (0.72, 0.9),
        ];
        let report = fit_isotonic_bins(&pairs, 0.10);
        let mut prev = 0.0f32;
        for b in &report.bins {
            assert!(
                b.isotonic_outcome + 1e-4 >= prev,
                "isotonic decreased at {:.2}",
                b.center
            );
            prev = b.isotonic_outcome;
        }
        let mut prev_m = 0u8;
        for &(x, y) in &report.curve {
            assert!(y >= prev_m, "curve decreased at {x}");
            prev_m = y;
        }
    }

    #[test]
    fn higher_fit_never_lower_match() {
        for a in 0..50 {
            for b in (a + 1)..51 {
                let fa = a as f32 / 50.0;
                let fb = b as f32 / 50.0;
                assert!(fit_to_match_percent(fb) >= fit_to_match_percent(fa));
            }
        }
    }
}
