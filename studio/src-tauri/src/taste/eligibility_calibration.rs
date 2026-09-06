//! C3/C4: eligibility decision-layer calibration + board-size sims.
//!
//! Simulates the New candidate set under Content Fit_v1 eligibility
//! (Recommended / Exploratory / Held). Does not diversify.

use crate::storage::db::Database;
use crate::taste::eligibility::EligibilityState;
use crate::taste::eval::{
    rating_bucket, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
};
use crate::taste::match_calibration::{
    bucket_outcome, fit_isotonic_bins, fit_to_match_percent, MatchCalibrationReport,
};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord, MediaKind,
    RetrievalKind, RetrievalSource,
};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::score_candidates_from_cache;
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const BOARD_SIZES: &[usize] = &[5, 10, 12, 20, 25];

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BucketEligibility {
    pub n: usize,
    pub eligible_pct: f32,
    pub recommended_pct: f32,
    pub exploratory_pct: f32,
    pub held_pct: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSim {
    pub size: usize,
    pub n: usize,
    pub loved_share: f32,
    pub liked_share: f32,
    pub meh_share: f32,
    pub disliked_share: f32,
    pub mean_match: f32,
    pub exploratory_share: f32,
    pub mean_bucket_score: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityFold {
    pub seed: u64,
    pub holdout_n: usize,
    pub pool_n: usize,
    pub eligible_count: usize,
    pub recommended_count: usize,
    pub exploratory_count: usize,
    pub held_count: usize,
    pub loved: BucketEligibility,
    pub liked: BucketEligibility,
    pub meh: BucketEligibility,
    pub disliked: BucketEligibility,
    pub precision_eligible: f32,
    pub recall_eligible_positives: f32,
    pub high_fit_medium_conf_held_pct: f32,
    pub held_reasons: HashMap<String, usize>,
    pub boards: Vec<BoardSim>,
    pub match_pairs: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityCalibrationReport {
    pub algorithm_version: String,
    pub seeds: Vec<u64>,
    pub holdout_frac: f32,
    pub folds: Vec<EligibilityFold>,
    pub mean_eligible_count: MetricSummary,
    pub mean_loved_eligible_pct: MetricSummary,
    pub mean_liked_eligible_pct: MetricSummary,
    pub mean_disliked_eligible_pct: MetricSummary,
    pub mean_precision_eligible: MetricSummary,
    pub mean_recall_eligible_positives: MetricSummary,
    pub mean_recommended_share: MetricSummary,
    pub mean_high_fit_held_pct: MetricSummary,
    pub mean_board12_disliked_share: MetricSummary,
    pub mean_board12_loved_liked_share: MetricSummary,
    pub match_calibration: MatchCalibrationReport,
    pub gate: String,
}

fn id_of_scored(c: &ScoredCandidate) -> Option<String> {
    c.candidate
        .tmdb_id
        .map(|id| identity_key(Some(id), &c.candidate.title, c.candidate.year))
}

fn held_rated<'a>(all: &'a [FilmRecord], held: &HashSet<String>) -> Vec<&'a FilmRecord> {
    all.iter()
        .filter(|f| {
            f.rating.is_some() && held.contains(&identity_key(f.tmdb_id, &f.title, f.year))
        })
        .collect()
}

fn state_of(c: &ScoredCandidate) -> EligibilityState {
    match c.eligibility.state.as_str() {
        "recommended" => EligibilityState::Recommended,
        "exploratory" => EligibilityState::Exploratory,
        _ => EligibilityState::Held,
    }
}

fn bucket_stats(rows: &[&ScoredCandidate]) -> BucketEligibility {
    let n = rows.len();
    if n == 0 {
        return BucketEligibility::default();
    }
    let mut rec = 0usize;
    let mut exp = 0usize;
    let mut held = 0usize;
    for r in rows {
        match state_of(r) {
            EligibilityState::Recommended => rec += 1,
            EligibilityState::Exploratory => exp += 1,
            EligibilityState::Held => held += 1,
        }
    }
    let nf = n as f32;
    BucketEligibility {
        n,
        eligible_pct: (rec + exp) as f32 / nf,
        recommended_pct: rec as f32 / nf,
        exploratory_pct: exp as f32 / nf,
        held_pct: held as f32 / nf,
    }
}

fn board_sim(
    eligible_ranked: &[&ScoredCandidate],
    bucket_of: &HashMap<String, RatingBucket>,
    size: usize,
) -> BoardSim {
    let take: Vec<_> = eligible_ranked.iter().take(size).copied().collect();
    let n = take.len();
    if n == 0 {
        return BoardSim {
            size,
            n: 0,
            loved_share: 0.0,
            liked_share: 0.0,
            meh_share: 0.0,
            disliked_share: 0.0,
            mean_match: 0.0,
            exploratory_share: 0.0,
            mean_bucket_score: 0.0,
        };
    }
    let mut loved = 0usize;
    let mut liked = 0usize;
    let mut meh = 0usize;
    let mut disliked = 0usize;
    let mut match_sum = 0.0f32;
    let mut exp = 0usize;
    let mut bucket_score = 0.0f32;
    let mut labeled = 0usize;
    for c in &take {
        match_sum += fit_to_match_percent(c.eligibility.predicted_fit) as f32;
        if state_of(c) == EligibilityState::Exploratory {
            exp += 1;
        }
        let Some(id) = id_of_scored(c) else { continue };
        let Some(b) = bucket_of.get(&id) else { continue };
        labeled += 1;
        match b {
            RatingBucket::Loved => {
                loved += 1;
                bucket_score += 1.0;
            }
            RatingBucket::Liked => {
                liked += 1;
                bucket_score += 0.75;
            }
            RatingBucket::Meh => {
                meh += 1;
                bucket_score += 0.45;
            }
            RatingBucket::Disliked => {
                disliked += 1;
                bucket_score += 0.15;
            }
        }
    }
    let nf = n as f32;
    let lf = labeled.max(1) as f32;
    BoardSim {
        size,
        n,
        loved_share: loved as f32 / lf,
        liked_share: liked as f32 / lf,
        meh_share: meh as f32 / lf,
        disliked_share: disliked as f32 / lf,
        mean_match: match_sum / nf,
        exploratory_share: exp as f32 / nf,
        mean_bucket_score: bucket_score / lf,
    }
}

fn film_as_eval_candidate(f: &FilmRecord) -> Candidate {
    Candidate {
        tmdb_id: f.tmdb_id,
        title: f.title.clone(),
        year: f.year,
        poster: f.poster.clone(),
        genres: f.genres.clone(),
        credits: f.credits.clone(),
        keywords: f.keywords.clone(),
        runtime: f.runtime,
        vote_count: f.vote_count,
        watchlist: false,
        sources: vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: "holdout-eval".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }],
        friend_affinity: 0.0,
        tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
    }
}

fn evaluate_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
) -> Result<(EligibilityFold, Vec<(f32, f32)>), String> {
    let held = held_rated(all_films, &inputs.held_out);
    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let candidates = select_fair_pool(pool.by_key, 1000);
    let semantic = score_candidates_from_cache(db, &inputs.training_films, &candidates);
    let scored_pool = score_pool_with_semantic(&inputs.profile, &candidates, &semantic).ranked;

    // Depth: how many New-eligible rows the live retrieval pool produces.
    let mut recommended_count = 0usize;
    let mut exploratory_count = 0usize;
    let mut held_count = 0usize;
    let mut eligible_ranked: Vec<&ScoredCandidate> = Vec::new();
    for c in &scored_pool {
        match state_of(c) {
            EligibilityState::Recommended => {
                recommended_count += 1;
                eligible_ranked.push(c);
            }
            EligibilityState::Exploratory => {
                exploratory_count += 1;
                eligible_ranked.push(c);
            }
            EligibilityState::Held => held_count += 1,
        }
    }
    eligible_ranked.sort_by(|a, b| {
        b.eligibility
            .predicted_fit
            .partial_cmp(&a.eligibility.predicted_fit)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.score
                    .total
                    .partial_cmp(&a.score.total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Decision-layer rates: force-score every held-out rated film (retrieval
    // recall is out of scope — C1 asks whether Content Fit would admit them).
    let holdout_cands: Vec<Candidate> = held.iter().map(|f| film_as_eval_candidate(f)).collect();
    let holdout_semantic =
        score_candidates_from_cache(db, &inputs.training_films, &holdout_cands);
    // Score individually so Held rows are kept (score_pool drops !passed).
    let holdout_scored: Vec<ScoredCandidate> = holdout_cands
        .iter()
        .map(|c| {
            let semantic = c
                .tmdb_id
                .and_then(|id| holdout_semantic.get(&id))
                .cloned()
                .unwrap_or_default();
            crate::taste::score::score_candidate_with_semantic(&inputs.profile, c, &semantic)
        })
        .collect();
    // Relative bands from the holdout Content distribution (same compression as live).
    let mut holdout_scored = holdout_scored;
    crate::taste::eligibility::apply_pool_bands(&mut holdout_scored);

    let mut bucket_of = HashMap::new();
    for f in &held {
        if let Some(r) = f.rating {
            bucket_of.insert(
                identity_key(f.tmdb_id, &f.title, f.year),
                rating_bucket(r),
            );
        }
    }

    let mut by_bucket: HashMap<RatingBucket, Vec<&ScoredCandidate>> = HashMap::new();
    let mut held_reasons: HashMap<String, usize> = HashMap::new();
    let mut match_pairs = Vec::new();
    let mut high_fit_med = 0usize;
    let mut high_fit_med_held = 0usize;

    for c in &holdout_scored {
        let Some(id) = id_of_scored(c) else { continue };
        let Some(bucket) = bucket_of.get(&id).copied() else {
            continue;
        };
        by_bucket.entry(bucket).or_default().push(c);
        match_pairs.push((
            c.eligibility.predicted_fit,
            bucket_outcome(match bucket {
                RatingBucket::Loved => "loved",
                RatingBucket::Liked => "liked",
                RatingBucket::Meh => "meh",
                RatingBucket::Disliked => "disliked",
            }),
        ));
        if state_of(c) == EligibilityState::Held {
            *held_reasons
                .entry(c.eligibility.primary_reason.clone())
                .or_default() += 1;
        }
    // Also treat high-fit held diagnostic with the live excellent band.
        if c.eligibility.predicted_fit >= 0.67 && c.eligibility.confidence >= 0.35 {
            high_fit_med += 1;
            if state_of(c) == EligibilityState::Held {
                high_fit_med_held += 1;
            }
        }
    }

    let loved = bucket_stats(by_bucket.get(&RatingBucket::Loved).map(|v| v.as_slice()).unwrap_or(&[]));
    let liked = bucket_stats(by_bucket.get(&RatingBucket::Liked).map(|v| v.as_slice()).unwrap_or(&[]));
    let meh = bucket_stats(by_bucket.get(&RatingBucket::Meh).map(|v| v.as_slice()).unwrap_or(&[]));
    let disliked =
        bucket_stats(by_bucket.get(&RatingBucket::Disliked).map(|v| v.as_slice()).unwrap_or(&[]));

    let eligible_holdout: Vec<&ScoredCandidate> = by_bucket
        .values()
        .flatten()
        .copied()
        .filter(|c| state_of(c).board_eligible())
        .collect();
    let positives: HashSet<String> = held
        .iter()
        .filter(|f| {
            matches!(
                f.rating.map(rating_bucket),
                Some(RatingBucket::Loved | RatingBucket::Liked)
            )
        })
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect();
    let eligible_pos = eligible_holdout
        .iter()
        .filter(|c| id_of_scored(c).map(|id| positives.contains(&id)).unwrap_or(false))
        .count();
    let precision_eligible = if eligible_holdout.is_empty() {
        0.0
    } else {
        eligible_pos as f32 / eligible_holdout.len() as f32
    };
    let recall_eligible_positives = if positives.is_empty() {
        0.0
    } else {
        let in_pool_pos = by_bucket
            .get(&RatingBucket::Loved)
            .into_iter()
            .flatten()
            .chain(by_bucket.get(&RatingBucket::Liked).into_iter().flatten())
            .filter(|c| state_of(c).board_eligible())
            .count();
        in_pool_pos as f32 / positives.len() as f32
    };

    // Board sims among holdout-eligible ordered by Content fit (contamination check).
    let mut holdout_eligible_ranked: Vec<&ScoredCandidate> = eligible_holdout.clone();
    holdout_eligible_ranked.sort_by(|a, b| {
        b.eligibility
            .predicted_fit
            .partial_cmp(&a.eligibility.predicted_fit)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let boards: Vec<_> = BOARD_SIZES
        .iter()
        .map(|&s| board_sim(&holdout_eligible_ranked, &bucket_of, s))
        .collect();

    let fold = EligibilityFold {
        seed,
        holdout_n: held.len(),
        pool_n: scored_pool.len(),
        eligible_count: recommended_count + exploratory_count,
        recommended_count,
        exploratory_count,
        held_count,
        loved,
        liked,
        meh,
        disliked,
        precision_eligible,
        recall_eligible_positives,
        high_fit_medium_conf_held_pct: if high_fit_med == 0 {
            0.0
        } else {
            high_fit_med_held as f32 / high_fit_med as f32
        },
        held_reasons,
        boards,
        match_pairs: match_pairs.len(),
    };
    let _ = eligible_ranked; // pool depth already captured in counts
    Ok((fold, match_pairs))
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

fn gate_report(report: &EligibilityCalibrationReport) -> String {
    let mut fail = Vec::new();
    if report.mean_eligible_count.mean < 25.0 {
        fail.push(format!(
            "thin_depth:{:.1}",
            report.mean_eligible_count.mean
        ));
    }
    if report.mean_eligible_count.min < 12.0 {
        fail.push(format!("fold_too_thin:{:.0}", report.mean_eligible_count.min));
    }
    let sep_loved =
        report.mean_loved_eligible_pct.mean - report.mean_disliked_eligible_pct.mean;
    if sep_loved < 0.15 {
        fail.push(format!("loved_disliked_sep:{sep_loved:.3}"));
    }
    let sep_liked =
        report.mean_liked_eligible_pct.mean - report.mean_disliked_eligible_pct.mean;
    if sep_liked < 0.08 {
        fail.push(format!("liked_disliked_sep:{sep_liked:.3}"));
    }
    if report.mean_recommended_share.mean < 0.25 {
        fail.push(format!(
            "recommended_share:{:.3}",
            report.mean_recommended_share.mean
        ));
    }
    if report.mean_high_fit_held_pct.mean > 0.40 {
        fail.push(format!(
            "high_fit_held:{:.3}",
            report.mean_high_fit_held_pct.mean
        ));
    }
    if report.mean_board12_disliked_share.mean > 0.35 {
        fail.push(format!(
            "board12_dislike:{:.3}",
            report.mean_board12_disliked_share.mean
        ));
    }
    // Monotonicity of MATCH_CURVE_V1 is unit-tested; also check fitted curve.
    let mut prev = 0u8;
    for &(_, y) in &report.match_calibration.curve {
        if y < prev {
            fail.push("match_curve_non_monotonic".into());
            break;
        }
        prev = y;
    }

    if fail.is_empty() {
        "PASS_C1".into()
    } else {
        format!("HOLD_C1: {}", fail.join(", "))
    }
}

pub fn run_eligibility_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<EligibilityCalibrationReport, String> {
    let mut folds = Vec::new();
    let mut all_pairs = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        let (fold, pairs) = evaluate_fold(db, seed, &inputs, films)?;
        all_pairs.extend(pairs);
        folds.push(fold);
    }

    let eligible_counts: Vec<_> = folds.iter().map(|f| f.eligible_count as f32).collect();
    let loved_elig: Vec<_> = folds.iter().map(|f| f.loved.eligible_pct).collect();
    let liked_elig: Vec<_> = folds.iter().map(|f| f.liked.eligible_pct).collect();
    let dis_elig: Vec<_> = folds.iter().map(|f| f.disliked.eligible_pct).collect();
    let prec: Vec<_> = folds.iter().map(|f| f.precision_eligible).collect();
    let recall: Vec<_> = folds.iter().map(|f| f.recall_eligible_positives).collect();
    let rec_share: Vec<_> = folds
        .iter()
        .map(|f| {
            let e = f.eligible_count.max(1) as f32;
            f.recommended_count as f32 / e
        })
        .collect();
    let high_fit: Vec<_> = folds
        .iter()
        .map(|f| f.high_fit_medium_conf_held_pct)
        .collect();
    let b12_dis: Vec<_> = folds
        .iter()
        .filter_map(|f| f.boards.iter().find(|b| b.size == 12).map(|b| b.disliked_share))
        .collect();
    let b12_pos: Vec<_> = folds
        .iter()
        .filter_map(|f| {
            f.boards
                .iter()
                .find(|b| b.size == 12)
                .map(|b| b.loved_share + b.liked_share)
        })
        .collect();

    let match_calibration = fit_isotonic_bins(&all_pairs, 0.05);
    let mut report = EligibilityCalibrationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        seeds: seeds.to_vec(),
        holdout_frac,
        folds,
        mean_eligible_count: summarize(&eligible_counts),
        mean_loved_eligible_pct: summarize(&loved_elig),
        mean_liked_eligible_pct: summarize(&liked_elig),
        mean_disliked_eligible_pct: summarize(&dis_elig),
        mean_precision_eligible: summarize(&prec),
        mean_recall_eligible_positives: summarize(&recall),
        mean_recommended_share: summarize(&rec_share),
        mean_high_fit_held_pct: summarize(&high_fit),
        mean_board12_disliked_share: summarize(&b12_dis),
        mean_board12_loved_liked_share: summarize(&b12_pos),
        match_calibration,
        gate: String::new(),
    };
    report.gate = gate_report(&report);
    Ok(report)
}

pub fn write_eligibility_calibration_artifact(
    out_dir: &Path,
    report: &EligibilityCalibrationReport,
) -> Result<String, String> {
    let bench = out_dir.join("benchmarks");
    std::fs::create_dir_all(&bench).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let stamped = bench.join(format!("eligibility-calibration-{stamp}.json"));
    let latest = bench.join("eligibility-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&stamped, &body).map_err(|e| e.to_string())?;
    std::fs::write(&latest, &body).map_err(|e| e.to_string())?;
    Ok(stamped.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::eligibility::{classify_eligibility, EligibilityInput};

    #[test]
    fn gate_rejects_thin_boards() {
        let zero = MetricSummary {
            mean: 0.0,
            min: 0.0,
            max: 0.0,
            stdev: 0.0,
        };
        let mut report = EligibilityCalibrationReport {
            algorithm_version: "test".into(),
            seeds: vec![1],
            holdout_frac: 0.15,
            folds: vec![],
            mean_eligible_count: MetricSummary {
                mean: 4.0,
                min: 2.0,
                max: 5.0,
                stdev: 1.0,
            },
            mean_loved_eligible_pct: MetricSummary {
                mean: 0.9,
                min: 0.9,
                max: 0.9,
                stdev: 0.0,
            },
            mean_liked_eligible_pct: MetricSummary {
                mean: 0.8,
                min: 0.8,
                max: 0.8,
                stdev: 0.0,
            },
            mean_disliked_eligible_pct: MetricSummary {
                mean: 0.1,
                min: 0.1,
                max: 0.1,
                stdev: 0.0,
            },
            mean_precision_eligible: zero.clone(),
            mean_recall_eligible_positives: zero.clone(),
            mean_recommended_share: MetricSummary {
                mean: 0.5,
                min: 0.5,
                max: 0.5,
                stdev: 0.0,
            },
            mean_high_fit_held_pct: zero.clone(),
            mean_board12_disliked_share: MetricSummary {
                mean: 0.1,
                min: 0.1,
                max: 0.1,
                stdev: 0.0,
            },
            mean_board12_loved_liked_share: zero,
            match_calibration: fit_isotonic_bins(&[(0.5, 0.7), (0.7, 0.9)], 0.1),
            gate: String::new(),
        };
        report.gate = gate_report(&report);
        assert!(report.gate.starts_with("HOLD_C1"), "{}", report.gate);
    }

    #[test]
    fn classifier_inputs_exclude_retrieval_fields() {
        // Compile-time / API guard: EligibilityInput has no source/grade fields.
        let _ = std::mem::size_of::<EligibilityInput>();
        let d = classify_eligibility(&EligibilityInput {
            predicted_fit: 0.8,
            confidence: 0.5,
            hydration_completeness: 0.7,
            semantic_coverage: true,
            semantic_negative: 0.1,
            semantic_margin: 0.2,
        });
        assert!(d.state.board_eligible());
    }
}
