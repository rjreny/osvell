//! Milestone B ranking diagnostics over frozen A3 retrieval.
//! Compares Legacy vs Content-only vs Content+Craft without board diversification.

use crate::storage::db::Database;
use crate::taste::eval::{
    rating_bucket, recall_at, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
};
use crate::taste::family_fit::{score_pool_family_fit, FamilyFitResult, FitMode};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord,
};
use crate::taste::score::{score_candidate_with_semantic, ScoredCandidate};
use crate::taste::semantic::{score_candidates_from_cache, SemanticScore};
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingMetrics {
    pub ndcg_at_10: f32,
    pub ndcg_at_25: f32,
    pub ndcg_at_100: f32,
    pub ndcg_at_250: f32,
    pub precision_at_10: f32,
    pub precision_at_25: f32,
    pub loved_vs_disliked_pairwise: f32,
    pub liked_vs_disliked_pairwise: f32,
    pub mean_rank_loved: Option<f32>,
    pub mean_rank_liked: Option<f32>,
    pub mean_rank_meh: Option<f32>,
    pub mean_rank_disliked: Option<f32>,
    pub dislike_rate_top_10: f32,
    pub dislike_rate_top_25: f32,
    pub dislike_rate_top_50: f32,
    pub recall_at_250: f32,
    pub recall_at_1000: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeMetrics {
    pub mode: String,
    pub metrics: RankingMetrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingFoldReport {
    pub seed: u64,
    pub held_out: usize,
    pub pool_size: usize,
    pub modes: Vec<ModeMetrics>,
    pub sample_diagnostics: Vec<CandidateDiag>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateDiag {
    pub title: String,
    pub tmdb_id: Option<i64>,
    pub bucket: String,
    pub fit: f32,
    pub confidence: f32,
    pub families: crate::taste::family_fit::FamilyScores,
    pub counterfactual: crate::taste::family_fit::Counterfactuals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingBenchmarkReport {
    pub protocol: String,
    pub algorithm_version: String,
    pub folds: usize,
    pub holdout_frac: f32,
    pub seeds: Vec<u64>,
    pub fold_reports: Vec<RankingFoldReport>,
    pub mean_by_mode: Vec<ModeMean>,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeMean {
    pub mode: String,
    pub ndcg_at_10: MetricSummary,
    pub ndcg_at_25: MetricSummary,
    pub ndcg_at_100: MetricSummary,
    pub ndcg_at_250: MetricSummary,
    pub precision_at_10: MetricSummary,
    pub precision_at_25: MetricSummary,
    pub loved_vs_disliked_pairwise: MetricSummary,
    pub liked_vs_disliked_pairwise: MetricSummary,
    pub mean_rank_loved: MetricSummary,
    pub mean_rank_liked: MetricSummary,
    pub mean_rank_disliked: MetricSummary,
    pub dislike_rate_top_10: MetricSummary,
    pub dislike_rate_top_25: MetricSummary,
    pub dislike_rate_top_50: MetricSummary,
    pub recall_at_250: MetricSummary,
}

fn relevance(bucket: RatingBucket) -> f32 {
    match bucket {
        RatingBucket::Loved => 3.0,
        RatingBucket::Liked => 2.0,
        RatingBucket::Meh => 1.0,
        RatingBucket::Disliked => 0.0,
    }
}

fn held_out_rated<'a>(films: &'a [FilmRecord], held: &HashSet<String>) -> Vec<&'a FilmRecord> {
    films
        .iter()
        .filter(|f| {
            f.rating.is_some() && held.contains(&identity_key(f.tmdb_id, &f.title, f.year))
        })
        .collect()
}

fn id_of(c: &Candidate) -> String {
    identity_key(c.tmdb_id, &c.title, c.year)
}

fn rank_map(ordered_ids: &[String]) -> HashMap<String, usize> {
    ordered_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i + 1))
        .collect()
}

fn ndcg_at(ordered: &[String], rel: &HashMap<String, f32>, k: usize) -> f32 {
    let mut dcg = 0.0;
    for (i, id) in ordered.iter().take(k).enumerate() {
        let r = rel.get(id).copied().unwrap_or(0.0);
        if r > 0.0 {
            dcg += (2f32.powf(r) - 1.0) / ((i as f32 + 2.0).log2());
        }
    }
    let mut ideal: Vec<f32> = rel.values().copied().filter(|r| *r > 0.0).collect();
    ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let mut idcg = 0.0;
    for (i, r) in ideal.into_iter().take(k).enumerate() {
        idcg += (2f32.powf(r) - 1.0) / ((i as f32 + 2.0).log2());
    }
    if idcg <= 1e-8 {
        0.0
    } else {
        dcg / idcg
    }
}

fn precision_positive_at(
    ordered: &[String],
    positives: &HashSet<String>,
    k: usize,
) -> f32 {
    if k == 0 {
        return 0.0;
    }
    let hit = ordered
        .iter()
        .take(k)
        .filter(|id| positives.contains(*id))
        .count();
    hit as f32 / k as f32
}

fn pairwise_accuracy(
    ranks: &HashMap<String, usize>,
    higher: &[(String, RatingBucket)],
    lower: &[(String, RatingBucket)],
) -> f32 {
    let mut ok = 0u32;
    let mut total = 0u32;
    for (hid, _) in higher {
        let Some(&hr) = ranks.get(hid) else { continue };
        for (lid, _) in lower {
            let Some(&lr) = ranks.get(lid) else { continue };
            total += 1;
            if hr < lr {
                ok += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        ok as f32 / total as f32
    }
}

fn mean_rank_for(
    ranks: &HashMap<String, usize>,
    ids: &[String],
) -> Option<f32> {
    let vals: Vec<f32> = ids
        .iter()
        .filter_map(|id| ranks.get(id).map(|r| *r as f32))
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum::<f32>() / vals.len() as f32)
    }
}

fn dislike_rate(ordered: &[String], disliked: &HashSet<String>, k: usize) -> f32 {
    if k == 0 {
        return 0.0;
    }
    let hit = ordered
        .iter()
        .take(k)
        .filter(|id| disliked.contains(*id))
        .count();
    hit as f32 / k.min(ordered.len()).max(1) as f32
}

fn metrics_for_ordering(
    ordered_ids: &[String],
    held: &[&FilmRecord],
    positive_ids: &HashSet<String>,
) -> RankingMetrics {
    let mut rel = HashMap::new();
    let mut by_bucket: HashMap<RatingBucket, Vec<String>> = HashMap::new();
    let mut disliked = HashSet::new();
    for film in held {
        let id = identity_key(film.tmdb_id, &film.title, film.year);
        let Some(rating) = film.rating else { continue };
        let bucket = rating_bucket(rating);
        rel.insert(id.clone(), relevance(bucket));
        by_bucket.entry(bucket).or_default().push(id.clone());
        if bucket == RatingBucket::Disliked {
            disliked.insert(id);
        }
    }
    let ranks = rank_map(ordered_ids);
    let loved = by_bucket
        .get(&RatingBucket::Loved)
        .cloned()
        .unwrap_or_default();
    let liked = by_bucket
        .get(&RatingBucket::Liked)
        .cloned()
        .unwrap_or_default();
    let meh = by_bucket
        .get(&RatingBucket::Meh)
        .cloned()
        .unwrap_or_default();
    let disliked_list = by_bucket
        .get(&RatingBucket::Disliked)
        .cloned()
        .unwrap_or_default();

    let loved_pairs: Vec<(String, RatingBucket)> = loved
        .iter()
        .map(|id| (id.clone(), RatingBucket::Loved))
        .collect();
    let liked_pairs: Vec<(String, RatingBucket)> = liked
        .iter()
        .map(|id| (id.clone(), RatingBucket::Liked))
        .collect();
    let disliked_pairs: Vec<(String, RatingBucket)> = disliked_list
        .iter()
        .map(|id| (id.clone(), RatingBucket::Disliked))
        .collect();

    RankingMetrics {
        ndcg_at_10: ndcg_at(ordered_ids, &rel, 10),
        ndcg_at_25: ndcg_at(ordered_ids, &rel, 25),
        ndcg_at_100: ndcg_at(ordered_ids, &rel, 100),
        ndcg_at_250: ndcg_at(ordered_ids, &rel, 250),
        precision_at_10: precision_positive_at(ordered_ids, positive_ids, 10),
        precision_at_25: precision_positive_at(ordered_ids, positive_ids, 25),
        loved_vs_disliked_pairwise: pairwise_accuracy(&ranks, &loved_pairs, &disliked_pairs),
        liked_vs_disliked_pairwise: pairwise_accuracy(&ranks, &liked_pairs, &disliked_pairs),
        mean_rank_loved: mean_rank_for(&ranks, &loved),
        mean_rank_liked: mean_rank_for(&ranks, &liked),
        mean_rank_meh: mean_rank_for(&ranks, &meh),
        mean_rank_disliked: mean_rank_for(&ranks, &disliked_list),
        dislike_rate_top_10: dislike_rate(ordered_ids, &disliked, 10),
        dislike_rate_top_25: dislike_rate(ordered_ids, &disliked, 25),
        dislike_rate_top_50: dislike_rate(ordered_ids, &disliked, 50),
        recall_at_250: recall_at(ordered_ids, positive_ids, 250),
        recall_at_1000: recall_at(ordered_ids, positive_ids, 1000),
    }
}

fn legacy_order(
    profile: &crate::taste::features::FeatureProfile,
    candidates: &[Candidate],
    semantic: &HashMap<i64, SemanticScore>,
) -> Vec<String> {
    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .map(|c| {
            let s = c
                .tmdb_id
                .and_then(|id| semantic.get(&id))
                .cloned()
                .unwrap_or_default();
            score_candidate_with_semantic(profile, c, &s)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.candidate
                    .tmdb_id
                    .unwrap_or(i64::MAX)
                    .cmp(&b.candidate.tmdb_id.unwrap_or(i64::MAX))
            })
    });
    scored
        .into_iter()
        .map(|s| identity_key(s.candidate.tmdb_id, &s.candidate.title, s.candidate.year))
        .collect()
}

fn family_order(
    profile: &crate::taste::features::FeatureProfile,
    candidates: &[Candidate],
    semantic: &HashMap<i64, SemanticScore>,
    mode: FitMode,
) -> (Vec<String>, Vec<(Candidate, FamilyFitResult)>) {
    let ranked = score_pool_family_fit(profile, candidates, semantic, mode);
    let ids = ranked
        .iter()
        .map(|(c, _)| id_of(c))
        .collect();
    (ids, ranked)
}

fn evaluate_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
) -> Result<RankingFoldReport, String> {
    let held = held_out_rated(all_films, &inputs.held_out);
    let positive_ids: HashSet<String> = held
        .iter()
        .filter(|f| f.rating.unwrap_or(0.0) >= 3.5)
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect();
    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    // Frozen A3 examination: fair pool at 1000.
    let candidates = select_fair_pool(pool.by_key, 1000);
    let semantic = score_candidates_from_cache(db, &inputs.training_films, &candidates);

    let legacy_ids = legacy_order(&inputs.profile, &candidates, &semantic);
    let (content_ids, _) =
        family_order(&inputs.profile, &candidates, &semantic, FitMode::ContentOnly);
    let (both_ids, both_ranked) =
        family_order(&inputs.profile, &candidates, &semantic, FitMode::ContentAndCraft);

    let modes = vec![
        ModeMetrics {
            mode: "legacy".into(),
            metrics: metrics_for_ordering(&legacy_ids, &held, &positive_ids),
        },
        ModeMetrics {
            mode: "content".into(),
            metrics: metrics_for_ordering(&content_ids, &held, &positive_ids),
        },
        ModeMetrics {
            mode: "contentCraft".into(),
            metrics: metrics_for_ordering(&both_ids, &held, &positive_ids),
        },
    ];

    // Sample diagnostics: first few held-out titles that appear in the pool.
    let mut sample_diagnostics = Vec::new();
    for film in held.iter().take(40) {
        let key = identity_key(film.tmdb_id, &film.title, film.year);
        let Some((_, fit)) = both_ranked.iter().find(|(c, _)| id_of(c) == key) else {
            continue;
        };
        sample_diagnostics.push(CandidateDiag {
            title: film.title.clone(),
            tmdb_id: film.tmdb_id,
            bucket: format!("{:?}", rating_bucket(film.rating.unwrap_or(3.0))),
            fit: fit.fit,
            confidence: fit.confidence,
            families: fit.families.clone(),
            counterfactual: fit.counterfactual.clone(),
        });
        if sample_diagnostics.len() >= 8 {
            break;
        }
    }

    Ok(RankingFoldReport {
        seed,
        held_out: held.len(),
        pool_size: candidates.len(),
        modes,
        sample_diagnostics,
    })
}

fn summarize(xs: &[f32]) -> MetricSummary {
    if xs.is_empty() {
        return MetricSummary::default();
    }
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
    MetricSummary {
        mean,
        min: xs.iter().copied().fold(f32::INFINITY, f32::min),
        max: xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        stdev: var.sqrt(),
    }
}

fn mean_modes(folds: &[RankingFoldReport]) -> Vec<ModeMean> {
    let names = ["legacy", "content", "contentCraft"];
    names
        .into_iter()
        .map(|name| {
            let rows: Vec<&RankingMetrics> = folds
                .iter()
                .filter_map(|f| f.modes.iter().find(|m| m.mode == name).map(|m| &m.metrics))
                .collect();
            let col = |f: fn(&RankingMetrics) -> f32| -> MetricSummary {
                summarize(&rows.iter().map(|m| f(m)).collect::<Vec<_>>())
            };
            let col_opt = |f: fn(&RankingMetrics) -> Option<f32>| -> MetricSummary {
                summarize(
                    &rows
                        .iter()
                        .filter_map(|m| f(m))
                        .collect::<Vec<_>>(),
                )
            };
            ModeMean {
                mode: name.into(),
                ndcg_at_10: col(|m| m.ndcg_at_10),
                ndcg_at_25: col(|m| m.ndcg_at_25),
                ndcg_at_100: col(|m| m.ndcg_at_100),
                ndcg_at_250: col(|m| m.ndcg_at_250),
                precision_at_10: col(|m| m.precision_at_10),
                precision_at_25: col(|m| m.precision_at_25),
                loved_vs_disliked_pairwise: col(|m| m.loved_vs_disliked_pairwise),
                liked_vs_disliked_pairwise: col(|m| m.liked_vs_disliked_pairwise),
                mean_rank_loved: col_opt(|m| m.mean_rank_loved),
                mean_rank_liked: col_opt(|m| m.mean_rank_liked),
                mean_rank_disliked: col_opt(|m| m.mean_rank_disliked),
                dislike_rate_top_10: col(|m| m.dislike_rate_top_10),
                dislike_rate_top_25: col(|m| m.dislike_rate_top_25),
                dislike_rate_top_50: col(|m| m.dislike_rate_top_50),
                recall_at_250: col(|m| m.recall_at_250),
            }
        })
        .collect()
}

fn decide_gate(means: &[ModeMean]) -> String {
    let legacy = means.iter().find(|m| m.mode == "legacy");
    let content = means.iter().find(|m| m.mode == "content");
    let both = means.iter().find(|m| m.mode == "contentCraft");
    let (Some(legacy), Some(both)) = (legacy, both) else {
        return "HOLD_B_RANK: missing mode summaries".into();
    };
    let pairwise_up = both.loved_vs_disliked_pairwise.mean
        >= legacy.loved_vs_disliked_pairwise.mean + 0.015
        || both.liked_vs_disliked_pairwise.mean
            >= legacy.liked_vs_disliked_pairwise.mean + 0.015;
    let ndcg_up = both.ndcg_at_250.mean >= legacy.ndcg_at_250.mean + 0.01
        || both.ndcg_at_100.mean >= legacy.ndcg_at_100.mean + 0.01;
    let loved_earlier = both.mean_rank_loved.mean + 15.0 < legacy.mean_rank_loved.mean;
    let dislikes_later = both.mean_rank_disliked.mean
        > legacy.mean_rank_disliked.mean + 15.0
        || (both.dislike_rate_top_25.mean + 0.02 < legacy.dislike_rate_top_25.mean
            && legacy.dislike_rate_top_25.mean > 0.0);
    let better_order = pairwise_up || ndcg_up || loved_earlier;
    let fewer_dislikes = dislikes_later
        || (legacy.dislike_rate_top_25.mean == 0.0
            && both.dislike_rate_top_25.mean == 0.0
            && both.mean_rank_disliked.mean >= legacy.mean_rank_disliked.mean);

    let content_note = content
        .map(|c| {
            format!(
                " content-only: lovedRank {:.0} dislikedRank {:.0} loved>dis {:.3}",
                c.mean_rank_loved.mean,
                c.mean_rank_disliked.mean,
                c.loved_vs_disliked_pairwise.mean
            )
        })
        .unwrap_or_default();

    if better_order && fewer_dislikes && (pairwise_up || ndcg_up || loved_earlier) {
        format!(
            "PASS_B1: contentCraft improves preference ordering vs legacy (loved>dis {:.3}→{:.3}, ndcg@250 {:.3}→{:.3}, lovedRank {:.0}→{:.0}, dislikedRank {:.0}→{:.0}).{content_note}",
            legacy.loved_vs_disliked_pairwise.mean,
            both.loved_vs_disliked_pairwise.mean,
            legacy.ndcg_at_250.mean,
            both.ndcg_at_250.mean,
            legacy.mean_rank_loved.mean,
            both.mean_rank_loved.mean,
            legacy.mean_rank_disliked.mean,
            both.mean_rank_disliked.mean
        )
    } else if better_order || fewer_dislikes {
        format!(
            "MIXED_B1: direction not unambiguous across folds (loved>dis {:.3}→{:.3}, ndcg@250 {:.3}→{:.3}, lovedRank {:.0}→{:.0}, dislikedRank {:.0}→{:.0}).{content_note}",
            legacy.loved_vs_disliked_pairwise.mean,
            both.loved_vs_disliked_pairwise.mean,
            legacy.ndcg_at_250.mean,
            both.ndcg_at_250.mean,
            legacy.mean_rank_loved.mean,
            both.mean_rank_loved.mean,
            legacy.mean_rank_disliked.mean,
            both.mean_rank_disliked.mean
        )
    } else {
        format!(
            "HOLD_B1: contentCraft did not beat legacy — debug family model before Form/Quality/board.{content_note}"
        )
    }
}

pub fn run_ranking_benchmark(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<RankingBenchmarkReport, String> {
    let mut fold_reports = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        fold_reports.push(evaluate_fold(db, seed, &inputs, films)?);
    }
    let mean_by_mode = mean_modes(&fold_reports);
    let gate = decide_gate(&mean_by_mode);
    Ok(RankingBenchmarkReport {
        protocol: "milestone-b1-family-fit-ranking".into(),
        algorithm_version: ALGORITHM_VERSION.into(),
        folds: seeds.len(),
        holdout_frac,
        seeds: seeds.to_vec(),
        fold_reports,
        mean_by_mode,
        gate,
    })
}

pub fn write_ranking_artifact(
    taste_runs_dir: &std::path::Path,
    report: &RankingBenchmarkReport,
) -> Result<String, String> {
    let dir = taste_runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("ranking-b1-{stamp}.json"));
    let latest = dir.join("ranking-b1-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndcg_prefers_loved_first() {
        let mut rel = HashMap::new();
        rel.insert("a".into(), 3.0);
        rel.insert("b".into(), 0.0);
        let good = ndcg_at(&["a".into(), "b".into()], &rel, 2);
        let bad = ndcg_at(&["b".into(), "a".into()], &rel, 2);
        assert!(good > bad);
    }

    #[test]
    fn pairwise_counts_order() {
        let ranks = HashMap::from([("loved".into(), 1usize), ("disliked".into(), 5usize)]);
        let higher = vec![("loved".into(), RatingBucket::Loved)];
        let lower = vec![("disliked".into(), RatingBucket::Disliked)];
        assert!((pairwise_accuracy(&ranks, &higher, &lower) - 1.0).abs() < 1e-5);
    }
}
