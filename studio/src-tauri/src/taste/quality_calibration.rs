//! Milestone B4 Quality calibration: positive/negative TMDB prior vs Content-only.
//! vote_count is shrinkage only — never an independent popularity boost.

use crate::storage::db::Database;
use crate::taste::eval::{
    rating_bucket, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
    BENCHMARK_HOLDOUT_FRAC, BENCHMARK_SEEDS,
};
use crate::taste::family_fit::{
    score_pool_family_fit_full, FamilyFitConfig, FamilyFitResult, FitPriors,
};
use crate::taste::quality::{load_quality_catalog, QualityCatalog, QualityConfig};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord,
};
use crate::taste::semantic::score_candidates_from_cache;
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const LARGE_MOVE: i32 = 50;

#[derive(Debug, Clone)]
struct AblationSpec {
    name: String,
    config: FamilyFitConfig,
    note: Option<String>,
}

fn ablation_matrix() -> Vec<AblationSpec> {
    vec![
        AblationSpec {
            name: "contentOnly".into(),
            config: FamilyFitConfig::content_only(),
            note: None,
        },
        AblationSpec {
            name: "qualityPositive".into(),
            config: FamilyFitConfig::with_quality(QualityConfig::positive_only()),
            note: Some("acclaimed movies get small lift".into()),
        },
        AblationSpec {
            name: "qualityNegative".into(),
            config: FamilyFitConfig::with_quality(QualityConfig::negative_only()),
            note: Some("poorly received movies get small penalty".into()),
        },
        AblationSpec {
            name: "fullQuality".into(),
            config: FamilyFitConfig::with_quality(QualityConfig::full()),
            note: Some("both directions".into()),
        },
    ]
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankMovement {
    pub positive_rank_gain: f32,
    pub positive_rank_loss: f32,
    pub negative_rank_gain: f32,
    pub negative_rank_loss: f32,
    pub median_abs_delta: f32,
    pub p90_abs_delta: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LargeMove {
    pub title: String,
    pub tmdb_id: Option<i64>,
    pub bucket: String,
    pub vote_average: Option<f32>,
    pub vote_count: Option<i64>,
    pub shrunk_quality: Option<f32>,
    pub content_rank: usize,
    pub quality_rank: usize,
    pub rank_delta: i32,
    pub quality_contribution: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AblationDelta {
    pub loved_vs_disliked: f32,
    pub liked_vs_disliked: f32,
    pub mean_rank_loved: Option<f32>,
    pub mean_rank_disliked: Option<f32>,
    pub ndcg_at_100: f32,
    pub ndcg_at_250: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AblationMetrics {
    pub name: String,
    pub note: Option<String>,
    pub loved_vs_disliked: f32,
    pub liked_vs_disliked: f32,
    pub mean_rank_loved: Option<f32>,
    pub mean_rank_disliked: Option<f32>,
    pub ndcg_at_100: f32,
    pub ndcg_at_250: f32,
    pub recall_at_250: f32,
    pub delta_vs_content: AblationDelta,
    pub movement: RankMovement,
    pub large_moves: Vec<LargeMove>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCalibrationFold {
    pub seed: u64,
    pub pool_size: usize,
    pub held_out: usize,
    pub catalog_size: usize,
    pub population_mean: f32,
    pub ablations: Vec<AblationMetrics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeanAblation {
    pub name: String,
    pub note: Option<String>,
    pub loved_vs_disliked: MetricSummary,
    pub liked_vs_disliked: MetricSummary,
    pub mean_rank_loved: MetricSummary,
    pub mean_rank_disliked: MetricSummary,
    pub ndcg_at_100: MetricSummary,
    pub ndcg_at_250: MetricSummary,
    pub delta_loved_vs_disliked: MetricSummary,
    pub delta_mean_rank_loved: MetricSummary,
    pub delta_mean_rank_disliked: MetricSummary,
    pub delta_ndcg_at_250: MetricSummary,
    pub positive_rank_gain: MetricSummary,
    pub positive_rank_loss: MetricSummary,
    pub negative_rank_gain: MetricSummary,
    pub negative_rank_loss: MetricSummary,
    pub median_abs_delta: MetricSummary,
    pub p90_abs_delta: MetricSummary,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCalibrationReport {
    pub protocol: String,
    pub algorithm_version: String,
    pub folds: usize,
    pub holdout_frac: f32,
    pub seeds: Vec<u64>,
    pub fold_reports: Vec<QualityCalibrationFold>,
    pub mean_ablations: Vec<MeanAblation>,
    pub gate: String,
}

fn id_of(c: &Candidate) -> String {
    identity_key(c.tmdb_id, &c.title, c.year)
}

fn held_out_rated<'a>(films: &'a [FilmRecord], held: &HashSet<String>) -> Vec<&'a FilmRecord> {
    films
        .iter()
        .filter(|f| {
            f.rating.is_some() && held.contains(&identity_key(f.tmdb_id, &f.title, f.year))
        })
        .collect()
}

fn relevance(bucket: RatingBucket) -> f32 {
    match bucket {
        RatingBucket::Loved => 3.0,
        RatingBucket::Liked => 2.0,
        RatingBucket::Meh => 1.0,
        RatingBucket::Disliked => 0.0,
    }
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

fn pairwise_better(a: &[String], b: &[String], ranks: &HashMap<String, usize>) -> f32 {
    let mut ok = 0usize;
    let mut n = 0usize;
    for left in a {
        let Some(&rl) = ranks.get(left) else { continue };
        for right in b {
            let Some(&rr) = ranks.get(right) else { continue };
            n += 1;
            if rl < rr {
                ok += 1;
            }
        }
    }
    if n == 0 {
        0.5
    } else {
        ok as f32 / n as f32
    }
}

fn mean_rank(ids: &[String], ranks: &HashMap<String, usize>) -> Option<f32> {
    let mut sum = 0.0;
    let mut n = 0.0;
    for id in ids {
        if let Some(&r) = ranks.get(id) {
            sum += r as f32;
            n += 1.0;
        }
    }
    if n < 1.0 {
        None
    } else {
        Some(sum / n)
    }
}

fn order_ids(ranked: &[(Candidate, FamilyFitResult)]) -> Vec<String> {
    ranked.iter().map(|(c, _)| id_of(c)).collect()
}

fn rank_map(ids: &[String]) -> HashMap<String, usize> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i + 1))
        .collect()
}

fn metrics_core(
    ordered: &[String],
    held: &[&FilmRecord],
) -> (
    f32,
    f32,
    Option<f32>,
    Option<f32>,
    f32,
    f32,
    f32,
    HashMap<String, usize>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let ranks = rank_map(ordered);
    let mut rel = HashMap::new();
    let mut loved = Vec::new();
    let mut liked = Vec::new();
    let mut disliked = Vec::new();
    for f in held {
        let id = identity_key(f.tmdb_id, &f.title, f.year);
        let Some(rating) = f.rating else { continue };
        let bucket = rating_bucket(rating);
        rel.insert(id.clone(), relevance(bucket));
        match bucket {
            RatingBucket::Loved => loved.push(id),
            RatingBucket::Liked => liked.push(id),
            RatingBucket::Disliked => disliked.push(id),
            RatingBucket::Meh => {}
        }
    }
    let loved_vs = pairwise_better(&loved, &disliked, &ranks);
    let liked_vs = pairwise_better(&liked, &disliked, &ranks);
    let loved_rank = mean_rank(&loved, &ranks);
    let dis_rank = mean_rank(&disliked, &ranks);
    let ndcg100 = ndcg_at(ordered, &rel, 100);
    let ndcg250 = ndcg_at(ordered, &rel, 250);
    let positives: HashSet<String> = loved.iter().chain(liked.iter()).cloned().collect();
    let hit = ordered
        .iter()
        .take(250)
        .filter(|id| positives.contains(*id))
        .count();
    let recall250 = if positives.is_empty() {
        0.0
    } else {
        hit as f32 / positives.len() as f32
    };
    (
        loved_vs,
        liked_vs,
        loved_rank,
        dis_rank,
        ndcg100,
        ndcg250,
        recall250,
        ranks,
        loved,
        liked,
        disliked,
    )
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn movement_stats(
    content_ranks: &HashMap<String, usize>,
    model_ranks: &HashMap<String, usize>,
    positives: &[String],
    disliked: &[String],
) -> RankMovement {
    let mut pos_gain = 0.0;
    let mut pos_loss = 0.0;
    let mut neg_gain = 0.0;
    let mut neg_loss = 0.0;
    let mut abs_deltas = Vec::new();
    for id in positives {
        let cr = content_ranks.get(id).copied().unwrap_or(usize::MAX) as i32;
        let mr = model_ranks.get(id).copied().unwrap_or(usize::MAX) as i32;
        let d = cr - mr;
        abs_deltas.push(d.abs() as f32);
        if d > 0 {
            pos_gain += d as f32;
        } else if d < 0 {
            pos_loss += (-d) as f32;
        }
    }
    for id in disliked {
        let cr = content_ranks.get(id).copied().unwrap_or(usize::MAX) as i32;
        let mr = model_ranks.get(id).copied().unwrap_or(usize::MAX) as i32;
        let d = cr - mr;
        abs_deltas.push(d.abs() as f32);
        if d > 0 {
            neg_gain += d as f32;
        } else if d < 0 {
            neg_loss += (-d) as f32;
        }
    }
    abs_deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    RankMovement {
        positive_rank_gain: pos_gain,
        positive_rank_loss: pos_loss,
        negative_rank_gain: neg_gain,
        negative_rank_loss: neg_loss,
        median_abs_delta: percentile(&abs_deltas, 0.5),
        p90_abs_delta: percentile(&abs_deltas, 0.9),
    }
}

fn large_moves_for(
    content_ranks: &HashMap<String, usize>,
    model_ranked: &[(Candidate, FamilyFitResult)],
    held: &[&FilmRecord],
) -> Vec<LargeMove> {
    let held_by_id: HashMap<String, &FilmRecord> = held
        .iter()
        .map(|f| (identity_key(f.tmdb_id, &f.title, f.year), *f))
        .collect();
    let mut out = Vec::new();
    for (i, (c, fit)) in model_ranked.iter().enumerate() {
        let id = id_of(c);
        let Some(film) = held_by_id.get(&id) else {
            continue;
        };
        let Some(&cr) = content_ranks.get(&id) else {
            continue;
        };
        let mr = i + 1;
        let delta = cr as i32 - mr as i32;
        if delta.abs() < LARGE_MOVE {
            continue;
        }
        let q = &fit.quality_detail;
        out.push(LargeMove {
            title: film.title.clone(),
            tmdb_id: film.tmdb_id,
            bucket: film
                .rating
                .map(rating_bucket)
                .map(|b| format!("{b:?}"))
                .unwrap_or_else(|| "unknown".into()),
            vote_average: q.vote_average,
            vote_count: q.vote_count,
            shrunk_quality: q.shrunk_quality,
            content_rank: cr,
            quality_rank: mr,
            rank_delta: delta,
            quality_contribution: q.fit_contribution,
        });
    }
    out.sort_by_key(|m| -m.rank_delta.abs());
    out.truncate(24);
    out
}

fn gate_component(m: &MeanAblation) -> String {
    if m.name == "contentOnly" {
        return "CONTROL".into();
    }
    let loved_ok = m.delta_loved_vs_disliked.mean >= -0.005
        && m.delta_loved_vs_disliked.min >= -0.02;
    let loved_rank_ok =
        m.delta_mean_rank_loved.mean <= 2.0 && m.delta_mean_rank_loved.max <= 12.0;
    let disliked_rank_ok = m.delta_mean_rank_disliked.mean >= -3.0;
    let ndcg_ok = m.delta_ndcg_at_250.mean >= -0.0015;
    let movement_ok = m.positive_rank_gain.mean + 1.0 >= m.positive_rank_loss.mean
        && m.negative_rank_gain.mean <= m.negative_rank_loss.mean + m.positive_rank_gain.mean * 0.5;
    // One-fold carry: pairwise stdev large relative to mean.
    let stable = m.delta_loved_vs_disliked.stdev <= 0.01
        || (m.delta_loved_vs_disliked.mean.abs() < 1e-4 && m.delta_loved_vs_disliked.stdev < 0.005);
    let meaningful = m.delta_loved_vs_disliked.mean > 0.004
        || m.delta_mean_rank_loved.mean < -2.0
        || m.delta_ndcg_at_250.mean > 0.001;

    if loved_ok && loved_rank_ok && disliked_rank_ok && ndcg_ok && movement_ok && stable {
        if meaningful {
            "PASS_INCLUDE".into()
        } else {
            "HOLD_NEUTRAL".into()
        }
    } else {
        "FAIL_KEEP_ZERO".into()
    }
}

fn evaluate_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
    catalog: &QualityCatalog,
) -> Result<QualityCalibrationFold, String> {
    let held = held_out_rated(all_films, &inputs.held_out);
    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let candidates = select_fair_pool(pool.by_key, 1000);
    let semantic = score_candidates_from_cache(db, &inputs.training_films, &candidates);
    let priors = FitPriors {
        form: None,
        continuity: None,
        quality: Some(catalog),
    };

    let content_ranked = score_pool_family_fit_full(
        &inputs.profile,
        &candidates,
        &semantic,
        &FamilyFitConfig::content_only(),
        priors,
    );
    let content_ids = order_ids(&content_ranked);
    let (
        c_loved,
        c_liked,
        c_loved_rank,
        c_dis_rank,
        c_ndcg100,
        c_ndcg250,
        _,
        content_ranks,
        positives_loved,
        positives_liked,
        disliked,
    ) = metrics_core(&content_ids, &held);
    let positives: Vec<String> = positives_loved
        .iter()
        .chain(positives_liked.iter())
        .cloned()
        .collect();

    let mut ablations = Vec::new();
    for spec in ablation_matrix() {
        let ranked = if spec.name == "contentOnly" {
            content_ranked.clone()
        } else {
            score_pool_family_fit_full(
                &inputs.profile,
                &candidates,
                &semantic,
                &spec.config,
                priors,
            )
        };
        let ids = order_ids(&ranked);
        let (
            loved,
            liked,
            loved_rank,
            dis_rank,
            ndcg100,
            ndcg250,
            recall250,
            model_ranks,
            _,
            _,
            _,
        ) = metrics_core(&ids, &held);
        let movement = movement_stats(&content_ranks, &model_ranks, &positives, &disliked);
        let large_moves = if spec.name == "contentOnly" {
            Vec::new()
        } else {
            large_moves_for(&content_ranks, &ranked, &held)
        };
        ablations.push(AblationMetrics {
            name: spec.name,
            note: spec.note,
            loved_vs_disliked: loved,
            liked_vs_disliked: liked,
            mean_rank_loved: loved_rank,
            mean_rank_disliked: dis_rank,
            ndcg_at_100: ndcg100,
            ndcg_at_250: ndcg250,
            recall_at_250: recall250,
            delta_vs_content: AblationDelta {
                loved_vs_disliked: loved - c_loved,
                liked_vs_disliked: liked - c_liked,
                mean_rank_loved: match (loved_rank, c_loved_rank) {
                    (Some(a), Some(b)) => Some(a - b),
                    _ => None,
                },
                mean_rank_disliked: match (dis_rank, c_dis_rank) {
                    (Some(a), Some(b)) => Some(a - b),
                    _ => None,
                },
                ndcg_at_100: ndcg100 - c_ndcg100,
                ndcg_at_250: ndcg250 - c_ndcg250,
            },
            movement,
            large_moves,
        });
    }

    Ok(QualityCalibrationFold {
        seed,
        pool_size: candidates.len(),
        held_out: held.len(),
        catalog_size: catalog.by_tmdb.len(),
        population_mean: catalog.population_mean,
        ablations,
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

fn mean_ablations(folds: &[QualityCalibrationFold]) -> Vec<MeanAblation> {
    if folds.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = folds[0].ablations.iter().map(|a| a.name.clone()).collect();
    names
        .into_iter()
        .map(|name| {
            let rows: Vec<&AblationMetrics> = folds
                .iter()
                .filter_map(|f| f.ablations.iter().find(|a| a.name == name))
                .collect();
            let note = rows.first().and_then(|r| r.note.clone());
            let mut m = MeanAblation {
                name,
                note,
                loved_vs_disliked: summarize(
                    &rows.iter().map(|r| r.loved_vs_disliked).collect::<Vec<_>>(),
                ),
                liked_vs_disliked: summarize(
                    &rows.iter().map(|r| r.liked_vs_disliked).collect::<Vec<_>>(),
                ),
                mean_rank_loved: summarize(
                    &rows
                        .iter()
                        .filter_map(|r| r.mean_rank_loved)
                        .collect::<Vec<_>>(),
                ),
                mean_rank_disliked: summarize(
                    &rows
                        .iter()
                        .filter_map(|r| r.mean_rank_disliked)
                        .collect::<Vec<_>>(),
                ),
                ndcg_at_100: summarize(
                    &rows.iter().map(|r| r.ndcg_at_100).collect::<Vec<_>>(),
                ),
                ndcg_at_250: summarize(
                    &rows.iter().map(|r| r.ndcg_at_250).collect::<Vec<_>>(),
                ),
                delta_loved_vs_disliked: summarize(
                    &rows
                        .iter()
                        .map(|r| r.delta_vs_content.loved_vs_disliked)
                        .collect::<Vec<_>>(),
                ),
                delta_mean_rank_loved: summarize(
                    &rows
                        .iter()
                        .filter_map(|r| r.delta_vs_content.mean_rank_loved)
                        .collect::<Vec<_>>(),
                ),
                delta_mean_rank_disliked: summarize(
                    &rows
                        .iter()
                        .filter_map(|r| r.delta_vs_content.mean_rank_disliked)
                        .collect::<Vec<_>>(),
                ),
                delta_ndcg_at_250: summarize(
                    &rows
                        .iter()
                        .map(|r| r.delta_vs_content.ndcg_at_250)
                        .collect::<Vec<_>>(),
                ),
                positive_rank_gain: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.positive_rank_gain)
                        .collect::<Vec<_>>(),
                ),
                positive_rank_loss: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.positive_rank_loss)
                        .collect::<Vec<_>>(),
                ),
                negative_rank_gain: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.negative_rank_gain)
                        .collect::<Vec<_>>(),
                ),
                negative_rank_loss: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.negative_rank_loss)
                        .collect::<Vec<_>>(),
                ),
                median_abs_delta: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.median_abs_delta)
                        .collect::<Vec<_>>(),
                ),
                p90_abs_delta: summarize(
                    &rows
                        .iter()
                        .map(|r| r.movement.p90_abs_delta)
                        .collect::<Vec<_>>(),
                ),
                gate: String::new(),
            };
            m.gate = gate_component(&m);
            m
        })
        .collect()
}

fn decide_gate(means: &[MeanAblation]) -> String {
    let mut include = Vec::new();
    let mut fail = Vec::new();
    let mut hold = Vec::new();
    for m in means {
        if m.name == "contentOnly" {
            continue;
        }
        match m.gate.as_str() {
            "PASS_INCLUDE" => include.push(m.name.clone()),
            "FAIL_KEEP_ZERO" => fail.push(m.name.clone()),
            _ => hold.push(m.name.clone()),
        }
    }
    if include.is_empty() {
        format!(
            "HOLD_B4: no Quality variant earned inclusion (fail={fail:?}, hold={hold:?}). Fit_v1 = Content; move to eligibility/presentation layer."
        )
    } else {
        format!(
            "PASS_B4_PARTIAL: include {include:?}; keep zero for fail={fail:?} hold={hold:?}."
        )
    }
}

pub fn run_quality_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<QualityCalibrationReport, String> {
    let catalog = load_quality_catalog(db)?;
    let mut fold_reports = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        fold_reports.push(evaluate_fold(db, seed, &inputs, films, &catalog)?);
    }
    let mean_ablations = mean_ablations(&fold_reports);
    let gate = decide_gate(&mean_ablations);
    Ok(QualityCalibrationReport {
        protocol: "milestone-b4-quality-prior".into(),
        algorithm_version: ALGORITHM_VERSION.into(),
        folds: seeds.len(),
        holdout_frac,
        seeds: seeds.to_vec(),
        fold_reports,
        mean_ablations,
        gate,
    })
}

pub fn write_quality_calibration_artifact(
    taste_runs_dir: &std::path::Path,
    report: &QualityCalibrationReport,
) -> Result<String, String> {
    let dir = taste_runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("quality-calibration-{stamp}.json"));
    let latest = dir.join("quality-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ablation_matrix_tests_polarity_independently() {
        let names: Vec<_> = ablation_matrix().into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "contentOnly",
                "qualityPositive",
                "qualityNegative",
                "fullQuality"
            ]
        );
    }
}
