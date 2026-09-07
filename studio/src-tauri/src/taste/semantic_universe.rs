//! E1 — Expand the semantic discovery universe (catalog + embeddings only).
//!
//! Does not change Content Fit, eligibility, Match, D1 diversify, or semantic
//! query algorithms. Popularity / vote_count is an indexing gate only.

use crate::catalog::tmdb;
use crate::storage::db::Database;
use crate::taste::semantic::{self, EMBEDDING_MODEL};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Production v1 target after the recall curve saturates (bench may choose lower).
pub const DEFAULT_UNIVERSE_TARGET: usize = 10_000;

/// Tier 1: well-reviewed features.
pub const TIER1_MIN_VOTES: i64 = 500;
/// Tier 2: still enough signal for useful embeddings.
pub const TIER2_MIN_VOTES: i64 = 50;
pub const MIN_OVERVIEW_CHARS: usize = 40;
pub const MIN_RUNTIME_MINUTES: i32 = 40;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniverseExpandReport {
    pub target: usize,
    pub catalog_before: usize,
    pub catalog_after: usize,
    pub embeddings_before: usize,
    pub embeddings_after: usize,
    pub harvested: usize,
    pub embedded: usize,
    pub skipped_tv: usize,
    pub skipped_short: usize,
    pub skipped_thin_overview: usize,
    pub errors: Vec<String>,
    pub index_bytes_est: u64,
}

#[derive(Debug, Clone)]
struct DiscoverHit {
    tmdb_id: i64,
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    poster_path: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i64>,
    runtime: Option<i32>,
    genres_json: String,
}

fn count_embeddings(db: &Database) -> usize {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM taste_embeddings WHERE model = ?1",
            params![EMBEDDING_MODEL],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
}

fn count_movies(db: &Database) -> usize {
    db.conn()
        .query_row("SELECT COUNT(*) FROM movies WHERE tmdb_id IS NOT NULL", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
}

fn estimate_index_bytes(db: &Database) -> u64 {
    db.conn()
        .query_row(
            "SELECT COALESCE(SUM(LENGTH(vector_json)), 0) FROM taste_embeddings WHERE model = ?1",
            params![EMBEDDING_MODEL],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64
}

/// SQL predicate shared by harvest eligibility and index load.
pub fn movie_index_eligible_sql(alias: &str) -> String {
    format!(
        "{alias}.tmdb_id IS NOT NULL
         AND COALESCE({alias}.tmdb_media_type, 'movie') = 'movie'
         AND {alias}.canonical_title IS NOT NULL
         AND LENGTH(TRIM({alias}.canonical_title)) > 0
         AND {alias}.overview IS NOT NULL
         AND LENGTH(TRIM({alias}.overview)) >= {MIN_OVERVIEW_CHARS}
         AND ({alias}.runtime IS NULL OR {alias}.runtime >= {MIN_RUNTIME_MINUTES})
         AND COALESCE({alias}.vote_count, 0) >= {TIER2_MIN_VOTES}"
    )
}

fn movie_exists(db: &Database, tmdb_id: i64) -> bool {
    db.conn()
        .query_row(
            "SELECT 1 FROM movies WHERE tmdb_id = ?1 AND COALESCE(tmdb_media_type, 'movie') = 'movie' LIMIT 1",
            params![tmdb_id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
}

fn upsert_discover_stub(db: &Database, hit: &DiscoverHit) -> Result<bool, String> {
    if movie_exists(db, hit.tmdb_id) {
        // Refresh vote_count / overview when thin rows already exist.
        db.conn()
            .execute(
                r#"UPDATE movies SET
                    overview = COALESCE(NULLIF(TRIM(overview), ''), ?2),
                    vote_average = COALESCE(?3, vote_average),
                    vote_count = CASE
                      WHEN COALESCE(vote_count, 0) > COALESCE(?4, 0) THEN vote_count
                      ELSE ?4 END,
                    poster_path = COALESCE(NULLIF(poster_path, ''), ?5),
                    runtime = COALESCE(runtime, ?6),
                    genres_json = CASE
                      WHEN genres_json IS NULL OR genres_json = '[]' THEN ?7
                      ELSE genres_json END
                 WHERE tmdb_id = ?1 AND COALESCE(tmdb_media_type, 'movie') = 'movie'"#,
                params![
                    hit.tmdb_id,
                    hit.overview,
                    hit.vote_average,
                    hit.vote_count,
                    hit.poster_path,
                    hit.runtime,
                    hit.genres_json,
                ],
            )
            .map_err(|e| e.to_string())?;
        return Ok(false);
    }
    let id = Uuid::new_v4().to_string();
    db.conn()
        .execute(
            r#"INSERT INTO movies(
                id, canonical_title, release_year, tmdb_id, tmdb_media_type, poster_path,
                overview, runtime, vote_average, vote_count, genres_json, enriched_at
             ) VALUES (?1, ?2, ?3, ?4, 'movie', ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))"#,
            params![
                id,
                hit.title,
                hit.year,
                hit.tmdb_id,
                hit.poster_path,
                hit.overview,
                hit.runtime,
                hit.vote_average,
                hit.vote_count,
                hit.genres_json,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(true)
}

fn parse_discover_page(body: &str) -> Result<(usize, Vec<DiscoverHit>), String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("discover JSON: {e}"))?;
    let total_pages = v["total_pages"].as_u64().unwrap_or(1) as usize;
    let mut hits = Vec::new();
    let Some(results) = v["results"].as_array() else {
        return Ok((total_pages, hits));
    };
    for row in results {
        let Some(tmdb_id) = row["id"].as_i64() else {
            continue;
        };
        let title = row["title"]
            .as_str()
            .or_else(|| row["original_title"].as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let overview = row["overview"].as_str().map(|s| s.trim().to_string());
        let year = row["release_date"]
            .as_str()
            .and_then(|d| d.get(0..4))
            .and_then(|y| y.parse().ok());
        let genres = row["genre_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|g| g.as_i64())
                    .filter_map(|id| match id {
                        28 => Some("Action"),
                        12 => Some("Adventure"),
                        16 => Some("Animation"),
                        35 => Some("Comedy"),
                        80 => Some("Crime"),
                        99 => Some("Documentary"),
                        18 => Some("Drama"),
                        10751 => Some("Family"),
                        14 => Some("Fantasy"),
                        36 => Some("History"),
                        27 => Some("Horror"),
                        10402 => Some("Music"),
                        9648 => Some("Mystery"),
                        10749 => Some("Romance"),
                        878 => Some("Science Fiction"),
                        53 => Some("Thriller"),
                        10752 => Some("War"),
                        37 => Some("Western"),
                        _ => None,
                    })
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let genres_json = serde_json::to_string(&genres).unwrap_or_else(|_| "[]".into());
        hits.push(DiscoverHit {
            tmdb_id,
            title,
            year,
            overview,
            poster_path: row["poster_path"].as_str().map(str::to_string),
            vote_average: row["vote_average"].as_f64(),
            vote_count: row["vote_count"].as_i64(),
            runtime: None,
            genres_json,
        });
    }
    Ok((total_pages, hits))
}

fn hit_passes_filters(hit: &DiscoverHit, report: &mut UniverseExpandReport) -> bool {
    if hit
        .overview
        .as_ref()
        .map(|o| o.trim().len() < MIN_OVERVIEW_CHARS)
        .unwrap_or(true)
    {
        report.skipped_thin_overview += 1;
        return false;
    }
    if let Some(rt) = hit.runtime {
        if rt > 0 && rt < MIN_RUNTIME_MINUTES {
            report.skipped_short += 1;
            return false;
        }
    }
    if hit.vote_count.unwrap_or(0) < TIER2_MIN_VOTES {
        return false;
    }
    true
}

/// Pull quality-controlled feature films from TMDB discover into `movies`.
pub fn harvest_discover_universe(
    db: &Database,
    target_catalog: usize,
    min_votes: i64,
) -> Result<UniverseExpandReport, String> {
    let api_key = tmdb::get_api_key()?
        .ok_or_else(|| "Add a TMDB key in Settings to expand the semantic universe".to_string())?;
    let mut report = UniverseExpandReport {
        target: target_catalog,
        catalog_before: count_movies(db),
        catalog_after: 0,
        embeddings_before: count_embeddings(db),
        embeddings_after: 0,
        harvested: 0,
        embedded: 0,
        skipped_tv: 0,
        skipped_short: 0,
        skipped_thin_overview: 0,
        errors: Vec::new(),
        index_bytes_est: 0,
    };

    let mut page = 1usize;
    let mut total_pages = 500usize;
    while count_eligible_unembedded(db) + count_embeddings(db) < target_catalog && page <= total_pages {
        let path = format!(
            "/discover/movie?include_adult=false&include_video=false&language=en-US&page={page}&sort_by=vote_count.desc&vote_count.gte={min_votes}&with_runtime.gte={MIN_RUNTIME_MINUTES}"
        );
        let body = match tmdb::tmdb_get_public(&api_key, &path) {
            Ok(b) => b,
            Err(err) => {
                report.errors.push(err);
                break;
            }
        };
        let (pages, hits) = parse_discover_page(&body)?;
        total_pages = pages.max(1).min(500);
        if hits.is_empty() {
            break;
        }
        for hit in hits {
            if !hit_passes_filters(&hit, &mut report) {
                continue;
            }
            match upsert_discover_stub(db, &hit) {
                Ok(true) => report.harvested += 1,
                Ok(false) => {}
                Err(err) => {
                    if report.errors.len() < 12 {
                        report.errors.push(err);
                    }
                }
            }
        }
        page += 1;
        // Soft TMDB politeness.
        std::thread::sleep(std::time::Duration::from_millis(40));
    }

    report.catalog_after = count_movies(db);
    report.embeddings_after = count_embeddings(db);
    report.index_bytes_est = estimate_index_bytes(db);
    Ok(report)
}

fn count_eligible_unembedded(db: &Database) -> usize {
    let pred = movie_index_eligible_sql("m");
    let sql = format!(
        "SELECT COUNT(*) FROM movies m
         WHERE {pred}
           AND NOT EXISTS (
             SELECT 1 FROM taste_embeddings e
             WHERE e.tmdb_id = m.tmdb_id AND e.model = ?1
           )"
    );
    db.conn()
        .query_row(&sql, params![EMBEDDING_MODEL], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as usize
}

/// Embed eligible catalog movies missing vectors, ordered by vote_count desc.
pub fn embed_missing_universe(
    db: &Database,
    openrouter_key: &str,
    limit: usize,
) -> Result<UniverseExpandReport, String> {
    let mut report = UniverseExpandReport {
        target: limit,
        catalog_before: count_movies(db),
        catalog_after: count_movies(db),
        embeddings_before: count_embeddings(db),
        embeddings_after: 0,
        harvested: 0,
        embedded: 0,
        skipped_tv: 0,
        skipped_short: 0,
        skipped_thin_overview: 0,
        errors: Vec::new(),
        index_bytes_est: 0,
    };
    let pred = movie_index_eligible_sql("m");
    let sql = format!(
        "SELECT m.tmdb_id FROM movies m
         WHERE {pred}
           AND NOT EXISTS (
             SELECT 1 FROM taste_embeddings e
             WHERE e.tmdb_id = m.tmdb_id AND e.model = ?1
           )
         ORDER BY COALESCE(m.vote_count, 0) DESC, m.tmdb_id ASC
         LIMIT ?2"
    );
    let mut stmt = db.conn().prepare(&sql).map_err(|e| e.to_string())?;
    let ids: Vec<i64> = stmt
        .query_map(params![EMBEDDING_MODEL, limit as i64], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    if ids.is_empty() {
        report.embeddings_after = count_embeddings(db);
        report.index_bytes_est = estimate_index_bytes(db);
        return Ok(report);
    }
    let (n, err) = semantic::embed_tmdb_ids(db, openrouter_key, &ids);
    semantic::invalidate_semantic_index_cache();
    report.embedded = n;
    if let Some(e) = err {
        report.errors.push(e);
    }
    report.embeddings_after = count_embeddings(db);
    report.index_bytes_est = estimate_index_bytes(db);
    Ok(report)
}

/// Harvest then embed until the searchable index reaches `target` (or resources exhaust).
pub fn ensure_semantic_universe(
    db: &Database,
    openrouter_key: &str,
    target: usize,
) -> Result<UniverseExpandReport, String> {
    let before = count_embeddings(db);
    let mut report = UniverseExpandReport {
        target,
        catalog_before: count_movies(db),
        catalog_after: 0,
        embeddings_before: before,
        embeddings_after: before,
        harvested: 0,
        embedded: 0,
        skipped_tv: 0,
        skipped_short: 0,
        skipped_thin_overview: 0,
        errors: Vec::new(),
        index_bytes_est: 0,
    };

    if before < target {
        // Prefer filling from local catalog first.
        let local = embed_missing_universe(db, openrouter_key, target.saturating_sub(before))?;
        report.embedded += local.embedded;
        report.errors.extend(local.errors);
    }

    let mut emb = count_embeddings(db);
    if emb < target {
        let need = target - emb;
        // Over-harvest a bit: not every discover hit embeds cleanly.
        let harvest_target = emb + need + need / 5 + 200;
        let harvested = harvest_discover_universe(db, harvest_target, TIER2_MIN_VOTES)?;
        report.harvested += harvested.harvested;
        report.skipped_thin_overview += harvested.skipped_thin_overview;
        report.skipped_short += harvested.skipped_short;
        report.errors.extend(harvested.errors);

        let more = embed_missing_universe(db, openrouter_key, target.saturating_sub(count_embeddings(db)))?;
        report.embedded += more.embedded;
        report.errors.extend(more.errors);
        emb = count_embeddings(db);
    }

    report.catalog_after = count_movies(db);
    report.embeddings_after = emb;
    report.index_bytes_est = estimate_index_bytes(db);
    Ok(report)
}

/// Diagnostic: New-board titles only reachable via semantic generators.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SemanticNoveltyStats {
    pub new_count: usize,
    pub also_graph_reachable: usize,
    pub semantic_only: usize,
}

pub fn semantic_novelty_for_candidates(
    candidates: &[crate::taste::score::ScoredCandidate],
) -> SemanticNoveltyStats {
    use crate::taste::retrieve::RetrievalKind;
    let mut stats = SemanticNoveltyStats {
        new_count: candidates.len(),
        ..Default::default()
    };
    for c in candidates {
        let has_semantic = c.candidate.sources.iter().any(|s| {
            matches!(
                s.kind,
                RetrievalKind::SemanticFilmLocal | RetrievalKind::SemanticProfile
            )
        });
        let has_graph = c.candidate.sources.iter().any(|s| {
            matches!(
                s.kind,
                RetrievalKind::Related
                    | RetrievalKind::Filmography
                    | RetrievalKind::Collection
                    | RetrievalKind::Friend
                    | RetrievalKind::Watchlist
            )
        });
        if has_semantic && !has_graph {
            stats.semantic_only += 1;
        } else if has_graph {
            stats.also_graph_reachable += 1;
        } else if has_semantic {
            stats.semantic_only += 1;
        }
    }
    stats
}

/// Same diagnostic on raw retrieval candidates (no scoring wrapper).
pub fn semantic_novelty_for_retrieval(
    candidates: &[crate::taste::retrieve::Candidate],
) -> SemanticNoveltyStats {
    use crate::taste::retrieve::RetrievalKind;
    let mut stats = SemanticNoveltyStats {
        new_count: candidates.len(),
        ..Default::default()
    };
    for c in candidates {
        let has_semantic = c.sources.iter().any(|s| {
            matches!(
                s.kind,
                RetrievalKind::SemanticFilmLocal | RetrievalKind::SemanticProfile
            )
        });
        let has_graph = c.sources.iter().any(|s| {
            matches!(
                s.kind,
                RetrievalKind::Related
                    | RetrievalKind::Filmography
                    | RetrievalKind::Collection
                    | RetrievalKind::Friend
                    | RetrievalKind::Watchlist
            )
        });
        if has_semantic && !has_graph {
            stats.semantic_only += 1;
        } else if has_graph {
            stats.also_graph_reachable += 1;
        } else if has_semantic {
            stats.semantic_only += 1;
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_sql_mentions_vote_and_overview_floors() {
        let sql = movie_index_eligible_sql("m");
        assert!(sql.contains("vote_count"));
        assert!(sql.contains("overview"));
        assert!(sql.contains("movie"));
    }

    #[test]
    fn novelty_counts_semantic_only() {
        use crate::taste::explain::EligibilityTrace;
        use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
        use crate::taste::score::{CandidateScore, CandidateView, ScoredCandidate};

        let row = |kinds: &[RetrievalKind]| ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(1),
                title: "X".into(),
                year: Some(2000),
                poster: None,
                watchlist: false,
                sources: kinds
                    .iter()
                    .map(|k| RetrievalSource {
                        kind: *k,
                        label: "x".into(),
                        seed_tmdb_id: None,
                        seed_rating: None,
                        similarity: None,
                        neighbor_rank: None,
                    })
                    .collect(),
                directors: vec![],
                genres: vec![],
                modes: vec![],
                media_kind: MediaKind::Movie,
                runtime: Some(100),
                vote_count: Some(100),
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
                total: 0.0,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: vec![],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec![],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![],
            hidden_features: vec![],
            eligibility: EligibilityTrace::default(),
            quality_prior: 0.0,
            has_quality_prior: false,
        };
        let only = semantic_novelty_for_candidates(&[row(&[RetrievalKind::SemanticProfile])]);
        assert_eq!(only.semantic_only, 1);
        let both = semantic_novelty_for_candidates(&[row(&[
            RetrievalKind::SemanticProfile,
            RetrievalKind::Related,
        ])]);
        assert_eq!(both.also_graph_reachable, 1);
        assert_eq!(both.semantic_only, 0);
    }
}
