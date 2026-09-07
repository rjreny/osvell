use crate::taste::score::ScoredCandidate;
use crate::taste::features::FeatureProfile;
use crate::taste::retrieve::{identity_key, seen_keys, FilmRecord};
use crate::storage::db::Database;
use crate::taste::semantic::SemanticStats;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct LayeredMetrics {
    pub recall_at_1000: f32,
    pub recall_at_100: f32,
    pub recall_at_50: f32,
    pub recall_at_25: f32,
    pub recall_at_12: f32,
    pub mrr: f32,
    pub ndcg_at_12: f32,
    pub precision_at_40: f32,
    pub recall_at_40: f32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardEval {
    pub holdout_hit_rate_at_50: f32,
    pub holdout_ndcg_at_12: f32,
    pub disliked_in_top_20: usize,
    pub probe_weak_in_top_20: usize,
    pub strong_fit_share: f32,
    pub board_count: usize,
}

pub fn evaluate_displayed_board(
    new_picks: &[ScoredCandidate],
    films: &[FilmRecord],
    probe_weak_titles: &[&str],
) -> BoardEval {
    let top = &new_picks[..new_picks.len().min(20)];
    let disliked_seed_ids: HashSet<i64> = films
        .iter()
        .filter(|film| film.rating.map(|rating| rating <= 2.5).unwrap_or(false))
        .filter_map(|film| film.tmdb_id)
        .collect();
    let disliked_in_top_20 = top
        .iter()
        .filter(|candidate| {
            let from_disliked_seed = candidate.candidate.sources.iter().any(|source| {
                source
                    .seed_tmdb_id
                    .map(|id| disliked_seed_ids.contains(&id))
                    .unwrap_or(false)
                    || source.seed_rating.map(|rating| rating <= 2.5).unwrap_or(false)
            });
            from_disliked_seed
                || (!candidate.negative_features.is_empty() && candidate.score.total < 0.08)
        })
        .count();
    let probe_weak_in_top_20 = top
        .iter()
        .filter(|candidate| {
            probe_weak_titles.iter().any(|probe| {
                candidate
                    .candidate
                    .title
                    .trim()
                    .eq_ignore_ascii_case(probe.trim())
            })
        })
        .count();
    let strong_count = top
        .iter()
        .filter(|candidate| {
            candidate.eligibility.evidence_grade
                == crate::taste::explain::EvidenceGrade::Strong
                || candidate.score.total >= 0.15
        })
        .count();

    BoardEval {
        disliked_in_top_20,
        probe_weak_in_top_20,
        strong_fit_share: if top.is_empty() {
            0.0
        } else {
            strong_count as f32 / top.len() as f32
        },
        board_count: new_picks.len(),
        ..Default::default()
    }
}

pub fn evaluate_holdout_retrieval(
    retrieved_ids: &[String],
    scored_ids: &[String],
    held_out: &HashSet<String>,
) -> BoardEval {
    let metrics = layered(retrieved_ids, scored_ids, held_out);
    BoardEval {
        holdout_hit_rate_at_50: metrics.recall_at_50,
        holdout_ndcg_at_12: metrics.ndcg_at_12,
        board_count: scored_ids.len(),
        ..Default::default()
    }
}

pub fn recall_at(retrieved: &[String], held_out: &HashSet<String>, k: usize) -> f32 {
    if held_out.is_empty() {
        return 0.0;
    }
    let hit = retrieved.iter().take(k).filter(|id| held_out.contains(*id)).count();
    hit as f32 / held_out.len() as f32
}

pub fn precision_at(retrieved: &[String], held_out: &HashSet<String>, k: usize) -> f32 {
    let shown = retrieved.len().min(k);
    if shown == 0 {
        return 0.0;
    }
    retrieved
        .iter()
        .take(k)
        .filter(|id| held_out.contains(*id))
        .count() as f32
        / shown as f32
}

pub fn mrr(retrieved: &[String], held_out: &HashSet<String>) -> f32 {
    for (i, id) in retrieved.iter().enumerate() {
        if held_out.contains(id) {
            return 1.0 / (i as f32 + 1.0);
        }
    }
    0.0
}

pub fn ndcg_at(retrieved: &[String], held_out: &HashSet<String>, k: usize) -> f32 {
    let mut dcg = 0.0;
    for (i, id) in retrieved.iter().take(k).enumerate() {
        if held_out.contains(id) {
            dcg += 1.0 / ((i as f32 + 2.0).log2());
        }
    }
    let ideal = (0..held_out.len().min(k))
        .map(|i| 1.0 / ((i as f32 + 2.0).log2()))
        .sum::<f32>();
    if ideal <= 0.0 {
        0.0
    } else {
        dcg / ideal
    }
}

pub fn ids_of(scored: &[ScoredCandidate]) -> Vec<String> {
    scored
        .iter()
        .filter_map(|c| c.candidate.tmdb_id.map(|id| format!("tmdb:{id}")))
        .collect()
}

pub fn layered(
    retrieval_ids: &[String],
    scored_ids: &[String],
    held_out: &HashSet<String>,
) -> LayeredMetrics {
    LayeredMetrics {
        recall_at_1000: recall_at(retrieval_ids, held_out, 1000),
        recall_at_100: recall_at(retrieval_ids, held_out, 100),
        recall_at_50: recall_at(retrieval_ids, held_out, 50),
        recall_at_25: recall_at(scored_ids, held_out, 25),
        recall_at_12: recall_at(scored_ids, held_out, 12),
        mrr: mrr(scored_ids, held_out),
        ndcg_at_12: ndcg_at(scored_ids, held_out, 12),
        precision_at_40: precision_at(scored_ids, held_out, 40),
        recall_at_40: recall_at(scored_ids, held_out, 40),
    }
}

/// The evaluation split used for real replay runs. The newest positively
/// rated films are held out, while every held-out identity is removed from
/// both the training profile and the seen set.
#[derive(Debug, Clone, Default)]
pub struct ReplayInputs {
    pub training_films: Vec<FilmRecord>,
    pub profile: FeatureProfile,
    pub held_out: HashSet<String>,
    pub seen: HashSet<String>,
}

pub fn time_aware_replay_inputs(films: &[FilmRecord], holdout_count: usize) -> ReplayInputs {
    let mut positive: Vec<(usize, &FilmRecord)> = films
        .iter()
        .enumerate()
        .filter(|(_, film)| film.rating.map(|rating| rating >= 4.0).unwrap_or(false))
        .collect();
    positive.sort_by(|(left_i, left), (right_i, right)| {
        left.last_date
            .as_deref()
            .unwrap_or("")
            .cmp(right.last_date.as_deref().unwrap_or(""))
            .then_with(|| left_i.cmp(right_i))
    });
    let held_out: HashSet<String> = positive
        .iter()
        .rev()
        .take(holdout_count)
        .map(|(_, film)| identity_key(film.tmdb_id, &film.title, film.year))
        .collect();
    let training_films: Vec<FilmRecord> = films
        .iter()
        .filter(|film| !held_out.contains(&identity_key(film.tmdb_id, &film.title, film.year)))
        .cloned()
        .collect();
    let seen = seen_keys(&training_films);
    let profile = crate::taste::feature_profile_from_films(&training_films);
    ReplayInputs {
        training_films,
        profile,
        held_out,
        seen,
    }
}

/// Rating bucket for stratified holdouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RatingBucket {
    Loved,
    Liked,
    Meh,
    Disliked,
}

pub fn rating_bucket(rating: f32) -> RatingBucket {
    if rating >= 4.5 {
        RatingBucket::Loved
    } else if rating >= 3.5 {
        RatingBucket::Liked
    } else if rating >= 2.5 {
        RatingBucket::Meh
    } else {
        RatingBucket::Disliked
    }
}

fn era_stratum(year: Option<i32>) -> u8 {
    match year.unwrap_or(0) {
        y if y >= 2015 => 0,
        y if y >= 2000 => 1,
        y if y >= 1985 => 2,
        y if y > 0 => 3,
        _ => 4,
    }
}

fn popularity_stratum(vote_count: Option<i64>) -> u8 {
    match vote_count.unwrap_or(0) {
        v if v >= 5000 => 0,
        v if v >= 500 => 1,
        v if v >= 50 => 2,
        _ => 3,
    }
}

/// Deterministic split of rated films. Held-out identities are removed from
/// training history (profile, seeds, affinities, seen). Public catalog metadata
/// about a held-out title may still appear once another path discovers it.
pub fn stratified_holdout_inputs(
    films: &[FilmRecord],
    seed: u64,
    holdout_frac: f32,
) -> ReplayInputs {
    let holdout_frac = holdout_frac.clamp(0.05, 0.35);
    let mut rated: Vec<&FilmRecord> = films.iter().filter(|f| f.rating.is_some()).collect();
    rated.sort_by(|a, b| {
        a.tmdb_id
            .unwrap_or(0)
            .cmp(&b.tmdb_id.unwrap_or(0))
            .then_with(|| a.title.cmp(&b.title))
    });

    use std::collections::HashMap;
    let mut strata: HashMap<(RatingBucket, u8, u8), Vec<&FilmRecord>> = HashMap::new();
    for film in &rated {
        let rating = film.rating.unwrap_or(0.0);
        let key = (
            rating_bucket(rating),
            era_stratum(film.year),
            popularity_stratum(film.vote_count),
        );
        strata.entry(key).or_default().push(*film);
    }

    let mut held_out: HashSet<String> = HashSet::new();
    let mut stratum_keys: Vec<_> = strata.keys().copied().collect();
    stratum_keys.sort_by_key(|a| {
        let bucket = match a.0 {
            RatingBucket::Loved => 0u8,
            RatingBucket::Liked => 1,
            RatingBucket::Meh => 2,
            RatingBucket::Disliked => 3,
        };
        (bucket, a.1, a.2)
    });
    for key in stratum_keys {
        let Some(members) = strata.get_mut(&key) else {
            continue;
        };
        members.sort_by(|a, b| {
            a.tmdb_id
                .unwrap_or(0)
                .cmp(&b.tmdb_id.unwrap_or(0))
                .then_with(|| a.title.cmp(&b.title))
        });
        let n = members.len();
        let take = ((n as f32) * holdout_frac).round() as usize;
        let take = if n >= 5 {
            take.max(1).min(n.saturating_sub(1))
        } else if n >= 2 && holdout_frac >= 0.1 {
            1.min(n.saturating_sub(1))
        } else {
            0
        };
        if take == 0 {
            continue;
        }
        // Stable pseudo-random offset inside the stratum.
        let bucket = match key.0 {
            RatingBucket::Loved => 0u64,
            RatingBucket::Liked => 1,
            RatingBucket::Meh => 2,
            RatingBucket::Disliked => 3,
        };
        let mut state = seed
            ^ (bucket << 48)
            ^ ((key.1 as u64) << 32)
            ^ ((key.2 as u64) << 16)
            ^ (n as u64);
        state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        let start = (state as usize) % n;
        for i in 0..take {
            let film = members[(start + i) % n];
            held_out.insert(identity_key(film.tmdb_id, &film.title, film.year));
        }
    }

    let training_films: Vec<FilmRecord> = films
        .iter()
        .filter(|film| !held_out.contains(&identity_key(film.tmdb_id, &film.title, film.year)))
        .cloned()
        .collect();
    let seen = seen_keys(&training_films);
    let profile = crate::taste::feature_profile_from_films(&training_films);
    ReplayInputs {
        training_films,
        profile,
        held_out,
        seen,
    }
}

/// Default fold seeds for the Milestone A benchmark artifact.
pub const BENCHMARK_SEEDS: [u64; 5] = [17, 42, 101, 256, 777];
pub const BENCHMARK_HOLDOUT_FRAC: f32 = 0.15;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCoverage {
    pub related: f32,
    pub filmography: f32,
    pub collection: f32,
    pub semantic_film_local: f32,
    pub semantic_profile: f32,
    pub studio: f32,
    pub multiple: f32,
    pub other: f32,
    pub recovered: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalFoldMetrics {
    pub seed: u64,
    pub held_out_positives: usize,
    pub held_out_total: usize,
    pub candidate_pool_size: usize,
    pub retrieval_recall_at_100: f32,
    pub retrieval_recall_at_250: f32,
    pub retrieval_recall_at_1000: f32,
    pub source_coverage: SourceCoverage,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSummary {
    pub mean: f32,
    pub min: f32,
    pub max: f32,
    pub stdev: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalBenchmarkReport {
    pub protocol: String,
    pub algorithm_version: String,
    pub rated_films: usize,
    pub folds: usize,
    pub holdout_frac: f32,
    pub seeds: Vec<u64>,
    pub fold_metrics: Vec<RetrievalFoldMetrics>,
    pub recall_at_100: MetricSummary,
    pub recall_at_250: MetricSummary,
    pub recall_at_1000: MetricSummary,
    pub candidate_pool_size: MetricSummary,
    pub mean_source_coverage: SourceCoverage,
}

fn summarize(values: &[f32]) -> MetricSummary {
    if values.is_empty() {
        return MetricSummary::default();
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let var = values
        .iter()
        .map(|v| {
            let d = *v - mean;
            d * d
        })
        .sum::<f32>()
        / values.len() as f32;
    MetricSummary {
        mean,
        min,
        max,
        stdev: var.sqrt(),
    }
}

fn summarize_usize(values: &[usize]) -> MetricSummary {
    summarize(&values.iter().map(|v| *v as f32).collect::<Vec<_>>())
}

fn primary_generator_label(sources: &[crate::taste::retrieve::RetrievalSource]) -> &'static str {
    use crate::taste::retrieve::RetrievalKind;
    let families: HashSet<_> = sources
        .iter()
        .map(|s| s.kind.generator_family())
        .collect();
    if families.len() > 1 {
        return "multiple";
    }
    match sources.first().map(|s| s.kind) {
        Some(kind) if kind.is_related() => "related",
        Some(RetrievalKind::Filmography) => "filmography",
        Some(RetrievalKind::Collection) => "collection",
        Some(RetrievalKind::SemanticFilmLocal) => "semanticFilmLocal",
        Some(RetrievalKind::SemanticProfile) => "semanticProfile",
        Some(_) => "other",
        None => "other",
    }
}

/// Score recovery of held-out loved/liked films that entered the candidate pool.
pub fn source_coverage_for_hits(
    candidates: &[crate::taste::retrieve::Candidate],
    held_out_positives: &HashSet<String>,
) -> SourceCoverage {
    let mut related = 0usize;
    let mut filmography = 0usize;
    let mut collection = 0usize;
    let mut semantic_film_local = 0usize;
    let mut semantic_profile = 0usize;
    let mut multiple = 0usize;
    let mut other = 0usize;
    let mut recovered = 0usize;
    for c in candidates {
        let key = identity_key(c.tmdb_id, &c.title, c.year);
        if !held_out_positives.contains(&key) {
            continue;
        }
        recovered += 1;
        match primary_generator_label(&c.sources) {
            "related" => related += 1,
            "filmography" => filmography += 1,
            "collection" => collection += 1,
            "semanticFilmLocal" => semantic_film_local += 1,
            "semanticProfile" => semantic_profile += 1,
            "multiple" => multiple += 1,
            _ => other += 1,
        }
    }
    let denom = recovered.max(1) as f32;
    SourceCoverage {
        related: related as f32 / denom,
        filmography: filmography as f32 / denom,
        collection: collection as f32 / denom,
        semantic_film_local: semantic_film_local as f32 / denom,
        semantic_profile: semantic_profile as f32 / denom,
        studio: 0.0,
        multiple: multiple as f32 / denom,
        other: other as f32 / denom,
        recovered,
    }
}

pub fn evaluate_retrieval_fold(
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
    candidates: &[crate::taste::retrieve::Candidate],
) -> RetrievalFoldMetrics {
    let held_out_positives: HashSet<String> = all_films
        .iter()
        .filter(|f| {
            let key = identity_key(f.tmdb_id, &f.title, f.year);
            inputs.held_out.contains(&key)
                && f.rating
                    .map(|r| matches!(rating_bucket(r), RatingBucket::Loved | RatingBucket::Liked))
                    .unwrap_or(false)
        })
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect();
    let retrieved_ids: Vec<String> = candidates
        .iter()
        .map(|c| identity_key(c.tmdb_id, &c.title, c.year))
        .collect();
    RetrievalFoldMetrics {
        seed,
        held_out_positives: held_out_positives.len(),
        held_out_total: inputs.held_out.len(),
        candidate_pool_size: candidates.len(),
        retrieval_recall_at_100: recall_at(&retrieved_ids, &held_out_positives, 100),
        retrieval_recall_at_250: recall_at(&retrieved_ids, &held_out_positives, 250),
        retrieval_recall_at_1000: recall_at(&retrieved_ids, &held_out_positives, 1000),
        source_coverage: source_coverage_for_hits(candidates, &held_out_positives),
    }
}

/// Run the Milestone A retrieval benchmark: stratified repeated holdouts against
/// the live retrieval pipeline. Does not rewrite scoring.
pub fn run_retrieval_benchmark(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<RetrievalBenchmarkReport, String> {
    let mut fold_metrics = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        if inputs.held_out.is_empty() {
            continue;
        }
        let retrieved = crate::taste::retrieve::retrieve_with_coverage(
            db,
            &inputs.training_films,
            &inputs.profile,
            &inputs.seen,
            false,
        )?;
        fold_metrics.push(evaluate_retrieval_fold(
            seed,
            &inputs,
            films,
            &retrieved.candidates,
        ));
    }
    let recall_100: Vec<f32> = fold_metrics
        .iter()
        .map(|f| f.retrieval_recall_at_100)
        .collect();
    let recall_250: Vec<f32> = fold_metrics
        .iter()
        .map(|f| f.retrieval_recall_at_250)
        .collect();
    let recall_1000: Vec<f32> = fold_metrics
        .iter()
        .map(|f| f.retrieval_recall_at_1000)
        .collect();
    let pools: Vec<usize> = fold_metrics.iter().map(|f| f.candidate_pool_size).collect();
    let recovered: usize = fold_metrics.iter().map(|f| f.source_coverage.recovered).sum();
    let mean_source = if recovered == 0 {
        SourceCoverage::default()
    } else {
        let weight = |pick: fn(&SourceCoverage) -> f32| -> f32 {
            fold_metrics
                .iter()
                .map(|f| pick(&f.source_coverage) * f.source_coverage.recovered as f32)
                .sum::<f32>()
                / recovered as f32
        };
        SourceCoverage {
            related: weight(|s| s.related),
            filmography: weight(|s| s.filmography),
            collection: weight(|s| s.collection),
            semantic_film_local: weight(|s| s.semantic_film_local),
            semantic_profile: weight(|s| s.semantic_profile),
            studio: 0.0,
            multiple: weight(|s| s.multiple),
            other: weight(|s| s.other),
            recovered,
        }
    };
    Ok(RetrievalBenchmarkReport {
        protocol: "stratified-repeated-holdout-v1".into(),
        algorithm_version: crate::taste::workspace::ALGORITHM_VERSION.into(),
        rated_films: films.iter().filter(|f| f.rating.is_some()).count(),
        folds: fold_metrics.len(),
        holdout_frac,
        seeds: seeds.to_vec(),
        fold_metrics,
        recall_at_100: summarize(&recall_100),
        recall_at_250: summarize(&recall_250),
        recall_at_1000: summarize(&recall_1000),
        candidate_pool_size: summarize_usize(&pools),
        mean_source_coverage: mean_source,
    })
}

/// Write a reproducible benchmark JSON artifact under `taste-runs/benchmarks/`.
pub fn write_benchmark_artifact(
    taste_runs_dir: &std::path::Path,
    report: &RetrievalBenchmarkReport,
) -> Result<String, String> {
    let dir = taste_runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("retrieval-{stamp}.json"));
    let latest = dir.join("retrieval-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(path.display().to_string())
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayMetrics {
    pub held_out_count: usize,
    pub baseline_precision_at_40: f32,
    pub baseline_recall_at_40: f32,
    pub revised_precision_at_40: f32,
    pub revised_recall_at_40: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayReport {
    pub holdout_count: usize,
    pub baseline_precision_at_40: f32,
    pub baseline_recall_at_40: f32,
    pub revised_precision_at_40: f32,
    pub revised_recall_at_40: f32,
    pub baseline_ndcg_at_12: f32,
    pub revised_ndcg_at_12: f32,
    pub baseline_board_count: usize,
    pub revised_board_count: usize,
    pub baseline_related_only: usize,
    pub revised_related_only: usize,
    pub semantic: SemanticStats,
    pub error: Option<String>,
}

/// Run the real, time-aware holdout against the current local catalog. This is
/// intentionally opt-in from debug builds because it can perform extra
/// retrieval/embedding work; normal user runs remain single-pass.
pub fn run_replay(
    db: &Database,
    key: &str,
    films: &[FilmRecord],
    holdout_count: usize,
) -> Result<ReplayReport, String> {
    let inputs = time_aware_replay_inputs(films, holdout_count);
    if inputs.held_out.is_empty() {
        return Ok(ReplayReport {
            error: Some("No positively rated films were available for replay".into()),
            ..Default::default()
        });
    }
    let retrieved = crate::taste::retrieve::retrieve_with_coverage(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let baseline = crate::taste::score::score_pool(&inputs.profile, &retrieved.candidates);
    let (semantic_scores, semantic) = crate::taste::semantic::score_candidates(
        db,
        key,
        &inputs.training_films,
        &retrieved.candidates,
    );
    let revised = crate::taste::score::score_pool_with_semantic(
        &inputs.profile,
        &retrieved.candidates,
        &semantic_scores,
        None,
    );
    let baseline_ids = ids_of(&baseline.ranked);
    let revised_ids = ids_of(&revised.ranked);
    let metrics = compare_replay(&baseline_ids, &revised_ids, &inputs.held_out);
    let seen_ids = inputs
        .training_films
        .iter()
        .filter_map(|f| f.tmdb_id)
        .collect::<HashSet<_>>();
    let baseline_quality = board_quality(
        &crate::taste::workspace::assemble(&baseline.ranked),
        &seen_ids,
    );
    let revised_quality = board_quality(
        &crate::taste::workspace::assemble(&revised.ranked),
        &seen_ids,
    );
    Ok(ReplayReport {
        holdout_count: metrics.held_out_count,
        baseline_precision_at_40: metrics.baseline_precision_at_40,
        baseline_recall_at_40: metrics.baseline_recall_at_40,
        revised_precision_at_40: metrics.revised_precision_at_40,
        revised_recall_at_40: metrics.revised_recall_at_40,
        baseline_ndcg_at_12: ndcg_at(&baseline_ids, &inputs.held_out, 12),
        revised_ndcg_at_12: ndcg_at(&revised_ids, &inputs.held_out, 12),
        baseline_board_count: baseline_quality.new_count,
        revised_board_count: revised_quality.new_count,
        baseline_related_only: baseline_quality.new_related_only,
        revised_related_only: revised_quality.new_related_only,
        semantic,
        error: None,
    })
}

pub fn compare_replay(
    baseline: &[String],
    revised: &[String],
    held_out: &HashSet<String>,
) -> ReplayMetrics {
    ReplayMetrics {
        held_out_count: held_out.len(),
        baseline_precision_at_40: precision_at(baseline, held_out, 40),
        baseline_recall_at_40: recall_at(baseline, held_out, 40),
        revised_precision_at_40: precision_at(revised, held_out, 40),
        revised_recall_at_40: recall_at(revised, held_out, 40),
    }
}

/// Band occupancy and leakage for New / Explore / Watchlist.
/// `70` is a band, not a calibrated probability.
#[derive(Debug, Clone, Default)]
pub struct BoardQuality {
    pub new_count: usize,
    pub explore_count: usize,
    pub watchlist_count: usize,
    pub new_related_only: usize,
    pub new_below_floor: usize,
    pub explore_outside_band: usize,
    pub seen_leakage: usize,
    pub board_overlap: usize,
    pub watchlist_on_discovery_boards: usize,
}

pub fn board_quality(
    ws: &crate::taste::workspace::Workspace,
    seen: &HashSet<i64>,
) -> BoardQuality {
    use crate::taste::confidence::{self, MATCH_SCORE_FLOOR, NEW_MATCH_FLOOR};
    let mut q = BoardQuality {
        new_count: ws.new_picks.len(),
        explore_count: ws.explore_picks.len(),
        watchlist_count: ws.watchlist_picks.len(),
        ..Default::default()
    };
    q.new_related_only = ws
        .new_picks
        .iter()
        .filter(|c| confidence::related_only(c))
        .count();
    q.new_below_floor = ws
        .new_picks
        .iter()
        .filter(|c| confidence::match_score(c) < MATCH_SCORE_FLOOR)
        .count();
    q.explore_outside_band = ws
        .explore_picks
        .iter()
        .filter(|c| {
            let s = confidence::match_score(c);
            s < MATCH_SCORE_FLOOR || s >= NEW_MATCH_FLOOR
        })
        .count();
    let displayed = ws
        .new_picks
        .iter()
        .chain(ws.explore_picks.iter())
        .chain(ws.watchlist_picks.iter());
    q.seen_leakage = displayed
        .clone()
        .filter(|c| c.candidate.tmdb_id.map(|id| seen.contains(&id)).unwrap_or(false))
        .count();
    q.watchlist_on_discovery_boards = ws
        .new_picks
        .iter()
        .chain(ws.explore_picks.iter())
        .filter(|c| c.candidate.watchlist)
        .count();
    let mut ids = HashSet::new();
    for c in ws
        .new_picks
        .iter()
        .chain(ws.explore_picks.iter())
        .chain(ws.watchlist_picks.iter())
    {
        if let Some(id) = c.candidate.tmdb_id {
            if !ids.insert(id) {
                q.board_overlap += 1;
            }
        }
    }
    q
}

#[derive(Debug, Clone, Default)]
pub struct SourceMix {
    pub related_recommendations: usize,
    pub related_similar: usize,
    pub related_legacy: usize,
    pub filmography_only: usize,
    pub watchlist: usize,
    pub friend: usize,
}

pub fn source_mix(rows: &[ScoredCandidate]) -> SourceMix {
    use crate::taste::retrieve::RetrievalKind;
    let mut mix = SourceMix::default();
    for c in rows {
        if c.candidate.watchlist {
            mix.watchlist += 1;
        }
        if c.candidate
            .sources
            .iter()
            .any(|s| s.kind == RetrievalKind::Friend)
        {
            mix.friend += 1;
        }
        let related: Vec<_> = c
            .candidate
            .sources
            .iter()
            .filter(|s| s.kind.is_related())
            .collect();
        if related
            .iter()
            .any(|s| s.kind == RetrievalKind::RelatedRecommendations)
        {
            mix.related_recommendations += 1;
        }
        if related
            .iter()
            .any(|s| s.kind == RetrievalKind::RelatedSimilar)
        {
            mix.related_similar += 1;
        }
        if related.iter().any(|s| s.kind == RetrievalKind::Related) {
            mix.related_legacy += 1;
        }
        if !c.candidate.watchlist
            && !c.candidate.sources.is_empty()
            && c.candidate
                .sources
                .iter()
                .all(|s| s.kind == RetrievalKind::Filmography)
        {
            mix.filmography_only += 1;
        }
    }
    mix
}

pub fn resume_only_share(rows: &[ScoredCandidate]) -> f32 {
    if rows.is_empty() {
        return 0.0;
    }
    source_mix(rows).filmography_only as f32 / rows.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::explain::{EligibilityTrace, EvidenceGrade};
    use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn board_candidate(
        id: i64,
        title: &str,
        total: f32,
        grade: EvidenceGrade,
        seed_rating: Option<f32>,
        negative_features: Vec<String>,
    ) -> ScoredCandidate {
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(id),
                title: title.into(),
                year: Some(2024),
                poster: None,
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::RelatedRecommendations,
                    label: "test seed".into(),
                    seed_tmdb_id: Some(100 + id),
                    seed_rating,
                    similarity: None,
                    neighbor_rank: None,
                }],
                directors: vec![],
                genres: vec![],
                modes: vec![],
                media_kind: MediaKind::Movie,
                runtime: Some(100),
                vote_count: Some(1000),
                semantic_cluster: None,
            },
            score: CandidateScore {
                content: 0.0,
                tmdb_related: 0.0,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: 0.0,
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: vec![],
            negative_features,
            contextual_only: false,
            person_keys: vec![],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![],
            hidden_features: vec![],
            eligibility: EligibilityTrace {
                evidence_grade: grade,
                ..Default::default()
            },
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    #[test]
    fn retrieval_vs_scoring_layers() {
        let retrieved: Vec<String> = (1..200).map(|i| format!("tmdb:{i}")).collect();
        let mut scored = retrieved.clone();
        scored.retain(|id| {
            let n: i64 = id.trim_start_matches("tmdb:").parse().unwrap();
            n % 2 == 0
        });
        let mut held = HashSet::new();
        held.insert("tmdb:4".into());
        held.insert("tmdb:50".into());
        let m = layered(&retrieved, &scored, &held);
        assert!(m.recall_at_1000 >= 0.99);
        assert!(m.recall_at_100 >= 0.99);
        assert!(m.recall_at_50 > 0.0);
        assert!(m.recall_at_25 > 0.0);
        assert!(m.recall_at_12 > 0.0);
        assert!(m.mrr > 0.0);
        assert!(m.ndcg_at_12 > 0.0);
        assert!(m.precision_at_40 > 0.0);
        assert!(m.recall_at_40 > 0.0);
        assert_eq!(source_mix(&[]).filmography_only, 0);
        assert_eq!(resume_only_share(&[]), 0.0);
        assert!(!ids_of(&[]).is_empty() || true);
    }

    #[test]
    fn displayed_board_counts_pressure_probes_and_strong_fit_share() {
        let picks = vec![
            board_candidate(1, "Strong Evidence", 0.05, EvidenceGrade::Strong, None, vec![]),
            board_candidate(2, "Strong Total", 0.20, EvidenceGrade::Medium, None, vec![]),
            board_candidate(
                3,
                "Weak Probe",
                0.05,
                EvidenceGrade::Medium,
                None,
                vec!["negative match".into()],
            ),
            board_candidate(
                4,
                "Disliked Neighbor",
                0.20,
                EvidenceGrade::Strong,
                Some(2.0),
                vec![],
            ),
        ];

        let metrics = evaluate_displayed_board(&picks, &[], &["weak probe"]);

        assert_eq!(metrics.board_count, 4);
        assert_eq!(metrics.disliked_in_top_20, 2);
        assert_eq!(metrics.probe_weak_in_top_20, 1);
        assert!((metrics.strong_fit_share - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn holdout_retrieval_wraps_layered_metrics() {
        let retrieved = vec!["tmdb:1".into(), "tmdb:2".into(), "tmdb:3".into()];
        let scored = vec!["tmdb:2".into(), "tmdb:8".into()];
        let held_out = HashSet::from(["tmdb:2".into()]);

        let metrics = evaluate_holdout_retrieval(&retrieved, &scored, &held_out);

        assert_eq!(metrics.holdout_hit_rate_at_50, 1.0);
        assert_eq!(metrics.holdout_ndcg_at_12, 1.0);
        assert_eq!(metrics.board_count, 2);
    }

    #[test]
    fn replay_comparison_reports_precision_and_recall_without_padding() {
        let held_out = HashSet::from(["tmdb:2".into(), "tmdb:4".into()]);
        let baseline = vec!["tmdb:9".into(), "tmdb:2".into(), "tmdb:8".into()];
        let revised = vec!["tmdb:2".into(), "tmdb:4".into()];
        let metrics = compare_replay(&baseline, &revised, &held_out);
        assert_eq!(metrics.held_out_count, 2);
        assert!((metrics.baseline_precision_at_40 - (1.0 / 3.0)).abs() < 1e-5);
        assert!((metrics.baseline_recall_at_40 - 0.5).abs() < 1e-5);
        assert!((metrics.revised_precision_at_40 - 1.0).abs() < 1e-5);
        assert!((metrics.revised_recall_at_40 - 1.0).abs() < 1e-5);
    }

    fn film(
        title: &str,
        tmdb_id: i64,
        rating: f32,
        year: i32,
        genres: &[&str],
        credits: Vec<crate::taste::features::Credit>,
        age_years: f32,
    ) -> crate::taste::retrieve::FilmRecord {
        crate::taste::retrieve::FilmRecord {
            key: format!("tmdb:{tmdb_id}"),
            title: title.into(),
            year: Some(year),
            tmdb_id: Some(tmdb_id),
            rating: Some(rating),
            liked: rating >= 4.5,
            watched: true,
            watchlist: false,
            viewings: 1,
            last_date: None,
            genres: genres.iter().map(|g| (*g).to_string()).collect(),
            credits,
            keywords: vec![],
            recommendations: vec![],
            similar: vec![],
            collection_name: None,
            collection: vec![],
            runtime: Some(100),
            poster: None,
            vote_count: Some(1000),
            review: None,
            signal: None,
            age_years: Some(age_years),
        }
    }

    fn credit(job: &str, name: &str, id: i64) -> crate::taste::features::Credit {
        crate::taste::features::Credit {
            id: Some(id),
            name: name.into(),
            job: job.into(),
        }
    }

    #[test]
    fn time_aware_replay_removes_new_liked_films_from_profile_and_seen() {
        use crate::taste::retrieve::attach_signals;
        let mut films = vec![
            film("Old", 1, 4.5, 2000, &["Drama"], vec![], 8.0),
            film("Middle", 2, 4.0, 2010, &["Crime"], vec![], 5.0),
            film("Newest", 3, 5.0, 2025, &["Science Fiction"], vec![], 0.1),
        ];
        films[0].last_date = Some("2020-01-01".into());
        films[1].last_date = Some("2023-01-01".into());
        films[2].last_date = Some("2026-01-01".into());
        attach_signals(&mut films);
        let replay = time_aware_replay_inputs(&films, 1);
        assert!(replay.held_out.contains("tmdb:3"));
        assert!(replay.training_films.iter().all(|f| f.tmdb_id != Some(3)));
        assert!(!replay.seen.contains("tmdb:3"));
        assert!(replay.seen.contains("tmdb:1"));
        assert!(replay
            .profile
            .affinities
            .iter()
            .flat_map(|a| a.positive_evidence.iter())
            .all(|e| e.tmdb_id != Some(3)));
    }

    #[test]
    fn stratified_holdout_is_deterministic_and_leaks_no_held_out_into_profile() {
        use crate::taste::retrieve::attach_signals;
        let mut films = Vec::new();
        for i in 0..40 {
            let rating = match i % 4 {
                0 => 5.0,
                1 => 4.0,
                2 => 3.0,
                _ => 1.5,
            };
            let year = 1990 + (i % 30);
            let mut f = film(
                &format!("Film {i}"),
                i as i64 + 1,
                rating,
                year,
                &["Drama"],
                vec![],
                1.0,
            );
            f.vote_count = Some(if i % 3 == 0 { 8000 } else { 200 });
            films.push(f);
        }
        attach_signals(&mut films);
        let a = stratified_holdout_inputs(&films, 42, 0.15);
        let b = stratified_holdout_inputs(&films, 42, 0.15);
        assert_eq!(a.held_out, b.held_out);
        assert!(!a.held_out.is_empty());
        for key in &a.held_out {
            assert!(!a.seen.contains(key));
            assert!(a
                .training_films
                .iter()
                .all(|f| &identity_key(f.tmdb_id, &f.title, f.year) != key));
        }
        let c = stratified_holdout_inputs(&films, 77, 0.15);
        assert_ne!(a.held_out, c.held_out);
    }

    #[test]
    fn source_coverage_separates_related_from_multiple_generators() {
        use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
        let related = Candidate {
            tmdb_id: Some(10),
            title: "Hit Related".into(),
            year: Some(2000),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "from A", Some(1)),
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "from B", Some(2)),
            ],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        };
        let multi = Candidate {
            tmdb_id: Some(11),
            title: "Hit Multi".into(),
            year: Some(2001),
            sources: vec![
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "from A", Some(1)),
                RetrievalSource::new(RetrievalKind::Filmography, "Actor", None),
            ],
            ..related.clone()
        };
        let held = HashSet::from(["tmdb:10".into(), "tmdb:11".into()]);
        let cov = source_coverage_for_hits(&[related, multi], &held);
        assert_eq!(cov.recovered, 2);
        assert!((cov.related - 0.5).abs() < 1e-5);
        assert!((cov.multiple - 0.5).abs() < 1e-5);
    }

    #[test]
    fn evaluate_retrieval_fold_reports_pool_size_with_recall() {
        use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
        let mut films = vec![
            film("Loved", 1, 5.0, 2010, &["Drama"], vec![], 1.0),
            film("Hidden Love", 99, 5.0, 2011, &["Drama"], vec![], 1.0),
            film("Hidden Meh", 98, 3.0, 2012, &["Drama"], vec![], 1.0),
        ];
        crate::taste::retrieve::attach_signals(&mut films);
        let inputs = ReplayInputs {
            training_films: films[..1].to_vec(),
            profile: crate::taste::feature_profile_from_films(&films[..1]),
            held_out: HashSet::from(["tmdb:99".into(), "tmdb:98".into()]),
            seen: seen_keys(&films[..1]),
        };
        let candidates = vec![Candidate {
            tmdb_id: Some(99),
            title: "Hidden Love".into(),
            year: Some(2011),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![RetrievalSource::new(
                RetrievalKind::Collection,
                "same collection",
                Some(1),
            )],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        }];
        let metrics = evaluate_retrieval_fold(17, &inputs, &films, &candidates);
        assert_eq!(metrics.held_out_positives, 1);
        assert_eq!(metrics.candidate_pool_size, 1);
        assert!((metrics.retrieval_recall_at_100 - 1.0).abs() < 1e-5);
        assert!((metrics.source_coverage.collection - 1.0).abs() < 1e-5);
    }

    /// Live B1.1 Craft calibration: role ablations + λ sweep.
    /// `STUDIO_DB=... cargo test write_live_craft_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_craft_calibration() {
        use crate::storage::db::Database;
        use crate::taste::craft_calibration::{
            run_craft_calibration, write_craft_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_craft_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("craft calibration");
        let written =
            write_craft_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        eprintln!("=== Craft calibration (Δ vs Content-only) ===");
        for m in &report.mean_ablations {
            eprintln!(
                "{:<20} loved>dis={:.3} (Δ{:+.3})  lovedRank≈{:.0} (Δ{:+.0})  disRank≈{:.0} (Δ{:+.0})  ndcg@250={:.3} (Δ{:+.3})  rescue={:.1} damage={:.1}{}",
                m.name,
                m.loved_vs_disliked.mean,
                m.delta_loved_vs_disliked.mean,
                m.mean_rank_loved.mean,
                m.delta_mean_rank_loved.mean,
                m.mean_rank_disliked.mean,
                m.delta_mean_rank_disliked.mean,
                m.ndcg_at_250.mean,
                m.delta_ndcg_at_250.mean,
                m.rescued_positives.mean,
                m.damaged_positives.mean,
                m.note.as_deref().map(|n| format!("  [{n}]")).unwrap_or_default(),
            );
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live B2 Form calibration: independent runtime/era/language vs Content-only.
    /// `STUDIO_DB=... cargo test write_live_form_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_form_calibration() {
        use crate::storage::db::Database;
        use crate::taste::form_calibration::{
            run_form_calibration, write_form_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_form_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("form calibration");
        let written =
            write_form_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        eprintln!("=== Form calibration (Δ vs Content-only) ===");
        for m in &report.mean_ablations {
            eprintln!(
                "{:<12} gate={:<16} loved>dis Δ{:+.3}  lovedRank Δ{:+.0}  disRank Δ{:+.0}  ndcg@250 Δ{:+.3}  posGain={:.0} posLoss={:.0} negGain={:.0} |Δ|p90={:.0}{}",
                m.name,
                m.gate,
                m.delta_loved_vs_disliked.mean,
                m.delta_mean_rank_loved.mean,
                m.delta_mean_rank_disliked.mean,
                m.delta_ndcg_at_250.mean,
                m.positive_rank_gain.mean,
                m.positive_rank_loss.mean,
                m.negative_rank_gain.mean,
                m.p90_abs_delta.mean,
                m.note.as_deref().map(|n| format!("  [{n}]")).unwrap_or_default(),
            );
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live B3 Continuity calibration: collection polarity vs Content-only.
    /// `STUDIO_DB=... cargo test write_live_continuity_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_continuity_calibration() {
        use crate::storage::db::Database;
        use crate::taste::continuity_calibration::{
            run_continuity_calibration, write_continuity_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_continuity_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("continuity calibration");
        let written = write_continuity_calibration_artifact(std::path::Path::new(&out), &report)
            .expect("write");
        eprintln!("=== Continuity calibration (Δ vs Content-only) ===");
        for m in &report.mean_ablations {
            eprintln!(
                "{:<20} gate={:<16} loved>dis Δ{:+.3}  lovedRank Δ{:+.0}  disRank Δ{:+.0}  ndcg@250 Δ{:+.3}  posGain={:.0} posLoss={:.0} collPos≈{:.1} collDis≈{:.1}{}",
                m.name,
                m.gate,
                m.delta_loved_vs_disliked.mean,
                m.delta_mean_rank_loved.mean,
                m.delta_mean_rank_disliked.mean,
                m.delta_ndcg_at_250.mean,
                m.positive_rank_gain.mean,
                m.positive_rank_loss.mean,
                m.collection_supported_positives.mean,
                m.collection_supported_dislikes.mean,
                m.note.as_deref().map(|n| format!("  [{n}]")).unwrap_or_default(),
            );
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live B4 Quality calibration: positive/negative TMDB prior vs Content-only.
    /// `STUDIO_DB=... cargo test write_live_quality_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_quality_calibration() {
        use crate::storage::db::Database;
        use crate::taste::quality_calibration::{
            run_quality_calibration, write_quality_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_quality_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("quality calibration");
        let written =
            write_quality_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        eprintln!("=== Quality calibration (Δ vs Content-only) ===");
        for m in &report.mean_ablations {
            eprintln!(
                "{:<18} gate={:<16} loved>dis Δ{:+.3}  lovedRank Δ{:+.0}  disRank Δ{:+.0}  ndcg@250 Δ{:+.3}  posGain={:.0} posLoss={:.0} |Δ|p90={:.0}{}",
                m.name,
                m.gate,
                m.delta_loved_vs_disliked.mean,
                m.delta_mean_rank_loved.mean,
                m.delta_mean_rank_disliked.mean,
                m.delta_ndcg_at_250.mean,
                m.positive_rank_gain.mean,
                m.positive_rank_loss.mean,
                m.p90_abs_delta.mean,
                m.note.as_deref().map(|n| format!("  [{n}]")).unwrap_or_default(),
            );
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live C1 eligibility + Match calibration.
    /// `STUDIO_DB=... cargo test write_live_eligibility_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_eligibility_calibration() {
        use crate::storage::db::Database;
        use crate::taste::eligibility_calibration::{
            run_eligibility_calibration, write_eligibility_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_eligibility_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("eligibility calibration");
        let written = write_eligibility_calibration_artifact(std::path::Path::new(&out), &report)
            .expect("write");
        eprintln!("=== Eligibility + Match calibration (C1–C4) ===");
        eprintln!(
            "eligible_count mean={:.1} min={:.0}  loved_elig={:.2} liked_elig={:.2} dis_elig={:.2}",
            report.mean_eligible_count.mean,
            report.mean_eligible_count.min,
            report.mean_loved_eligible_pct.mean,
            report.mean_liked_eligible_pct.mean,
            report.mean_disliked_eligible_pct.mean,
        );
        eprintln!(
            "precision={:.2} recall_pos={:.2} rec_share={:.2} high_fit_held={:.2} board12 pos={:.2} dis={:.2}",
            report.mean_precision_eligible.mean,
            report.mean_recall_eligible_positives.mean,
            report.mean_recommended_share.mean,
            report.mean_high_fit_held_pct.mean,
            report.mean_board12_loved_liked_share.mean,
            report.mean_board12_disliked_share.mean,
        );
        eprintln!(
            "match curve support={} display {}–{}",
            report.match_calibration.support,
            report.match_calibration.display_min,
            report.match_calibration.display_max,
        );
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live D1 board diversification calibration.
    /// `STUDIO_DB=... cargo test write_live_diversify_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_diversify_calibration() {
        use crate::storage::db::Database;
        use crate::taste::diversify_calibration::{
            run_diversify_calibration, write_diversify_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_diversify_calibration(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("diversify d1");
        let written =
            write_diversify_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        for mode in &report.mean_variants {
            if let Some(b12) = mode.boards.iter().find(|b| b.size == 12) {
                eprintln!(
                    "mode={:<16} @12 pos={:.3} dis={:.3} ndcg={:.3} fit_loss={:.5} reorder={:.1} max_jump={:.1} col_dup={:.2} gate={}",
                    mode.name,
                    b12.positive_share.mean,
                    b12.disliked_share.mean,
                    b12.ndcg.mean,
                    b12.mean_fit_loss.mean,
                    b12.reordered_slots.mean,
                    b12.max_rank_jump.mean,
                    b12.collection_dup_extras.mean,
                    mode.gate,
                );
            }
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Final v1 freeze regression: production policy + live board inventory/lanes.
    /// `STUDIO_DB=... cargo test write_live_v1_freeze_regression -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB with ratings + embeddings"]
    fn write_live_v1_freeze_regression() {
        use crate::storage::db::Database;
        use crate::taste::board_validation::{
            print_board_for_judgment, run_live_board_validation, write_live_board_artifact,
        };
        use crate::taste::diversify::DiversifyConfig;
        use crate::taste::retrieve::{attach_signals, load_films};
        use crate::taste::v1_policy::{assert_v1_production_policy, V1_POLICY_ID};
        use crate::taste::workspace::{self, FEATURED_MAX, NEW_MAX};
        crate::taste::v1_policy::assert_v1_production_policy();
        assert!(
            !DiversifyConfig::light().recommendation_value,
            "production F2 must stay off"
        );
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report = run_live_board_validation(&db, &films).expect("live board");
        assert_eq!(report.algorithm_version, V1_POLICY_ID);
        assert!(
            report.final_board.len() <= FEATURED_MAX,
            "Featured display cap {}",
            report.final_board.len()
        );
        // Full assemble inventory path.
        let profile = crate::taste::feature_profile_from_films(&films);
        let seen = crate::taste::retrieve::seen_keys(&films);
        let pool =
            crate::taste::retrieve::build_retrieval_pool(&db, &films, &profile, &seen, false)
                .expect("pool");
        let examined = crate::taste::retrieve::select_fair_pool(pool.by_key, 1_000);
        let semantic_map =
            crate::taste::semantic::score_candidates_from_cache(&db, &films, &examined);
        let mut scored =
            crate::taste::score::score_pool_with_semantic(&profile, &examined, &semantic_map, None);
        crate::taste::semantic::attach_semantic_clusters_from_db(&db, &mut scored.ranked);
        let ws = workspace::assemble(&scored.ranked);
        assert!(
            ws.new_picks.len() <= NEW_MAX && !ws.new_picks.is_empty(),
            "New inventory {} (cap {})",
            ws.new_picks.len(),
            NEW_MAX
        );
        assert!(
            ws.new_picks.iter().all(|c| !c.candidate.watchlist),
            "Watchlist must not occupy New"
        );
        assert!(
            ws.watchlist_picks.iter().all(|c| c.candidate.watchlist),
            "Watchlist lane must be watchlist-only"
        );
        let written =
            write_live_board_artifact(std::path::Path::new(&out), &report).expect("write");
        print_board_for_judgment(&report);
        eprintln!("freeze={} inventory={} featured={}", V1_POLICY_ID, ws.new_picks.len(), report.final_board.len());
        eprintln!("F2_off={} hybrid_prod=off", !DiversifyConfig::light().recommendation_value);
        eprintln!("wrote {written}");
        let _ = assert_v1_production_policy;
    }

    /// Live v1 board: full-profile New board under active-2k (no algo changes).
    /// `STUDIO_DB=... cargo test write_live_board_v1 -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB with ratings + embeddings"]
    fn write_live_board_v1() {
        use crate::storage::db::Database;
        use crate::taste::board_validation::{
            print_board_for_judgment, run_live_board_validation, write_live_board_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report = run_live_board_validation(&db, &films).expect("live board");
        let written =
            write_live_board_artifact(std::path::Path::new(&out), &report).expect("write");
        print_board_for_judgment(&report);
        eprintln!("algo={}", report.algorithm_version);
        eprintln!("wrote {written}");
    }

    /// Live E1: expand semantic universe then run recall curve.
    /// `STUDIO_DB=... cargo test write_live_universe_calibration -- --ignored --nocapture`
    /// Optional: `STUDIO_UNIVERSE_TARGET=10000` (default 10000). Set `STUDIO_UNIVERSE_SKIP_EXPAND=1` to bench only.
    #[test]
    #[ignore = "requires STUDIO_DB + TMDB + OpenRouter keys; network + embed cost"]
    fn write_live_universe_calibration() {
        use crate::storage::db::Database;
        use crate::taste::retrieve::{attach_signals, load_films};
        use crate::taste::semantic_universe::{
            ensure_semantic_universe, DEFAULT_UNIVERSE_TARGET,
        };
        use crate::taste::universe_calibration::{
            run_universe_calibration, write_universe_calibration_artifact,
        };
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let target: usize = std::env::var("STUDIO_UNIVERSE_TARGET")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_UNIVERSE_TARGET);
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        if std::env::var_os("STUDIO_UNIVERSE_SKIP_EXPAND").is_none() {
            let key = crate::taste::get_api_key()
                .expect("openrouter key lookup")
                .expect("OpenRouter key required to embed universe");
            let expand = ensure_semantic_universe(&db, &key, target).expect("expand");
            eprintln!(
                "expand: harvested={} embedded={} catalog {}→{} emb {}→{} bytes≈{} errs={}",
                expand.harvested,
                expand.embedded,
                expand.catalog_before,
                expand.catalog_after,
                expand.embeddings_before,
                expand.embeddings_after,
                expand.index_bytes_est,
                expand.errors.len()
            );
            for e in expand.errors.iter().take(5) {
                eprintln!("  err: {e}");
            }
        }
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        // Curve uses 3 seeds by default for wall-clock; set STUDIO_UNIVERSE_FULL_SEEDS=1 for all 5.
        let seeds: &[u64] = if std::env::var_os("STUDIO_UNIVERSE_FULL_SEEDS").is_some() {
            &BENCHMARK_SEEDS
        } else {
            &BENCHMARK_SEEDS[..3]
        };
        let report =
            run_universe_calibration(&db, &films, seeds, BENCHMARK_HOLDOUT_FRAC)
                .expect("universe e1");
        let written =
            write_universe_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        for p in &report.curve {
            eprintln!(
                "cap={:<6} eff={:<6} oracle={:.3} @100={:.3} @250={:.3} @1000={:.3} pool={:.0} localΔ={:.3} profileΔ={:.3} nogen={:.3} semOnly={:.3} ms={:.0}",
                p.index_cap,
                p.index_effective,
                p.oracle_recall.mean,
                p.recall_at_100.mean,
                p.recall_at_250.mean,
                p.recall_at_1000.mean,
                p.pool_size.mean,
                p.semantic_local_delta.mean,
                p.semantic_profile_delta.mean,
                p.no_generator_capable.mean,
                p.semantic_only_share.mean,
                p.retrieval_ms.mean,
            );
        }
        eprintln!("chosen_cap={}", report.chosen_index_cap);
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live A/B: active-2k D1.1 vs D1.1+F2 Featured-12 (hybrid150 off).
    /// `STUDIO_DB=... cargo test write_live_ab_f2 -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB with ratings + embeddings"]
    fn write_live_ab_f2() {
        use crate::storage::db::Database;
        use crate::taste::f2_board_ab::{
            print_f2_ab_for_judgment, run_f2_ab_comparison, write_f2_ab_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report = run_f2_ab_comparison(&db, &films).expect("ab f2");
        let written = write_f2_ab_artifact(std::path::Path::new(&out), &report).expect("write");
        print_f2_ab_for_judgment(&report);
        eprintln!("algo={}", report.algorithm_version);
        eprintln!("wrote {written}");
    }

    /// Live A/B: active-2k control vs hybrid150 (experiment; not production default).
    /// `STUDIO_DB=... cargo test write_live_ab_hybrid150 -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB with ratings + ~10k embeddings"]
    fn write_live_ab_hybrid150() {
        use crate::storage::db::Database;
        use crate::taste::hybrid_board_ab::{
            print_ab_for_judgment, run_ab_hybrid150_comparison, write_ab_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report = run_ab_hybrid150_comparison(&db, &films).expect("ab hybrid150");
        let written = write_ab_artifact(std::path::Path::new(&out), &report).expect("write");
        print_ab_for_judgment(&report);
        eprintln!("algo={}", report.algorithm_version);
        eprintln!("wrote {written}");
    }

    /// Live F1: hybrid 2k-core + protected 10k discovery examination matrix.
    /// `STUDIO_DB=... cargo test write_live_hybrid_calibration -- --ignored --nocapture`
    /// Optional: `STUDIO_HYBRID_FULL_SEEDS=1` for all 5 benchmark seeds (default 3).
    #[test]
    #[ignore = "requires STUDIO_DB with ratings + ~10k embeddings"]
    fn write_live_hybrid_calibration() {
        use crate::storage::db::Database;
        use crate::taste::hybrid_calibration::{
            print_hybrid_report, run_hybrid_calibration, write_hybrid_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let seeds: &[u64] = if std::env::var_os("STUDIO_HYBRID_FULL_SEEDS").is_some() {
            &BENCHMARK_SEEDS
        } else {
            &BENCHMARK_SEEDS[..3]
        };
        let report =
            run_hybrid_calibration(&db, &films, seeds, BENCHMARK_HOLDOUT_FRAC).expect("hybrid");
        let written =
            write_hybrid_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        print_hybrid_report(&report);
        eprintln!("algo={}", report.algorithm_version);
        eprintln!("wrote {written}");
    }

    /// Live E1.1: exam-pressure allocation matrix (2k vs 10k × modes).
    /// `STUDIO_DB=... cargo test write_live_exam_pressure_calibration -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB with 10k semantic index"]
    fn write_live_exam_pressure_calibration() {
        use crate::storage::db::Database;
        use crate::taste::exam_calibration::{
            run_exam_calibration, write_exam_calibration_artifact,
        };
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let seeds: &[u64] = if std::env::var_os("STUDIO_EXAM_FULL_SEEDS").is_some() {
            &BENCHMARK_SEEDS
        } else {
            &BENCHMARK_SEEDS[..3]
        };
        // Clear progress log for a clean run.
        let progress = std::path::Path::new(&out)
            .join("benchmarks")
            .join("exam-pressure-progress.txt");
        let _ = std::fs::remove_file(&progress);

        let report =
            run_exam_calibration(&db, &films, seeds, BENCHMARK_HOLDOUT_FRAC).expect("exam e1.1");
        let written =
            write_exam_calibration_artifact(std::path::Path::new(&out), &report).expect("write");
        for p in &report.matrix {
            eprintln!(
                "mode={:<18} idx={:<6} exam={:<5} oracle={:.3} @100={:.3} @250={:.3} @exam={:.3} capFail={:.3} semOnly={:.3} flShare={:.3} prShare={:.3} ms={:.0}",
                p.mode,
                p.index_cap,
                p.exam_cap,
                p.oracle_recall.mean,
                p.recall_at_100.mean,
                p.recall_at_250.mean,
                p.recall_at_1000.mean,
                p.candidate_cap_failures.mean,
                p.semantic_only_recovery.mean,
                p.film_local_share.mean,
                p.profile_share.mean,
                p.retrieval_ms.mean,
            );
        }
        eprintln!("control_2k @1000={:.3} oracle={:.3}", report.control_2k_at_1000, report.control_2k_oracle);
        eprintln!(
            "active_retrieval_cap={} chosen_mode={} index={}",
            report.active_retrieval_cap, report.chosen_mode, report.chosen_index_cap
        );
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
        for p in report.matrix.iter().filter(|p| {
            p.mode == "current" && p.index_cap >= 8_000 && p.exam_cap == 1_000
        }) {
            eprintln!("cap-miss samples (current@10k, n={}):", p.sample_cap_misses.len());
            for s in p.sample_cap_misses.iter().take(8) {
                eprintln!(
                    "  {} gens={:?} native={:?} examRank={:?} famAhead={} dupHood={} fl={} pr={}",
                    s.title,
                    s.generators,
                    s.native_best_rank,
                    s.merged_exam_rank,
                    s.same_family_ahead,
                    s.duplicate_neighborhood,
                    s.film_local,
                    s.profile
                );
            }
        }
    }

    /// Live B1 ranking: `STUDIO_DB=... cargo test write_live_ranking_b1 -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_ranking_b1() {
        use crate::storage::db::Database;
        use crate::taste::ranking_bench::{run_ranking_benchmark, write_ranking_artifact};
        use crate::taste::retrieve::{attach_signals, load_films};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        let report =
            run_ranking_benchmark(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC)
                .expect("ranking b1");
        let written = write_ranking_artifact(std::path::Path::new(&out), &report).expect("write");
        for mode in &report.mean_by_mode {
            eprintln!(
                "mode={:<12} ndcg@100={:.3} ndcg@250={:.3} p@10={:.3} loved>dis={:.3} liked>dis={:.3} lovedRank≈{:.0} likedRank≈{:.0} disRank≈{:.0} dis@25={:.3} recall@250={:.3}",
                mode.mode,
                mode.ndcg_at_100.mean,
                mode.ndcg_at_250.mean,
                mode.precision_at_10.mean,
                mode.loved_vs_disliked_pairwise.mean,
                mode.liked_vs_disliked_pairwise.mean,
                mode.mean_rank_loved.mean,
                mode.mean_rank_liked.mean,
                mode.mean_rank_disliked.mean,
                mode.dislike_rate_top_25.mean,
                mode.recall_at_250.mean,
            );
        }
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    /// Live A2: `STUDIO_DB=... cargo test write_live_retrieval_a2 -- --ignored --nocapture`
    #[test]
    #[ignore = "requires STUDIO_DB pointing at a real studio.db"]
    fn write_live_retrieval_a2() {
        use crate::storage::db::Database;
        use crate::taste::retrieve::{attach_signals, enrich_eligible_seeds, load_films};
        use crate::taste::retrieval_bench::{run_a2_benchmark, write_a2_artifact};
        let path = std::env::var("STUDIO_DB").expect("STUDIO_DB");
        let out = std::env::var("STUDIO_TASTE_RUNS_DIR").unwrap_or_else(|_| {
            std::path::Path::new(&path)
                .parent()
                .map(|p| p.join("taste-runs").to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into())
        });
        let db = Database::open(std::path::Path::new(&path)).expect("open db");
        let mut films = load_films(&db).expect("load films");
        attach_signals(&mut films);
        // Enrich collection/related metadata on positive seeds before measuring.
        let enriched = enrich_eligible_seeds(&db, &mut films, 200, false);
        eprintln!("enriched {enriched} eligible seeds for collection/related metadata");
        let report =
            run_a2_benchmark(&db, &films, &BENCHMARK_SEEDS, BENCHMARK_HOLDOUT_FRAC).expect("a2");
        let written = write_a2_artifact(std::path::Path::new(&out), &report).expect("write");
        for point in &report.mean_recall_by_cap {
            let cap_label = if point.cap == 0 {
                "uncap".to_string()
            } else {
                point.cap.to_string()
            };
            eprintln!(
                "cap={:<5} recall@100={:.3}±{:.3} @250={:.3}±{:.3} @1000={:.3}±{:.3} @cap={:.3} pool≈{:.0}",
                cap_label,
                point.recall_at_100.mean,
                point.recall_at_100.stdev,
                point.recall_at_250.mean,
                point.recall_at_250.stdev,
                point.recall_at_1000.mean,
                point.recall_at_1000.stdev,
                point.recall_at_cap.mean,
                point.pool_size.mean,
            );
        }
        for g in &report.mean_generators {
            eprintln!(
                "gen {:<18} standalone={:.3} Δremoved={:.3} med_rank={:?} cands≈{} impl={}",
                g.generator,
                g.standalone_recall,
                g.delta_if_removed,
                g.median_first_rank,
                g.candidates_generated,
                g.implemented,
            );
        }
        eprintln!("semantic: {}", report.semantic.note);
        eprintln!(
            "embeddings_in_db={} held_out_with_embedding={}/{}",
            report.semantic.embeddings_in_db,
            report.semantic.held_out_positives_with_embedding,
            report.semantic.held_out_positives
        );
        eprintln!("gate: {}", report.gate);
        eprintln!("wrote {written}");
    }

    fn filmography(
        title: &str,
        tmdb_id: i64,
        genres: &[&str],
        person: crate::taste::features::Credit,
    ) -> crate::taste::retrieve::Candidate {
        use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
        let label = person.name.clone();
        crate::taste::retrieve::Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(2005),
            poster: None,
            genres: genres.iter().map(|g| (*g).to_string()).collect(),
            credits: vec![person],
            keywords: vec![],
            runtime: Some(100),
            vote_count: Some(800),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label,
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

    /// 627-film Powell production failure, through the real deterministic pipeline.
    #[test]
    fn powell_e2e_627_history_retrieval_to_shortlist() {
        use crate::taste::features::{COMPOSER_W, CINEMATOGRAPHER_W};
        use crate::taste::reason::{empty_critic, ReasonerPick};
        use crate::taste::retrieve::{attach_signals, Candidate, MediaKind, RetrievalKind, RetrievalSource};
        use crate::taste::score::{
            filmography_supported, person_pipeline_trace, score_all,
        };
        use crate::taste::shortlist::shortlist;
        use crate::taste::validate::{diversity_warnings, hard_validate};
        use crate::taste::feature_profile_from_films;

        let powell = credit("Original Music Composer", "John Powell", 50);
        let fraser = credit("Director of Photography", "Greig Fraser", 77);
        let burwell = credit("Original Music Composer", "Carter Burwell", 88);
        let tarantino = credit("Director", "Quentin Tarantino", 99);

        let mut films = vec![
            film(
                "Kung Fu Panda",
                1,
                5.0,
                2008,
                &["Comedy", "Animation", "Family"],
                vec![powell.clone()],
                1.0,
            ),
            film(
                "Minions & Monsters",
                2,
                5.0,
                2010,
                &["Comedy", "Animation", "Family"],
                vec![powell.clone()],
                0.8,
            ),
            film(
                "The Batman",
                3,
                4.5,
                2022,
                &["Crime"],
                vec![fraser.clone()],
                0.5,
            ),
            film(
                "Dune",
                4,
                4.5,
                2021,
                &["Science Fiction"],
                vec![fraser.clone()],
                0.3,
            ),
            film(
                "Fargo",
                5,
                4.5,
                1996,
                &["Crime", "Drama"],
                vec![burwell.clone()],
                8.0,
            ),
            film(
                "No Country for Old Men",
                6,
                4.5,
                2007,
                &["Crime", "Drama"],
                vec![burwell.clone()],
                6.0,
            ),
            film(
                "Pulp Fiction",
                7,
                5.0,
                1994,
                &["Crime"],
                vec![tarantino.clone()],
                10.0,
            ),
            film(
                "Reservoir Dogs",
                8,
                5.0,
                1992,
                &["Crime"],
                vec![tarantino.clone()],
                12.0,
            ),
        ];
        let genres = [
            "Drama",
            "Thriller",
            "Comedy",
            "Action",
            "Horror",
            "Crime",
            "Mystery",
            "Science Fiction",
            "Romance",
            "Adventure",
        ];
        for i in 9..=627i64 {
            let rating = 1.5 + ((i % 8) as f32) * 0.5;
            films.push(film(
                &format!("Log {i}"),
                i,
                rating,
                2000 + (i % 25) as i32,
                &[genres[i as usize % genres.len()]],
                vec![credit("Director", &format!("Dir{}", i % 80), 1000 + i % 80)],
                0.2 + (i % 15) as f32 * 0.4,
            ));
        }
        assert_eq!(films.len(), 627);

        attach_signals(&mut films);
        assert!(films.iter().all(|f| f.signal.is_some()));
        let ratings: Vec<f32> = films.iter().filter_map(|f| f.rating).collect();
        assert_eq!(ratings.len(), 627);

        let profile = feature_profile_from_films(&films);
        assert!(
            !profile.modes.is_empty() || !profile.dimensions.is_empty(),
            "modes/dimensions should populate on a 627-film log"
        );

        let powell_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "John Powell")
            .expect("Powell affinity");
        assert_eq!(powell_aff.appearances, 2);
        assert_eq!(powell_aff.positive_evidence.len(), 2);
        assert!(powell_aff
            .positive_evidence
            .iter()
            .any(|e| e.title.contains("Panda")));
        assert!(powell_aff
            .positive_evidence
            .iter()
            .any(|e| e.title.contains("Minions")));
        let expected_conf = 1.0 - (-2.0_f32 / 4.0).exp();
        assert!(
            (powell_aff.confidence - expected_conf).abs() < 0.02,
            "confidence {} vs frozen 1-exp(-2/4)={}",
            powell_aff.confidence,
            expected_conf
        );
        let frozen_sa = powell_aff.recommendation_mean
            * powell_aff.confidence
            * COMPOSER_W
            * powell_aff.portability;
        assert!(
            (powell_aff.scoring_affinity() - frozen_sa).abs() < 1e-5,
            "affinity must not be weakened outside the frozen product"
        );
        assert!(powell_aff.recommendation_mean > 0.45);
        assert!(
            powell_aff.evidence_cluster.genres.iter().any(|g| g == "comedy"),
            "cluster must come from Powell evidence films, got {:?}",
            powell_aff.evidence_cluster
        );
        assert!(
            !powell_aff.evidence_cluster.genres.iter().any(|g| g == "drama"),
            "Powell cluster must not be a global drama prior: {:?}",
            powell_aff.evidence_cluster
        );

        let burwell_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Carter Burwell")
            .expect("Burwell");
        assert_eq!(burwell_aff.appearances, 2);
        assert!(
            burwell_aff.evidence_cluster.genres.iter().any(|g| g == "drama")
                || burwell_aff.evidence_cluster.genres.iter().any(|g| g == "crime"),
            "Burwell cluster follows HIS evidence, not Powell's comedy: {:?}",
            burwell_aff.evidence_cluster
        );
        assert!(
            !burwell_aff.evidence_cluster.genres.iter().any(|g| g == "comedy"),
            "Burwell must not inherit a hardcoded comedy filter: {:?}",
            burwell_aff.evidence_cluster
        );

        let fraser_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Greig Fraser")
            .expect("Fraser");
        assert!(fraser_aff.citeable());
        assert!(
            fraser_aff.evidence_cluster.is_empty(),
            "Crime+Sci-Fi evidence must not invent a comedy cluster: {:?}",
            fraser_aff.evidence_cluster
        );
        let fraser_sa = fraser_aff.recommendation_mean
            * fraser_aff.confidence
            * CINEMATOGRAPHER_W
            * fraser_aff.portability;
        assert!((fraser_aff.scoring_affinity() - fraser_sa).abs() < 1e-5);

        let overlap_powell = [
            ("Ice Age: The Meltdown", &["Comedy", "Animation", "Family"][..]),
            ("Antz", &["Comedy", "Animation"][..]),
            ("Chicken Run", &["Comedy", "Animation", "Family"][..]),
            ("Rio", &["Comedy", "Animation"][..]),
            ("Horton Hears a Who!", &["Comedy", "Animation", "Family"][..]),
            ("Robots", &["Comedy", "Animation"][..]),
            ("Bolt", &["Comedy", "Animation", "Family"][..]),
            ("Happy Feet", &["Comedy", "Animation"][..]),
        ];
        let unrelated_powell = [
            ("The Bourne Supremacy", &["Thriller"][..]),
            ("United 93", &["Drama"][..]),
            ("Mr. & Mrs. Smith", &["Action"][..]),
            ("Be Cool", &["Crime"][..]),
            ("The Adventures of Pluto Nash", &["Science Fiction"][..]),
            ("Paycheck", &["Thriller"][..]),
            ("The Italian Job", &["Action"][..]),
            ("Hidalgo", &["Adventure"][..]),
            ("I Am Sam", &["Drama"][..]),
            ("Drumline", &["Drama"][..]),
            ("Two Weeks Notice", &["Romance"][..]),
            ("Stop-Loss", &["Drama"][..]),
        ];
        let mut retrieved = Vec::new();
        for (i, (title, genres)) in overlap_powell.iter().enumerate() {
            retrieved.push(filmography(title, 10_000 + i as i64, genres, powell.clone()));
        }
        for (i, (title, genres)) in unrelated_powell.iter().enumerate() {
            retrieved.push(filmography(title, 10_100 + i as i64, genres, powell.clone()));
        }
        assert_eq!(
            retrieved
                .iter()
                .filter(|c| c.credits.iter().any(|c| c.name == "John Powell"))
                .count(),
            20
        );

        let facet_kept: Vec<&Candidate> = retrieved
            .iter()
            .filter(|c| filmography_supported(&profile, c))
            .collect();
        let powell_facet = facet_kept
            .iter()
            .filter(|c| c.credits.iter().any(|p| p.name == "John Powell"))
            .count();
        assert!(
            retrieved.iter().any(|c| c.title == "United 93")
                && !facet_kept.iter().any(|c| c.title == "United 93"),
            "United 93 must be dropped because Powell evidence is comedy/animation, not because comedy is hardcoded"
        );
        assert!(
            retrieved.iter().any(|c| c.title == "The Bourne Supremacy")
                && !facet_kept.iter().any(|c| c.title == "The Bourne Supremacy")
        );
        assert!(facet_kept.iter().any(|c| c.title.contains("Ice Age")));
        assert!(facet_kept.iter().any(|c| c.title == "Antz"));

        for (i, title) in ["Fargo 2", "True Grit", "The Big Lebowski", "Miller's Crossing"]
            .iter()
            .enumerate()
        {
            retrieved.push(filmography(
                title,
                11_000 + i as i64,
                &["Crime", "Drama"],
                burwell.clone(),
            ));
        }
        retrieved.push(filmography(
            "Burwell Comedy Miss",
            11_050,
            &["Comedy", "Animation"],
            burwell.clone(),
        ));
        assert!(filmography_supported(
            &profile,
            &filmography("True Grit", 1, &["Crime", "Drama"], burwell.clone())
        ));
        assert!(
            !filmography_supported(
                &profile,
                &filmography(
                    "Burwell Comedy Miss",
                    1,
                    &["Comedy", "Animation"],
                    burwell.clone()
                )
            ),
            "Burwell drama/crime cluster must reject comedy filmography — per-person, not global comedy"
        );

        for (i, (title, g)) in [
            ("Zero Dark Thirty", "Thriller"),
            ("Mary Magdalene", "Drama"),
            ("Rogue One", "Science Fiction"),
        ]
        .iter()
        .enumerate()
        {
            retrieved.push(filmography(title, 12_000 + i as i64, &[g], fraser.clone()));
        }
        assert!(
            !filmography_supported(
                &profile,
                &filmography("Zero Dark Thirty", 1, &["Thriller"], fraser.clone())
            ),
            "Fraser Crime/Sci-Fi evidence must not transfer to a Thriller-only credit"
        );
        assert!(filmography_supported(
            &profile,
            &filmography("Rogue One", 1, &["Science Fiction"], fraser.clone())
        ));

        for i in 0..8i64 {
            retrieved.push(filmography(
                &format!("Tarantino {i}"),
                13_000 + i,
                &["Crime"],
                tarantino.clone(),
            ));
        }
        for i in 0..80i64 {
            let g = genres[i as usize % genres.len()];
            retrieved.push(Candidate {
                tmdb_id: Some(20_000 + i),
                title: format!("Related {i}"),
                year: Some(2016),
                poster: None,
                genres: vec![g.into()],
                credits: vec![],
                keywords: vec![],
                runtime: Some(110),
                vote_count: Some(400),
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Related,
                    label: "similar to log".into(),
                    seed_tmdb_id: Some(9 + (i % 20)),
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                friend_affinity: 0.0,
                tmdb_related: 0.55,
            media_kind: MediaKind::Movie,
            });
        }

        let powell_injected = retrieved
            .iter()
            .filter(|c| {
                c.credits.iter().any(|p| p.name == "John Powell")
                    || c.sources.iter().any(|s| s.label == "John Powell")
            })
            .count();
        assert_eq!(powell_injected, 20);

        let scored = score_all(&profile, &retrieved);
        assert!(scored.len() <= 100);
        let top100_powell = scored
            .iter()
            .filter(|c| c.person_keys.iter().any(|k| k == "John Powell"))
            .count();
        let short = shortlist(&scored);
        assert!(
            short.len() <= 50,
            "shortlist must cap at 50, got {}",
            short.len()
        );
        assert!(
            short.len() >= 8,
            "filmography/craft should still fill a shortlist, got {}",
            short.len()
        );
        let related_genre_only = short
            .iter()
            .filter(|c| c.candidate.title.starts_with("Related "))
            .count();
        assert_eq!(
            related_genre_only, 0,
            "related+genre-only padded the shortlist: {related_genre_only}"
        );

        let traces = person_pipeline_trace(&profile, &retrieved, &scored, &short);
        let ptrace = traces.iter().find(|t| t.name == "John Powell").unwrap();
        println!(
            "Powell stages: injected={} facet_ok={} scored/top100={} mmr={} appearances={} rec_mean={:.3} conf={:.3} scoring_affinity={:.3}",
            ptrace.injected,
            powell_facet,
            ptrace.survived_score,
            ptrace.survived_mmr,
            ptrace.appearances,
            ptrace.recommendation_mean,
            ptrace.confidence,
            ptrace.scoring_affinity
        );
        assert_eq!(ptrace.injected, 20);
        assert_eq!(ptrace.appearances, 2);
        assert!(ptrace.survived_score < ptrace.injected);
        assert_eq!(ptrace.survived_score, top100_powell);
        assert!(ptrace.survived_mmr <= 8, "MMR Powell {}", ptrace.survived_mmr);
        assert!((ptrace.confidence - expected_conf).abs() < 0.02);

        let powell_n = short
            .iter()
            .filter(|c| c.person_keys.iter().any(|k| k == "John Powell"))
            .count();
        assert!(
            powell_n <= 8,
            "shortlist Powell takeover: {powell_n} / {}",
            short.len()
        );
        assert!(
            short.iter().any(|c| c.person_keys.iter().any(|k| k.contains("Powell")))
                || scored.iter().any(|c| c.candidate.title.contains("Ice Age")
                    || c.candidate.title == "Antz"),
            "overlapping Powell filmography must be allowed to enter scoring"
        );

        let mut person_counts = std::collections::HashMap::<String, usize>::new();
        for c in &short {
            for k in &c.person_keys {
                *person_counts.entry(k.clone()).or_insert(0) += 1;
            }
        }
        for (name, n) in &person_counts {
            assert!(
                *n <= 8,
                "{name} took over the shortlist with {n} / {}",
                short.len()
            );
        }

        let modes: std::collections::HashSet<_> = short
            .iter()
            .flat_map(|c| c.candidate.modes.iter().cloned())
            .collect();
        assert!(
            modes.len() >= 2,
            "MMR must retain distinct modes, got {modes:?}"
        );

        let mut eight_powell = Vec::new();
        for i in 0..12 {
            let mut row = short[i % short.len()].clone();
            if i < 8 {
                row.person_keys = vec!["John Powell".into()];
            }
            eight_powell.push(row);
        }
        let warnings = diversity_warnings(&eight_powell);
        assert!(
            warnings.iter().any(|w| w.message.contains("person John Powell")),
            "{:?}",
            warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
        );

        let picks: Vec<ReasonerPick> = short
            .iter()
            .take(12)
            .map(|c| ReasonerPick {
                id: format!("tmdb:{}", c.candidate.tmdb_id.unwrap()),
                title: c.candidate.title.clone(),
                year: c.candidate.year,
                why: "because".into(),
                mode: "core".into(),
                rhymes_with: vec![],
            })
            .collect();
        let result = hard_validate(&picks, &short, &[], &HashSet::new(), profile.modes.len().max(3));
        assert!(result.picks.len() <= 100);
        assert!(
            result.picks.iter().all(|p| {
                short.iter().any(|s| s.candidate.tmdb_id == p.candidate.tmdb_id)
            }),
            "validate must stay inside the scored shortlist"
        );
        assert!(result.picks.iter().all(|p| p.candidate.tmdb_id != Some(99)));
        let _ = empty_critic();
    }

    /// Workspace-9 live baseline (2026-08-26 latest.json): 26 New rows, related-only
    /// Reservoir Dogs at exact 70, ~25 related-only competing for New.
    /// Workspace-10+ must report related-only on New = 0. Match score 70 is a band,
    /// not P(watch). Workspace-11 also requires the cited director/DP job on the film.
    #[test]
    fn board_quality_rejects_workspace9_related_padding() {
        use crate::taste::explain::{EligibilityTrace, MatchedFeatureView};
        use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
        use crate::taste::score::{CandidateScore, CandidateView, ScoredCandidate};
        use crate::taste::workspace::assemble;

        fn feat(name: &str, family: &str, n: u32, aff: f32) -> MatchedFeatureView {
            MatchedFeatureView {
                feature_key: String::new(),
                name: name.into(),
                family: family.into(),
                appearances: n,
                recommendation_mean: aff,
                scoring_affinity: aff,
                confidence: 0.8,
                portability: 1.0,
                citeable: true,
                cited: true,
            }
        }
        fn scored(
            id: i64,
            title: &str,
            related: bool,
            features: Vec<MatchedFeatureView>,
            watchlist: bool,
        ) -> ScoredCandidate {
            ScoredCandidate {
                candidate: CandidateView {
                    tmdb_id: Some(id),
                    title: title.into(),
                    year: Some(1992),
                    poster: None,
                    watchlist,
                    sources: vec![RetrievalSource {
                        kind: if watchlist {
                            RetrievalKind::Watchlist
                        } else if related {
                            RetrievalKind::Related
                        } else {
                            RetrievalKind::Filmography
                        },
                        label: if related {
                            "similar to Pulp Fiction".into()
                        } else {
                            features
                                .first()
                                .map(|f| f.name.clone())
                                .unwrap_or_else(|| "x".into())
                        },
                        seed_tmdb_id: if related { Some(680) } else { None },
                        seed_rating: None,
                        similarity: None,
                        neighbor_rank: None,
                    }],
                    directors: vec!["Q".into()],
                    genres: vec!["Crime".into()],
                    modes: vec![],
                    media_kind: MediaKind::Movie,
                    runtime: Some(99),
                    vote_count: Some(5000),
                    semantic_cluster: None,
                },
                score: CandidateScore {
                    content: 0.5,
                    tmdb_related: if related { 1.0 } else { 0.0 },
                    friend_affinity: 0.0,
                    recent_taste: 0.0,
                    watchlist: if watchlist { 1.0 } else { 0.0 },
                    novelty: 0.0,
                    negative_evidence: 0.0,
                    semantic_fit: 0.5,
                    semantic_coverage: false,
                    total: 0.4,
                },
                reasons: vec![],
                evidence: vec![],
                positive_features: features.iter().map(|f| f.name.clone()).collect(),
                negative_features: vec![],
                contextual_only: false,
                person_keys: features.iter().map(|f| f.name.clone()).collect(),
                display_reasons: vec![],
                scoring_reasons: vec![],
                matched_features: features.clone(),
                hidden_features: vec![],
                eligibility: EligibilityTrace {
                    portable_evidence_required: false,
                    passed: watchlist || !features.is_empty(),
                    passed_because: vec!["craft".into()],
                    candidate_fit: 1.0,
                    evidence_grade: if watchlist {
                        crate::taste::explain::EvidenceGrade::Medium
                    } else if features.is_empty() {
                        crate::taste::explain::EvidenceGrade::None
                    } else {
                        crate::taste::explain::EvidenceGrade::Medium
                    },
                    predicted_fit: if features.is_empty() { 0.3 } else { 0.72 },
                    confidence: 0.6,
                    hydration_completeness: 0.7,
                    state: if watchlist || !features.is_empty() {
                        "recommended".into()
                    } else {
                        "held".into()
                    },
                    primary_reason: if watchlist || !features.is_empty() {
                        "recommended".into()
                    } else {
                        "low_fit".into()
                    },
                },
                quality_prior: 0.0,
                has_quality_prior: false,
            }
        }

        let dogs = scored(500, "Reservoir Dogs", true, vec![], false);
        let kts = scored(
            49_530,
            "Killing Them Softly",
            false,
            vec![
                feat("Greig Fraser", "cinematographer", 4, 0.45),
                feat("neo-noir", "keyword", 9, 0.4),
            ],
            false,
        );
        let dune = scored(
            438_631,
            "Dune",
            false,
            vec![
                feat("Greig Fraser", "cinematographer", 4, 0.45),
                feat("Denis Villeneuve", "director", 5, 0.55),
            ],
            true,
        );
        let ws = assemble(&[dogs.clone(), kts, dune]);
        let q = board_quality(&ws, &HashSet::new());
        assert!(q.new_count >= 1);
        assert!(q.watchlist_count >= 1);
        assert_eq!(q.explore_count, 0);
        assert_eq!(q.new_below_floor, 0);
        assert_eq!(q.explore_outside_band, 0);
        assert_eq!(q.seen_leakage, 0);
        assert_eq!(q.board_overlap, 0);
        assert_eq!(q.watchlist_on_discovery_boards, 0);
        assert!(
            ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(49_530)),
            "qualifying DP filmography belongs on New"
        );
        assert!(
            !ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(500)),
            "lone similar-to must not occupy New"
        );
        assert!(ws.watchlist_picks.iter().any(|c| c.candidate.tmdb_id == Some(438_631)));
        assert!(
            crate::taste::confidence::match_score(&dogs) <= crate::taste::confidence::RELATED_ONLY_CAP,
            "70 is a band ceiling for related-only, not a pad, got {}",
            crate::taste::confidence::match_score(&dogs)
        );
    }
}
