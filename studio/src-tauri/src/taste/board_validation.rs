//! Live v1 board validation under frozen active-2k policy.
//! Diagnostics only — does not change retrieval, scoring, eligibility, or D1.

use crate::storage::db::Database;
use crate::taste::confidence::{self, filter_reason};
use crate::taste::diversify::{
    broad_mode_key, collection_key, director_key, fit_of, semantic_key,
};
use crate::taste::eligibility::EligibilityState;
use crate::taste::exam_policy::{exam_mode, V1_ACTIVE_SEMANTIC_CAP};
use crate::taste::family_fit::{score_family_fit_with_config, CraftConfig};
use crate::taste::match_calibration::fit_to_match_percent;
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord, GeneratorFamily,
};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::{self, score_candidates_from_cache, SemanticScore};
use crate::taste::workspace::{self, ALGORITHM_VERSION};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

const BOARD_DISPLAY: usize = 12;
const ELIGIBLE_DISPLAY: usize = 25;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalDiag {
    pub generators: Vec<String>,
    pub semantic_film_local: bool,
    pub semantic_profile: bool,
    pub seed_labels: Vec<String>,
    pub profile_labels: Vec<String>,
    pub best_neighbor_rank: Option<u32>,
    pub best_similarity: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDiag {
    pub semantic_positive: f32,
    pub semantic_negative: f32,
    pub margin: f32,
    pub motif_affinity: f32,
    pub genre_affinity: f32,
    pub mode_affinity: f32,
    pub content_confidence: f32,
    pub content_score: f32,
    pub semantic_coverage: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCandidateDiag {
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub match_percent: u8,
    pub eligibility_state: String,
    pub eligibility_reason: String,
    pub raw_content_rank: usize,
    pub final_board_rank: Option<usize>,
    pub diversify_delta: Option<i32>,
    pub content: ContentDiag,
    pub retrieval: RetrievalDiag,
    pub judgment: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDiag {
    pub collection: Option<String>,
    pub director: Option<String>,
    pub broad_mode: Option<String>,
    pub semantic: Option<String>,
    /// Human-readable primary cluster for concentration analysis.
    pub primary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcentrationRow {
    pub raw_rank: usize,
    pub title: String,
    pub year: Option<i32>,
    pub fit: f32,
    pub match_percent: u8,
    pub cluster: ClusterDiag,
    pub selected: bool,
    pub final_board_rank: Option<usize>,
    pub closest_selected_duplicate: Option<String>,
    pub closest_selected_duplicate_rank: Option<usize>,
    pub fit_delta_to_duplicate: Option<f32>,
    /// Board #12 fit − this fit (positive ⇒ weaker than floor / would need rescue).
    pub fit_delta_to_board_floor: Option<f32>,
    /// If not selected: nearest different-cluster board member by fit (who crowded the scarce slot).
    pub displaced_by: Option<String>,
    pub fit_delta_to_displacer: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanePopulations {
    pub new_candidates: usize,
    pub new_recommended: usize,
    pub new_exploratory: usize,
    pub new_held: usize,
    pub watchlist_candidates: usize,
    pub watchlist_recommended: usize,
    pub new_board_fit_first: Option<f32>,
    pub new_board_fit_twelfth: Option<f32>,
    pub new_board_fit_span: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBoardValidationReport {
    pub algorithm_version: String,
    pub policy: String,
    pub active_retrieval_cap: usize,
    pub stored_embeddings: usize,
    pub exam_mode: String,
    pub exam_cap: usize,
    pub rated_films: usize,
    pub retrieval_ms: f32,
    pub score_ms: f32,
    pub examined: usize,
    pub eligible_n: usize,
    pub recommended_n: usize,
    pub exploratory_n: usize,
    pub held_n: usize,
    pub lanes: LanePopulations,
    pub concentration: Vec<ConcentrationRow>,
    pub final_board: Vec<BoardCandidateDiag>,
    pub top_eligible: Vec<BoardCandidateDiag>,
    pub top_new_eligible: Vec<BoardCandidateDiag>,
    pub watchlist_board: Vec<BoardCandidateDiag>,
    pub new_filter_counts: HashMap<String, usize>,
    pub notes: Vec<String>,
}

fn family_name(f: GeneratorFamily) -> &'static str {
    match f {
        GeneratorFamily::Related => "Related",
        GeneratorFamily::Filmography => "Filmography",
        GeneratorFamily::Collection => "Collection",
        GeneratorFamily::SemanticFilmLocal => "SemanticFilmLocal",
        GeneratorFamily::SemanticProfile => "SemanticProfile",
        GeneratorFamily::Friend => "Friend",
        GeneratorFamily::Watchlist => "Watchlist",
        GeneratorFamily::Discovery => "Discovery",
        GeneratorFamily::Exploration => "Exploration",
    }
}

fn retrieval_diag(c: &Candidate) -> RetrievalDiag {
    let mut gens: Vec<String> = c
        .sources
        .iter()
        .map(|s| family_name(s.kind.generator_family()).to_string())
        .collect();
    gens.sort();
    gens.dedup();
    let film_local = c
        .sources
        .iter()
        .any(|s| s.kind.generator_family() == GeneratorFamily::SemanticFilmLocal);
    let profile = c
        .sources
        .iter()
        .any(|s| s.kind.generator_family() == GeneratorFamily::SemanticProfile);
    let mut seed_labels = Vec::new();
    let mut profile_labels = Vec::new();
    let mut best_rank = None;
    let mut best_sim = None::<f32>;
    for s in &c.sources {
        match s.kind.generator_family() {
            GeneratorFamily::SemanticFilmLocal => {
                if !s.label.is_empty() {
                    seed_labels.push(s.label.clone());
                }
                if let Some(r) = s.neighbor_rank {
                    best_rank = Some(best_rank.map_or(r, |b: u32| b.min(r)));
                }
                if let Some(v) = s.similarity {
                    best_sim = Some(best_sim.map_or(v, |b| b.max(v)));
                }
            }
            GeneratorFamily::SemanticProfile => {
                if !s.label.is_empty() {
                    profile_labels.push(s.label.clone());
                }
                if let Some(r) = s.neighbor_rank {
                    best_rank = Some(best_rank.map_or(r, |b: u32| b.min(r)));
                }
                if let Some(v) = s.similarity {
                    best_sim = Some(best_sim.map_or(v, |b| b.max(v)));
                }
            }
            _ => {}
        }
    }
    seed_labels.sort();
    seed_labels.dedup();
    profile_labels.sort();
    profile_labels.dedup();
    RetrievalDiag {
        generators: gens,
        semantic_film_local: film_local,
        semantic_profile: profile,
        seed_labels,
        profile_labels,
        best_neighbor_rank: best_rank,
        best_similarity: best_sim,
    }
}

fn content_diag(
    profile: &crate::taste::features::FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
) -> ContentDiag {
    let fit = score_family_fit_with_config(profile, candidate, semantic, &CraftConfig::fit_v1());
    ContentDiag {
        semantic_positive: fit.content_detail.semantic_positive,
        semantic_negative: fit.content_detail.semantic_negative,
        margin: fit.content_detail.semantic_margin,
        motif_affinity: fit.content_detail.motif_affinity,
        genre_affinity: fit.content_detail.genre_affinity,
        mode_affinity: fit.content_detail.mode_affinity,
        content_confidence: fit.families.content.confidence,
        content_score: fit.families.content.score,
        semantic_coverage: semantic.coverage,
    }
}

fn cluster_diag(row: &ScoredCandidate) -> ClusterDiag {
    let collection = collection_key(row);
    let director = director_key(row);
    let broad_mode = broad_mode_key(row);
    let semantic = semantic_key(row);
    let emb_cluster = crate::taste::diversify::semantic_cluster_key(row);
    let primary = collection
        .clone()
        .map(|c| format!("collection:{c}"))
        .or_else(|| emb_cluster.clone().map(|s| format!("cluster:{s}")))
        .or_else(|| semantic.clone().map(|s| format!("semantic:{s}")))
        .or_else(|| broad_mode.clone().map(|m| format!("mode:{m}")))
        .or_else(|| director.clone().map(|d| format!("director:{d}")))
        .unwrap_or_else(|| "unclustered".into());
    ClusterDiag {
        collection,
        director,
        broad_mode,
        semantic: emb_cluster.or(semantic),
        primary,
    }
}

fn clusters_overlap(a: &ClusterDiag, b: &ClusterDiag) -> bool {
    match (&a.collection, &b.collection) {
        (Some(x), Some(y)) if x == y => return true,
        _ => {}
    }
    match (&a.semantic, &b.semantic) {
        (Some(x), Some(y)) if x == y => return true,
        _ => {}
    }
    match (&a.broad_mode, &b.broad_mode) {
        (Some(x), Some(y)) if x == y => return true,
        _ => {}
    }
    match (&a.director, &b.director) {
        (Some(x), Some(y)) if x == y => return true,
        _ => {}
    }
    false
}

fn build_concentration(
    raw_new: &[ScoredCandidate],
    board: &[ScoredCandidate],
) -> Vec<ConcentrationRow> {
    let board_keys: HashSet<String> = board
        .iter()
        .map(|c| identity_key(c.candidate.tmdb_id, &c.candidate.title, c.candidate.year))
        .collect();
    let board_meta: Vec<(usize, String, f32, ClusterDiag)> = board
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                i + 1,
                c.candidate.title.clone(),
                fit_of(c),
                cluster_diag(c),
            )
        })
        .collect();
    let board_floor = board_meta.last().map(|(_, _, f, _)| *f);

    raw_new
        .iter()
        .take(25)
        .enumerate()
        .map(|(i, row)| {
            let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
            let fit = fit_of(row);
            let cluster = cluster_diag(row);
            let selected = board_keys.contains(&key);
            let final_board_rank = board
                .iter()
                .position(|b| {
                    identity_key(b.candidate.tmdb_id, &b.candidate.title, b.candidate.year) == key
                })
                .map(|p| p + 1);

            // Closest selected row that shares a D1 cluster (excluding self).
            let mut closest_dup: Option<(usize, String, f32)> = None;
            for (br, title, bfit, bcluster) in &board_meta {
                if selected && final_board_rank == Some(*br) {
                    continue;
                }
                if !clusters_overlap(&cluster, bcluster) {
                    continue;
                }
                let delta = fit - *bfit;
                let better = match &closest_dup {
                    None => true,
                    Some((_, _, d)) => delta.abs() < d.abs(),
                };
                if better {
                    closest_dup = Some((*br, title.clone(), delta));
                }
            }

            // If not selected: nearest different-cluster board member by |Δfit|
            // (the scarce-slot rival), preferring the board floor when fits are close.
            let displaced = if !selected {
                board_meta
                    .iter()
                    .filter(|(_, _, _, bcluster)| !clusters_overlap(&cluster, bcluster))
                    .min_by(|a, b| {
                        (a.2 - fit)
                            .abs()
                            .partial_cmp(&(b.2 - fit).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(br, title, bfit, _)| (format!("#{br} {title}"), *bfit - fit))
            } else {
                None
            };

            ConcentrationRow {
                raw_rank: i + 1,
                title: row.candidate.title.clone(),
                year: row.candidate.year,
                fit,
                match_percent: fit_to_match_percent(row.eligibility.predicted_fit),
                cluster,
                selected,
                final_board_rank,
                closest_selected_duplicate: closest_dup.as_ref().map(|(_, t, _)| t.clone()),
                closest_selected_duplicate_rank: closest_dup.as_ref().map(|(r, _, _)| *r),
                fit_delta_to_duplicate: closest_dup.map(|(_, _, d)| d),
                fit_delta_to_board_floor: board_floor.map(|f| f - fit),
                displaced_by: displaced.as_ref().map(|(s, _)| s.clone()),
                fit_delta_to_displacer: displaced.map(|(_, d)| d),
            }
        })
        .collect()
}

fn build_diag(
    row: &ScoredCandidate,
    candidate: &Candidate,
    profile: &crate::taste::features::FeatureProfile,
    semantic: &SemanticScore,
    content_rank: usize,
    board_rank: Option<usize>,
    pre_diversify_rank: Option<usize>,
) -> BoardCandidateDiag {
    let diversify_delta = match (board_rank, pre_diversify_rank) {
        (Some(b), Some(p)) => Some(b as i32 - p as i32),
        _ => None,
    };
    BoardCandidateDiag {
        title: row.candidate.title.clone(),
        year: row.candidate.year,
        tmdb_id: row.candidate.tmdb_id,
        match_percent: fit_to_match_percent(row.eligibility.predicted_fit),
        eligibility_state: row.eligibility.state.clone(),
        eligibility_reason: row.eligibility.primary_reason.clone(),
        raw_content_rank: content_rank,
        final_board_rank: board_rank,
        diversify_delta,
        content: content_diag(profile, candidate, semantic),
        retrieval: retrieval_diag(candidate),
        judgment: String::new(), // filled manually by user
    }
}

pub fn run_live_board_validation(
    db: &Database,
    films: &[FilmRecord],
) -> Result<LiveBoardValidationReport, String> {
    let rated = films.iter().filter(|f| f.rating.is_some()).count();
    let profile = crate::taste::feature_profile_from_films(films);
    let seen = crate::taste::retrieve::seen_keys(films);

    let stored = db
        .conn()
        .query_row("SELECT COUNT(*) FROM taste_embeddings", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as usize;
    let active_effective =
        semantic::semantic_universe_stats(db, &HashSet::new(), None).semantic_index_movies;

    let t0 = Instant::now();
    // Production path uses active-2k default when no bench override is set.
    let pool = build_retrieval_pool(db, films, &profile, &seen, false)?;
    let examined = select_fair_pool(pool.by_key.clone(), 1000);
    let retrieval_ms = t0.elapsed().as_secs_f32() * 1000.0;

    let t1 = Instant::now();
    let semantic_map = score_candidates_from_cache(db, films, &examined);
    let mut scored_pool = score_pool_with_semantic(&profile, &examined, &semantic_map, None);
    crate::taste::semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.ranked);
    crate::taste::semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.dropped_contextual);
    let score_ms = t1.elapsed().as_secs_f32() * 1000.0;

    // Map candidates by identity for provenance/content recompute.
    let mut cand_by_key: HashMap<String, Candidate> = HashMap::new();
    for c in &examined {
        cand_by_key.insert(identity_key(c.tmdb_id, &c.title, c.year), c.clone());
    }

    // Content ranks among all scored-passed rows (pre-assemble).
    let mut content_ordered = scored_pool.ranked.clone();
    content_ordered.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.candidate.title.cmp(&b.candidate.title))
    });
    let content_rank_of: HashMap<String, usize> = content_ordered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                identity_key(r.candidate.tmdb_id, &r.candidate.title, r.candidate.year),
                i + 1,
            )
        })
        .collect();

    let recommended_n = content_ordered
        .iter()
        .filter(|c| {
            !c.candidate.watchlist
                && c.eligibility.state == EligibilityState::Recommended.as_str()
        })
        .count();
    let exploratory_n = content_ordered
        .iter()
        .filter(|c| {
            !c.candidate.watchlist
                && c.eligibility.state == EligibilityState::Exploratory.as_str()
        })
        .count();
    let new_held = scored_pool
        .dropped_contextual
        .iter()
        .filter(|c| !c.candidate.watchlist)
        .count();
    let held_n = scored_pool.dropped_contextual_total;
    let watchlist_candidates = content_ordered
        .iter()
        .filter(|c| c.candidate.watchlist)
        .count()
        + scored_pool
            .dropped_contextual
            .iter()
            .filter(|c| c.candidate.watchlist)
            .count();
    let watchlist_recommended = content_ordered
        .iter()
        .filter(|c| c.candidate.watchlist && c.eligibility.state == "recommended")
        .count();
    let new_candidates = recommended_n + exploratory_n + new_held;

    // Pre-diversify New shortlist order (Recommended→Exploratory, raw Content).
    let mut pre_div: Vec<ScoredCandidate> = content_ordered
        .iter()
        .filter(|c| {
            matches!(
                c.eligibility.state.as_str(),
                "recommended" | "exploratory"
            ) && !c.candidate.watchlist
        })
        .cloned()
        .collect();
    // Mirror assemble: Recommended first, then Exploratory.
    let mut rec: Vec<_> = pre_div
        .iter()
        .filter(|c| c.eligibility.state == "recommended")
        .cloned()
        .collect();
    let mut exp: Vec<_> = pre_div
        .iter()
        .filter(|c| c.eligibility.state == "exploratory")
        .cloned()
        .collect();
    rec.extend(exp.drain(..));
    pre_div = rec;
    let pre_div_rank_of: HashMap<String, usize> = pre_div
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                identity_key(r.candidate.tmdb_id, &r.candidate.title, r.candidate.year),
                i + 1,
            )
        })
        .collect();

    let ws = workspace::assemble(&scored_pool.ranked);
    let board: Vec<ScoredCandidate> = ws.new_picks.iter().take(BOARD_DISPLAY).cloned().collect();

    let mut final_board = Vec::new();
    for (i, row) in board.iter().enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        let Some(cand) = cand_by_key.get(&key) else {
            continue;
        };
        let sem = row
            .candidate
            .tmdb_id
            .and_then(|id| semantic_map.get(&id))
            .cloned()
            .unwrap_or_default();
        let content_rank = *content_rank_of.get(&key).unwrap_or(&9999);
        let pre = pre_div_rank_of.get(&key).copied();
        final_board.push(build_diag(
            row,
            cand,
            &profile,
            &sem,
            content_rank,
            Some(i + 1),
            pre,
        ));
    }

    let mut top_eligible = Vec::new();
    for (i, row) in content_ordered.iter().take(ELIGIBLE_DISPLAY).enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        let Some(cand) = cand_by_key.get(&key) else {
            continue;
        };
        let sem = row
            .candidate
            .tmdb_id
            .and_then(|id| semantic_map.get(&id))
            .cloned()
            .unwrap_or_default();
        let board_rank = board
            .iter()
            .position(|b| {
                identity_key(b.candidate.tmdb_id, &b.candidate.title, b.candidate.year) == key
            })
            .map(|p| p + 1);
        let pre = pre_div_rank_of.get(&key).copied();
        top_eligible.push(build_diag(
            row,
            cand,
            &profile,
            &sem,
            i + 1,
            board_rank,
            pre,
        ));
    }

    // New-only Content order (what the user actually judges on New for you).
    let new_pool: Vec<ScoredCandidate> = content_ordered
        .iter()
        .filter(|c| confidence::occupies_new(c))
        .cloned()
        .collect();
    let mut top_new_eligible = Vec::new();
    for (i, row) in new_pool.iter().take(ELIGIBLE_DISPLAY).enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        let Some(cand) = cand_by_key.get(&key) else {
            continue;
        };
        let sem = row
            .candidate
            .tmdb_id
            .and_then(|id| semantic_map.get(&id))
            .cloned()
            .unwrap_or_default();
        let board_rank = board
            .iter()
            .position(|b| {
                identity_key(b.candidate.tmdb_id, &b.candidate.title, b.candidate.year) == key
            })
            .map(|p| p + 1);
        let pre = pre_div_rank_of.get(&key).copied();
        top_new_eligible.push(build_diag(
            row,
            cand,
            &profile,
            &sem,
            i + 1,
            board_rank,
            pre,
        ));
    }

    let mut watchlist_board = Vec::new();
    for (i, row) in ws.watchlist_picks.iter().take(BOARD_DISPLAY).enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        let Some(cand) = cand_by_key.get(&key) else {
            continue;
        };
        let sem = row
            .candidate
            .tmdb_id
            .and_then(|id| semantic_map.get(&id))
            .cloned()
            .unwrap_or_default();
        let content_rank = *content_rank_of.get(&key).unwrap_or(&9999);
        watchlist_board.push(build_diag(
            row,
            cand,
            &profile,
            &sem,
            content_rank,
            Some(i + 1),
            None,
        ));
    }

    // Why examined / scored rows are absent from New.
    let mut new_filter_counts: HashMap<String, usize> = HashMap::new();
    let mut audit_rows = scored_pool.ranked.clone();
    audit_rows.extend(scored_pool.dropped_contextual.clone());
    for row in &audit_rows {
        if confidence::occupies_new(row) {
            *new_filter_counts.entry("occupies_new".into()).or_insert(0) += 1;
            continue;
        }
        if row.candidate.watchlist {
            *new_filter_counts.entry("watchlist".into()).or_insert(0) += 1;
            continue;
        }
        let reason = filter_reason(row).unwrap_or_else(|| {
            if confidence::unreleased_new_row(row) {
                "unreleased".into()
            } else {
                format!("other:{}", row.eligibility.state)
            }
        });
        *new_filter_counts.entry(reason).or_insert(0) += 1;
    }
    *new_filter_counts
        .entry("new_board_len".into())
        .or_insert(0) = ws.new_picks.len();
    *new_filter_counts
        .entry("active_index_effective".into())
        .or_insert(0) = active_effective;

    let fit_first = board.first().map(|c| c.eligibility.predicted_fit);
    let fit_twelfth = board.get(11).map(|c| c.eligibility.predicted_fit);
    let fit_span = match (fit_first, fit_twelfth) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    let lanes = LanePopulations {
        new_candidates,
        new_recommended: recommended_n,
        new_exploratory: exploratory_n,
        new_held,
        watchlist_candidates,
        watchlist_recommended,
        new_board_fit_first: fit_first,
        new_board_fit_twelfth: fit_twelfth,
        new_board_fit_span: fit_span,
    };

    // Raw New Content order 1–25 vs final board — D1 concentration diagnostic.
    let concentration = build_concentration(&new_pool, &board);

    let mut notes = vec![
        "Frozen v1: quality-first G ordering, Content Fit C1/Match, Craft/Form/Continuity/Quality Fit λ=0, calibrated Match, light diversity only inside |ΔG|≤0.04.".into(),
        "Lane routing: C1 scarce admission on New-capable only; watchlist ranked as a separate surface.".into(),
        "judgment fields left empty for manual Good/Plausible/Bad/AlreadyKnownIssue labels.".into(),
        "diversifyDelta = finalBoardRank − preDiversifyContentRank (negative ⇒ promoted by D1).".into(),
        "D1.1: scarce slots choose among candidates within ε_fit of best remaining; no weak-candidate rescue beyond ε.".into(),
        format!(
            "examMode={} activeCap={} activeEffective={} storedEmbeddings={}",
            exam_mode().as_str(),
            V1_ACTIVE_SEMANTIC_CAP,
            active_effective,
            stored
        ),
        format!(
            "lanes: new={{cand={}, rec={}, exp={}, held={}}} watch={{cand={}, rec={}}} board_fit_span={:?}",
            lanes.new_candidates,
            lanes.new_recommended,
            lanes.new_exploratory,
            lanes.new_held,
            lanes.watchlist_candidates,
            lanes.watchlist_recommended,
            lanes.new_board_fit_span
        ),
    ];
    if ws.new_picks.len() < BOARD_DISPLAY {
        notes.push(format!(
            "WARN: New board only has {} titles (want {}).",
            ws.new_picks.len(),
            BOARD_DISPLAY
        ));
    }

    Ok(LiveBoardValidationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        policy: "active2k-v1".into(),
        active_retrieval_cap: V1_ACTIVE_SEMANTIC_CAP,
        stored_embeddings: stored,
        exam_mode: exam_mode().as_str().into(),
        exam_cap: 1000,
        rated_films: rated,
        retrieval_ms,
        score_ms,
        examined: examined.len(),
        eligible_n: content_ordered.len(),
        recommended_n,
        exploratory_n,
        held_n,
        lanes,
        concentration,
        final_board,
        top_eligible,
        top_new_eligible,
        watchlist_board,
        new_filter_counts,
        notes,
    })
}

pub fn write_live_board_artifact(
    runs_dir: &Path,
    report: &LiveBoardValidationReport,
) -> Result<String, String> {
    let dir = runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("live-board-v1-{stamp}.json"));
    let latest = dir.join("live-board-v1-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    std::fs::write(&latest, &body).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn print_board_for_judgment(report: &LiveBoardValidationReport) {
    eprintln!("=== Live board v1 (active-2k + lane routing) ===");
    eprintln!(
        "rated={} examined={} storedEmb={} activeCap={} ms_ret={:.0} ms_score={:.0}",
        report.rated_films,
        report.examined,
        report.stored_embeddings,
        report.active_retrieval_cap,
        report.retrieval_ms,
        report.score_ms
    );
    eprintln!(
        "NEW lane: candidates={} recommended={} exploratory={} held={}",
        report.lanes.new_candidates,
        report.lanes.new_recommended,
        report.lanes.new_exploratory,
        report.lanes.new_held
    );
    eprintln!(
        "WATCHLIST lane: candidates={} recommended={}",
        report.lanes.watchlist_candidates, report.lanes.watchlist_recommended
    );
    eprintln!(
        "board fit: #1={:?} #12={:?} span={:?}",
        report.lanes.new_board_fit_first,
        report.lanes.new_board_fit_twelfth,
        report.lanes.new_board_fit_span
    );
    eprintln!("newFilterCounts: {:?}", report.new_filter_counts);
    for n in &report.notes {
        eprintln!("note: {n}");
    }
    eprintln!("\n--- Final New board (judge each) ---");
    if report.final_board.is_empty() {
        eprintln!("(empty)");
    }
    for c in &report.final_board {
        eprintln!(
            "#{:<2} {} ({})  match={}%  state={}  contentRank={}  boardRank={}  divΔ={:?}",
            c.final_board_rank.unwrap_or(0),
            c.title,
            c.year.map(|y| y.to_string()).unwrap_or_default(),
            c.match_percent,
            c.eligibility_state,
            c.raw_content_rank,
            c.final_board_rank.unwrap_or(0),
            c.diversify_delta
        );
        eprintln!(
            "     Content: pos={:.3} neg={:.3} margin={:.3} motif={:.3} genre={:.3} mode={:.3} conf={:.3} cov={}",
            c.content.semantic_positive,
            c.content.semantic_negative,
            c.content.margin,
            c.content.motif_affinity,
            c.content.genre_affinity,
            c.content.mode_affinity,
            c.content.content_confidence,
            c.content.semantic_coverage
        );
        eprintln!(
            "     Retrieval: {:?} fl={} pr={} seeds={:?} profile={:?} nRank={:?} sim={:?}",
            c.retrieval.generators,
            c.retrieval.semantic_film_local,
            c.retrieval.semantic_profile,
            c.retrieval.seed_labels.iter().take(3).collect::<Vec<_>>(),
            c.retrieval.profile_labels.iter().take(2).collect::<Vec<_>>(),
            c.retrieval.best_neighbor_rank,
            c.retrieval.best_similarity
        );
        eprintln!("     reason={}", c.eligibility_reason);
    }
    eprintln!("\n--- Concentration: raw New ranks 1–25 vs final board ---");
    eprintln!(
        "{:<3} {:<32} {:>6} {:<28} {:>3} {:>4}  {}",
        "#", "title", "fit", "cluster", "sel", "brd", "dup / displace"
    );
    for r in &report.concentration {
        let year = r
            .year
            .map(|y| format!(" ({y})"))
            .unwrap_or_default();
        let title = format!("{}{year}", r.title);
        let title = if title.len() > 32 {
            format!("{}…", &title[..31])
        } else {
            title
        };
        let cluster = if r.cluster.primary.len() > 28 {
            format!("{}…", &r.cluster.primary[..27])
        } else {
            r.cluster.primary.clone()
        };
        let sel = if r.selected { "Y" } else { "n" };
        let brd = r
            .final_board_rank
            .map(|b| b.to_string())
            .unwrap_or_else(|| "-".into());
        let mut note = String::new();
        if let (Some(dup), Some(d)) = (
            &r.closest_selected_duplicate,
            r.fit_delta_to_duplicate,
        ) {
            note.push_str(&format!(
                "dup=#{} {} Δ={:+.4}",
                r.closest_selected_duplicate_rank.unwrap_or(0),
                dup,
                d
            ));
        }
        if let (Some(who), Some(d)) = (&r.displaced_by, r.fit_delta_to_displacer) {
            if !note.is_empty() {
                note.push_str(" | ");
            }
            note.push_str(&format!("vs {who} Δ={:+.4}", d));
        }
        if let Some(floor_d) = r.fit_delta_to_board_floor {
            if !r.selected {
                if !note.is_empty() {
                    note.push_str(" | ");
                }
                note.push_str(&format!("vs#12 Δ={:+.4}", floor_d));
            }
        }
        eprintln!(
            "{:<3} {:<32} {:>6.4} {:<28} {:>3} {:>4}  {}",
            r.raw_rank, title, r.fit, cluster, sel, brd, note
        );
    }
    eprintln!("\n--- Focus ranks 8–20 ---");
    for r in report.concentration.iter().filter(|r| (8..=20).contains(&r.raw_rank)) {
        eprintln!(
            "r#{} {} fit={:.4} selected={} board={:?} cluster={} sem={:?} mode={:?} col={:?}",
            r.raw_rank,
            r.title,
            r.fit,
            r.selected,
            r.final_board_rank,
            r.cluster.primary,
            r.cluster.semantic,
            r.cluster.broad_mode,
            r.cluster.collection
        );
        if let Some(dup) = &r.closest_selected_duplicate {
            eprintln!(
                "     closest selected dup: #{} {} fitΔ={:?}",
                r.closest_selected_duplicate_rank.unwrap_or(0),
                dup,
                r.fit_delta_to_duplicate
            );
        }
        if let Some(who) = &r.displaced_by {
            eprintln!(
                "     displaced_by: {} fitΔ={:?} vs#12={:?}",
                who, r.fit_delta_to_displacer, r.fit_delta_to_board_floor
            );
        }
    }
    // Near-equal diverse alternates under the sports/franchise cluster.
    if let Some(anchor) = report
        .concentration
        .iter()
        .find(|r| r.title.to_ascii_lowercase().contains("next karate"))
    {
        eprintln!(
            "\n--- Near-equal vs '{}' (fit={:.4}, raw#{}) ---",
            anchor.title, anchor.fit, anchor.raw_rank
        );
        let mut near: Vec<_> = report
            .concentration
            .iter()
            .filter(|r| {
                r.raw_rank != anchor.raw_rank
                    && (r.fit - anchor.fit).abs() <= 0.012
                    && !clusters_overlap(&r.cluster, &anchor.cluster)
            })
            .collect();
        near.sort_by(|a, b| {
            (a.fit - anchor.fit)
                .abs()
                .partial_cmp(&(b.fit - anchor.fit).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if near.is_empty() {
            eprintln!("(none within |Δfit|≤0.012 with different D1 cluster)");
        } else {
            for r in near {
                eprintln!(
                    "  r#{} {} fit={:.4} Δ={:+.4} selected={} cluster={}",
                    r.raw_rank,
                    r.title,
                    r.fit,
                    r.fit - anchor.fit,
                    r.selected,
                    r.cluster.primary
                );
            }
        }
    }

    eprintln!("\n--- Top New-eligible by Content (occupies_new) ---");
    for c in &report.top_new_eligible {
        let group = if c.content.semantic_coverage
            && c.content.semantic_positive >= 0.50
            && c.content.semantic_negative >= 0.50
            && c.content.margin < 0.08
        {
            "A-polar"
        } else if c.content.semantic_coverage && c.content.margin >= 0.10 {
            "B-clear"
        } else {
            "other"
        };
        eprintln!(
            "n#{:<2} {} ({}) match={}% {} board={:?} {} pos={:.3} neg={:.3} m={:.3} mode={:.2} gens={:?}",
            c.raw_content_rank,
            c.title,
            c.year.map(|y| y.to_string()).unwrap_or_default(),
            c.match_percent,
            c.eligibility_state,
            c.final_board_rank,
            group,
            c.content.semantic_positive,
            c.content.semantic_negative,
            c.content.margin,
            c.content.mode_affinity,
            c.retrieval.generators
        );
    }
    eprintln!("\n--- Watchlist board (context; not New) ---");
    for c in &report.watchlist_board {
        eprintln!(
            "w#{:<2} {} ({}) match={}% contentRank={}",
            c.final_board_rank.unwrap_or(0),
            c.title,
            c.year.map(|y| y.to_string()).unwrap_or_default(),
            c.match_percent,
            c.raw_content_rank
        );
    }
    eprintln!("\n--- Top 25 overall eligible (includes watchlist) ---");
    for c in &report.top_eligible {
        eprintln!(
            "c#{:<2} {} ({}) match={}% {} board={:?} gens={:?}",
            c.raw_content_rank,
            c.title,
            c.year.map(|y| y.to_string()).unwrap_or_default(),
            c.match_percent,
            c.eligibility_state,
            c.final_board_rank,
            c.retrieval.generators
        );
    }
}
