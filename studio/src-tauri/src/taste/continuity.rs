//! Milestone B3 Continuity — collection/franchise only.
//!
//! Scoring uses the user's observed ratings of other collection members.
//! Collection retrieval provenance is never a taste signal.
//! Live Fit_v1 keeps Continuity contribution at 0 until a variant earns inclusion.

use crate::models::LibraryItem;
use crate::taste::retrieve::{Candidate, FilmRecord};
use serde::Serialize;
use std::collections::HashMap;

const SHRINK_K: f32 = 3.5;
/// Correlated franchise observations: each extra sibling adds less independent evidence.
const SIBLING_CORRELATION: f32 = 0.45;
const CONTINUITY_BOUND: f32 = 0.08;
/// Mild extra authority for consistent negatives (still bounded).
const NEGATIVE_AUTHORITY: f32 = 1.15;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityConfig {
    pub collection_positive: bool,
    pub collection_negative: bool,
    pub lambda: f32,
}

impl Default for ContinuityConfig {
    fn default() -> Self {
        Self::off()
    }
}

impl ContinuityConfig {
    pub fn off() -> Self {
        Self {
            collection_positive: false,
            collection_negative: false,
            lambda: 0.0,
        }
    }

    pub fn collection_positive_only() -> Self {
        Self {
            collection_positive: true,
            collection_negative: false,
            lambda: 1.0,
        }
    }

    pub fn collection_negative_only() -> Self {
        Self {
            collection_positive: false,
            collection_negative: true,
            lambda: 1.0,
        }
    }

    pub fn full() -> Self {
        Self {
            collection_positive: true,
            collection_negative: true,
            lambda: 1.0,
        }
    }

    pub fn any_enabled(&self) -> bool {
        (self.collection_positive || self.collection_negative) && self.lambda.abs() > 1e-8
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiblingObs {
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub rating: f32,
    pub positive: bool,
    pub negative: bool,
    pub weight: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityPrior {
    /// Normalized collection name → rated siblings (training only).
    pub by_collection: HashMap<String, Vec<SiblingObs>>,
    /// Known membership map (does not imply affinity).
    pub tmdb_to_collection: HashMap<i64, String>,
    /// Collection list sizes for diagnostic tight vs broad franchise shape.
    pub collection_span: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityResult {
    pub score: f32,
    pub confidence: f32,
    pub support: f32,
    pub collection: Option<String>,
    pub rated_sibling_count: u32,
    pub positive_n_eff: f32,
    pub negative_n_eff: f32,
    pub positive_continuity: f32,
    pub negative_continuity: f32,
    pub net_continuity: f32,
    pub shrinkage: f32,
    pub fit_contribution: f32,
    pub franchise_shape: String,
    pub sibling_bucket: String,
}

fn normalize_collection(name: &str) -> String {
    name.trim().to_lowercase()
}

fn tmdb_id_from_item(item: &LibraryItem) -> Option<i64> {
    item.id.strip_prefix("tmdb:").and_then(|s| s.parse().ok())
}

pub fn build_continuity_prior(films: &[FilmRecord]) -> ContinuityPrior {
    let mut prior = ContinuityPrior::default();
    for film in films {
        let Some(raw_name) = film
            .collection_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let key = normalize_collection(raw_name);
        prior
            .collection_span
            .entry(key.clone())
            .and_modify(|n| *n = (*n).max(film.collection.len().saturating_add(1)))
            .or_insert(film.collection.len().saturating_add(1));

        if let Some(id) = film.tmdb_id {
            prior.tmdb_to_collection.insert(id, key.clone());
        }
        for item in &film.collection {
            if let Some(id) = tmdb_id_from_item(item) {
                prior.tmdb_to_collection.insert(id, key.clone());
            }
        }

        let Some(rating) = film.rating else { continue };
        let positive = rating >= 4.0;
        let negative = rating <= 2.5;
        if !positive && !negative && !(3.0..4.0).contains(&rating) {
            continue;
        }
        // Meh ratings contribute tiny weight toward neutrality, not polarity.
        let weight = film
            .signal
            .as_ref()
            .map(|s| s.preference_weight.max(0.2))
            .unwrap_or(1.0)
            * if positive || negative {
                1.0
            } else {
                0.25
            };
        prior
            .by_collection
            .entry(key)
            .or_default()
            .push(SiblingObs {
                tmdb_id: film.tmdb_id,
                title: film.title.clone(),
                rating,
                positive,
                negative,
                weight,
            });
    }
    prior
}

fn n_eff_from_count(n: f32) -> f32 {
    if n <= 1e-6 {
        0.0
    } else {
        // Saturating: 1 → ~1, 2–3 useful, 4–6 strong, 10+ little extra.
        n / (1.0 + (n - 1.0).max(0.0) * SIBLING_CORRELATION)
    }
}

fn sibling_bucket(count: u32) -> &'static str {
    match count {
        0 => "0",
        1 => "1",
        2 | 3 => "2-3",
        _ => "4+",
    }
}

fn franchise_shape(span: usize) -> &'static str {
    if span <= 5 {
        "sequel_heavy"
    } else if span <= 12 {
        "mid_franchise"
    } else {
        "broad_universe"
    }
}

fn resolve_collection<'a>(
    prior: &'a ContinuityPrior,
    candidate: &Candidate,
) -> Option<&'a str> {
    let id = candidate.tmdb_id?;
    prior.tmdb_to_collection.get(&id).map(|s| s.as_str())
}

pub fn score_continuity(
    prior: &ContinuityPrior,
    candidate: &Candidate,
    config: &ContinuityConfig,
) -> ContinuityResult {
    if !config.any_enabled() {
        return ContinuityResult::default();
    }
    let Some(collection) = resolve_collection(prior, candidate) else {
        return ContinuityResult {
            franchise_shape: "unknown".into(),
            sibling_bucket: "0".into(),
            ..Default::default()
        };
    };
    let siblings = prior
        .by_collection
        .get(collection)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let cand_id = candidate.tmdb_id;
    let mut pos_w = 0.0;
    let mut neg_w = 0.0;
    let mut pos_sum = 0.0;
    let mut neg_sum = 0.0;
    let mut rated_n = 0u32;
    for s in siblings {
        if cand_id.is_some() && s.tmdb_id == cand_id {
            continue;
        }
        rated_n += 1;
        if s.positive {
            pos_w += s.weight;
            pos_sum += s.weight * ((s.rating - 3.5) / 1.5).clamp(0.0, 1.0);
        } else if s.negative {
            neg_w += s.weight;
            neg_sum += s.weight * ((3.0 - s.rating) / 2.0).clamp(0.0, 1.0);
        }
    }
    let pos_n_eff = n_eff_from_count(pos_w);
    let neg_n_eff = n_eff_from_count(neg_w);
    let pos_raw = if pos_w > 1e-4 { pos_sum / pos_w } else { 0.0 };
    let neg_raw = if neg_w > 1e-4 { neg_sum / neg_w } else { 0.0 };
    let pos_shrink = pos_n_eff / (pos_n_eff + SHRINK_K);
    let neg_shrink = neg_n_eff / (neg_n_eff + SHRINK_K);
    let positive_continuity = if config.collection_positive {
        pos_raw * pos_shrink
    } else {
        0.0
    };
    let negative_continuity = if config.collection_negative {
        neg_raw * neg_shrink * NEGATIVE_AUTHORITY
    } else {
        0.0
    };
    let net = (positive_continuity - negative_continuity).clamp(-1.0, 1.0);
    let shrinkage = ((pos_shrink + neg_shrink) * 0.5).clamp(0.0, 1.0);
    let support = pos_n_eff + neg_n_eff;
    let confidence = (support / (support + SHRINK_K)).clamp(0.05, 0.95);
    let fit_contribution =
        (net * CONTINUITY_BOUND * config.lambda).clamp(-CONTINUITY_BOUND, CONTINUITY_BOUND);
    let span = prior.collection_span.get(collection).copied().unwrap_or(0);
    ContinuityResult {
        score: fit_contribution,
        confidence,
        support,
        collection: Some(collection.to_string()),
        rated_sibling_count: rated_n,
        positive_n_eff: pos_n_eff,
        negative_n_eff: neg_n_eff,
        positive_continuity,
        negative_continuity,
        net_continuity: net,
        shrinkage,
        fit_contribution,
        franchise_shape: franchise_shape(span).into(),
        sibling_bucket: sibling_bucket(rated_n).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::MediaKind;

    fn film(
        title: &str,
        id: i64,
        rating: f32,
        collection: &str,
        siblings: &[(i64, &str)],
    ) -> FilmRecord {
        use crate::models::LibraryItem;
        FilmRecord {
            key: format!("tmdb:{id}"),
            title: title.into(),
            year: Some(2010),
            tmdb_id: Some(id),
            rating: Some(rating),
            liked: rating >= 4.0,
            watched: true,
            watchlist: false,
            viewings: 1,
            last_date: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            recommendations: vec![],
            similar: vec![],
            collection_name: Some(collection.into()),
            collection: siblings
                .iter()
                .map(|(sid, st)| {
                    LibraryItem::catalog(
                        format!("tmdb:{sid}"),
                        (*st).into(),
                        Some(2011),
                        None,
                        None,
                        None,
                    )
                })
                .collect(),
            runtime: Some(120),
            poster: None,
            vote_count: None,
            review: None,
            signal: None,
            age_years: None,
        }
    }

    fn cand(id: i64, title: &str) -> Candidate {
        Candidate {
            tmdb_id: Some(id),
            title: title.into(),
            year: Some(2015),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: Some(120),
            vote_count: None,
            watchlist: false,
            sources: vec![],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        }
    }

    #[test]
    fn single_loved_sibling_is_weak() {
        let prior = build_continuity_prior(&[film(
            "Creed",
            1,
            4.5,
            "Rocky Collection",
            &[(2, "Rocky"), (99, "Creed III")],
        )]);
        let r = score_continuity(
            &prior,
            &cand(99, "Creed III"),
            &ContinuityConfig::full(),
        );
        assert_eq!(r.rated_sibling_count, 1);
        assert!(
            r.fit_contribution.abs() < 0.035,
            "one sibling should stay weak, got {}",
            r.fit_contribution
        );
        assert!(r.fit_contribution > 0.0);
    }

    #[test]
    fn repeated_loves_beat_single_entry() {
        let films = vec![
            film("Rocky", 1, 4.5, "Rocky Collection", &[(99, "Creed III")]),
            film("Rocky II", 2, 4.5, "Rocky Collection", &[(99, "Creed III")]),
            film("Rocky III", 3, 4.0, "Rocky Collection", &[(99, "Creed III")]),
            film("Creed", 4, 4.5, "Rocky Collection", &[(99, "Creed III")]),
        ];
        let prior = build_continuity_prior(&films);
        let strong = score_continuity(
            &prior,
            &cand(99, "Creed III"),
            &ContinuityConfig::collection_positive_only(),
        );
        let weak_prior = build_continuity_prior(&[film(
            "Rocky",
            1,
            4.5,
            "Rocky Collection",
            &[(99, "Creed III")],
        )]);
        let weak = score_continuity(
            &weak_prior,
            &cand(99, "Creed III"),
            &ContinuityConfig::collection_positive_only(),
        );
        assert!(
            strong.fit_contribution > weak.fit_contribution,
            "4 loves ({}) should beat 1 love ({})",
            strong.fit_contribution,
            weak.fit_contribution
        );
        assert!(strong.rated_sibling_count >= 4);
        assert_eq!(strong.sibling_bucket, "4+");
    }

    #[test]
    fn mixed_franchise_stays_near_neutral() {
        let films = vec![
            film("A", 1, 4.5, "Mixed", &[(99, "X")]),
            film("B", 2, 3.0, "Mixed", &[(99, "X")]),
            film("C", 3, 3.0, "Mixed", &[(99, "X")]),
            film("D", 4, 2.0, "Mixed", &[(99, "X")]),
            film("E", 5, 1.5, "Mixed", &[(99, "X")]),
        ];
        let prior = build_continuity_prior(&films);
        let r = score_continuity(&prior, &cand(99, "X"), &ContinuityConfig::full());
        assert!(
            r.fit_contribution.abs() < 0.04,
            "mixed history should be near-neutral, got {}",
            r.fit_contribution
        );
    }

    #[test]
    fn repeated_dislikes_yield_negative_continuity() {
        let films = vec![
            film("T1", 1, 1.5, "Transformers", &[(99, "T6")]),
            film("T2", 2, 2.0, "Transformers", &[(99, "T6")]),
            film("T3", 3, 1.0, "Transformers", &[(99, "T6")]),
        ];
        let prior = build_continuity_prior(&films);
        let r = score_continuity(
            &prior,
            &cand(99, "T6"),
            &ContinuityConfig::collection_negative_only(),
        );
        assert!(r.negative_continuity > 0.2);
        assert!(r.fit_contribution < -0.01);
        assert!(r.fit_contribution >= -CONTINUITY_BOUND - 1e-5);
    }

    #[test]
    fn retrieval_provenance_alone_does_not_score() {
        // Candidate not in membership map → no continuity, even if sources claim collection.
        use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
        let prior = ContinuityPrior::default();
        let mut c = cand(42, "Orphan");
        c.sources.push(RetrievalSource::new(
            RetrievalKind::Collection,
            "same collection as Seed (Fake)",
            Some(1),
        ));
        let r = score_continuity(&prior, &c, &ContinuityConfig::full());
        assert_eq!(r.fit_contribution, 0.0);
        assert!(r.collection.is_none());
    }

    #[test]
    fn off_config_is_zero() {
        let films = vec![
            film("Rocky", 1, 4.5, "Rocky Collection", &[(99, "Creed III")]),
            film("Rocky II", 2, 4.5, "Rocky Collection", &[(99, "Creed III")]),
        ];
        let prior = build_continuity_prior(&films);
        let r = score_continuity(&prior, &cand(99, "Creed III"), &ContinuityConfig::off());
        assert_eq!(r.score, 0.0);
    }
}
