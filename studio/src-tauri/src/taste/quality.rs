//! Milestone B4 Quality prior — Bayesian/shrunk TMDB reception.
//!
//! `vote_count` only controls shrinkage confidence in `vote_average`.
//! It must never become an independent popularity boost.
//! Live Fit_v1 keeps Quality contribution at 0 until a variant earns inclusion.

use crate::storage::db::Database;
use crate::taste::retrieve::Candidate;
use serde::Serialize;
use std::collections::HashMap;

/// Prior strength (pseudo-counts). Higher → stronger pull toward population mean.
const PRIOR_M: f32 = 800.0;
/// Typical TMDB movie mean used when catalog is empty.
const FALLBACK_POP_MEAN: f32 = 6.5;
/// Scale: ~2 TMDB points from mean → |prior| ≈ 1 before clamp.
const CENTER_SCALE: f32 = 2.0;
const QUALITY_BOUND: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityConfig {
    pub positive: bool,
    pub negative: bool,
    pub lambda: f32,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self::off()
    }
}

impl QualityConfig {
    pub fn off() -> Self {
        Self {
            positive: false,
            negative: false,
            lambda: 0.0,
        }
    }

    pub fn positive_only() -> Self {
        Self {
            positive: true,
            negative: false,
            lambda: 1.0,
        }
    }

    pub fn negative_only() -> Self {
        Self {
            positive: false,
            negative: true,
            lambda: 1.0,
        }
    }

    pub fn full() -> Self {
        Self {
            positive: true,
            negative: true,
            lambda: 1.0,
        }
    }

    pub fn any_enabled(&self) -> bool {
        (self.positive || self.negative) && self.lambda.abs() > 1e-8
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityStats {
    pub vote_average: f32,
    pub vote_count: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCatalog {
    pub by_tmdb: HashMap<i64, QualityStats>,
    pub population_mean: f32,
    pub prior_m: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityResult {
    pub score: f32,
    pub confidence: f32,
    pub support: f32,
    pub vote_average: Option<f32>,
    pub vote_count: Option<i64>,
    pub shrunk_quality: Option<f32>,
    pub quality_prior: f32,
    pub fit_contribution: f32,
    pub missing: bool,
}

pub fn load_quality_catalog(db: &Database) -> Result<QualityCatalog, String> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT tmdb_id, vote_average, vote_count
             FROM movies
             WHERE tmdb_id IS NOT NULL
               AND vote_average IS NOT NULL
               AND vote_count IS NOT NULL
               AND vote_count > 0",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)? as f32,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut by_tmdb = HashMap::new();
    let mut sum = 0.0_f32;
    let mut wsum = 0.0_f32;
    for row in rows.flatten() {
        let (tid, avg, count) = row;
        if !(0.0..=10.0).contains(&avg) || count <= 0 {
            continue;
        }
        // Population mean: lightly weight by log votes so mega-hits don't own the prior.
        let w = (count as f32).ln().max(1.0);
        sum += avg * w;
        wsum += w;
        by_tmdb.insert(
            tid,
            QualityStats {
                vote_average: avg,
                vote_count: count,
            },
        );
    }
    let population_mean = if wsum > 1e-4 {
        sum / wsum
    } else {
        FALLBACK_POP_MEAN
    };
    Ok(QualityCatalog {
        by_tmdb,
        population_mean,
        prior_m: PRIOR_M,
    })
}

/// Bayesian shrink of vote_average toward population mean.
/// `vote_count` only sets the mixture weight — never an additive popularity term.
pub fn shrunk_quality(avg: f32, count: i64, pop_mean: f32, m: f32) -> f32 {
    let n = count.max(0) as f32;
    let mm = m.max(1.0);
    (n / (n + mm)) * avg + (mm / (n + mm)) * pop_mean
}

fn centered_prior(shrunk: f32, pop_mean: f32) -> f32 {
    ((shrunk - pop_mean) / CENTER_SCALE).clamp(-1.0, 1.0)
}

pub fn score_quality(
    catalog: &QualityCatalog,
    candidate: &Candidate,
    config: &QualityConfig,
) -> QualityResult {
    if !config.any_enabled() {
        return QualityResult::default();
    }
    let Some(tid) = candidate.tmdb_id else {
        return QualityResult {
            missing: true,
            ..Default::default()
        };
    };
    let Some(stats) = catalog.by_tmdb.get(&tid).copied() else {
        // vote_count alone without average → neutral (no popularity leakage).
        return QualityResult {
            vote_count: candidate.vote_count,
            missing: true,
            ..Default::default()
        };
    };
    let shrunk = shrunk_quality(
        stats.vote_average,
        stats.vote_count,
        catalog.population_mean,
        catalog.prior_m,
    );
    let mut prior = centered_prior(shrunk, catalog.population_mean);
    if !config.positive {
        prior = prior.min(0.0);
    }
    if !config.negative {
        prior = prior.max(0.0);
    }
    // Confidence from vote_count shrinkage weight only.
    let n = stats.vote_count.max(0) as f32;
    let conf_w = n / (n + catalog.prior_m);
    let fit_contribution =
        (prior * QUALITY_BOUND * config.lambda).clamp(-QUALITY_BOUND, QUALITY_BOUND);
    QualityResult {
        score: fit_contribution,
        confidence: conf_w.clamp(0.05, 0.95),
        support: conf_w,
        vote_average: Some(stats.vote_average),
        vote_count: Some(stats.vote_count),
        shrunk_quality: Some(shrunk),
        quality_prior: prior,
        fit_contribution,
        missing: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::MediaKind;

    fn cand(id: i64) -> Candidate {
        Candidate {
            tmdb_id: Some(id),
            title: "X".into(),
            year: Some(2020),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: Some(100),
            vote_count: None,
            watchlist: false,
            sources: vec![],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        }
    }

    fn catalog(entries: &[(i64, f32, i64)], pop: f32) -> QualityCatalog {
        let mut by_tmdb = HashMap::new();
        for &(id, avg, n) in entries {
            by_tmdb.insert(
                id,
                QualityStats {
                    vote_average: avg,
                    vote_count: n,
                },
            );
        }
        QualityCatalog {
            by_tmdb,
            population_mean: pop,
            prior_m: PRIOR_M,
        }
    }

    #[test]
    fn low_vote_high_average_is_weak_positive() {
        let cat = catalog(&[(1, 8.8, 60)], 6.5);
        let r = score_quality(&cat, &cand(1), &QualityConfig::positive_only());
        let strong = score_quality(
            &catalog(&[(2, 8.2, 18_000)], 6.5),
            &cand(2),
            &QualityConfig::positive_only(),
        );
        assert!(r.fit_contribution > 0.0);
        assert!(
            strong.fit_contribution > r.fit_contribution,
            "high-n acclaimed ({}) should beat thin acclaimed ({})",
            strong.fit_contribution,
            r.fit_contribution
        );
        assert!(r.fit_contribution <= QUALITY_BOUND + 1e-5);
    }

    #[test]
    fn high_vote_poor_average_is_negative() {
        let cat = catalog(&[(1, 6.2, 20_000)], 6.5);
        let r = score_quality(&cat, &cand(1), &QualityConfig::negative_only());
        assert!(r.fit_contribution < 0.0);
        assert!(r.fit_contribution >= -QUALITY_BOUND - 1e-5);
    }

    #[test]
    fn missing_votes_are_neutral() {
        let cat = catalog(&[], 6.5);
        let r = score_quality(&cat, &cand(99), &QualityConfig::full());
        assert!(r.missing);
        assert_eq!(r.fit_contribution, 0.0);
    }

    #[test]
    fn vote_count_alone_without_average_does_not_boost() {
        let cat = catalog(&[], 6.5);
        let mut c = cand(7);
        c.vote_count = Some(500_000);
        let r = score_quality(&cat, &c, &QualityConfig::full());
        assert_eq!(r.fit_contribution, 0.0);
        assert!(r.missing);
    }

    #[test]
    fn off_config_is_zero() {
        let cat = catalog(&[(1, 9.0, 50_000)], 6.5);
        let r = score_quality(&cat, &cand(1), &QualityConfig::off());
        assert_eq!(r.score, 0.0);
    }

    #[test]
    fn shrunk_formula_mixes_toward_population() {
        let s = shrunk_quality(10.0, 0, 6.5, 800.0);
        assert!((s - 6.5).abs() < 1e-5);
        let s2 = shrunk_quality(10.0, 800, 6.5, 800.0);
        assert!((s2 - 8.25).abs() < 1e-3);
    }
}
