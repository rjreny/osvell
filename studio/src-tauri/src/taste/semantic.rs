use crate::storage::db::Database;
use crate::taste::features::{keyword_strength, Credit, Keyword, KeywordStrength};
use crate::taste::retrieve::{Candidate, FilmRecord};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const EMBEDDING_MODEL: &str = "qwen/qwen3-embedding-4b";
const EMBEDDING_ENDPOINT: &str = "https://openrouter.ai/api/v1/embeddings";
const EMBEDDING_BATCH_SIZE: usize = 32;
const EMBEDDING_TEXT_LIMIT: usize = 5000;
const TOP_HISTORY_MATCHES: usize = 4;

/// 0 = unlimited. E1 curve benches set this without changing retrieve signatures.
static SEMANTIC_INDEX_CAP: AtomicUsize = AtomicUsize::new(0);

/// Cached full semantic index (uncapped). Caps applied after load.
static INDEX_CACHE: OnceLock<Mutex<Option<Vec<IndexRow>>>> = OnceLock::new();

fn index_cache() -> &'static Mutex<Option<Vec<IndexRow>>> {
    INDEX_CACHE.get_or_init(|| Mutex::new(None))
}

/// Drop cached index after universe expansion embeds new rows.
pub fn invalidate_semantic_index_cache() {
    if let Ok(mut guard) = index_cache().lock() {
        *guard = None;
    }
}

/// Run `f` with the semantic search index truncated to top-`cap` by vote_count.
/// `None` restores the thread's previous override (does not mean "unlimited").
/// For a full-index bench cell, pass `Some(n)` with n ≥ stored embeddings.
pub fn with_semantic_index_cap<T>(cap: Option<usize>, f: impl FnOnce() -> T) -> T {
    let prev = SEMANTIC_INDEX_CAP.swap(cap.unwrap_or(0), Ordering::SeqCst);
    let out = f();
    SEMANTIC_INDEX_CAP.store(prev, Ordering::SeqCst);
    out
}

fn effective_semantic_index_cap() -> usize {
    let override_cap = SEMANTIC_INDEX_CAP.load(Ordering::SeqCst);
    if override_cap > 0 {
        override_cap
    } else {
        crate::taste::exam_policy::active_semantic_index_cap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticScore {
    pub positive_similarity: f32,
    pub negative_similarity: f32,
    pub fit: f32,
    pub coverage: bool,
    pub positive_matches: usize,
    pub negative_matches: usize,
}

impl Default for SemanticScore {
    fn default() -> Self {
        Self {
            positive_similarity: 0.0,
            negative_similarity: 0.0,
            fit: 0.5,
            coverage: false,
            positive_matches: 0,
            negative_matches: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStats {
    pub model: String,
    pub rated_items: usize,
    pub candidate_items: usize,
    pub rated_coverage: usize,
    pub candidate_coverage: usize,
    pub cache_hits: usize,
    pub remote_embeddings: usize,
    pub failed_items: usize,
    pub error: Option<String>,
    /// New-board titles only reachable via semantic generators (diagnostic).
    #[serde(default)]
    pub semantic_only_new: usize,
    /// New-board titles also reachable via related/person/collection/etc.
    #[serde(default)]
    pub also_graph_reachable_new: usize,
}

#[derive(Debug, Clone)]
struct TextItem {
    tmdb_id: i64,
    text: String,
    hash: String,
}

#[derive(Debug, Clone)]
struct RatedVector {
    vector: Vec<f32>,
    positive_weight: f32,
    negative_weight: f32,
}

/// Fetch/cache the small set of vectors needed by one taste run and score every
/// candidate against the strongest positive and negative history matches.
///
/// ponytail: this intentionally uses an in-memory O(candidates * rated-history)
/// cosine scan; the app's local catalog is small enough that a vector database
/// would add more failure modes than value.
pub fn score_candidates(
    db: &Database,
    key: &str,
    films: &[FilmRecord],
    candidates: &[Candidate],
) -> (HashMap<i64, SemanticScore>, SemanticStats) {
    let mut stats = SemanticStats {
        model: EMBEDDING_MODEL.into(),
        rated_items: films.iter().filter(|f| f.rating.is_some()).count(),
        candidate_items: candidates.iter().filter_map(|c| c.tmdb_id).count(),
        ..Default::default()
    };

    let mut history_items = Vec::new();
    let mut positive_weights = HashMap::new();
    let mut negative_weights = HashMap::new();
    let mut seen_history = HashSet::new();
    for film in films.iter().filter(|f| f.rating.is_some()) {
        let Some(id) = film.tmdb_id else { continue };
        if !seen_history.insert(id) {
            continue;
        }
        history_items.push(text_item_for_film(db, film));
        let weight = film
            .signal
            .as_ref()
            .map(|s| s.recommendation_weight.max(0.05))
            .unwrap_or(1.0);
        if film.rating.unwrap_or(3.0) >= 4.0 {
            positive_weights.insert(id, weight);
        } else if film.rating.unwrap_or(3.0) <= 2.5 {
            negative_weights.insert(id, weight);
        }
    }

    let mut candidate_items = Vec::new();
    let mut seen_candidates = HashSet::new();
    for candidate in candidates {
        let Some(id) = candidate.tmdb_id else { continue };
        if seen_candidates.insert(id) {
            candidate_items.push(text_item_for_candidate(db, candidate));
        }
    }

    let all_items = history_items
        .iter()
        .chain(candidate_items.iter())
        .cloned()
        .collect::<Vec<_>>();
    let vectors = load_or_fetch_vectors(db, key, &all_items, &mut stats);

    let mut history_vectors = HashMap::new();
    for item in &history_items {
        if let Some(vector) = vectors.get(&item.tmdb_id) {
            history_vectors.insert(
                item.tmdb_id,
                RatedVector {
                    vector: vector.clone(),
                    positive_weight: positive_weights.get(&item.tmdb_id).copied().unwrap_or(0.0),
                    negative_weight: negative_weights.get(&item.tmdb_id).copied().unwrap_or(0.0),
                },
            );
        }
    }
    stats.rated_coverage = history_vectors.len();

    let mut scores = HashMap::new();
    for item in &candidate_items {
        let Some(vector) = vectors.get(&item.tmdb_id) else {
            continue;
        };
        let score = semantic_score(vector, &history_vectors);
        scores.insert(item.tmdb_id, score);
    }
    stats.candidate_coverage = scores.len();
    (scores, stats)
}

/// Cache-only semantic scores for family-fit / ranking benchmarks.
/// Does not call OpenRouter; missing embeddings yield no entry (unknown ≠ negative).
pub fn score_candidates_from_cache(
    db: &Database,
    films: &[FilmRecord],
    candidates: &[Candidate],
) -> HashMap<i64, SemanticScore> {
    let mut positive_weights = HashMap::new();
    let mut negative_weights = HashMap::new();
    let mut history_ids = Vec::new();
    let mut seen_history = HashSet::new();
    for film in films.iter().filter(|f| f.rating.is_some()) {
        let Some(id) = film.tmdb_id else { continue };
        if !seen_history.insert(id) {
            continue;
        }
        history_ids.push(id);
        let weight = film
            .signal
            .as_ref()
            .map(|s| s.recommendation_weight.max(0.05))
            .unwrap_or(1.0);
        if film.rating.unwrap_or(3.0) >= 4.0 {
            positive_weights.insert(id, weight);
        } else if film.rating.unwrap_or(3.0) <= 2.5 {
            negative_weights.insert(id, weight);
        }
    }

    let mut history_vectors = HashMap::new();
    for id in history_ids {
        let Some(vector) = load_embedding_by_tmdb(db, id) else {
            continue;
        };
        history_vectors.insert(
            id,
            RatedVector {
                vector,
                positive_weight: positive_weights.get(&id).copied().unwrap_or(0.0),
                negative_weight: negative_weights.get(&id).copied().unwrap_or(0.0),
            },
        );
    }

    let mut scores = HashMap::new();
    let mut seen_cand = HashSet::new();
    for candidate in candidates {
        let Some(id) = candidate.tmdb_id else { continue };
        if !seen_cand.insert(id) {
            continue;
        }
        let Some(vector) = load_embedding_by_tmdb(db, id) else {
            continue;
        };
        scores.insert(id, semantic_score(&vector, &history_vectors));
    }
    scores
}

/// Load cached embeddings for scored rows and stamp D1 semantic_cluster ids.
pub fn attach_semantic_clusters_from_db(
    db: &Database,
    rows: &mut [crate::taste::score::ScoredCandidate],
) {
    use crate::taste::diversify::{assign_semantic_clusters, SEMANTIC_CLUSTER_SIM};
    let mut vectors = HashMap::new();
    for row in rows.iter() {
        let Some(id) = row.candidate.tmdb_id else {
            continue;
        };
        if vectors.contains_key(&id) {
            continue;
        }
        if let Some(v) = load_embedding_by_tmdb(db, id) {
            vectors.insert(id, v);
        }
    }
    assign_semantic_clusters(rows, &vectors, SEMANTIC_CLUSTER_SIM);
}

/// Load any cached embedding for a TMDB id (hash drift ignored — unknown if absent).
pub fn load_embedding_by_tmdb(db: &Database, tmdb_id: i64) -> Option<Vec<f32>> {
    let raw: String = db
        .conn()
        .query_row(
            "SELECT vector_json FROM taste_embeddings WHERE tmdb_id = ?1 AND model = ?2 LIMIT 1",
            params![tmdb_id, EMBEDDING_MODEL],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    parse_vector_json(&raw)
}

fn semantic_score(candidate: &[f32], history: &HashMap<i64, RatedVector>) -> SemanticScore {
    let mut positives = Vec::new();
    let mut negatives = Vec::new();
    for row in history.values() {
        let similarity = cosine(candidate, &row.vector).max(0.0);
        if row.positive_weight > 0.0 {
            positives.push((similarity, row.positive_weight));
        }
        if row.negative_weight > 0.0 {
            negatives.push((similarity, row.negative_weight));
        }
    }
    let (positive_similarity, positive_matches) = weighted_top_mean(positives);
    let (negative_similarity, negative_matches) = weighted_top_mean(negatives);
    if positive_matches == 0 {
        return SemanticScore::default();
    }
    const NEGATIVE_WEIGHT: f32 = 1.35;
    const MARGIN_STRETCH: f32 = 4.5;
    let margin =
        (positive_similarity - NEGATIVE_WEIGHT * negative_similarity).clamp(-1.5, 1.5);
    let stretched = (margin * MARGIN_STRETCH).tanh();
    SemanticScore {
        positive_similarity,
        negative_similarity,
        fit: ((stretched + 1.0) / 2.0).clamp(0.0, 1.0),
        coverage: true,
        positive_matches,
        negative_matches,
    }
}

fn weighted_top_mean(mut values: Vec<(f32, f32)>) -> (f32, usize) {
    values.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let selected = values.into_iter().take(TOP_HISTORY_MATCHES).collect::<Vec<_>>();
    let weight = selected.iter().map(|(_, w)| *w).sum::<f32>();
    if weight <= 0.0 {
        return (0.0, 0);
    }
    (
        selected.iter().map(|(similarity, w)| similarity * w).sum::<f32>() / weight,
        selected.len(),
    )
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut aa = 0.0;
    let mut bb = 0.0;
    for (left, right) in a.iter().zip(b) {
        dot += left * right;
        aa += left * left;
        bb += right * right;
    }
    let denom = aa.sqrt() * bb.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

fn load_or_fetch_vectors(
    db: &Database,
    key: &str,
    items: &[TextItem],
    stats: &mut SemanticStats,
) -> HashMap<i64, Vec<f32>> {
    let mut vectors = HashMap::new();
    let mut missing = Vec::new();
    for item in items {
        match load_cached_vector(db, item) {
            Ok(Some(vector)) => {
                vectors.insert(item.tmdb_id, vector);
                stats.cache_hits += 1;
            }
            Ok(None) => missing.push(item.clone()),
            Err(err) => {
                missing.push(item.clone());
                stats.error.get_or_insert(err);
            }
        }
    }

    for batch in missing.chunks(EMBEDDING_BATCH_SIZE) {
        let inputs = batch.iter().map(|item| item.text.clone()).collect::<Vec<_>>();
        match request_embeddings_with_retry(key, &inputs) {
            Ok(batch_vectors) if batch_vectors.len() == batch.len() => {
                for (item, vector) in batch.iter().zip(batch_vectors) {
                    if store_cached_vector(db, item, &vector).is_err() {
                        stats.error.get_or_insert("Could not cache semantic embedding".into());
                    }
                    vectors.insert(item.tmdb_id, vector);
                    stats.remote_embeddings += 1;
                }
            }
            Ok(_) => {
                stats.failed_items += batch.len();
                stats.error.get_or_insert("OpenRouter returned an incomplete embedding batch".into());
            }
            Err(err) => {
                stats.failed_items += batch.len();
                stats.error.get_or_insert(err);
            }
        }
    }
    stats.failed_items = stats.failed_items.min(items.len());
    vectors
}

fn request_embeddings_with_retry(key: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let mut last_error = None;
    for attempt in 0..3 {
        match request_embeddings(key, inputs) {
            Ok(vectors) => return Ok(vectors),
            Err(err) if attempt < 2 && embedding_error_is_retryable(&err) => {
                last_error = Some(err);
                std::thread::sleep(Duration::from_millis(500 * (attempt as u64 + 1)));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| "Embedding request failed".into()))
}

fn embedding_error_is_retryable(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("http 429")
        || lower.contains("engine_overloaded")
        || lower.contains("model busy")
        || lower.contains("http 502")
        || lower.contains("http 503")
}

fn request_embeddings(key: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::json!({
        "model": EMBEDDING_MODEL,
        "input": inputs,
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(90))
        .timeout_write(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build();
    let response = match agent
        .post(EMBEDDING_ENDPOINT)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .set("HTTP-Referer", "https://github.com/rjreny/osvell")
        .set("X-Title", "Osvell Taste Embeddings")
        .set("User-Agent", "Osvell/0.10 (local film app)")
        .send_string(&body.to_string())
    {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("OpenRouter embeddings HTTP {code}: {}", clip_error(&text)));
        }
        Err(err) => return Err(err.to_string()),
    };
    let value: Value = serde_json::from_str(&response).map_err(|e| e.to_string())?;
    if let Some(error) = value.get("error") {
        return Err(format!("OpenRouter embeddings error: {}", clip_error(&error.to_string())));
    }
    let data = value["data"]
        .as_array()
        .ok_or_else(|| "OpenRouter embeddings response had no data".to_string())?;
    let mut output = vec![None; inputs.len()];
    for row in data {
        let Some(index) = row["index"].as_u64().map(|n| n as usize) else {
            continue;
        };
        if index >= output.len() {
            continue;
        }
        let Some(vector) = row["embedding"].as_array() else {
            continue;
        };
        let parsed = vector
            .iter()
            .filter_map(|v| v.as_f64().map(|n| n as f32))
            .collect::<Vec<_>>();
        if parsed.len() == vector.len() && !parsed.is_empty() && parsed.iter().all(|v| v.is_finite()) {
            output[index] = Some(parsed);
        }
    }
    output
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "OpenRouter embeddings response omitted an item".into())
}

fn load_cached_vector(db: &Database, item: &TextItem) -> Result<Option<Vec<f32>>, String> {
    let row = db
        .conn()
        .query_row(
            "SELECT content_hash, dimension, vector_json FROM taste_embeddings WHERE tmdb_id = ?1 AND model = ?2",
            params![item.tmdb_id, EMBEDDING_MODEL],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((hash, dimension, raw)) = row else {
        return Ok(None);
    };
    if hash != item.hash {
        return Ok(None);
    }
    let vector = serde_json::from_str::<Vec<f32>>(&raw).map_err(|e| e.to_string())?;
    if dimension < 1 || vector.len() != dimension as usize || vector.iter().any(|v| !v.is_finite()) {
        return Ok(None);
    }
    Ok(Some(vector))
}

fn store_cached_vector(db: &Database, item: &TextItem, vector: &[f32]) -> Result<(), String> {
    let raw = serde_json::to_string(vector).map_err(|e| e.to_string())?;
    db.conn()
        .execute(
            r#"INSERT INTO taste_embeddings(tmdb_id, model, content_hash, dimension, vector_json, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)
               ON CONFLICT(tmdb_id, model) DO UPDATE SET
                 content_hash = excluded.content_hash,
                 dimension = excluded.dimension,
                 vector_json = excluded.vector_json,
                 updated_at = excluded.updated_at"#,
            params![
                item.tmdb_id,
                EMBEDDING_MODEL,
                item.hash,
                vector.len() as i64,
                raw,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Embed specific TMDB ids from catalog metadata (E1 universe expansion).
pub fn embed_tmdb_ids(db: &Database, key: &str, tmdb_ids: &[i64]) -> (usize, Option<String>) {
    let mut items = Vec::new();
    for &tmdb_id in tmdb_ids {
        let Ok(Some((title, year, genres_json, credits_json, keywords_json))) = db
            .conn()
            .query_row(
                "SELECT canonical_title, release_year, genres_json, credits_json, keywords_json
             FROM movies WHERE tmdb_id = ?1 LIMIT 1",
                params![tmdb_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i32>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
        else {
            continue;
        };
        let genres = parse_string_list(genres_json.as_deref());
        let credits = parse_credits_blob(credits_json.as_deref());
        let keywords = parse_keywords_blob(keywords_json.as_deref());
        items.push(text_item(
            db, tmdb_id, &title, year, &genres, &credits, &keywords,
        ));
    }
    if items.is_empty() {
        return (0, None);
    }
    let mut stats = SemanticStats::default();
    let _ = load_or_fetch_vectors(db, key, &items, &mut stats);
    (stats.remote_embeddings, stats.error)
}

fn parse_string_list(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .ok()
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| {
                    if let Some(s) = v.as_str() {
                        Some(s.to_string())
                    } else if let Some(id) = v.as_i64() {
                        tmdb_genre_name(id).map(str::to_string)
                    } else {
                        v.get("name")
                            .and_then(|n| n.as_str())
                            .map(str::to_string)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tmdb_genre_name(id: i64) -> Option<&'static str> {
    Some(match id {
        28 => "Action",
        12 => "Adventure",
        16 => "Animation",
        35 => "Comedy",
        80 => "Crime",
        99 => "Documentary",
        18 => "Drama",
        10751 => "Family",
        14 => "Fantasy",
        36 => "History",
        27 => "Horror",
        10402 => "Music",
        9648 => "Mystery",
        10749 => "Romance",
        878 => "Science Fiction",
        10770 => "TV Movie",
        53 => "Thriller",
        10752 => "War",
        37 => "Western",
        _ => return None,
    })
}

fn parse_credits_blob(raw: Option<&str>) -> Vec<Credit> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(crew) = v.get("crew").and_then(|c| c.as_array()) {
        for c in crew {
            let name = c["name"].as_str().unwrap_or("").trim();
            let job = c["job"].as_str().unwrap_or("").trim();
            if name.is_empty() || job.is_empty() {
                continue;
            }
            out.push(Credit {
                id: c["id"].as_i64(),
                name: name.into(),
                job: job.into(),
            });
        }
    }
    if let Some(cast) = v.get("cast").and_then(|c| c.as_array()) {
        for c in cast.iter().take(8) {
            let name = c["name"].as_str().unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            out.push(Credit {
                id: c["id"].as_i64(),
                name: name.into(),
                job: "Actor".into(),
            });
        }
    }
    out
}

fn parse_keywords_blob(raw: Option<&str>) -> Vec<Keyword> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<Value>>(raw)
        .ok()
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| {
                    let name = v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| v.as_str())?
                        .trim();
                    if name.is_empty() {
                        None
                    } else {
                        Some(Keyword {
                            name: name.into(),
                            id: v.get("id").and_then(|i| i.as_i64()),
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text_item_for_film(db: &Database, film: &FilmRecord) -> TextItem {
    text_item(
        db,
        film.tmdb_id.unwrap_or(0),
        &film.title,
        film.year,
        &film.genres,
        &film.credits,
        &film.keywords,
    )
}

fn text_item_for_candidate(db: &Database, candidate: &Candidate) -> TextItem {
    text_item(
        db,
        candidate.tmdb_id.unwrap_or(0),
        &candidate.title,
        candidate.year,
        &candidate.genres,
        &candidate.credits,
        &candidate.keywords,
    )
}

fn text_item(
    db: &Database,
    tmdb_id: i64,
    title: &str,
    year: Option<i32>,
    genres: &[String],
    credits: &[Credit],
    keywords: &[Keyword],
) -> TextItem {
    let (overview, tagline) = db
        .conn()
        .query_row(
            "SELECT overview, tagline FROM movies WHERE tmdb_id = ?1 LIMIT 1",
            params![tmdb_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or((None, None));
    let text = canonical_text(title, year, overview.as_deref(), tagline.as_deref(), genres, credits, keywords);
    TextItem {
        tmdb_id,
        hash: text_hash(&text),
        text,
    }
}

pub fn canonical_text(
    title: &str,
    year: Option<i32>,
    overview: Option<&str>,
    tagline: Option<&str>,
    genres: &[String],
    credits: &[Credit],
    keywords: &[Keyword],
) -> String {
    let mut lines = vec![format!(
        "Title: {}{}",
        title.trim(),
        year.map(|y| format!(" ({y})")).unwrap_or_default()
    )];
    if let Some(tagline) = tagline.filter(|v| !v.trim().is_empty()) {
        lines.push(format!("Tagline: {}", clean_text(tagline)));
    }
    if let Some(overview) = overview.filter(|v| !v.trim().is_empty()) {
        lines.push(format!("Overview: {}", clean_text(overview)));
    }
    let genres = genres
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .take(8)
        .collect::<Vec<_>>();
    if !genres.is_empty() {
        lines.push(format!("Genres: {}", genres.join(", ")));
    }
    for (label, names) in credit_sections(credits) {
        if !names.is_empty() {
            lines.push(format!("{label}: {}", names.join(", ")));
        }
    }
    let keywords = keywords
        .iter()
        .filter(|k| matches!(keyword_strength(&k.name), KeywordStrength::Strong | KeywordStrength::Thematic))
        .map(|k| k.name.trim())
        .filter(|k| !k.is_empty())
        .take(12)
        .collect::<Vec<_>>();
    if !keywords.is_empty() {
        lines.push(format!("Themes: {}", keywords.join(", ")));
    }
    truncate_chars(&lines.join("\n"), EMBEDDING_TEXT_LIMIT)
}

fn credit_sections(credits: &[Credit]) -> Vec<(&'static str, Vec<String>)> {
    let mut sections: Vec<(&'static str, Vec<String>, usize)> = vec![
        ("Director", Vec::new(), 2),
        ("Writer", Vec::new(), 2),
        ("Cinematographer", Vec::new(), 2),
        ("Composer", Vec::new(), 2),
        ("Actors", Vec::new(), 5),
    ];
    let mut seen = HashSet::new();
    for credit in credits {
        let lower = credit.job.to_ascii_lowercase();
        let section = if lower == "director" {
            Some(0)
        } else if lower.contains("writer") || lower.contains("screenplay") {
            Some(1)
        } else if lower.contains("cinematograph") || lower.contains("photograph") {
            Some(2)
        } else if lower.contains("composer") || lower.contains("music") {
            Some(3)
        } else if lower == "actor" {
            Some(4)
        } else {
            None
        };
        let Some(index) = section else { continue };
        let name = credit.name.trim();
        if name.is_empty() || !seen.insert((index, name.to_ascii_lowercase())) {
            continue;
        }
        if sections[index].1.len() < sections[index].2 {
            sections[index].1.push(name.to_string());
        }
    }
    sections
        .into_iter()
        .map(|(label, names, _)| (label, names))
        .collect()
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    value.chars().take(limit).collect::<String>()
}

fn text_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn clip_error(value: &str) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flat, 300)
}

// --- Milestone A3: semantic discovery (retrieval, not Taste fit) ---

const PROFILE_CLUSTER_COUNT: usize = 4;
const PROFILE_NEIGHBORS: usize = 75;
/// Conservative retrieval prune only — not Milestone B scoring.
const NEG_DOMINANCE_MARGIN: f32 = 0.12;
const NEG_DOMINANCE_FLOOR: f32 = 0.65;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SemanticUniverseStats {
    pub semantic_index_movies: usize,
    pub semantic_index_unseen_movies: usize,
    pub heldout_positive_index_coverage: Option<f32>,
    pub local_seeds_used: usize,
    pub profile_queries_used: usize,
    pub local_candidates: usize,
    pub profile_candidates: usize,
    pub pruned_by_negative: usize,
}

#[derive(Debug, Clone)]
struct IndexRow {
    tmdb_id: i64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    runtime: Option<i32>,
    vote_count: i64,
    vector: Vec<f32>,
}

fn parse_vector_json(raw: &str) -> Option<Vec<f32>> {
    serde_json::from_str::<Vec<f32>>(raw).ok().filter(|v| !v.is_empty())
}

/// Load the embedding search universe: all cached vectors joined to catalog metadata.
fn load_semantic_index(db: &Database) -> Result<Vec<IndexRow>, String> {
    {
        let guard = index_cache().lock().map_err(|e| e.to_string())?;
        if let Some(cached) = guard.as_ref() {
            let mut out = cached.clone();
            let cap = effective_semantic_index_cap();
            if out.len() > cap {
                out.truncate(cap);
            }
            return Ok(out);
        }
    }

    let loaded = load_semantic_index_from_db(db)?;
    if let Ok(mut guard) = index_cache().lock() {
        *guard = Some(loaded.clone());
    }
    let mut out = loaded;
    let cap = effective_semantic_index_cap();
    if out.len() > cap {
        out.truncate(cap);
    }
    Ok(out)
}

fn load_semantic_index_from_db(db: &Database) -> Result<Vec<IndexRow>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT e.tmdb_id, e.vector_json,
                   COALESCE(m.canonical_title, 'tmdb:' || e.tmdb_id),
                   m.release_year,
                   COALESCE(m.poster_override_url, m.poster_path),
                   m.runtime,
                   COALESCE(m.vote_count, 0),
                   COALESCE(m.tmdb_media_type, 'movie'),
                   m.overview
            FROM taste_embeddings e
            LEFT JOIN movies m ON m.tmdb_id = e.tmdb_id
            WHERE e.model = ?1
            "#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![EMBEDDING_MODEL], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i32>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i32>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (tmdb_id, vector_json, title, year, poster, runtime, vote_count, media_type, overview) =
            row;
        let Some(vector) = parse_vector_json(&vector_json) else {
            continue;
        };
        if title.trim().is_empty() {
            continue;
        }
        if media_type != "movie" {
            continue;
        }
        if let Some(ov) = &overview {
            if ov.trim().len() < 40 {
                continue;
            }
        }
        if let Some(rt) = runtime {
            if rt > 0 && rt < 40 {
                continue;
            }
        }
        out.push(IndexRow {
            tmdb_id,
            title,
            year,
            poster: crate::letterboxd::posters::poster_url(poster),
            runtime,
            vote_count,
            vector,
        });
    }
    out.sort_by(|a, b| {
        b.vote_count
            .cmp(&a.vote_count)
            .then_with(|| a.tmdb_id.cmp(&b.tmdb_id))
    });
    Ok(out)
}

pub fn semantic_universe_stats(
    db: &Database,
    seen: &HashSet<String>,
    held_out_positive_ids: Option<&HashSet<i64>>,
) -> SemanticUniverseStats {
    let Ok(index) = load_semantic_index(db) else {
        return SemanticUniverseStats::default();
    };
    let unseen = index
        .iter()
        .filter(|row| !seen.contains(&format!("tmdb:{}", row.tmdb_id)))
        .count();
    let coverage = held_out_positive_ids.map(|ids| {
        if ids.is_empty() {
            return 0.0;
        }
        let hit = ids
            .iter()
            .filter(|id| index.iter().any(|row| row.tmdb_id == **id))
            .count();
        hit as f32 / ids.len() as f32
    });
    SemanticUniverseStats {
        semantic_index_movies: index.len(),
        semantic_index_unseen_movies: unseen,
        heldout_positive_index_coverage: coverage,
        ..Default::default()
    }
}

fn film_vector_map(index: &[IndexRow]) -> HashMap<i64, Vec<f32>> {
    index
        .iter()
        .map(|row| (row.tmdb_id, row.vector.clone()))
        .collect()
}

fn positive_training<'a>(films: &'a [FilmRecord]) -> Vec<&'a FilmRecord> {
    let mut out: Vec<&FilmRecord> = films
        .iter()
        .filter(|f| f.tmdb_id.is_some())
        .filter(|f| f.rating.map(|r| r >= 4.0).unwrap_or(false))
        .collect();
    out.sort_by(|a, b| {
        let ar = a.rating.unwrap_or(0.0);
        let br = b.rating.unwrap_or(0.0);
        br.partial_cmp(&ar)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let aw = a
                    .signal
                    .as_ref()
                    .map(|s| s.recommendation_weight)
                    .unwrap_or(0.0);
                let bw = b
                    .signal
                    .as_ref()
                    .map(|s| s.recommendation_weight)
                    .unwrap_or(0.0);
                bw.partial_cmp(&aw)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.tmdb_id.cmp(&b.tmdb_id))
    });
    out
}

fn negative_vectors(films: &[FilmRecord], vectors: &HashMap<i64, Vec<f32>>) -> Vec<Vec<f32>> {
    films
        .iter()
        .filter(|f| f.rating.map(|r| r <= 2.5).unwrap_or(false))
        .filter_map(|f| f.tmdb_id)
        .filter_map(|id| vectors.get(&id).cloned())
        .collect()
}

fn select_diverse_local_seeds<'a>(
    positives: &[&'a FilmRecord],
    vectors: &HashMap<i64, Vec<f32>>,
) -> Vec<&'a FilmRecord> {
    use crate::taste::exam_policy::{local_seed_cap, seed_diversity_max_sim};
    let seed_cap = local_seed_cap();
    let max_sim = seed_diversity_max_sim();
    let mut selected: Vec<&FilmRecord> = Vec::new();
    for film in positives {
        let Some(id) = film.tmdb_id else { continue };
        let Some(vec) = vectors.get(&id) else { continue };
        let too_close = selected.iter().any(|picked| {
            let pid = picked.tmdb_id.unwrap();
            cosine(vec, &vectors[&pid]) >= max_sim
        });
        if too_close {
            continue;
        }
        selected.push(film);
        if selected.len() >= seed_cap {
            break;
        }
    }
    // If diversity filtered too aggressively, backfill by rating order.
    // Consolidation mode: never backfill past diversity — overlapping seeds
    // are exactly what crowds the examination cut under a wide index.
    if !crate::taste::exam_policy::semantic_consolidate()
        && selected.len() < seed_cap.min(positives.len())
    {
        for film in positives {
            if selected.iter().any(|s| s.tmdb_id == film.tmdb_id) {
                continue;
            }
            if film.tmdb_id.and_then(|id| vectors.get(&id)).is_none() {
                continue;
            }
            selected.push(film);
            if selected.len() >= seed_cap {
                break;
            }
        }
    }
    selected
}

fn centroid(vectors: &[&[f32]]) -> Option<Vec<f32>> {
    let first = vectors.first()?;
    let dim = first.len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return None;
    }
    let mut acc = vec![0.0f32; dim];
    for v in vectors {
        for (i, x) in v.iter().enumerate() {
            acc[i] += *x;
        }
    }
    let n = vectors.len() as f32;
    for x in &mut acc {
        *x /= n;
    }
    Some(acc)
}

/// Deterministic farthest-point clusters over positive embeddings.
fn positive_cluster_centroids(
    positives: &[&FilmRecord],
    vectors: &HashMap<i64, Vec<f32>>,
    k: usize,
) -> Vec<(String, Vec<f32>)> {
    let mut points: Vec<(i64, &Vec<f32>)> = positives
        .iter()
        .filter_map(|f| {
            let id = f.tmdb_id?;
            let v = vectors.get(&id)?;
            Some((id, v))
        })
        .collect();
    points.sort_by_key(|(id, _)| *id);
    if points.is_empty() {
        return Vec::new();
    }
    let k = k.min(points.len()).max(1);
    let mut centers: Vec<usize> = Vec::new();
    centers.push(0);
    while centers.len() < k {
        let mut best_i = 0usize;
        let mut best_dist = -1.0f32;
        for (i, (_, vec)) in points.iter().enumerate() {
            if centers.contains(&i) {
                continue;
            }
            let min_sim = centers
                .iter()
                .map(|&c| cosine(vec, points[c].1))
                .fold(f32::INFINITY, f32::min);
            let dist = 1.0 - min_sim;
            if dist > best_dist {
                best_dist = dist;
                best_i = i;
            }
        }
        centers.push(best_i);
    }
    let mut buckets: Vec<Vec<&[f32]>> = vec![Vec::new(); k];
    for (_, vec) in &points {
        let mut best = 0usize;
        let mut best_sim = -1.0f32;
        for (ci, &cidx) in centers.iter().enumerate() {
            let sim = cosine(vec, points[cidx].1);
            if sim > best_sim {
                best_sim = sim;
                best = ci;
            }
        }
        buckets[best].push(vec.as_slice());
    }
    let mut out = Vec::new();
    if let Some(global) = centroid(&points.iter().map(|(_, v)| v.as_slice()).collect::<Vec<_>>()) {
        out.push(("global".into(), global));
    }
    for (i, bucket) in buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        if let Some(c) = centroid(bucket) {
            out.push((format!("cluster_{}", i + 1), c));
        }
    }
    out
}

fn max_sim_to(vector: &[f32], refs: &[Vec<f32>]) -> f32 {
    refs.iter()
        .map(|r| cosine(vector, r).max(0.0))
        .fold(0.0f32, f32::max)
}

fn pruned_by_negative(vector: &[f32], pos_refs: &[Vec<f32>], neg_refs: &[Vec<f32>]) -> bool {
    if neg_refs.is_empty() {
        return false;
    }
    let n = max_sim_to(vector, neg_refs);
    let p = max_sim_to(vector, pos_refs);
    n >= NEG_DOMINANCE_FLOOR && n > p + NEG_DOMINANCE_MARGIN
}

fn nearest_neighbors<'a>(
    query: &[f32],
    index: &'a [IndexRow],
    seen: &HashSet<String>,
    take: usize,
    pos_refs: &[Vec<f32>],
    neg_refs: &[Vec<f32>],
) -> (Vec<(&'a IndexRow, f32, u32)>, usize) {
    let mut scored: Vec<(&IndexRow, f32)> = index
        .iter()
        .filter(|row| !seen.contains(&format!("tmdb:{}", row.tmdb_id)))
        .map(|row| (row, cosine(query, &row.vector).max(0.0)))
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.tmdb_id.cmp(&b.0.tmdb_id))
    });
    let mut pruned = 0usize;
    let mut out = Vec::new();
    for (rank, (row, sim)) in scored.into_iter().enumerate() {
        if pruned_by_negative(&row.vector, pos_refs, neg_refs) {
            pruned += 1;
            continue;
        }
        out.push((row, sim, (rank + 1) as u32));
        if out.len() >= take {
            break;
        }
    }
    (out, pruned)
}

fn candidate_from_index(
    row: &IndexRow,
    kind: crate::taste::retrieve::RetrievalKind,
    label: String,
    seed_tmdb_id: Option<i64>,
    seed_rating: Option<f32>,
    similarity: f32,
    neighbor_rank: u32,
) -> Candidate {
    use crate::taste::retrieve::{MediaKind, RetrievalSource};
    Candidate {
        tmdb_id: Some(row.tmdb_id),
        title: row.title.clone(),
        year: row.year,
        poster: row.poster.clone(),
        genres: Vec::new(),
        credits: Vec::new(),
        keywords: Vec::new(),
        runtime: row.runtime,
        vote_count: None,
        watchlist: false,
        sources: vec![RetrievalSource::new(kind, label, seed_tmdb_id)
            .with_rating(seed_rating)
            .with_similarity(similarity, neighbor_rank)],
        friend_affinity: 0.0,
        tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
    }
}

/// Discover candidates from the embedding index. Provenance only — not Taste fit.
pub fn retrieve_semantic_candidates(
    db: &Database,
    films: &[FilmRecord],
    seen: &HashSet<String>,
) -> Vec<Candidate> {
    let Ok(index) = load_semantic_index(db) else {
        return Vec::new();
    };
    if index.is_empty() {
        return Vec::new();
    }
    let vectors = film_vector_map(&index);
    let positives = positive_training(films);
    if positives.is_empty() {
        return Vec::new();
    }
    let pos_refs: Vec<Vec<f32>> = positives
        .iter()
        .filter_map(|f| f.tmdb_id.and_then(|id| vectors.get(&id).cloned()))
        .collect();
    let neg_refs = negative_vectors(films, &vectors);
    let mut by_key: HashMap<String, Candidate> = HashMap::new();
    let mut pruned_total = 0usize;

    // --- SemanticFilmLocal ---
    let seeds = select_diverse_local_seeds(&positives, &vectors);
    for seed in &seeds {
        let Some(sid) = seed.tmdb_id else { continue };
        let Some(q) = vectors.get(&sid) else { continue };
        let (neighbors, pruned) = nearest_neighbors(
            q,
            &index,
            seen,
            crate::taste::exam_policy::local_neighbors(),
            &pos_refs,
            &neg_refs,
        );
        pruned_total += pruned;
        for (row, sim, rank) in neighbors {
            let key = format!("tmdb:{}", row.tmdb_id);
            let incoming = candidate_from_index(
                row,
                crate::taste::retrieve::RetrievalKind::SemanticFilmLocal,
                format!("semantically near {}", seed.title),
                Some(sid),
                seed.rating,
                sim,
                rank,
            );
            merge_semantic_candidate(&mut by_key, key, incoming);
        }
    }

    // --- SemanticProfile: global + cluster centroids ---
    let queries = positive_cluster_centroids(&positives, &vectors, PROFILE_CLUSTER_COUNT);
    for (query_name, q) in &queries {
        let (neighbors, pruned) =
            nearest_neighbors(q, &index, seen, PROFILE_NEIGHBORS, &pos_refs, &neg_refs);
        pruned_total += pruned;
        for (row, sim, rank) in neighbors {
            let key = format!("tmdb:{}", row.tmdb_id);
            let incoming = candidate_from_index(
                row,
                crate::taste::retrieve::RetrievalKind::SemanticProfile,
                format!("taste profile · {query_name}"),
                None,
                None,
                sim,
                rank,
            );
            merge_semantic_candidate(&mut by_key, key, incoming);
        }
    }

    let _ = pruned_total; // counted in diagnostics via universe stats when needed
    let mut out: Vec<Candidate> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        let as_ = a
            .sources
            .iter()
            .filter_map(|s| s.similarity)
            .fold(0.0f32, f32::max);
        let bs = b
            .sources
            .iter()
            .filter_map(|s| s.similarity)
            .fold(0.0f32, f32::max);
        bs.partial_cmp(&as_)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tmdb_id.cmp(&b.tmdb_id))
    });
    out
}

fn merge_semantic_candidate(map: &mut HashMap<String, Candidate>, key: String, incoming: Candidate) {
    map.entry(key)
        .and_modify(|existing| {
            for src in incoming.sources.clone() {
                if !existing.sources.iter().any(|s| {
                    s.kind == src.kind
                        && s.seed_tmdb_id == src.seed_tmdb_id
                        && s.label == src.label
                }) {
                    existing.sources.push(src);
                }
            }
            if existing.poster.is_none() {
                existing.poster = incoming.poster.clone();
            }
            if existing.runtime.is_none() {
                existing.runtime = incoming.runtime;
            }
        })
        .or_insert(incoming);
}

/// Full A3 retrieval diagnostics for one fold's training set.
pub fn retrieve_semantic_with_stats(
    db: &Database,
    films: &[FilmRecord],
    seen: &HashSet<String>,
) -> (Vec<Candidate>, SemanticUniverseStats) {
    let Ok(index) = load_semantic_index(db) else {
        return (Vec::new(), SemanticUniverseStats::default());
    };
    let unseen = index
        .iter()
        .filter(|row| !seen.contains(&format!("tmdb:{}", row.tmdb_id)))
        .count();
    let vectors = film_vector_map(&index);
    let positives = positive_training(films);
    let seeds = select_diverse_local_seeds(&positives, &vectors);
    let queries = positive_cluster_centroids(&positives, &vectors, PROFILE_CLUSTER_COUNT);
    let candidates = retrieve_semantic_candidates(db, films, seen);
    let local_candidates = candidates
        .iter()
        .filter(|c| {
            c.sources.iter().any(|s| {
                s.kind == crate::taste::retrieve::RetrievalKind::SemanticFilmLocal
            })
        })
        .count();
    let profile_candidates = candidates
        .iter()
        .filter(|c| {
            c.sources
                .iter()
                .any(|s| s.kind == crate::taste::retrieve::RetrievalKind::SemanticProfile)
        })
        .count();
    (
        candidates,
        SemanticUniverseStats {
            semantic_index_movies: index.len(),
            semantic_index_unseen_movies: unseen,
            heldout_positive_index_coverage: None,
            local_seeds_used: seeds.len(),
            profile_queries_used: queries.len(),
            local_candidates,
            profile_candidates,
            pruned_by_negative: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(job: &str, name: &str) -> Credit {
        Credit {
            id: None,
            name: name.into(),
            job: job.into(),
        }
    }

    #[test]
    fn canonical_text_keeps_meaningful_metadata_and_omits_noisy_keywords() {
        let text = canonical_text(
            "Example",
            Some(2024),
            Some("A detective follows a nonlinear timeline."),
            Some("A short tagline"),
            &["Drama".into()],
            &[credit("Director", "A Director"), credit("Actor", "A Star")],
            &[
                Keyword { id: None, name: "nonlinear".into() },
                Keyword { id: None, name: "woman director".into() },
            ],
        );
        assert!(text.contains("Overview:"));
        assert!(text.contains("Director: A Director"));
        assert!(text.contains("Themes: nonlinear"));
        assert!(!text.contains("woman director"));
    }

    #[test]
    fn cosine_handles_dimension_mismatch() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_dominance_prunes_only_extreme_cases() {
        let pos = vec![vec![1.0, 0.0]];
        let neg = vec![vec![0.0, 1.0]];
        // Orthogonal to both — keep
        assert!(!pruned_by_negative(&[0.7, 0.7], &pos, &neg));
        // Strongly negative, weak positive — prune
        assert!(pruned_by_negative(&[0.1, 0.95], &pos, &neg));
    }

    #[test]
    fn cluster_centroids_include_global_and_clusters() {
        let mut vectors = HashMap::new();
        vectors.insert(1, vec![1.0, 0.0]);
        vectors.insert(2, vec![0.95, 0.05]);
        vectors.insert(3, vec![0.0, 1.0]);
        vectors.insert(4, vec![0.05, 0.95]);
        let films: Vec<FilmRecord> = (1..=4)
            .map(|id| FilmRecord {
                key: format!("tmdb:{id}"),
                title: format!("F{id}"),
                year: Some(2000),
                tmdb_id: Some(id),
                rating: Some(5.0),
                liked: true,
                watched: true,
                watchlist: false,
                viewings: 1,
                last_date: None,
                genres: vec![],
                credits: vec![],
                keywords: vec![],
                recommendations: vec![],
                similar: vec![],
                collection_name: None,
                collection: vec![],
                runtime: Some(100),
                poster: None,
                vote_count: Some(10),
                review: None,
                signal: None,
                age_years: None,
            })
            .collect();
        let refs: Vec<&FilmRecord> = films.iter().collect();
        let qs = positive_cluster_centroids(&refs, &vectors, 2);
        assert!(qs.iter().any(|(n, _)| n == "global"));
        assert!(qs.len() >= 2);
    }

    #[test]
    fn positive_and_negative_matches_form_a_margin() {
        let mut history = HashMap::new();
        history.insert(
            1,
            RatedVector { vector: vec![1.0, 0.0], positive_weight: 1.0, negative_weight: 0.0 },
        );
        history.insert(
            2,
            RatedVector { vector: vec![0.0, 1.0], positive_weight: 0.0, negative_weight: 1.0 },
        );
        let score = semantic_score(&[1.0, 0.0], &history);
        assert!(score.fit > 0.75);
        assert_eq!(score.positive_matches, 1);
        assert_eq!(score.negative_matches, 1);
    }

    fn vector_with_cosine(similarity: f32) -> Vec<f32> {
        vec![similarity, (1.0 - similarity * similarity).sqrt()]
    }

    #[test]
    fn positive_margin_is_stretched_away_from_neutral() {
        let history = HashMap::from([
            (
                1,
                RatedVector {
                    vector: vector_with_cosine(0.80),
                    positive_weight: 1.0,
                    negative_weight: 0.0,
                },
            ),
            (
                2,
                RatedVector {
                    vector: vector_with_cosine(0.50),
                    positive_weight: 0.0,
                    negative_weight: 1.0,
                },
            ),
        ]);

        let score = semantic_score(&[1.0, 0.0], &history);

        assert!(score.fit > 0.70, "fit should stretch above neutral: {}", score.fit);
    }

    #[test]
    fn clear_positive_alignment_stays_high() {
        let history = HashMap::from([
            (
                1,
                RatedVector {
                    vector: vec![1.0, 0.0],
                    positive_weight: 1.0,
                    negative_weight: 0.0,
                },
            ),
            (
                2,
                RatedVector {
                    vector: vec![0.0, 1.0],
                    positive_weight: 0.0,
                    negative_weight: 1.0,
                },
            ),
        ]);

        let score = semantic_score(&[1.0, 0.0], &history);

        assert!(score.fit > 0.95, "clear positive fit should stay high: {}", score.fit);
    }

    #[test]
    fn near_equal_negative_alignment_is_below_neutral() {
        let history = HashMap::from([
            (
                1,
                RatedVector {
                    vector: vector_with_cosine(0.80),
                    positive_weight: 1.0,
                    negative_weight: 0.0,
                },
            ),
            (
                2,
                RatedVector {
                    vector: vector_with_cosine(0.78),
                    positive_weight: 0.0,
                    negative_weight: 1.0,
                },
            ),
        ]);

        let score = semantic_score(&[1.0, 0.0], &history);

        assert!(score.fit < 0.55, "negative similarity should suppress fit: {}", score.fit);
    }
}
