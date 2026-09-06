//! Milestone A2 retrieval diagnostics: oracle caps, marginal generator recall,
//! miss classification, and examination-rank histograms.

use crate::storage::db::Database;
use crate::taste::eval::{
    rating_bucket, recall_at, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
};
use crate::taste::features::{family_for_job, FeatureFamily};
use crate::taste::retrieve::{
    build_retrieval_pool, filter_pool_by_family, identity_key, pool_without_family, select_fair_pool,
    Candidate, FilmRecord, GeneratorFamily, RetrievalKind,
};
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub const ORACLE_CAPS: [usize; 4] = [1000, 2500, 5000, usize::MAX / 4];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapSweepPoint {
    pub cap: usize,
    pub recall_at_100: f32,
    pub recall_at_250: f32,
    pub recall_at_1000: f32,
    pub recall_at_cap: f32,
    pub pool_size: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankHistogram {
    pub r1_100: usize,
    pub r101_250: usize,
    pub r251_500: usize,
    pub r501_750: usize,
    pub r751_1000: usize,
    pub beyond_1000: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorMarginal {
    pub generator: String,
    pub standalone_recall: f32,
    pub union_without_recall: f32,
    pub delta_if_removed: f32,
    pub median_first_rank: Option<f32>,
    pub p90_first_rank: Option<f32>,
    pub candidates_generated: usize,
    pub implemented: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissedPositive {
    pub title: String,
    pub tmdb_id: Option<i64>,
    pub identity: String,
    pub available: MissBridges,
    pub attempted: MissAttempted,
    pub reason: String,
    pub category: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissBridges {
    pub actor_affinity: bool,
    pub director_affinity: bool,
    pub writer_affinity: bool,
    pub collection: bool,
    pub related_seed_edge: bool,
    pub embedding_cached: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissAttempted {
    pub in_uncapped_pool: bool,
    pub uncapped_rank: Option<usize>,
    pub in_cap_1000: bool,
    pub filmography_generated: bool,
    pub collection_generated: bool,
    pub related_generated: bool,
    pub semantic_note: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticUniverseAudit {
    pub film_local_generator_implemented: bool,
    pub profile_generator_implemented: bool,
    pub embeddings_in_db: usize,
    pub held_out_positives_with_embedding: usize,
    pub held_out_positives: usize,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A2FoldReport {
    pub seed: u64,
    pub held_out_positives: usize,
    pub uncapped_pool_size: usize,
    pub seeds_with_collection: usize,
    pub collection_candidates: usize,
    pub cap_sweep: Vec<CapSweepPoint>,
    pub rank_histogram_at_1000: RankHistogram,
    pub generators: Vec<GeneratorMarginal>,
    pub misses: Vec<MissedPositive>,
    pub miss_categories: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A2BenchmarkReport {
    pub protocol: String,
    pub algorithm_version: String,
    pub rated_films: usize,
    pub folds: usize,
    pub holdout_frac: f32,
    pub seeds: Vec<u64>,
    pub fold_reports: Vec<A2FoldReport>,
    pub mean_recall_by_cap: Vec<CapSweepSummary>,
    pub mean_generators: Vec<GeneratorMarginal>,
    pub semantic: SemanticUniverseAudit,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapSweepSummary {
    pub cap: usize,
    pub recall_at_100: MetricSummary,
    pub recall_at_250: MetricSummary,
    pub recall_at_1000: MetricSummary,
    pub recall_at_cap: MetricSummary,
    pub pool_size: MetricSummary,
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

fn held_out_positives(all_films: &[FilmRecord], held_out: &HashSet<String>) -> Vec<FilmRecord> {
    all_films
        .iter()
        .filter(|f| {
            let key = identity_key(f.tmdb_id, &f.title, f.year);
            held_out.contains(&key)
                && f.rating
                    .map(|r| matches!(rating_bucket(r), RatingBucket::Loved | RatingBucket::Liked))
                    .unwrap_or(false)
        })
        .cloned()
        .collect()
}

pub(crate) fn ids_of(cands: &[Candidate]) -> Vec<String> {
    cands
        .iter()
        .map(|c| identity_key(c.tmdb_id, &c.title, c.year))
        .collect()
}

fn positive_id_set(positives: &[FilmRecord]) -> HashSet<String> {
    positives
        .iter()
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect()
}

fn first_ranks(ordered: &[Candidate], positives: &HashSet<String>) -> Vec<usize> {
    let mut ranks = Vec::new();
    for (i, c) in ordered.iter().enumerate() {
        let key = identity_key(c.tmdb_id, &c.title, c.year);
        if positives.contains(&key) {
            ranks.push(i + 1);
        }
    }
    ranks
}

fn percentile(sorted: &[usize], p: f32) -> Option<f32> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p).round() as usize;
    Some(sorted[idx.min(sorted.len() - 1)] as f32)
}

fn rank_histogram(ranks: &[usize]) -> RankHistogram {
    let mut h = RankHistogram::default();
    for &r in ranks {
        match r {
            1..=100 => h.r1_100 += 1,
            101..=250 => h.r101_250 += 1,
            251..=500 => h.r251_500 += 1,
            501..=750 => h.r501_750 += 1,
            751..=1000 => h.r751_1000 += 1,
            _ => h.beyond_1000 += 1,
        }
    }
    h
}

fn generator_label(family: GeneratorFamily) -> &'static str {
    match family {
        GeneratorFamily::Related => "related",
        GeneratorFamily::Filmography => "filmography",
        GeneratorFamily::Collection => "collection",
        GeneratorFamily::SemanticFilmLocal => "semanticFilmLocal",
        GeneratorFamily::SemanticProfile => "semanticProfile",
        GeneratorFamily::Friend => "friend",
        GeneratorFamily::Watchlist => "watchlist",
        GeneratorFamily::Discovery => "discovery",
        GeneratorFamily::Exploration => "exploration",
    }
}

fn measure_generators(
    full_map: &HashMap<String, Candidate>,
    positives: &HashSet<String>,
    exam_cap: usize,
) -> Vec<GeneratorMarginal> {
    let full = select_fair_pool(full_map.clone(), exam_cap);
    let full_ids = ids_of(&full);
    let full_recall = recall_at(&full_ids, positives, exam_cap);
    let families = [
        GeneratorFamily::Related,
        GeneratorFamily::Filmography,
        GeneratorFamily::Collection,
        GeneratorFamily::SemanticFilmLocal,
        GeneratorFamily::SemanticProfile,
    ];
    let mut out = Vec::new();
    for family in families {
        let implemented = true; // all five families exist as generators as of A3
        // Semantic kinds report implemented=true once retrieve_semantic_candidates runs.
        let standalone_map = filter_pool_by_family(full_map, family);
        let candidates_generated = standalone_map.len();
        let standalone = select_fair_pool(standalone_map, exam_cap);
        let standalone_ids = ids_of(&standalone);
        let standalone_recall = recall_at(&standalone_ids, positives, exam_cap);
        let ranks = first_ranks(&standalone, positives);
        let mut sorted_ranks = ranks;
        sorted_ranks.sort_unstable();
        let without = pool_without_family(full_map, family);
        let without_pool = select_fair_pool(without, exam_cap);
        let without_recall = recall_at(&ids_of(&without_pool), positives, exam_cap);
        out.push(GeneratorMarginal {
            generator: generator_label(family).into(),
            standalone_recall,
            union_without_recall: without_recall,
            delta_if_removed: full_recall - without_recall,
            median_first_rank: percentile(&sorted_ranks, 0.5),
            p90_first_rank: percentile(&sorted_ranks, 0.9),
            candidates_generated,
            implemented,
        });
    }
    out
}

fn training_person_ids(films: &[FilmRecord], family: FeatureFamily) -> HashSet<i64> {
    films
        .iter()
        .filter(|f| {
            f.rating
                .map(|r| matches!(rating_bucket(r), RatingBucket::Loved | RatingBucket::Liked))
                .unwrap_or(false)
        })
        .flat_map(|f| f.credits.iter())
        .filter(|c| family_for_job(&c.job) == Some(family))
        .filter_map(|c| c.id)
        .collect()
}

fn related_neighbor_ids(films: &[FilmRecord]) -> HashSet<i64> {
    let mut ids = HashSet::new();
    for f in films.iter().filter(|f| {
        f.rating
            .map(|r| matches!(rating_bucket(r), RatingBucket::Loved | RatingBucket::Liked))
            .unwrap_or(false)
    }) {
        for item in f.recommendations.iter().chain(f.similar.iter()) {
            if let Some(id) = item
                .id
                .strip_prefix("tmdb:")
                .and_then(|s| s.parse().ok())
                .or_else(|| item.id.parse().ok())
            {
                ids.insert(id);
            }
        }
    }
    ids
}

fn collection_sibling_ids(films: &[FilmRecord]) -> HashSet<i64> {
    let mut ids = HashSet::new();
    for f in films.iter().filter(|f| {
        f.rating
            .map(|r| matches!(rating_bucket(r), RatingBucket::Loved | RatingBucket::Liked))
            .unwrap_or(false)
    }) {
        for item in &f.collection {
            if let Some(id) = item
                .id
                .strip_prefix("tmdb:")
                .and_then(|s| s.parse().ok())
                .or_else(|| item.id.parse().ok())
            {
                ids.insert(id);
            }
        }
    }
    ids
}

fn embedding_cached(db: &Database, tmdb_id: i64) -> bool {
    db.conn()
        .query_row(
            "SELECT 1 FROM taste_embeddings WHERE tmdb_id = ?1 LIMIT 1",
            rusqlite::params![tmdb_id],
            |_| Ok(()),
        )
        .ok()
        .is_some()
}

fn classify_miss(
    db: &Database,
    miss: &FilmRecord,
    training: &[FilmRecord],
    uncapped: &[Candidate],
    cap1000: &HashSet<String>,
) -> MissedPositive {
    let identity = identity_key(miss.tmdb_id, &miss.title, miss.year);
    let actors = training_person_ids(training, FeatureFamily::Actor);
    let directors = training_person_ids(training, FeatureFamily::Director);
    let writers = training_person_ids(training, FeatureFamily::Writer);
    let miss_actors: HashSet<i64> = miss
        .credits
        .iter()
        .filter(|c| family_for_job(&c.job) == Some(FeatureFamily::Actor))
        .filter_map(|c| c.id)
        .collect();
    let miss_directors: HashSet<i64> = miss
        .credits
        .iter()
        .filter(|c| family_for_job(&c.job) == Some(FeatureFamily::Director))
        .filter_map(|c| c.id)
        .collect();
    let miss_writers: HashSet<i64> = miss
        .credits
        .iter()
        .filter(|c| family_for_job(&c.job) == Some(FeatureFamily::Writer))
        .filter_map(|c| c.id)
        .collect();
    let actor_affinity = miss_actors.iter().any(|id| actors.contains(id));
    let director_affinity = miss_directors.iter().any(|id| directors.contains(id));
    let writer_affinity = miss_writers.iter().any(|id| writers.contains(id));
    let related_seed_edge = miss
        .tmdb_id
        .map(|id| related_neighbor_ids(training).contains(&id))
        .unwrap_or(false);
    let collection = miss
        .tmdb_id
        .map(|id| collection_sibling_ids(training).contains(&id))
        .unwrap_or(false);
    let embedding = miss
        .tmdb_id
        .map(|id| embedding_cached(db, id))
        .unwrap_or(false);

    let uncapped_rank = uncapped.iter().position(|c| {
        identity_key(c.tmdb_id, &c.title, c.year) == identity
    });
    let in_uncapped = uncapped_rank.is_some();
    let in_cap_1000 = cap1000.contains(&identity);

    let hit = uncapped
        .iter()
        .find(|c| identity_key(c.tmdb_id, &c.title, c.year) == identity);
    let filmography_generated = hit
        .map(|c| {
            c.sources
                .iter()
                .any(|s| s.kind == RetrievalKind::Filmography)
        })
        .unwrap_or(false);
    let collection_generated = hit
        .map(|c| c.sources.iter().any(|s| s.kind == RetrievalKind::Collection))
        .unwrap_or(false);
    let related_generated = hit
        .map(|c| c.sources.iter().any(|s| s.kind.is_related()))
        .unwrap_or(false);
    let semantic_local = hit
        .map(|c| {
            c.sources
                .iter()
                .any(|s| s.kind == RetrievalKind::SemanticFilmLocal)
        })
        .unwrap_or(false);
    let semantic_profile = hit
        .map(|c| {
            c.sources
                .iter()
                .any(|s| s.kind == RetrievalKind::SemanticProfile)
        })
        .unwrap_or(false);

    let (category, reason) = if in_uncapped && !in_cap_1000 {
        (
            "candidate_cap_failure",
            format!(
                "present in uncapped pool at rank {} but dropped by examination cap 1000",
                uncapped_rank.unwrap_or(0) + 1
            ),
        )
    } else if actor_affinity && !filmography_generated && !semantic_local && !semantic_profile {
        (
            "person_retrieval_failure",
            "shared actor affinity exists but filmography/semantic did not generate this candidate".into(),
        )
    } else if director_affinity && !filmography_generated && !semantic_local && !semantic_profile {
        (
            "person_retrieval_failure",
            "shared director affinity exists but filmography/semantic did not generate this candidate".into(),
        )
    } else if collection && !collection_generated {
        (
            "collection_metadata_failure",
            "training collection sibling exists but collection generator missed this title".into(),
        )
    } else if related_seed_edge && !related_generated {
        (
            "related_graph_failure",
            "TMDB related edge from a liked seed exists but related retrieval missed it".into(),
        )
    } else if embedding && !semantic_local && !semantic_profile && !in_uncapped {
        (
            "semantic_retrieval_failure",
            "embedding exists in index but neither SemanticFilmLocal nor SemanticProfile retrieved it".into(),
        )
    } else if !actor_affinity
        && !director_affinity
        && !writer_affinity
        && !collection
        && !related_seed_edge
        && !embedding
    {
        (
            "no_current_generator_capable",
            "no person/collection/related/semantic bridge from training history".into(),
        )
    } else if embedding && !in_uncapped {
        (
            "semantic_retrieval_failure",
            "in embedding index but not recovered by semantic or other generators".into(),
        )
    } else {
        (
            "no_current_generator_capable",
            "available bridges did not produce a retrieval hit".into(),
        )
    };

    MissedPositive {
        title: miss.title.clone(),
        tmdb_id: miss.tmdb_id,
        identity,
        available: MissBridges {
            actor_affinity,
            director_affinity,
            writer_affinity,
            collection,
            related_seed_edge,
            embedding_cached: embedding,
        },
        attempted: MissAttempted {
            in_uncapped_pool: in_uncapped,
            uncapped_rank: uncapped_rank.map(|r| r + 1),
            in_cap_1000,
            filmography_generated,
            collection_generated,
            related_generated,
            semantic_note: format!(
                "semanticFilmLocal={semantic_local} semanticProfile={semantic_profile}"
            ),
        },
        reason,
        category: category.into(),
    }
}

fn audit_semantic(db: &Database, positives: &[FilmRecord]) -> SemanticUniverseAudit {
    let held_ids: HashSet<i64> = positives.iter().filter_map(|f| f.tmdb_id).collect();
    let stats =
        crate::taste::semantic::semantic_universe_stats(db, &HashSet::new(), Some(&held_ids));
    let held_out_positives_with_embedding = positives
        .iter()
        .filter(|f| f.tmdb_id.map(|id| embedding_cached(db, id)).unwrap_or(false))
        .count();
    SemanticUniverseAudit {
        film_local_generator_implemented: true,
        profile_generator_implemented: true,
        embeddings_in_db: stats.semantic_index_movies,
        held_out_positives_with_embedding,
        held_out_positives: positives.len(),
        note: format!(
            "A3 semantic generators active. index_movies={} heldout_index_coverage={:?}",
            stats.semantic_index_movies, stats.heldout_positive_index_coverage
        ),
    }
}

fn evaluate_a2_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
) -> Result<A2FoldReport, String> {
    let positives = held_out_positives(all_films, &inputs.held_out);
    let positive_ids = positive_id_set(&positives);
    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let seeds_with_collection = inputs
        .training_films
        .iter()
        .filter(|f| !f.collection.is_empty())
        .count();
    let collection_candidates = filter_pool_by_family(&pool.by_key, GeneratorFamily::Collection).len();
    let uncapped = select_fair_pool(pool.by_key.clone(), usize::MAX / 4);
    let cap1000 = select_fair_pool(pool.by_key.clone(), 1000);
    let cap1000_ids: HashSet<String> = ids_of(&cap1000).into_iter().collect();

    let mut cap_sweep = Vec::new();
    for &cap in &ORACLE_CAPS {
        let ordered = select_fair_pool(pool.by_key.clone(), cap);
        let ids = ids_of(&ordered);
        let effective_cap = ordered.len();
        cap_sweep.push(CapSweepPoint {
            cap: if cap > 1_000_000 { 0 } else { cap },
            recall_at_100: recall_at(&ids, &positive_ids, 100),
            recall_at_250: recall_at(&ids, &positive_ids, 250),
            recall_at_1000: recall_at(&ids, &positive_ids, 1000),
            recall_at_cap: recall_at(&ids, &positive_ids, effective_cap.max(1)),
            pool_size: ordered.len(),
        });
    }

    let ranks_1000 = first_ranks(&cap1000, &positive_ids);
    let generators = measure_generators(&pool.by_key, &positive_ids, 1000);

    let mut misses = Vec::new();
    for miss in &positives {
        let key = identity_key(miss.tmdb_id, &miss.title, miss.year);
        if cap1000_ids.contains(&key) {
            continue;
        }
        misses.push(classify_miss(
            db,
            miss,
            &inputs.training_films,
            &uncapped,
            &cap1000_ids,
        ));
    }
    let mut miss_categories: HashMap<String, usize> = HashMap::new();
    for m in &misses {
        *miss_categories.entry(m.category.clone()).or_default() += 1;
    }

    Ok(A2FoldReport {
        seed,
        held_out_positives: positives.len(),
        uncapped_pool_size: uncapped.len(),
        seeds_with_collection,
        collection_candidates,
        cap_sweep,
        rank_histogram_at_1000: rank_histogram(&ranks_1000),
        generators,
        misses,
        miss_categories,
    })
}

fn mean_generators(folds: &[A2FoldReport]) -> Vec<GeneratorMarginal> {
    if folds.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = folds[0]
        .generators
        .iter()
        .map(|g| g.generator.clone())
        .collect();
    names
        .into_iter()
        .map(|name| {
            let rows: Vec<&GeneratorMarginal> = folds
                .iter()
                .filter_map(|f| f.generators.iter().find(|g| g.generator == name))
                .collect();
            let n = rows.len().max(1) as f32;
            let medians: Vec<f32> = rows.iter().filter_map(|g| g.median_first_rank).collect();
            let p90s: Vec<f32> = rows.iter().filter_map(|g| g.p90_first_rank).collect();
            GeneratorMarginal {
                generator: name,
                standalone_recall: rows.iter().map(|g| g.standalone_recall).sum::<f32>() / n,
                union_without_recall: rows.iter().map(|g| g.union_without_recall).sum::<f32>() / n,
                delta_if_removed: rows.iter().map(|g| g.delta_if_removed).sum::<f32>() / n,
                median_first_rank: if medians.is_empty() {
                    None
                } else {
                    Some(medians.iter().sum::<f32>() / medians.len() as f32)
                },
                p90_first_rank: if p90s.is_empty() {
                    None
                } else {
                    Some(p90s.iter().sum::<f32>() / p90s.len() as f32)
                },
                candidates_generated: (rows
                    .iter()
                    .map(|g| g.candidates_generated as f32)
                    .sum::<f32>()
                    / n)
                    .round() as usize,
                implemented: rows.first().map(|g| g.implemented).unwrap_or(false),
            }
        })
        .collect()
}

fn decide_gate(mean_by_cap: &[CapSweepSummary]) -> String {
    let at_1000 = mean_by_cap.iter().find(|c| c.cap == 1000);
    let at_2500 = mean_by_cap.iter().find(|c| c.cap == 2500);
    let at_5000 = mean_by_cap.iter().find(|c| c.cap == 5000);
    let uncapped = mean_by_cap.iter().find(|c| c.cap == 0);
    let r250 = at_1000.map(|c| c.recall_at_250.mean).unwrap_or(0.0);
    let r1000 = at_1000.map(|c| c.recall_at_1000.mean).unwrap_or(0.0);
    let r5000 = at_5000
        .or(at_2500)
        .map(|c| c.recall_at_cap.mean)
        .unwrap_or(0.0);
    let r_oracle = uncapped.map(|c| c.recall_at_cap.mean).unwrap_or(r5000);
            if r250 >= 0.15 && r1000 >= 0.45 && r_oracle >= 0.55 {
        format!(
            "START_B: recall@250={r250:.3} recall@1000={r1000:.3} oracle={r_oracle:.3} — scoring has enough material"
        )
    } else if r250 > 0.0 && r1000 > 0.40 && r_oracle > 0.58 {
        format!(
            "BORDERLINE_B: recall@250={r250:.3} recall@1000={r1000:.3} oracle={r_oracle:.3} — review miss categories before B"
        )
    } else if r_oracle - r1000 >= 0.25 {
        format!(
            "HOLD_B_CAP: generators find more (oracle={r_oracle:.3}) than cap 1000 keeps ({r1000:.3}); fix examination before B"
        )
    } else {
        format!(
            "HOLD_B_COVERAGE: oracle={r_oracle:.3} recall@1000={r1000:.3} recall@250={r250:.3} — discovery universe still the bottleneck"
        )
    }
}

/// Full Milestone A2 report on the same stratified folds as Milestone A.
pub fn run_a2_benchmark(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<A2BenchmarkReport, String> {
    let mut fold_reports = Vec::new();
    let mut all_positives_for_semantic = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        if inputs.held_out.is_empty() {
            continue;
        }
        let positives = held_out_positives(films, &inputs.held_out);
        all_positives_for_semantic.extend(positives);
        fold_reports.push(evaluate_a2_fold(db, seed, &inputs, films)?);
    }

    let mut mean_recall_by_cap = Vec::new();
    for (i, &cap) in ORACLE_CAPS.iter().enumerate() {
        let display_cap = if cap > 1_000_000 { 0 } else { cap };
        let r100: Vec<f32> = fold_reports
            .iter()
            .filter_map(|f| f.cap_sweep.get(i).map(|p| p.recall_at_100))
            .collect();
        let r250: Vec<f32> = fold_reports
            .iter()
            .filter_map(|f| f.cap_sweep.get(i).map(|p| p.recall_at_250))
            .collect();
        let r1000: Vec<f32> = fold_reports
            .iter()
            .filter_map(|f| f.cap_sweep.get(i).map(|p| p.recall_at_1000))
            .collect();
        let rcap: Vec<f32> = fold_reports
            .iter()
            .filter_map(|f| f.cap_sweep.get(i).map(|p| p.recall_at_cap))
            .collect();
        let pools: Vec<usize> = fold_reports
            .iter()
            .filter_map(|f| f.cap_sweep.get(i).map(|p| p.pool_size))
            .collect();
        mean_recall_by_cap.push(CapSweepSummary {
            cap: display_cap,
            recall_at_100: summarize(&r100),
            recall_at_250: summarize(&r250),
            recall_at_1000: summarize(&r1000),
            recall_at_cap: summarize(&rcap),
            pool_size: summarize_usize(&pools),
        });
    }

    let semantic = audit_semantic(db, &all_positives_for_semantic);
    let gate = decide_gate(&mean_recall_by_cap);

    Ok(A2BenchmarkReport {
        protocol: "stratified-repeated-holdout-a2-v1".into(),
        algorithm_version: ALGORITHM_VERSION.into(),
        rated_films: films.iter().filter(|f| f.rating.is_some()).count(),
        folds: fold_reports.len(),
        holdout_frac,
        seeds: seeds.to_vec(),
        mean_generators: mean_generators(&fold_reports),
        fold_reports,
        mean_recall_by_cap,
        semantic,
        gate,
    })
}

pub fn write_a2_artifact(
    taste_runs_dir: &std::path::Path,
    report: &A2BenchmarkReport,
) -> Result<String, String> {
    let dir = taste_runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("retrieval-a3-{stamp}.json"));
    let latest = dir.join("retrieval-a3-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    // Keep A2 filename aliases during the transition.
    let _ = std::fs::write(dir.join(format!("retrieval-a2-{stamp}.json")), &body);
    let _ = std::fs::write(dir.join("retrieval-a2-latest.json"), &body);
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::{MediaKind, RetrievalSource};

    fn cand(id: i64, kind: RetrievalKind) -> Candidate {
        Candidate {
            tmdb_id: Some(id),
            title: format!("T{id}"),
            year: Some(2000),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![RetrievalSource::new(kind, "x", Some(1))],
            friend_affinity: 0.0,
            tmdb_related: if kind.is_related() { 1.0 } else { 0.0 },
            media_kind: MediaKind::Movie,
        }
    }

    #[test]
    fn marginal_delta_detects_unique_filmography_coverage() {
        let mut map = HashMap::new();
        // Shared related+filmography hit
        let mut both = cand(10, RetrievalKind::RelatedRecommendations);
        both.sources.push(RetrievalSource::new(
            RetrievalKind::Filmography,
            "Actor",
            None,
        ));
        map.insert("tmdb:10".into(), both);
        // Filmography-only unique positive
        map.insert(
            "tmdb:11".into(),
            cand(11, RetrievalKind::Filmography),
        );
        // Related-only
        map.insert(
            "tmdb:12".into(),
            cand(12, RetrievalKind::RelatedRecommendations),
        );
        let positives = HashSet::from(["tmdb:10".into(), "tmdb:11".into()]);
        let gens = measure_generators(&map, &positives, 100);
        let filmography = gens.iter().find(|g| g.generator == "filmography").unwrap();
        assert!(
            filmography.delta_if_removed > 0.2,
            "removing filmography should lose the unique hit: {filmography:?}"
        );
        let related = gens.iter().find(|g| g.generator == "related").unwrap();
        assert!(related.delta_if_removed >= 0.0);
    }

    #[test]
    fn family_budget_keeps_non_related_in_early_tranche() {
        let mut map = HashMap::new();
        for i in 0..400 {
            map.insert(
                format!("tmdb:{i}"),
                cand(i, RetrievalKind::RelatedRecommendations),
            );
        }
        for i in 400..420 {
            map.insert(
                format!("tmdb:{i}"),
                cand(i, RetrievalKind::Filmography),
            );
        }
        let selected = select_fair_pool(map, 200);
        let filmography_in_top = selected
            .iter()
            .take(200)
            .filter(|c| {
                c.sources
                    .iter()
                    .any(|s| s.kind == RetrievalKind::Filmography)
            })
            .count();
        assert!(
            filmography_in_top >= 10,
            "family budget should reserve early slots for filmography, got {filmography_in_top}"
        );
    }
}
