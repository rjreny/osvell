//! Board diversification calibration: rawContent vs light vs strong.

use crate::storage::db::Database;
use crate::taste::diversify::{
    diversify_board, diversify_trace, duplicate_cluster_counts, DiversifyConfig,
};
use crate::taste::eval::{
    ndcg_at, rating_bucket, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, FilmRecord,
};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::score_candidates_from_cache;
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const BOARD_SIZES: &[usize] = &[5, 10, 12, 20, 25];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardVariantMetrics {
    pub size: usize,
    pub loved_share: f32,
    pub liked_share: f32,
    pub disliked_share: f32,
    pub positive_share: f32,
    pub mean_raw_fit: f32,
    pub mean_fit_loss_from_diversification: f32,
    pub max_fit_loss_from_diversification: f32,
    pub reordered_slots: usize,
    pub max_rank_jump: i32,
    pub ndcg: f32,
    pub positive_rescued: usize,
    pub positive_damaged: usize,
    pub collection_dup_extras: usize,
    pub director_dup_extras: usize,
    pub mode_dup_extras: usize,
    pub semantic_dup_extras: usize,
    pub semantic_cluster_dup_extras: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantFold {
    pub name: String,
    pub boards: Vec<BoardVariantMetrics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiversifyFold {
    pub seed: u64,
    pub eligible_n: usize,
    pub variants: Vec<VariantFold>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeanBoard {
    pub size: usize,
    pub positive_share: MetricSummary,
    pub disliked_share: MetricSummary,
    pub mean_fit_loss: MetricSummary,
    pub max_fit_loss: MetricSummary,
    pub max_rank_jump: MetricSummary,
    pub ndcg: MetricSummary,
    pub collection_dup_extras: MetricSummary,
    pub semantic_cluster_dup_extras: MetricSummary,
    pub reordered_slots: MetricSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeanVariant {
    pub name: String,
    pub boards: Vec<MeanBoard>,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiversifyCalibrationReport {
    pub algorithm_version: String,
    pub seeds: Vec<u64>,
    pub folds: Vec<DiversifyFold>,
    pub mean_variants: Vec<MeanVariant>,
    pub gate: String,
}

fn fit_of(c: &ScoredCandidate) -> f32 {
    if c.eligibility.predicted_fit > 0.0 {
        c.eligibility.predicted_fit
    } else {
        ((c.score.content + 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

fn state_ok(c: &ScoredCandidate) -> bool {
    matches!(
        c.eligibility.state.as_str(),
        "recommended" | "exploratory"
    ) || (c.eligibility.passed && c.eligibility.predicted_fit >= 0.50)
}

fn summarize(xs: &[f32]) -> MetricSummary {
    if xs.is_empty() {
        return MetricSummary {
            mean: 0.0,
            min: 0.0,
            max: 0.0,
            stdev: 0.0,
        };
    }
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    let min = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
    MetricSummary {
        mean,
        min,
        max,
        stdev: var.sqrt(),
    }
}

fn is_positive(b: &RatingBucket) -> bool {
    matches!(b, RatingBucket::Loved | RatingBucket::Liked)
}

fn board_metrics(
    raw: &[ScoredCandidate],
    board: &[ScoredCandidate],
    size: usize,
    bucket_of: &HashMap<String, RatingBucket>,
    held_out: &HashSet<String>,
) -> BoardVariantMetrics {
    let take: Vec<_> = board.iter().take(size).cloned().collect();
    let n = take.len().max(1);
    let trace = diversify_trace(raw, board, size);
    let (col, dir, mode, sem, sc) = duplicate_cluster_counts(&take);

    let mut loved = 0usize;
    let mut liked = 0usize;
    let mut disliked = 0usize;
    let mut labeled = 0usize;
    let mut fit_sum = 0.0f32;
    for c in &take {
        fit_sum += fit_of(c);
    }
    for c in &take {
        if let Some(tid) = c.candidate.tmdb_id {
            let id = format!("tmdb:{tid}");
            if let Some(b) = bucket_of.get(&id) {
                labeled += 1;
                match b {
                    RatingBucket::Loved => loved += 1,
                    RatingBucket::Liked => liked += 1,
                    RatingBucket::Disliked => disliked += 1,
                    RatingBucket::Meh => {}
                }
            }
        }
    }

    let board_ids: Vec<String> = take
        .iter()
        .filter_map(|c| c.candidate.tmdb_id.map(|id| format!("tmdb:{id}")))
        .collect();
    let ndcg = ndcg_at(&board_ids, held_out, size);

    let raw_top: HashSet<String> = raw
        .iter()
        .take(size)
        .filter_map(|c| c.candidate.tmdb_id.map(|id| format!("tmdb:{id}")))
        .collect();
    let board_top: HashSet<String> = board_ids.iter().cloned().collect();
    let mut positive_rescued = 0usize;
    let mut positive_damaged = 0usize;
    for (id, b) in bucket_of {
        if !is_positive(b) {
            continue;
        }
        let in_raw = raw_top.contains(id);
        let in_board = board_top.contains(id);
        if in_board && !in_raw {
            positive_rescued += 1;
        }
        if in_raw && !in_board {
            positive_damaged += 1;
        }
    }

    let lf = labeled.max(1) as f32;
    BoardVariantMetrics {
        size,
        loved_share: loved as f32 / lf,
        liked_share: liked as f32 / lf,
        disliked_share: disliked as f32 / lf,
        positive_share: (loved + liked) as f32 / lf,
        mean_raw_fit: fit_sum / n as f32,
        mean_fit_loss_from_diversification: trace.mean_fit_loss,
        max_fit_loss_from_diversification: trace.max_fit_loss,
        reordered_slots: trace.reordered_slots,
        max_rank_jump: trace.max_rank_jump,
        ndcg,
        positive_rescued,
        positive_damaged,
        collection_dup_extras: col,
        director_dup_extras: dir,
        mode_dup_extras: mode,
        semantic_dup_extras: sem,
        semantic_cluster_dup_extras: sc,
    }
}

fn evaluate_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
) -> Result<DiversifyFold, String> {
    let held: HashSet<String> = inputs.held_out.clone();
    let mut bucket_of = HashMap::new();
    for f in all_films {
        let id = identity_key(f.tmdb_id, &f.title, f.year);
        if held.contains(&id) {
            if let Some(r) = f.rating {
                bucket_of.insert(id, rating_bucket(r));
            }
        }
    }

    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let candidates = select_fair_pool(pool.by_key, 1000);
    let semantic = score_candidates_from_cache(db, &inputs.training_films, &candidates);
    let mut scored = score_pool_with_semantic(&inputs.profile, &candidates, &semantic, None).ranked;
    crate::taste::semantic::attach_semantic_clusters_from_db(db, &mut scored);
    let mut eligible: Vec<ScoredCandidate> = scored.into_iter().filter(|c| state_ok(c)).collect();
    eligible.sort_by(|a, b| {
        fit_of(b)
            .partial_cmp(&fit_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let named: Vec<(String, DiversifyConfig)> = vec![
        ("rawContent".into(), DiversifyConfig::raw()),
        ("fitTol0025".into(), DiversifyConfig::light_with_epsilon(0.0025)),
        ("fitTol0050".into(), DiversifyConfig::light_with_epsilon(0.005)),
        ("fitTol0075".into(), DiversifyConfig::light_with_epsilon(0.0075)),
        ("fitTol0100".into(), DiversifyConfig::light_with_epsilon(0.010)),
        ("lightDiversify".into(), DiversifyConfig::light()),
        ("strongDiversify".into(), DiversifyConfig::strong()),
    ];
    let mut variants = Vec::new();
    for (name, cfg) in &named {
        let board = diversify_board(&eligible, cfg);
        let boards: Vec<_> = BOARD_SIZES
            .iter()
            .map(|&s| board_metrics(&eligible, &board, s, &bucket_of, &held))
            .collect();
        variants.push(VariantFold {
            name: name.clone(),
            boards,
        });
    }

    Ok(DiversifyFold {
        seed,
        eligible_n: eligible.len(),
        variants,
    })
}

fn gate_variant(control: &MeanVariant, candidate: &MeanVariant) -> String {
    if candidate.name == "rawContent" {
        return "CONTROL".into();
    }
    let c12 = control.boards.iter().find(|b| b.size == 12);
    let v12 = candidate.boards.iter().find(|b| b.size == 12);
    let (Some(c12), Some(v12)) = (c12, v12) else {
        return "HOLD_NO_BOARD12".into();
    };

    let fit_tol = candidate.name.starts_with("fitTol") || candidate.name == "lightDiversify";
    let mut fail = Vec::new();
    if v12.positive_share.mean + 0.02 < c12.positive_share.mean {
        fail.push(format!(
            "pos_drop:{:.3}->{:.3}",
            c12.positive_share.mean, v12.positive_share.mean
        ));
    }
    if v12.disliked_share.mean > c12.disliked_share.mean + 0.02 {
        fail.push(format!(
            "dislike_up:{:.3}->{:.3}",
            c12.disliked_share.mean, v12.disliked_share.mean
        ));
    }
    let loss_cap = if fit_tol { 0.010 } else { 0.012 };
    if v12.mean_fit_loss.mean > loss_cap {
        fail.push(format!("fit_loss:{:.4}", v12.mean_fit_loss.mean));
    }
    if fit_tol {
        if v12.max_fit_loss.mean > 0.012 {
            fail.push(format!("max_fit_loss:{:.4}", v12.max_fit_loss.mean));
        }
    } else if v12.max_rank_jump.mean > 8.0 || v12.max_rank_jump.max > 12.0 {
        fail.push(format!(
            "rank_jump:mean={:.1} max={:.0}",
            v12.max_rank_jump.mean, v12.max_rank_jump.max
        ));
    }
    let cluster_ok = v12.collection_dup_extras.mean + 0.15 <= c12.collection_dup_extras.mean
        || c12.collection_dup_extras.mean < 0.5
        || v12.semantic_cluster_dup_extras.mean + 0.15 <= c12.semantic_cluster_dup_extras.mean;
    let diversity_moved =
        v12.reordered_slots.mean >= 0.5 || c12.collection_dup_extras.mean < 0.8;
    if !cluster_ok && c12.collection_dup_extras.mean >= 1.0 {
        fail.push(format!(
            "clusters:{:.2}->{:.2}",
            c12.collection_dup_extras.mean, v12.collection_dup_extras.mean
        ));
    }
    if !diversity_moved && c12.collection_dup_extras.mean >= 1.5 {
        fail.push("no_reorder_despite_clusters".into());
    }

    if fail.is_empty() {
        "PASS_INCLUDE".into()
    } else {
        format!("FAIL_KEEP_RAW: {}", fail.join(", "))
    }
}

pub fn run_diversify_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<DiversifyCalibrationReport, String> {
    let mut folds = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        folds.push(evaluate_fold(db, seed, &inputs, films)?);
    }

    let names = [
        "rawContent",
        "fitTol0025",
        "fitTol0050",
        "fitTol0075",
        "fitTol0100",
        "lightDiversify",
        "strongDiversify",
    ];
    let mut mean_variants = Vec::new();
    for name in names {
        let mut boards = Vec::new();
        for &size in BOARD_SIZES {
            let mut pos = Vec::new();
            let mut dis = Vec::new();
            let mut loss = Vec::new();
            let mut max_loss = Vec::new();
            let mut jump = Vec::new();
            let mut ndcg = Vec::new();
            let mut col = Vec::new();
            let mut sc = Vec::new();
            let mut reorder = Vec::new();
            for fold in &folds {
                if let Some(v) = fold.variants.iter().find(|v| v.name == name) {
                    if let Some(b) = v.boards.iter().find(|b| b.size == size) {
                        pos.push(b.positive_share);
                        dis.push(b.disliked_share);
                        loss.push(b.mean_fit_loss_from_diversification);
                        max_loss.push(b.max_fit_loss_from_diversification);
                        jump.push(b.max_rank_jump as f32);
                        ndcg.push(b.ndcg);
                        col.push(b.collection_dup_extras as f32);
                        sc.push(b.semantic_cluster_dup_extras as f32);
                        reorder.push(b.reordered_slots as f32);
                    }
                }
            }
            boards.push(MeanBoard {
                size,
                positive_share: summarize(&pos),
                disliked_share: summarize(&dis),
                mean_fit_loss: summarize(&loss),
                max_fit_loss: summarize(&max_loss),
                max_rank_jump: summarize(&jump),
                ndcg: summarize(&ndcg),
                collection_dup_extras: summarize(&col),
                semantic_cluster_dup_extras: summarize(&sc),
                reordered_slots: summarize(&reorder),
            });
        }
        mean_variants.push(MeanVariant {
            name: name.into(),
            boards,
            gate: String::new(),
        });
    }

    let control = mean_variants
        .iter()
        .find(|v| v.name == "rawContent")
        .cloned()
        .expect("rawContent");
    for v in &mut mean_variants {
        v.gate = gate_variant(&control, v);
    }

    let pick = ["fitTol0050", "fitTol0075", "fitTol0025", "fitTol0100", "lightDiversify"]
        .into_iter()
        .find(|n| {
            mean_variants
                .iter()
                .find(|v| v.name == *n)
                .map(|v| v.gate.starts_with("PASS"))
                .unwrap_or(false)
        })
        .unwrap_or("lightDiversify");

    let pick_gate = mean_variants
        .iter()
        .find(|v| v.name == pick)
        .map(|v| v.gate.clone())
        .unwrap_or_default();
    let gate = if pick_gate.starts_with("PASS") {
        format!("PASS_D1_1: {pick}")
    } else {
        format!("HOLD_D1_1: preferred={pick} gate={pick_gate}")
    };

    Ok(DiversifyCalibrationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        seeds: seeds.to_vec(),
        folds,
        mean_variants,
        gate,
    })
}

pub fn write_diversify_calibration_artifact(
    out_dir: &Path,
    report: &DiversifyCalibrationReport,
) -> Result<String, String> {
    let bench = out_dir.join("benchmarks");
    std::fs::create_dir_all(&bench).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let stamped = bench.join(format!("diversify-calibration-{stamp}.json"));
    let latest = bench.join("diversify-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&stamped, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(stamped.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::diversify::DiversifyMode;

    #[test]
    fn modes_cover_ablation_matrix() {
        assert_eq!(DiversifyMode::RawContent.as_str(), "rawContent");
        assert_eq!(DiversifyMode::LightDiversify.as_str(), "lightDiversify");
        assert_eq!(DiversifyMode::StrongDiversify.as_str(), "strongDiversify");
        assert!(DiversifyConfig::light().fit_equivalence);
        assert!((DiversifyConfig::light().tie_epsilon - 0.0075).abs() < 1e-6);
    }
}
