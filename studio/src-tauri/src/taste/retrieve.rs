use crate::catalog::tmdb;
use crate::letterboxd::posters::poster_url;
use crate::letterboxd::rss::parse_activity_payload;
use crate::models::LibraryItem;
use crate::storage::db::Database;
use crate::taste::features::{family_for_job, Credit, FeatureFamily, FeatureProfile, Keyword};
use crate::taste::preference::{
    interaction_signal, rating_profile, years_since, InteractionSignal, RatingProfile,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct FilmRecord {
    pub key: String,
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub rating: Option<f32>,
    pub liked: bool,
    pub watched: bool,
    pub watchlist: bool,
    pub viewings: u32,
    pub last_date: Option<String>,
    pub genres: Vec<String>,
    pub credits: Vec<Credit>,
    pub keywords: Vec<Keyword>,
    pub recommendations: Vec<LibraryItem>,
    pub similar: Vec<LibraryItem>,
    pub collection_name: Option<String>,
    pub collection: Vec<LibraryItem>,
    pub runtime: Option<i32>,
    pub poster: Option<String>,
    pub vote_count: Option<i64>,
    /// Optional personal Letterboxd review, distinct from TMDB review data.
    pub review: Option<String>,
    pub signal: Option<InteractionSignal>,
    pub age_years: Option<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Movie,
    TvSeries,
    TvEpisode,
    TvSpecial,
    Short,
    Other,
    Ambiguous,
}

impl MediaKind {
    pub fn is_movie(self) -> bool {
        matches!(self, MediaKind::Movie)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RetrievalKind {
    Related,
    RelatedRecommendations,
    RelatedSimilar,
    Filmography,
    Collection,
    /// Semantic neighbors of one strongly liked film (film-local).
    SemanticFilmLocal,
    /// Semantic neighbors of the aggregate positive taste representation.
    SemanticProfile,
    Friend,
    Watchlist,
    Exploration,
    Discovery,
}

/// Coarse generator family for examination priority / diagnostics.
/// Multiple edges inside one family are not independent taste votes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratorFamily {
    Related,
    Filmography,
    Collection,
    SemanticFilmLocal,
    SemanticProfile,
    Friend,
    Watchlist,
    Discovery,
    Exploration,
}

impl RetrievalKind {
    pub fn generator_family(self) -> GeneratorFamily {
        match self {
            Self::Related | Self::RelatedRecommendations | Self::RelatedSimilar => {
                GeneratorFamily::Related
            }
            Self::Filmography => GeneratorFamily::Filmography,
            Self::Collection => GeneratorFamily::Collection,
            Self::SemanticFilmLocal => GeneratorFamily::SemanticFilmLocal,
            Self::SemanticProfile => GeneratorFamily::SemanticProfile,
            Self::Friend => GeneratorFamily::Friend,
            Self::Watchlist => GeneratorFamily::Watchlist,
            Self::Discovery => GeneratorFamily::Discovery,
            Self::Exploration => GeneratorFamily::Exploration,
        }
    }
}

impl RetrievalKind {
    pub fn is_related(self) -> bool {
        matches!(
            self,
            Self::Related | Self::RelatedRecommendations | Self::RelatedSimilar
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalSource {
    pub kind: RetrievalKind,
    pub label: String,
    pub seed_tmdb_id: Option<i64>,
    #[serde(default)]
    pub seed_rating: Option<f32>,
    /// Retrieval provenance only (e.g. embedding cosine). Not Taste fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neighbor_rank: Option<u32>,
}

impl RetrievalSource {
    pub fn new(kind: RetrievalKind, label: impl Into<String>, seed_tmdb_id: Option<i64>) -> Self {
        Self {
            kind,
            label: label.into(),
            seed_tmdb_id,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }
    }

    pub fn with_rating(mut self, rating: Option<f32>) -> Self {
        self.seed_rating = rating;
        self
    }

    pub fn with_similarity(mut self, similarity: f32, neighbor_rank: u32) -> Self {
        self.similarity = Some(similarity);
        self.neighbor_rank = Some(neighbor_rank);
        self
    }
}

impl Default for RetrievalSource {
    fn default() -> Self {
        Self::new(RetrievalKind::Related, "", None)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedCoverage {
    pub eligible_seeds: usize,
    pub seeds_with_usable_related: usize,
    pub seeds_refreshed: usize,
    pub seeds_with_catalog: usize,
    pub candidates_with_catalog: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalResult {
    pub candidates: Vec<Candidate>,
    pub coverage: SeedCoverage,
}

const POOL_CAP: usize = 1000;
/// Soft per-family floor inside the examination/hydration tranche so one
/// high-volume generator (usually Related) cannot consume the entire budget.
const FAMILY_EXAM_BUDGET: usize = 180;
const PER_SEED_GUARANTEE: usize = 2;
const PER_PERSON_GUARANTEE: usize = 2;
const PER_COLLECTION_GUARANTEE: usize = 2;
/// Take the full stored TMDB recommendation page (see RELATED_LIST_STORE).
const RECS_PER_SEED: usize = crate::catalog::tmdb::RELATED_LIST_STORE;
const SIMILAR_PER_SEED: usize = 12;
const SIMILAR_SEED_CAP: usize = 60;
const FILMOGRAPHY_PER_PERSON: usize = 12;
const ACTOR_FILMOGRAPHY_CAP: usize = 4;
const COMPOSER_FILMOGRAPHY_CAP: usize = 4;
const COLLECTION_PER_SEED: usize = 16;
pub const SEED_HYDRATE_CAP: usize = 80;
/// Practical uncapped oracle for Milestone A2 (still sorted; not truncated).
pub const POOL_CAP_ORACLE: usize = usize::MAX / 4;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    pub genres: Vec<String>,
    pub credits: Vec<Credit>,
    pub keywords: Vec<Keyword>,
    pub runtime: Option<i32>,
    pub vote_count: Option<i64>,
    pub watchlist: bool,
    pub sources: Vec<RetrievalSource>,
    pub friend_affinity: f32,
    pub tmdb_related: f32,
    pub media_kind: MediaKind,
}

pub fn identity_key(tmdb_id: Option<i64>, title: &str, year: Option<i32>) -> String {
    if let Some(id) = tmdb_id {
        format!("tmdb:{id}")
    } else {
        format!("{}|{}", title.trim().to_lowercase(), year.unwrap_or(0))
    }
}

pub fn seen_keys(films: &[FilmRecord]) -> HashSet<String> {
    films
        .iter()
        .filter(|f| f.watched || f.rating.is_some())
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect()
}

pub fn load_films(db: &Database) -> Result<Vec<FilmRecord>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT
              COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')),
              COALESCE(m.canonical_title, json_extract(smr.raw_identity, '$.title'), smr.normalized_title),
              COALESCE(m.release_year, smr.release_year),
              m.tmdb_id,
              ums.current_rating,
              COALESCE(ums.liked, 0),
              COALESCE(ums.watched, 0),
              COALESCE(ums.watchlist, smr.on_watchlist, 0),
              (SELECT COUNT(*)
               FROM viewings v
               LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
               WHERE v.source_movie_record_id = smr.id
                 AND COALESCE(vp.counted, 1) = 1),
              COALESCE(ums.last_watched_at,
                (SELECT COALESCE(v.occurred_at, v.observed_at)
                 FROM viewings v
                 LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
                 WHERE v.source_movie_record_id = smr.id
                   AND COALESCE(vp.counted, 1) = 1
                 ORDER BY COALESCE(v.occurred_at, v.observed_at) DESC LIMIT 1),
                (SELECT COALESCE(re.occurred_at, re.observed_at) FROM rating_events re
                 WHERE re.source_movie_record_id = smr.id
                 ORDER BY COALESCE(re.occurred_at, re.observed_at) DESC LIMIT 1)
              ),
              m.genres_json,
              m.credits_json,
              m.cast_json,
              m.crew_json,
              m.keywords_json,
              m.similar_json,
              m.collection_name,
              m.collection_json,
              m.runtime,
              COALESCE(m.poster_override_url, m.poster_path, smr.cached_poster_url),
              m.vote_count,
              smr.raw_identity
            FROM source_movie_records smr
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            LEFT JOIN movies m ON m.id = ml.movie_id
            LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
            WHERE ums.current_rating IS NOT NULL
               OR ums.watched = 1
               OR ums.watchlist = 1
               OR smr.on_watchlist = 1
               OR json_extract(smr.raw_identity, '$.review') IS NOT NULL
            "#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let credits_json: Option<String> = row.get(11)?;
            let cast_json: Option<String> = row.get(12)?;
            let crew_json: Option<String> = row.get(13)?;
            let (recommendations, similar) = parse_related_lists(row.get::<_, Option<String>>(15)?);
            let collection_name = row
                .get::<_, Option<String>>(16)?
                .filter(|s| !s.trim().is_empty());
            let collection = parse_item_list_raw(row.get::<_, Option<String>>(17)?);
            let review = row
                .get::<_, Option<String>>(21)?
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|identity| {
                    identity
                        .get("review")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
                .filter(|review| !review.trim().is_empty());
            Ok(FilmRecord {
                key: row.get(0)?,
                title: display_title(&row.get::<_, String>(1)?),
                year: row.get(2)?,
                tmdb_id: row.get(3)?,
                rating: row.get::<_, Option<f64>>(4)?.map(|r| r as f32),
                liked: row.get::<_, i32>(5)? == 1,
                watched: row.get::<_, i32>(6)? == 1,
                watchlist: row.get::<_, i32>(7)? == 1,
                viewings: row.get::<_, i64>(8)? as u32,
                last_date: row.get(9)?,
                genres: json_vec(row.get::<_, Option<String>>(10)?),
                credits: parse_credits(
                    credits_json.as_deref(),
                    cast_json.as_deref(),
                    crew_json.as_deref(),
                ),
                keywords: parse_keywords(row.get::<_, Option<String>>(14)?),
                recommendations,
                similar,
                collection_name,
                collection,
                runtime: row.get(18)?,
                poster: poster_url(row.get(19)?),
                vote_count: row.get(20)?,
                review,
                signal: None,
                age_years: None,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut films = consolidate_films(rows.filter_map(|r| r.ok()).collect());
    let now = chrono::Utc::now();
    for film in &mut films {
        if let Some(date) = film.last_date.as_deref() {
            film.age_years = years_since(date, now);
        }
    }
    Ok(films)
}

fn consolidate_films(films: Vec<FilmRecord>) -> Vec<FilmRecord> {
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut consolidated: Vec<FilmRecord> = Vec::new();

    for film in films {
        if let Some(&position) = positions.get(&film.key) {
            let current = &mut consolidated[position];
            let is_newer = film.last_date > current.last_date;
            current.watched |= film.watched;
            current.watchlist |= film.watchlist;
            current.liked |= film.liked;
            current.viewings += film.viewings;
            if is_newer {
                current.last_date = film.last_date;
                if film.rating.is_some() {
                    current.rating = film.rating;
                }
            } else if current.rating.is_none() {
                current.rating = film.rating;
            }
            if current.review.is_none() {
                current.review = film.review;
            }
        } else {
            positions.insert(film.key.clone(), consolidated.len());
            consolidated.push(film);
        }
    }
    consolidated
}

pub fn attach_signals(films: &mut [FilmRecord]) {
    let ratings: Vec<f32> = films.iter().filter_map(|f| f.rating).collect();
    let profile = rating_profile(&ratings);
    for film in films {
        if let (Some(rating), Some(profile)) = (film.rating, profile.as_ref()) {
            film.signal = Some(interaction_signal(
                rating,
                profile,
                film.age_years,
                film.viewings.max(1),
                film.liked,
            ));
        }
    }
}

fn needs_related_hydrate(film: &FilmRecord) -> bool {
    if film.tmdb_id.is_none() || !eligible_positive_like(film) {
        return false;
    }
    if !has_usable_related(film) {
        return true;
    }
    // Legacy catalog writes stopped at 12. Refresh those so retrieval can use
    // the deeper RELATED_LIST_STORE neighbors.
    let legacy = crate::catalog::tmdb::RELATED_LIST_LEGACY_CAP;
    film.recommendations.len() == legacy || film.similar.len() == legacy
}

pub fn enrich_eligible_seeds(
    db: &Database,
    films: &mut [FilmRecord],
    cap: usize,
    force: bool,
) -> usize {
    let cap = if cap == 0 { SEED_HYDRATE_CAP } else { cap };
    let mut idxs: Vec<usize> = films
        .iter()
        .enumerate()
        .filter(|(_, f)| eligible_positive_like(f) && f.tmdb_id.is_some())
        .filter(|(_, f)| {
            force
                || needs_related_hydrate(f)
                || needs_catalog_hydrate(f)
                || needs_collection_enrich(db, f)
        })
        .map(|(i, _)| i)
        .collect();
    idxs.sort_by(|&a, &b| {
        let missing = |f: &FilmRecord| if needs_related_hydrate(f) { 0 } else { 1 };
        missing(&films[a])
            .cmp(&missing(&films[b]))
            .then_with(|| {
                seed_priority(&films[b])
                    .partial_cmp(&seed_priority(&films[a]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| films[a].tmdb_id.cmp(&films[b].tmdb_id))
            .then_with(|| films[a].title.cmp(&films[b].title))
    });
    let mut n = 0;
    for i in idxs.into_iter().take(cap) {
        let Some(tid) = films[i].tmdb_id else {
            continue;
        };
        if tmdb::refresh_movie_catalog(db, tid, force).is_ok() {
            n += 1;
            let _ = reload_catalog_fields(db, &mut films[i]);
        }
    }
    n
}

fn needs_catalog_hydrate(film: &FilmRecord) -> bool {
    if film.tmdb_id.is_none() || film.rating.is_none() {
        return false;
    }
    let has_crew_ids = film
        .credits
        .iter()
        .any(|c| c.id.is_some() && c.job != "Actor");
    !has_crew_ids || film.keywords.is_empty()
}

/// Seeds with unknown collection metadata (never written) should be enriched
/// before collection retrieval. Empty `[]` after enrich means "no collection".
fn needs_collection_enrich(db: &Database, film: &FilmRecord) -> bool {
    eligible_positive_like(film)
        && film.collection.is_empty()
        && film.collection_name.is_none()
        && film
            .tmdb_id
            .map(|id| collection_json_unknown(db, id))
            .unwrap_or(false)
}

fn collection_json_unknown(db: &Database, tmdb_id: i64) -> bool {
    db.conn()
        .query_row(
            "SELECT CASE
                WHEN collection_json IS NULL THEN 1
                ELSE 0
             END
             FROM movies WHERE tmdb_id = ?1 LIMIT 1",
            params![tmdb_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .map(|v| v == 1)
        // No movies row yet — treat as unknown so refresh can create it.
        .unwrap_or(true)
}

pub fn enrich_rated_library(
    db: &Database,
    films: &mut [FilmRecord],
    cap: usize,
    force: bool,
) -> usize {
    let mut idxs: Vec<usize> = films
        .iter()
        .enumerate()
        .filter(|(_, f)| force || needs_catalog_hydrate(f))
        .map(|(i, _)| i)
        .collect();
    idxs.sort_by(|&a, &b| {
        let score = |f: &FilmRecord| {
            let abs = f
                .rating
                .map(crate::taste::preference::absolute_preference)
                .unwrap_or(0.0)
                .abs();
            abs * crate::taste::preference::recency_weight(f.age_years)
        };
        score(&films[b])
            .partial_cmp(&score(&films[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| films[a].tmdb_id.cmp(&films[b].tmdb_id))
            .then_with(|| films[a].title.cmp(&films[b].title))
    });
    let mut n = 0;
    for i in idxs.into_iter().take(cap) {
        let Some(tid) = films[i].tmdb_id else {
            continue;
        };
        if tmdb::refresh_movie_catalog(db, tid, force).is_ok() {
            n += 1;
            let _ = reload_catalog_fields(db, &mut films[i]);
        }
    }
    n
}

fn reload_catalog_fields(db: &Database, film: &mut FilmRecord) -> Result<(), String> {
    let Some(tid) = film.tmdb_id else {
        return Ok(());
    };
    let row = db.conn().query_row(
        "SELECT genres_json, credits_json, cast_json, crew_json, keywords_json, similar_json,
                collection_name, collection_json, runtime, vote_count, poster_path
         FROM movies WHERE tmdb_id = ?1 LIMIT 1",
        params![tid],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i32>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        },
    );
    let Ok((
        genres,
        credits,
        cast,
        crew,
        keywords,
        similar,
        collection_name,
        collection_json,
        runtime,
        vote_count,
        poster,
    )) = row
    else {
        return Ok(());
    };
    film.genres = json_vec(genres);
    film.credits = parse_credits(credits.as_deref(), cast.as_deref(), crew.as_deref());
    film.keywords = parse_keywords(keywords);
    let (recommendations, similar_list) = parse_related_lists(similar);
    film.recommendations = recommendations;
    film.similar = similar_list;
    film.collection_name = collection_name.filter(|s| !s.trim().is_empty());
    film.collection = parse_item_list_raw(collection_json);
    if film.runtime.is_none() {
        film.runtime = runtime;
    }
    if film.vote_count.is_none() {
        film.vote_count = vote_count;
    }
    if film.poster.is_none() {
        film.poster = poster_url(poster);
    }
    Ok(())
}

pub fn user_profile(films: &[FilmRecord]) -> Option<RatingProfile> {
    let ratings: Vec<f32> = films.iter().filter_map(|f| f.rating).collect();
    rating_profile(&ratings)
}

pub fn retrieve(
    db: &Database,
    films: &[FilmRecord],
    profile: &FeatureProfile,
    seen: &HashSet<String>,
    force_refresh: bool,
) -> Result<Vec<Candidate>, String> {
    Ok(retrieve_with_coverage(db, films, profile, seen, force_refresh)?.candidates)
}

pub fn retrieve_with_coverage(
    db: &Database,
    films: &[FilmRecord],
    profile: &FeatureProfile,
    seen: &HashSet<String>,
    force_refresh: bool,
) -> Result<RetrievalResult, String> {
    retrieve_with_pool_cap(db, films, profile, seen, force_refresh, POOL_CAP)
}

/// Same generators as production retrieval; only the examination/pool cap changes.
pub fn retrieve_with_pool_cap(
    db: &Database,
    films: &[FilmRecord],
    profile: &FeatureProfile,
    seen: &HashSet<String>,
    force_refresh: bool,
    pool_cap: usize,
) -> Result<RetrievalResult, String> {
    let pool = build_retrieval_pool(db, films, profile, seen, force_refresh)?;
    let candidates_with_catalog = pool
        .by_key
        .values()
        .filter(|c| candidate_has_catalog(c))
        .count();
    let mut coverage = pool.coverage;
    coverage.candidates_with_catalog = candidates_with_catalog;
    let out = select_fair_pool(pool.by_key, pool_cap);
    let mut out = out;
    hydrate_local_metadata(db, &mut out)?;
    Ok(RetrievalResult {
        candidates: out,
        coverage,
    })
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalPool {
    pub by_key: HashMap<String, Candidate>,
    pub coverage: SeedCoverage,
}

/// Build the full deduped candidate map before examination-cap selection.
pub fn build_retrieval_pool(
    db: &Database,
    films: &[FilmRecord],
    profile: &FeatureProfile,
    seen: &HashSet<String>,
    force_refresh: bool,
) -> Result<RetrievalPool, String> {
    let mut by_key: HashMap<String, Candidate> = HashMap::new();
    let mut seeds: Vec<&FilmRecord> = films.iter().filter(|f| eligible_positive_like(f)).collect();
    seeds.sort_by(|a, b| {
        seed_priority(b)
            .partial_cmp(&seed_priority(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.tmdb_id
                    .unwrap_or(i64::MAX)
                    .cmp(&b.tmdb_id.unwrap_or(i64::MAX))
            })
            .then_with(|| a.title.cmp(&b.title))
    });

    let eligible_seeds = seeds.len();
    let seeds_with_usable_related = seeds.iter().filter(|s| has_usable_related(s)).count();

    for seed in &seeds {
        for item in seed.recommendations.iter().take(RECS_PER_SEED) {
            push_related(
                &mut by_key,
                seen,
                seed,
                item,
                RetrievalKind::RelatedRecommendations,
                format!("recommended from {}", seed.title),
            );
        }
    }
    for seed in seeds.iter().take(SIMILAR_SEED_CAP) {
        let allow_similar = seed_allows_similar(seed, profile);
        if !allow_similar {
            continue;
        }
        for item in seed.similar.iter().take(SIMILAR_PER_SEED) {
            push_related(
                &mut by_key,
                seen,
                seed,
                item,
                RetrievalKind::RelatedSimilar,
                format!("similar to {}", seed.title),
            );
        }
    }

    for seed in seeds.iter().filter(|s| !s.collection.is_empty()) {
        let label = seed
            .collection_name
            .as_deref()
            .unwrap_or("collection");
        for item in seed.collection.iter().take(COLLECTION_PER_SEED) {
            push_related(
                &mut by_key,
                seen,
                seed,
                item,
                RetrievalKind::Collection,
                format!("same collection as {} ({label})", seed.title),
            );
        }
    }

    let people: Vec<_> = profile
        .affinities
        .iter()
        .filter(|a| filmography_person_allowed(a))
        .collect();
    for person in people {
        let Some(pid) = person.key.id else { continue };
        let credits =
            tmdb::person_movie_credits_with_force(db, pid, force_refresh).unwrap_or_default();
        let take = filmography_credit_cap(person.key.family);
        for credit in credits
            .into_iter()
            .filter(|c| keep_person_credit(&c.job, person.key.family))
            .take(take)
        {
            let key = identity_key(Some(credit.tmdb_id), &credit.title, credit.year);
            if seen.contains(&key) {
                continue;
            }
            upsert_candidate(
                &mut by_key,
                Candidate {
                    tmdb_id: Some(credit.tmdb_id),
                    title: credit.title,
                    year: credit.year,
                    poster: None,
                    genres: Vec::new(),
                    credits: vec![Credit {
                        id: Some(pid),
                        name: person.key.name.clone(),
                        job: credit.job,
                    }],
                    keywords: Vec::new(),
                    runtime: None,
                    vote_count: None,
                    watchlist: false,
                    sources: vec![RetrievalSource::new(
                        RetrievalKind::Filmography,
                        person.key.name.clone(),
                        None,
                    )],
                    friend_affinity: 0.0,
                    tmdb_related: 0.0,
                    media_kind: MediaKind::Movie,
                },
            );
        }
    }

    for film in films.iter().filter(|f| f.watchlist && !f.watched) {
        let key = identity_key(film.tmdb_id, &film.title, film.year);
        if seen.contains(&key) {
            continue;
        }
        upsert_candidate(
            &mut by_key,
            Candidate {
                tmdb_id: film.tmdb_id,
                title: film.title.clone(),
                year: film.year,
                poster: film.poster.clone(),
                genres: film.genres.clone(),
                credits: film.credits.clone(),
                keywords: film.keywords.clone(),
                runtime: film.runtime,
                vote_count: film.vote_count,
                watchlist: true,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Watchlist,
                    label: "watchlist".into(),
                    seed_tmdb_id: None,
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                friend_affinity: 0.0,
                tmdb_related: 0.0,
                media_kind: MediaKind::Movie,
            },
        );
    }

    let friend_hits = friend_candidates(db, films, seen)?;
    for c in friend_hits {
        upsert_candidate(&mut by_key, c);
    }

    let semantic_hits =
        crate::taste::semantic::retrieve_semantic_candidates(db, films, seen);
    for c in semantic_hits {
        upsert_candidate(&mut by_key, c);
    }

    let seeds_with_catalog = seeds.iter().filter(|s| film_has_catalog(s)).count();
    Ok(RetrievalPool {
        by_key,
        coverage: SeedCoverage {
            eligible_seeds,
            seeds_with_usable_related,
            seeds_refreshed: 0,
            seeds_with_catalog,
            candidates_with_catalog: 0,
        },
    })
}

fn push_related(
    by_key: &mut HashMap<String, Candidate>,
    seen: &HashSet<String>,
    seed: &FilmRecord,
    item: &LibraryItem,
    kind: RetrievalKind,
    label: String,
) {
    let tmdb_id = tmdb_id_from_item(item);
    let key = identity_key(tmdb_id, &item.title, item.year);
    if seen.contains(&key) {
        return;
    }
    upsert_candidate(
        by_key,
        Candidate {
            tmdb_id,
            title: item.title.clone(),
            year: item.year,
            poster: item.poster.clone(),
            genres: Vec::new(),
            credits: Vec::new(),
            keywords: Vec::new(),
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![RetrievalSource::new(kind, label, seed.tmdb_id).with_rating(seed.rating)],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        },
    );
}

fn seed_priority(seed: &FilmRecord) -> f32 {
    seed.signal
        .as_ref()
        .map(|s| s.preference.affinity_preference * s.recommendation_weight)
        .unwrap_or(0.0)
}

fn has_usable_related(seed: &FilmRecord) -> bool {
    !seed.recommendations.is_empty() || !seed.similar.is_empty()
}

pub fn eligible_positive_like(seed: &FilmRecord) -> bool {
    let Some(signal) = seed.signal.as_ref() else {
        return false;
    };
    if seed.rating.is_none() {
        return false;
    }
    if signal.preference.affinity_preference <= 0.0 || signal.preference.absolute <= 0.0 {
        return false;
    }
    if signal.familiarity_strength >= 0.6 {
        return false;
    }
    if seed
        .genres
        .iter()
        .any(|g| g.eq_ignore_ascii_case("tv movie"))
    {
        return false;
    }
    true
}

/// Related expansion is for every eligible positive-like film. The source
/// provenance and later evidence grade decide whether a result is displayed.
fn seed_expands_related(seed: &FilmRecord, _profile: &FeatureProfile) -> bool {
    eligible_positive_like(seed)
}

fn seed_allows_similar(seed: &FilmRecord, _profile: &FeatureProfile) -> bool {
    eligible_positive_like(seed)
}

fn filmography_person_allowed(a: &crate::taste::features::FeatureAffinity) -> bool {
    if a.key.id.is_none() || !a.citeable() {
        return false;
    }
    match a.key.family {
        FeatureFamily::Director | FeatureFamily::Writer | FeatureFamily::Cinematographer => {
            a.recommendation_mean > 0.15
        }
        FeatureFamily::Actor => {
            a.appearances >= 3
                && a.recommendation_mean >= crate::taste::features::PORTABLE_CONTEXTUAL
        }
        FeatureFamily::Composer => a.appearances >= 4 && a.recommendation_mean > 0.22,
        _ => false,
    }
}

fn filmography_credit_cap(family: FeatureFamily) -> usize {
    match family {
        FeatureFamily::Actor => ACTOR_FILMOGRAPHY_CAP,
        FeatureFamily::Composer => COMPOSER_FILMOGRAPHY_CAP,
        _ => FILMOGRAPHY_PER_PERSON,
    }
}

fn film_has_catalog(film: &FilmRecord) -> bool {
    !film.genres.is_empty()
        && film.credits.iter().any(|c| c.id.is_some())
        && !film.keywords.is_empty()
        && (!film.recommendations.is_empty() || !film.similar.is_empty())
}

fn candidate_has_catalog(c: &Candidate) -> bool {
    !c.genres.is_empty()
        && c.credits.iter().any(|cr| cr.id.is_some())
        && (!c.keywords.is_empty() || c.runtime.is_some())
}

fn candidate_priority(c: &Candidate) -> i32 {
    if c.watchlist {
        return 500;
    }
    // Soft examination priority by generator *family*, not edge multiplicity.
    // Five TMDB recommendation edges from the same related graph must not outrank
    // one filmography + one collection hit solely by counting edges.
    let families: HashSet<GeneratorFamily> = c
        .sources
        .iter()
        .map(|s| s.kind.generator_family())
        .collect();
    let mut p = 0;
    for family in &families {
        p += match family {
            GeneratorFamily::Friend => 40,
            GeneratorFamily::Watchlist => 50,
            GeneratorFamily::Related => 10,
            GeneratorFamily::Filmography => 8,
            GeneratorFamily::Collection => 8,
            GeneratorFamily::SemanticFilmLocal => 12,
            GeneratorFamily::SemanticProfile => 11,
            GeneratorFamily::Discovery | GeneratorFamily::Exploration => 2,
        };
    }
    p + families.len() as i32
}

fn pool_order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    candidate_priority(a)
        .cmp(&candidate_priority(b))
        .then(
            a.sources
                .iter()
                .map(|s| s.kind.generator_family())
                .collect::<HashSet<_>>()
                .len()
                .cmp(
                    &b.sources
                        .iter()
                        .map(|s| s.kind.generator_family())
                        .collect::<HashSet<_>>()
                        .len(),
                ),
        )
        .then(a.tmdb_id.unwrap_or(0).cmp(&b.tmdb_id.unwrap_or(0)))
        .then(a.title.cmp(&b.title))
}

/// Examination order *within* one generator family.
/// Uses that family's retrieval provenance only — never cross-family edge count —
/// so a pure SemanticProfile neighbor is not buried under Related+Filmography rows
/// that happen to also carry a weak semantic edge.
///
/// E1.1 consolidation (SemanticFilmLocal / SemanticProfile): prioritize best native
/// neighbor rank, then bounded support from distinct seeds/queries — not raw hit count.
fn family_exam_order(
    map: &HashMap<String, Candidate>,
    family: GeneratorFamily,
    a: &str,
    b: &str,
) -> std::cmp::Ordering {
    let ca = &map[a];
    let cb = &map[b];
    let consolidate = crate::taste::exam_policy::semantic_consolidate()
        && matches!(
            family,
            GeneratorFamily::SemanticFilmLocal | GeneratorFamily::SemanticProfile
        );
    let score = |c: &Candidate| -> (u32, i32, OrderedFloat) {
        let mut rank = u32::MAX;
        let mut sim = f32::NEG_INFINITY;
        let mut seed_ids: HashSet<i64> = HashSet::new();
        let mut query_labels: HashSet<String> = HashSet::new();
        for s in &c.sources {
            if s.kind.generator_family() != family {
                continue;
            }
            if let Some(r) = s.neighbor_rank {
                rank = rank.min(r);
            }
            if let Some(v) = s.similarity {
                if v > sim {
                    sim = v;
                }
            }
            if let Some(sid) = s.seed_tmdb_id {
                seed_ids.insert(sid);
            } else if !s.label.is_empty() {
                query_labels.insert(s.label.clone());
            }
        }
        // Bound multi-seed support so 8 overlapping FilmLocal hits ≠ 8× priority.
        let support = if consolidate {
            seed_ids.len().max(query_labels.len()).min(3) as i32
        } else {
            0
        };
        (rank, -support, OrderedFloat(sim))
    };
    let (ra, sa, sim_a) = score(ca);
    let (rb, sb, sim_b) = score(cb);
    // Lower neighbor_rank first; higher bounded support; higher similarity; stable id.
    ra.cmp(&rb)
        .then(sa.cmp(&sb))
        .then(sim_b.cmp(&sim_a))
        .then(ca.tmdb_id.unwrap_or(0).cmp(&cb.tmdb_id.unwrap_or(0)))
        .then(ca.title.cmp(&cb.title))
}

/// Tiny wrapper so we can Ord-compare f32 similarity without pulling in ordered-float.
#[derive(Copy, Clone, PartialEq)]
struct OrderedFloat(f32);
impl Eq for OrderedFloat {}
impl PartialOrd for OrderedFloat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Copy, Clone)]
enum CapRespect {
    None,
    Soft,
    Hard,
}

pub fn select_fair_pool(map: HashMap<String, Candidate>, cap: usize) -> Vec<Candidate> {
    if map.is_empty() || cap == 0 {
        return Vec::new();
    }

    let mut by_seed: HashMap<i64, Vec<String>> = HashMap::new();
    let mut by_person: HashMap<String, Vec<String>> = HashMap::new();
    let mut by_collection: HashMap<String, Vec<String>> = HashMap::new();
    let mut by_family: HashMap<GeneratorFamily, Vec<String>> = HashMap::new();
    let mut must: Vec<String> = Vec::new();
    for (key, c) in &map {
        if c.watchlist || c.sources.iter().any(|s| s.kind == RetrievalKind::Friend) {
            must.push(key.clone());
        }
        for s in &c.sources {
            let family = s.kind.generator_family();
            by_family.entry(family).or_default().push(key.clone());
            if s.kind.is_related() {
                if let Some(seed) = s.seed_tmdb_id {
                    by_seed.entry(seed).or_default().push(key.clone());
                }
            }
            if s.kind == RetrievalKind::Filmography {
                by_person
                    .entry(s.label.clone())
                    .or_default()
                    .push(key.clone());
            }
            if s.kind == RetrievalKind::Collection {
                by_collection
                    .entry(s.label.clone())
                    .or_default()
                    .push(key.clone());
            }
        }
    }
    for keys in by_seed.values_mut() {
        keys.sort();
        keys.dedup();
        keys.sort_by(|a, b| pool_order(&map[b], &map[a]));
    }
    for keys in by_person.values_mut() {
        keys.sort();
        keys.dedup();
        keys.sort_by(|a, b| pool_order(&map[b], &map[a]));
    }
    for keys in by_collection.values_mut() {
        keys.sort();
        keys.dedup();
        keys.sort_by(|a, b| pool_order(&map[b], &map[a]));
    }
    // Family lists use family-native retrieval quality, not multi-edge pool_order.
    for (family, keys) in by_family.iter_mut() {
        keys.sort();
        keys.dedup();
        let fam = *family;
        keys.sort_by(|a, b| family_exam_order(&map, fam, a, b));
    }

    let mut selected: Vec<String> = Vec::new();
    let mut seen_sel = HashSet::new();
    let mut family_taken: HashMap<GeneratorFamily, usize> = HashMap::new();

    let primary_family = |key: &str| -> Option<GeneratorFamily> {
        let c = map.get(key)?;
        // Prefer semantic families when present so soft caps protect FilmLocal/Profile
        // separately rather than attributing a multi-edge row only to Related.
        let mut best: Option<(i32, GeneratorFamily)> = None;
        for s in &c.sources {
            let fam = s.kind.generator_family();
            let prio = match fam {
                GeneratorFamily::SemanticFilmLocal => 5,
                GeneratorFamily::SemanticProfile => 4,
                GeneratorFamily::Collection => 3,
                GeneratorFamily::Filmography => 2,
                GeneratorFamily::Related => 1,
                _ => 0,
            };
            best = match best {
                Some((bp, _)) if bp >= prio => best,
                _ => Some((prio, fam)),
            };
        }
        best.map(|(_, f)| f)
    };

    let under_soft_cap = |key: &str, family_taken: &HashMap<GeneratorFamily, usize>| -> bool {
        let Some(fam) = primary_family(key) else {
            return true;
        };
        let Some(limit) = crate::taste::exam_policy::family_soft_cap(fam, cap) else {
            return true;
        };
        family_taken.get(&fam).copied().unwrap_or(0) < limit
    };

    // Spill may exceed soft caps slightly, but never let one family eat the whole cut.
    let under_hard_cap = |key: &str, family_taken: &HashMap<GeneratorFamily, usize>| -> bool {
        let Some(fam) = primary_family(key) else {
            return true;
        };
        let Some(soft) = crate::taste::exam_policy::family_soft_cap(fam, cap) else {
            return true;
        };
        let hard = soft + soft / 2; // 1.5× soft
        family_taken.get(&fam).copied().unwrap_or(0) < hard
    };

    let mut push_key = |key: String,
                        selected: &mut Vec<String>,
                        family_taken: &mut HashMap<GeneratorFamily, usize>,
                        respect_cap: CapRespect| {
        if selected.len() >= cap {
            return;
        }
        let ok = match respect_cap {
            CapRespect::None => true,
            CapRespect::Soft => under_soft_cap(&key, family_taken),
            CapRespect::Hard => under_hard_cap(&key, family_taken),
        };
        if !ok {
            return;
        }
        if seen_sel.insert(key.clone()) {
            if let Some(fam) = primary_family(&key) {
                *family_taken.entry(fam).or_insert(0) += 1;
            }
            selected.push(key);
        }
    };

    must.sort();
    // Watchlist/friend are intentional, but they must not consume the entire
    // discovery examination window (@250). Cap the forced prefix; overflow
    // still enters later via family fill / rest.
    let must_prefix = must.len().min(32).min(cap / 8);
    for key in must.iter().take(must_prefix) {
        // Must rows bypass soft caps.
        push_key(key.clone(), &mut selected, &mut family_taken, CapRespect::None);
    }

    // Round-robin FIRST so Related/Filmography seed guarantees cannot consume
    // the entire @250 examination window before semantic ever appears.
    let family_order = [
        GeneratorFamily::SemanticFilmLocal,
        GeneratorFamily::SemanticProfile,
        GeneratorFamily::Collection,
        GeneratorFamily::Filmography,
        GeneratorFamily::Related,
        GeneratorFamily::Discovery,
        GeneratorFamily::Exploration,
        GeneratorFamily::Friend,
        GeneratorFamily::Watchlist,
    ];
    let early_cap = cap.min(400);
    let mut cursors: HashMap<GeneratorFamily, usize> = HashMap::new();
    while selected.len() < early_cap {
        let mut progressed = false;
        for family in family_order {
            if selected.len() >= early_cap {
                break;
            }
            let Some(keys) = by_family.get(&family) else {
                continue;
            };
            let cursor = cursors.entry(family).or_insert(0);
            while *cursor < keys.len() {
                let key = keys[*cursor].clone();
                *cursor += 1;
                let before = selected.len();
                push_key(key, &mut selected, &mut family_taken, CapRespect::Soft);
                if selected.len() > before {
                    progressed = true;
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
    }

    // Remaining must rows after the early balanced tranche.
    for key in must.into_iter().skip(must_prefix) {
        push_key(key, &mut selected, &mut family_taken, CapRespect::None);
    }

    // Diversity guarantees fill remaining slots after the early balanced tranche.
    // Graph guarantees may slightly exceed soft caps — intentional floors.
    let mut seed_ids: Vec<i64> = by_seed.keys().copied().collect();
    seed_ids.sort();
    for pass in 0..PER_SEED_GUARANTEE {
        for sid in &seed_ids {
            if let Some(keys) = by_seed.get(sid) {
                if let Some(key) = keys.get(pass) {
                    push_key(key.clone(), &mut selected, &mut family_taken, CapRespect::None);
                }
            }
        }
    }
    let mut people: Vec<String> = by_person.keys().cloned().collect();
    people.sort();
    for pass in 0..PER_PERSON_GUARANTEE {
        for name in &people {
            if let Some(keys) = by_person.get(name) {
                if let Some(key) = keys.get(pass) {
                    push_key(key.clone(), &mut selected, &mut family_taken, CapRespect::None);
                }
            }
        }
    }
    let mut collections: Vec<String> = by_collection.keys().cloned().collect();
    collections.sort();
    for pass in 0..PER_COLLECTION_GUARANTEE {
        for name in &collections {
            if let Some(keys) = by_collection.get(name) {
                if let Some(key) = keys.get(pass) {
                    push_key(key.clone(), &mut selected, &mut family_taken, CapRespect::None);
                }
            }
        }
    }

    // Soft additional budget per family before volume fill.
    for family in family_order {
        let Some(keys) = by_family.get(&family) else {
            continue;
        };
        let mut taken = 0usize;
        for key in keys {
            if selected.len() >= cap || taken >= FAMILY_EXAM_BUDGET {
                break;
            }
            let before = selected.len();
            push_key(key.clone(), &mut selected, &mut family_taken, CapRespect::Soft);
            if selected.len() > before {
                taken += 1;
            }
        }
    }

    // Volume fill: continue family round-robin under soft caps (not multi-edge dump).
    if crate::taste::exam_policy::family_soft_caps_enabled() {
        let mut fill_cursors: HashMap<GeneratorFamily, usize> = HashMap::new();
        loop {
            if selected.len() >= cap {
                break;
            }
            let mut progressed = false;
            for family in family_order {
                if selected.len() >= cap {
                    break;
                }
                let Some(keys) = by_family.get(&family) else {
                    continue;
                };
                let cursor = fill_cursors.entry(family).or_insert(0);
                while *cursor < keys.len() {
                    let key = keys[*cursor].clone();
                    *cursor += 1;
                    let before = selected.len();
                    push_key(key, &mut selected, &mut family_taken, CapRespect::Soft);
                    if selected.len() > before {
                        progressed = true;
                        break;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        // Spill: unused capacity may exceed soft caps up to 1.5× hard ceiling.
        let mut spill_cursors: HashMap<GeneratorFamily, usize> = HashMap::new();
        loop {
            if selected.len() >= cap {
                break;
            }
            let mut progressed = false;
            for family in family_order {
                if selected.len() >= cap {
                    break;
                }
                let Some(keys) = by_family.get(&family) else {
                    continue;
                };
                let cursor = spill_cursors.entry(family).or_insert(0);
                while *cursor < keys.len() {
                    let key = keys[*cursor].clone();
                    *cursor += 1;
                    let before = selected.len();
                    push_key(key, &mut selected, &mut family_taken, CapRespect::Hard);
                    if selected.len() > before {
                        progressed = true;
                        break;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        // Final drip: remaining slots without family flood (hard still applies).
        let mut rest: Vec<String> = map.keys().cloned().collect();
        rest.sort_by(|a, b| pool_order(&map[b], &map[a]).then(a.cmp(b)));
        for key in rest {
            push_key(key, &mut selected, &mut family_taken, CapRespect::Hard);
        }
    } else {
        let mut rest: Vec<String> = map.keys().cloned().collect();
        rest.sort_by(|a, b| pool_order(&map[b], &map[a]).then(a.cmp(b)));
        for key in rest {
            push_key(key, &mut selected, &mut family_taken, CapRespect::None);
        }
    }

    selected
        .into_iter()
        .filter_map(|k| map.get(&k).cloned())
        .collect()
}

/// Filter a candidate map to sources belonging to `family`, dropping empty rows.
pub fn filter_pool_by_family(
    map: &HashMap<String, Candidate>,
    family: GeneratorFamily,
) -> HashMap<String, Candidate> {
    let mut out = HashMap::new();
    for (key, c) in map {
        let sources: Vec<_> = c
            .sources
            .iter()
            .filter(|s| s.kind.generator_family() == family)
            .cloned()
            .collect();
        if sources.is_empty() {
            continue;
        }
        let mut clone = c.clone();
        clone.sources = sources;
        out.insert(key.clone(), clone);
    }
    out
}

/// Remove one generator family from every candidate; drop rows with no sources left.
pub fn pool_without_family(
    map: &HashMap<String, Candidate>,
    family: GeneratorFamily,
) -> HashMap<String, Candidate> {
    let mut out = HashMap::new();
    for (key, c) in map {
        let sources: Vec<_> = c
            .sources
            .iter()
            .filter(|s| s.kind.generator_family() != family)
            .cloned()
            .collect();
        if sources.is_empty() {
            continue;
        }
        let mut clone = c.clone();
        clone.sources = sources;
        out.insert(key.clone(), clone);
    }
    out
}

pub(crate) fn keep_person_credit(job: &str, family: FeatureFamily) -> bool {
    family_for_job(job) == Some(family)
}

fn upsert_candidate(map: &mut HashMap<String, Candidate>, incoming: Candidate) {
    if !incoming.media_kind.is_movie() {
        return;
    }
    let key = identity_key(incoming.tmdb_id, &incoming.title, incoming.year);
    map.entry(key)
        .and_modify(|existing| {
            for src in incoming.sources.clone() {
                if !existing.sources.iter().any(|s| {
                    s.kind == src.kind && s.seed_tmdb_id == src.seed_tmdb_id && s.label == src.label
                }) {
                    existing.sources.push(src);
                }
            }
            existing.tmdb_related = existing.tmdb_related.max(incoming.tmdb_related);
            existing.friend_affinity += incoming.friend_affinity;
            existing.watchlist |= incoming.watchlist;
            if existing.credits.is_empty() {
                existing.credits = incoming.credits.clone();
            }
            if existing.genres.is_empty() {
                existing.genres = incoming.genres.clone();
            }
            if existing.poster.is_none() {
                existing.poster = incoming.poster.clone();
            }
            if existing.tmdb_id.is_none() {
                existing.tmdb_id = incoming.tmdb_id;
            }
        })
        .or_insert(incoming);
}

fn tmdb_id_from_item(item: &LibraryItem) -> Option<i64> {
    item.id.strip_prefix("tmdb:").and_then(|s| s.parse().ok())
}

fn friend_candidates(
    db: &Database,
    films: &[FilmRecord],
    seen: &HashSet<String>,
) -> Result<Vec<Candidate>, String> {
    let user_by_tmdb: HashMap<i64, f32> = films
        .iter()
        .filter_map(|f| Some((f.tmdb_id?, f.rating?)))
        .collect();
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT f.id, f.username, fa.rating, fa.raw_payload, m.tmdb_id,
                   COALESCE(m.canonical_title, json_extract(smr.raw_identity, '$.title'))
            FROM friend_activity fa
            JOIN friends f ON f.id = fa.friend_id
            LEFT JOIN source_movie_records smr ON smr.id = fa.source_movie_record_id
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            LEFT JOIN movies m ON m.id = ml.movie_id
            WHERE fa.rating IS NOT NULL AND fa.rating >= 3.5
            "#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    #[derive(Default)]
    struct FriendAcc {
        username: String,
        pairs: Vec<(f32, f32)>,
        ratings: Vec<f32>,
    }
    let mut friends: HashMap<String, FriendAcc> = HashMap::new();
    let mut movie_friends: HashMap<String, Vec<(String, f32, f32)>> = HashMap::new();

    struct Row {
        friend_id: String,
        username: String,
        rating: f32,
        tmdb_id: Option<i64>,
        title: String,
        year: Option<i32>,
    }
    let mut parsed = Vec::new();
    for row in rows.flatten() {
        let (friend_id, username, rating, raw, tmdb_id, title_opt) = row;
        let Some(rating) = rating.map(|r| r as f32) else {
            continue;
        };
        let (parsed_title, year) = parse_activity_payload(&raw);
        let title = title_opt.filter(|s| !s.is_empty()).unwrap_or(parsed_title);
        let tmdb_id = tmdb_id.or_else(|| tmdb_id_from_raw(&raw));
        parsed.push(Row {
            friend_id,
            username,
            rating,
            tmdb_id,
            title,
            year,
        });
    }

    for row in &parsed {
        let acc = friends.entry(row.friend_id.clone()).or_insert(FriendAcc {
            username: row.username.clone(),
            ..Default::default()
        });
        acc.ratings.push(row.rating);
        if let Some(tid) = row.tmdb_id {
            if let Some(mine) = user_by_tmdb.get(&tid) {
                acc.pairs.push((*mine, row.rating));
            }
        }
    }

    let mut similarity: HashMap<String, f32> = HashMap::new();
    for (id, acc) in &friends {
        similarity.insert(id.clone(), friend_similarity(&acc.pairs, &acc.ratings));
    }

    for row in &parsed {
        if row.rating < 3.5 {
            continue;
        }
        let key = identity_key(row.tmdb_id, &row.title, row.year);
        if seen.contains(&key) {
            continue;
        }
        let sim = similarity.get(&row.friend_id).copied().unwrap_or(0.0);
        movie_friends
            .entry(key)
            .or_default()
            .push((row.username.clone(), row.rating, sim));
    }

    let mut out = Vec::new();
    for (_key, contribs) in movie_friends {
        let first = parsed
            .iter()
            .find(|r| identity_key(r.tmdb_id, &r.title, r.year) == _key);
        let Some(sample) = first else { continue };
        let friend_affinity = contribs
            .iter()
            .map(|(_, rating, sim)| ((rating - 3.0) / 2.0).clamp(-1.0, 1.0) * sim)
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        out.push(Candidate {
            tmdb_id: sample.tmdb_id,
            title: sample.title.clone(),
            year: sample.year,
            poster: None,
            genres: Vec::new(),
            credits: Vec::new(),
            keywords: Vec::new(),
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Friend,
                label: format!("loved by {} friend(s)", contribs.len()),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        });
    }
    Ok(out)
}

pub fn friend_identity_keys(db: &Database) -> Vec<String> {
    let mut stmt = match db
        .conn()
        .prepare("SELECT source_record_key FROM friend_activity")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(0))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

fn friend_similarity(pairs: &[(f32, f32)], _ratings: &[f32]) -> f32 {
    let n = pairs.len();
    if n < 5 {
        return 0.0;
    }
    let mean_a = pairs.iter().map(|(a, _)| a).sum::<f32>() / n as f32;
    let mean_b = pairs.iter().map(|(_, b)| b).sum::<f32>() / n as f32;
    let mut num = 0.0;
    let mut da = 0.0;
    let mut db = 0.0;
    for (a, b) in pairs {
        let za = a - mean_a;
        let zb = b - mean_b;
        num += za * zb;
        da += za * za;
        db += zb * zb;
    }
    if da <= 1e-6 || db <= 1e-6 {
        return 0.0;
    }
    let corr = (num / (da.sqrt() * db.sqrt())).clamp(-1.0, 1.0);
    let conf = 1.0 - (-(n as f32) / 8.0).exp();
    corr * conf
}

fn tmdb_id_from_raw(raw: &str) -> Option<i64> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(id) = v.get("tmdb_id").and_then(|x| x.as_i64()) {
            return Some(id);
        }
    }
    None
}

fn hydrate_local_metadata(db: &Database, candidates: &mut [Candidate]) -> Result<(), String> {
    for c in candidates.iter_mut() {
        let Some(tid) = c.tmdb_id else { continue };
        let row = db.conn().query_row(
            "SELECT genres_json, credits_json, cast_json, crew_json, keywords_json, runtime, vote_count, poster_path, canonical_title, release_year
             FROM movies WHERE tmdb_id = ?1",
            params![tid],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i32>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i32>>(9)?,
                ))
            },
        );
        if let Ok((
            genres,
            credits,
            cast,
            crew,
            keywords,
            runtime,
            vote_count,
            poster,
            title,
            year,
        )) = row
        {
            if c.genres.is_empty() {
                c.genres = json_vec(genres);
            }
            let parsed = parse_credits(credits.as_deref(), cast.as_deref(), crew.as_deref());
            if !parsed.is_empty() {
                c.credits = parsed;
            }
            if c.keywords.is_empty() {
                c.keywords = parse_keywords(keywords);
            }
            if c.runtime.is_none() {
                c.runtime = runtime;
            }
            if c.vote_count.is_none() {
                c.vote_count = vote_count;
            }
            if c.poster.is_none() {
                c.poster = poster_url(poster);
            }
            if let Some(t) = title {
                if !t.is_empty() {
                    c.title = t;
                }
            }
            if c.year.is_none() {
                c.year = year;
            }
        }
    }
    Ok(())
}

pub fn enrich_missing(
    db: &Database,
    candidates: &mut [Candidate],
    cap: usize,
    force: bool,
) -> usize {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|&a, &b| {
        let rank = |c: &Candidate| {
            let recs = c
                .sources
                .iter()
                .any(|s| s.kind == RetrievalKind::RelatedRecommendations);
            let needs = c.genres.is_empty() || c.credits.len() <= 1 || c.keywords.is_empty();
            // Explicit watchlist intent must get metadata before the much
            // larger related/recommendation pool. Otherwise a small hydrate
            // budget can silently leave saved titles without the credits or
            // keywords needed by the watchlist bridge.
            (!c.watchlist, !recs, !needs)
        };
        rank(&candidates[a]).cmp(&rank(&candidates[b]))
    });
    let mut n = 0;
    for i in order {
        if n >= cap {
            break;
        }
        let c = &mut candidates[i];
        let Some(tid) = c.tmdb_id else { continue };
        if !force && !c.genres.is_empty() && c.credits.len() > 1 && !c.keywords.is_empty() {
            continue;
        }
        if tmdb::refresh_movie_catalog(db, tid, force).is_ok() {
            n += 1;
        }
    }
    let _ = hydrate_local_metadata(db, candidates);
    n
}

fn display_title(raw: &str) -> String {
    let (title, _) = parse_activity_payload(raw);
    if title.trim().is_empty() {
        raw.to_string()
    } else {
        title
    }
}

fn json_vec(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn parse_keywords(raw: Option<String>) -> Vec<Keyword> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .ok()
        .map(|vals| {
            vals.iter()
                .filter_map(|v| {
                    Some(Keyword {
                        id: v["id"].as_i64(),
                        name: v["name"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_credits(credits: Option<&str>, cast: Option<&str>, crew: Option<&str>) -> Vec<Credit> {
    if let Some(raw) = credits {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            let mut out = Vec::new();
            if let Some(arr) = v["crew"].as_array() {
                for c in arr {
                    if let Some(name) = c["name"].as_str() {
                        out.push(Credit {
                            id: c["tmdbId"].as_i64().or_else(|| c["id"].as_i64()),
                            name: name.to_string(),
                            job: c["job"].as_str().unwrap_or("").to_string(),
                        });
                    }
                }
            }
            if let Some(arr) = v["cast"].as_array() {
                for c in arr.iter().take(8) {
                    if let Some(name) = c["name"].as_str() {
                        out.push(Credit {
                            id: c["tmdbId"].as_i64().or_else(|| c["id"].as_i64()),
                            name: name.to_string(),
                            job: "Actor".into(),
                        });
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
    }
    let mut out = Vec::new();
    if let Some(crew) = crew {
        if let Ok(names) = serde_json::from_str::<Vec<String>>(crew) {
            for entry in names {
                if let Some((name, job)) = entry.rsplit_once(" (") {
                    out.push(Credit {
                        id: None,
                        name: name.to_string(),
                        job: job.trim_end_matches(')').to_string(),
                    });
                }
            }
        }
    }
    if let Some(cast) = cast {
        if let Ok(names) = serde_json::from_str::<Vec<String>>(cast) {
            for entry in names.into_iter().take(8) {
                let name = entry.split(" as ").next().unwrap_or(&entry).trim();
                if !name.is_empty() {
                    out.push(Credit {
                        id: None,
                        name: name.to_string(),
                        job: "Actor".into(),
                    });
                }
            }
        }
    }
    out
}

fn parse_item_list_raw(raw: Option<String>) -> Vec<LibraryItem> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    parse_item_list(Some(&v))
}

fn parse_related_lists(raw: Option<String>) -> (Vec<LibraryItem>, Vec<LibraryItem>) {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return (Vec::new(), Vec::new());
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(obj) = v.as_object() {
            if obj.contains_key("recommendations") || obj.contains_key("similar") {
                return (
                    parse_item_list(obj.get("recommendations")),
                    parse_item_list(obj.get("similar")),
                );
            }
        }
        if v.as_array().is_some() {
            let items = parse_item_list(Some(&v));
            return (items, Vec::new());
        }
    }
    (Vec::new(), Vec::new())
}

fn parse_item_list(v: Option<&serde_json::Value>) -> Vec<LibraryItem> {
    let Some(v) = v else {
        return Vec::new();
    };
    if let Ok(items) = serde_json::from_value::<Vec<LibraryItem>>(v.clone()) {
        if items.iter().any(|item| !item.title.is_empty()) {
            return items;
        }
    }
    v.as_array()
        .map(|vals| {
            vals.iter()
                .filter_map(tmdb::library_item_from_movie_value)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_similar(raw: Option<String>) -> Vec<LibraryItem> {
    let (recs, similar) = parse_related_lists(raw);
    let mut out = recs;
    out.extend(similar);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn credits_preserve_catalog_and_raw_tmdb_person_ids() {
        for field in ["tmdbId", "id"] {
            let raw = format!(r#"{{"crew":[{{"{field}":1144092,"name":"Matt Johnson","job":"Writer"}}],"cast":[{{"{field}":42,"name":"Actor"}}]}}"#);
            let credits = parse_credits(Some(&raw), None, None);
            assert_eq!(credits[0].id, Some(1144092));
            assert_eq!(credits[1].id, Some(42));
            assert_eq!(credits[1].job, "Actor");
        }
        let credits = parse_credits(Some(r#"{"crew":[{"tmdbId":1144092,"id":999,"name":"Matt Johnson","job":"Writer"}]}"#), None, None);
        assert_eq!(credits[0].id, Some(1144092));
    }

    #[test]
    fn credits_keep_legacy_name_only_fallback() {
        let credits = parse_credits(None, Some(r#"["Actor as Character"]"#), Some(r#"["Matt Johnson (Writer)"]"#));
        assert_eq!(credits.len(), 2);
        assert!(credits.iter().all(|credit| credit.id.is_none()));
        assert_eq!(credits[0].name, "Matt Johnson");
        assert_eq!(credits[1].name, "Actor");
        let structured = parse_credits(Some(r#"{"crew":[{"name":"Matt Johnson","job":"Writer"}]}"#), None, None);
        assert_eq!(structured[0].id, None);
    }

    fn parsed_writer_profile() -> FeatureProfile {
        let credits = parse_credits(Some(r#"{"crew":[{"tmdbId":1144092,"name":"Matt Johnson","job":"Writer"}]}"#), None, None);
        let ratings = rating_profile(&[4.5; 8]).unwrap();
        let mut observations = Vec::new();
        for (id, title) in [(1, "Tony"), (2, "Nirvanna the Band the Show the Movie")] {
            observations.extend(crate::taste::features::observations_from_film(
                title, 4.5, Some(id),
                &interaction_signal(4.5, &ratings, Some(0.5), 1, false),
                Some(0.5), &[], &credits, &[], Some(2026), Some(100),
            ));
        }
        crate::taste::features::build_profile(&observations)
    }

    #[test]
    fn same_name_writers_do_not_share_affinity_or_legacy_feedback() {
        let mut profile = parsed_writer_profile();
        crate::taste::feedback::apply_feedback_adjustments(
            &mut profile, &HashMap::from([("Writer:matt johnson".into(), -0.15)]),
        );
        assert!(profile.affinities.iter().all(|a| a.feedback_adjustment == 0.0));
        let raw = r#"{"crew":[{"tmdbId":66824,"name":"Matt Johnson","job":"Writer"}]}"#;
        let mut candidate = Candidate {
            tmdb_id: Some(999), title: "Afterburn".into(), year: Some(2025), poster: None,
            genres: vec![], credits: parse_credits(Some(raw), None, None), keywords: vec![],
            runtime: Some(100), vote_count: Some(100), watchlist: false, sources: vec![],
            friend_affinity: 0.0, tmdb_related: 0.0, media_kind: MediaKind::Movie,
        };
        let unrelated = crate::taste::score::score_candidate(&profile, &candidate);
        assert!(unrelated.matched_features.iter().all(|f| f.name != "Matt Johnson"));
        candidate.credits[0].id = Some(1144092);
        let actual_writer = crate::taste::score::score_candidate(&profile, &candidate);
        assert!(actual_writer.matched_features.iter().any(|f| f.name == "Matt Johnson"));
        assert!(actual_writer.score.content > unrelated.score.content);
    }

    #[test]
    fn parsed_person_id_restores_cached_filmography_retrieval() {
        let db = Database::in_memory().unwrap();
        let credits = serde_json::to_string(&crate::catalog::tmdb::PersonCreditsCache {
            version: crate::catalog::tmdb::PERSON_CREDITS_CACHE_VERSION,
            credits: vec![crate::catalog::tmdb::PersonCredit {
                tmdb_id: 101,
                title: "BlackBerry".into(),
                year: Some(2023),
                job: "Writer".into(),
                order: None,
            }],
        })
        .unwrap();
        db.conn().execute(
            "INSERT INTO person_credits(person_id, credits_json, fetched_at) VALUES (?1, ?2, datetime('now'))",
            params![1144092, credits],
        ).unwrap();
        let candidates = retrieve(&db, &[], &parsed_writer_profile(), &HashSet::new(), false).unwrap();
        let film = candidates.iter().find(|c| c.tmdb_id == Some(101)).expect("cached writer filmography must be retrieved");
        assert_eq!(film.credits[0].id, Some(1144092));
        assert!(film.sources.iter().any(|s| s.kind == RetrievalKind::Filmography));
    }

    #[test]
    fn friend_overlap_gate() {
        assert_eq!(
            friend_similarity(&[(5.0, 5.0), (4.0, 4.0)], &[5.0, 4.0]),
            0.0
        );
        let pairs: Vec<(f32, f32)> = (0..20)
            .map(|i| (3.0 + (i % 3) as f32 * 0.5, 3.0 + (i % 3) as f32 * 0.5))
            .collect();
        assert!(friend_similarity(&pairs, &[]).abs() > 0.3);
    }

    #[test]
    fn parse_related_lists_keeps_provenance() {
        let tagged = serde_json::json!({
            "recommendations": [{"id":"tmdb:1","title":"Rec","year":2020}],
            "similar": [{"id":"tmdb:2","title":"Sim","year":2019}]
        });
        let (recs, similar) = parse_related_lists(Some(tagged.to_string()));
        assert_eq!(recs[0].title, "Rec");
        assert_eq!(similar[0].title, "Sim");
        let legacy = serde_json::json!([{"id":"tmdb:3","title":"Old","year":2010}]);
        let (recs, similar) = parse_related_lists(Some(legacy.to_string()));
        assert_eq!(recs[0].title, "Old");
        assert!(similar.is_empty());
    }

    #[test]
    fn identity_prefers_tmdb() {
        assert_eq!(identity_key(Some(99), "Heat", Some(1995)), "tmdb:99");
        assert_eq!(identity_key(None, "Heat", Some(1995)), "heat|1995");
    }

    #[test]
    fn consolidates_duplicate_source_records() {
        let record = |viewings, rating| FilmRecord {
            key: "tmdb:1571662".into(),
            title: "Tuner".into(),
            year: Some(2025),
            tmdb_id: Some(1571662),
            rating,
            liked: false,
            watched: true,
            watchlist: false,
            viewings,
            last_date: Some("2025-01-01".into()),
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            recommendations: vec![],
            similar: vec![],
            collection_name: None,
            collection: vec![],
            runtime: None,
            poster: None,
            vote_count: None,
            review: None,
            signal: None,
            age_years: None,
        };
        let films = consolidate_films(vec![record(1, None), record(0, Some(4.5))]);
        assert_eq!(films.len(), 1);
        assert_eq!(films[0].viewings, 1);
        assert_eq!(films[0].rating, Some(4.5));
    }

    #[test]
    fn load_films_uses_effective_viewings_for_taste_signals() {
        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        for (id, occurred_at) in [("first", "2026-02-28"), ("duplicate", "2026-03-01")] {
            tx.execute(
                "INSERT INTO source_movie_records(
                   id, source_type, source_record_key, normalized_title, release_year, raw_identity, created_at
                 ) VALUES (?1, 'letterboxd_export', ?1, 'source code', 2011, '{\"title\":\"Source Code\"}', '2026-03-01T00:00:00Z')",
                params![id],
            )
            .expect("source movie");
            tx.execute(
                "INSERT INTO viewings(
                   id, source_movie_record_id, source_record_key, occurred_at, observed_at, source_type, raw_payload
                 ) VALUES (?1, ?1, ?1, ?2, ?2, 'letterboxd_export', 'legacy payload')",
                params![id, occurred_at],
            )
            .expect("viewing");
        }
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let films = load_films(&db).expect("taste films");
        assert_eq!(films.len(), 1);
        assert_eq!(films[0].viewings, 1);
        assert_eq!(films[0].last_date.as_deref(), Some("2026-02-28"));
    }

    #[test]
    fn seed_rank_ignores_rewatch_boost() {
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let once = interaction_signal(4.5, &p, Some(1.0), 1, false);
        let many = interaction_signal(4.5, &p, Some(1.0), 7, false);
        let once_seed = once.preference.absolute * once.recommendation_weight;
        let many_seed = many.preference.absolute * many.recommendation_weight;
        assert!(many_seed < once_seed);
        assert!(many.preference_weight > once.preference_weight);
    }

    fn test_seed(title: &str, viewings: u32, credits: Vec<Credit>) -> FilmRecord {
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        FilmRecord {
            key: title.into(),
            title: title.into(),
            year: Some(2011),
            tmdb_id: Some(1),
            rating: Some(5.0),
            liked: true,
            watched: true,
            watchlist: false,
            viewings,
            last_date: None,
            genres: vec!["Drama".into()],
            credits,
            keywords: vec![],
            recommendations: vec![LibraryItem::catalog(
                "tmdb:99".into(),
                "Dirty".into(),
                Some(2005),
                None,
                None,
                None,
            )],
            similar: vec![],
            collection_name: None,
            collection: vec![],
            runtime: None,
            poster: None,
            vote_count: None,
            review: None,
            signal: Some(interaction_signal(5.0, &p, Some(0.4), viewings, false)),
            age_years: Some(0.4),
        }
    }

    #[test]
    fn twilight_positive_like_expands_without_citeable_director() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let condon = Credit {
            id: Some(99),
            name: "Bill Condon".into(),
            job: "Director".into(),
        };
        let obs = observations_from_film(
            "The Twilight Saga: Breaking Dawn - Part 1",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[condon.clone()],
            &[],
            Some(2011),
            None,
        );
        let profile = build_profile(&obs);
        let seed = test_seed("Twilight", 1, vec![condon]);
        assert!(
            seed_expands_related(&seed, &profile),
            "an eligible positive-like film expands even when its director is not citeable"
        );
    }

    #[test]
    fn citeable_person_seed_does_expand_related() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Dune",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            None,
        ));
        let profile = build_profile(&obs);
        let seed = test_seed("The Batman", 1, vec![dp.clone()]);
        assert!(seed_expands_related(&seed, &profile));
        let rewatch = test_seed("The Batman", 7, vec![dp.clone()]);
        assert!(
            !seed_expands_related(&rewatch, &profile),
            "familiar films must not flood related"
        );
        let mut family = test_seed("Teen Beach 2", 1, vec![dp]);
        family.tmdb_id = Some(3);
        family.genres = vec!["Family".into(), "Comedy".into()];
        assert!(
            seed_expands_related(&family, &profile),
            "family/kids seeds may expand; display still requires Medium evidence"
        );
    }

    #[test]
    fn writer_only_seed_does_not_expand_related() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let writer = Credit {
            id: Some(44),
            name: "Jonathan Aibel".into(),
            job: "Writer".into(),
        };
        let mut obs = observations_from_film(
            "Kung Fu Panda",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Animation".into(), "Family".into()],
            &[writer.clone()],
            &[],
            Some(2008),
            None,
        );
        obs.extend(observations_from_film(
            "Kung Fu Panda 2",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Animation".into(), "Family".into()],
            &[writer.clone()],
            &[],
            Some(2011),
            None,
        ));
        let profile = build_profile(&obs);
        let seed = test_seed("Kung Fu Panda", 1, vec![writer]);
        let mut seed = seed;
        seed.genres = vec!["Animation".into(), "Family".into()];
        assert!(
            seed_expands_related(&seed, &profile),
            "animation seeds may expand; weak similar-to still stays off New"
        );
    }

    #[test]
    fn composer_only_seed_expands_bounded_recommendations_and_similar() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let composer = Credit {
            id: Some(12),
            name: "Michael Giacchino".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[composer.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Up",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Animation".into(), "Family".into()],
            &[composer.clone()],
            &[],
            Some(2009),
            None,
        ));
        let profile = build_profile(&obs);
        let seed = test_seed("The Batman", 1, vec![composer]);
        assert!(
            seed_expands_related(&seed, &profile),
            "composer-led positive-like films still expand recommendations"
        );
        assert!(
            seed_allows_similar(&seed, &profile),
            "eligible seeds may use bounded similar-to retrieval"
        );
    }

    #[test]
    fn upsert_drops_non_movies() {
        let mut map = HashMap::new();
        upsert_candidate(
            &mut map,
            Candidate {
                tmdb_id: Some(1),
                title: "A Show".into(),
                year: Some(2020),
                poster: None,
                genres: vec![],
                credits: vec![],
                keywords: vec![],
                runtime: None,
                vote_count: None,
                watchlist: false,
                sources: vec![],
                friend_affinity: 0.0,
                tmdb_related: 0.0,
                media_kind: MediaKind::TvSeries,
            },
        );
        upsert_candidate(
            &mut map,
            Candidate {
                tmdb_id: Some(2),
                title: "A Movie".into(),
                year: Some(2020),
                poster: None,
                genres: vec![],
                credits: vec![],
                keywords: vec![],
                runtime: None,
                vote_count: None,
                watchlist: false,
                sources: vec![],
                friend_affinity: 0.0,
                tmdb_related: 0.0,
                media_kind: MediaKind::Movie,
            },
        );
        assert_eq!(map.len(), 1);
        assert_eq!(map.values().next().unwrap().title, "A Movie");
    }

    #[test]
    fn retrieve_membership_follows_current_seeds_not_a_union() {
        use crate::storage::db::Database;
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let db = Database::in_memory().unwrap();
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Dune",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            None,
        ));
        let profile = build_profile(&obs);
        let extra_from_dune = crate::models::LibraryItem::catalog(
            "tmdb:909".into(),
            "Only From Dune".into(),
            Some(2000),
            None,
            None,
            None,
        );
        let extra_from_batman = crate::models::LibraryItem::catalog(
            "tmdb:808".into(),
            "Only From Batman".into(),
            Some(2001),
            None,
            None,
            None,
        );
        let mut batman = test_seed("The Batman", 1, vec![dp.clone()]);
        batman.tmdb_id = Some(1);
        batman.recommendations = vec![extra_from_batman];
        let mut dune = test_seed("Dune", 1, vec![dp]);
        dune.tmdb_id = Some(2);
        dune.recommendations = vec![extra_from_dune];
        let seen = HashSet::new();
        let both = retrieve(&db, &[batman.clone(), dune.clone()], &profile, &seen, false).unwrap();
        assert!(both.iter().any(|c| c.tmdb_id == Some(909)));
        assert!(both.iter().any(|c| c.tmdb_id == Some(808)));
        let without_dune = retrieve(&db, &[batman], &profile, &seen, false).unwrap();
        assert!(
            without_dune.iter().all(|c| c.tmdb_id != Some(909)),
            "removed seed must not leave a candidate union, got {:?}",
            without_dune.iter().map(|c| &c.title).collect::<Vec<_>>()
        );
    }

    fn neighbor(id: i64, title: &str) -> LibraryItem {
        LibraryItem::catalog(
            format!("tmdb:{id}"),
            title.into(),
            Some(2000),
            None,
            None,
            None,
        )
    }

    #[test]
    fn upsert_merges_sources_from_every_seed() {
        let mut map = HashMap::new();
        let item = neighbor(50, "Shared");
        for seed_id in [1, 2, 3, 4, 5] {
            push_related(
                &mut map,
                &HashSet::new(),
                &{
                    let mut s = test_seed("Seed", 1, vec![]);
                    s.tmdb_id = Some(seed_id);
                    s
                },
                &item,
                RetrievalKind::RelatedRecommendations,
                format!("recommended from {seed_id}"),
            );
        }
        assert_eq!(map.len(), 1);
        let c = map.values().next().unwrap();
        assert_eq!(c.sources.len(), 5, "got {:?}", c.sources);
    }

    #[test]
    fn held_out_liked_neighbor_is_retrieved() {
        use crate::storage::db::Database;
        use crate::taste::features::build_profile;
        let db = Database::in_memory().unwrap();
        let profile = build_profile(&[]);
        let mut seed = test_seed("The Batman", 1, vec![]);
        seed.tmdb_id = Some(1);
        seed.recommendations = vec![neighbor(414_906, "Dune analog")];
        let mut seen = HashSet::new();
        seen.insert(identity_key(Some(1), "The Batman", Some(2011)));
        let out = retrieve(&db, &[seed], &profile, &seen, false).unwrap();
        assert!(
            out.iter().any(|c| c.tmdb_id == Some(414_906)),
            "a held-out liked neighbor of an eligible seed must re-enter the pool"
        );
        let mix = crate::taste::eval::source_mix(
            &out.iter()
                .map(|c| crate::taste::score::score_candidate(&profile, c))
                .collect::<Vec<_>>(),
        );
        assert!(mix.related_recommendations >= 1);
    }

    #[test]
    fn fifty_first_eligible_seed_still_expands() {
        use crate::storage::db::Database;
        use crate::taste::features::build_profile;
        use crate::taste::preference::{interaction_signal, rating_profile};
        let db = Database::in_memory().unwrap();
        let p = rating_profile(&[4.0; 8]).unwrap();
        let profile = build_profile(&[]);
        let mut films = Vec::new();
        for i in 0..51 {
            let mut seed = test_seed(&format!("Liked {i}"), 1, vec![]);
            seed.tmdb_id = Some(1000 + i);
            seed.rating = Some(4.5);
            seed.signal = Some(interaction_signal(4.5, &p, Some(0.4), 1, false));
            seed.recommendations = vec![neighbor(5000 + i, &format!("N{i}"))];
            films.push(seed);
        }
        let seen = HashSet::new();
        let out = retrieve(&db, &films, &profile, &seen, false).unwrap();
        assert!(
            out.iter().any(|c| c.tmdb_id == Some(5050)),
            "51st eligible seed must still contribute a neighbor"
        );
    }

    #[test]
    fn disliked_seed_does_not_expand() {
        use crate::storage::db::Database;
        use crate::taste::features::build_profile;
        use crate::taste::preference::{interaction_signal, rating_profile};
        let db = Database::in_memory().unwrap();
        let p = rating_profile(&[4.0; 8]).unwrap();
        let profile = build_profile(&[]);
        let mut seed = test_seed("Hated", 1, vec![]);
        seed.rating = Some(1.0);
        seed.signal = Some(interaction_signal(1.0, &p, Some(0.4), 1, false));
        seed.recommendations = vec![neighbor(77, "From hated")];
        let out = retrieve(&db, &[seed], &profile, &HashSet::new(), false).unwrap();
        assert!(
            out.iter().all(|c| c.tmdb_id != Some(77)),
            "disliked films must not seed neighbors"
        );
    }

    #[test]
    fn composer_seed_uses_recommendations_and_bounded_similar() {
        use crate::storage::db::Database;
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let db = Database::in_memory().unwrap();
        let p = rating_profile(&[4.0; 8]).unwrap();
        let composer = Credit {
            id: Some(12),
            name: "Michael Giacchino".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[composer.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Jurassic World",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into()],
            &[composer.clone()],
            &[],
            Some(2015),
            None,
        ));
        let profile = build_profile(&obs);
        let mut seed = test_seed("The Batman", 1, vec![composer]);
        seed.recommendations = vec![neighbor(11, "From recs")];
        seed.similar = vec![neighbor(22, "From similar")];
        let out = retrieve(&db, &[seed], &profile, &HashSet::new(), false).unwrap();
        assert!(out.iter().any(|c| c.tmdb_id == Some(11)));
        assert!(out.iter().any(|c| c.tmdb_id == Some(22)));
    }

    #[test]
    fn related_recommendations_count_as_related_only() {
        use crate::taste::features::build_profile;
        use crate::taste::score::score_candidate;
        let profile = build_profile(&[]);
        let c = Candidate {
            tmdb_id: Some(1),
            title: "Rec".into(),
            year: Some(2020),
            poster: None,
            genres: vec!["Drama".into()],
            credits: vec![],
            keywords: vec![],
            runtime: Some(100),
            vote_count: Some(10),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from X".into(),
                seed_tmdb_id: Some(9),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &c);
        assert!(crate::taste::confidence::related_only(&scored));
    }

    #[test]
    fn fair_pool_keeps_a_neighbor_per_seed() {
        let mut map = HashMap::new();
        for i in 0..80i64 {
            let mut seed = test_seed("S", 1, vec![]);
            seed.tmdb_id = Some(i);
            push_related(
                &mut map,
                &HashSet::new(),
                &seed,
                &neighbor(10_000 + i, &format!("only-{i}")),
                RetrievalKind::RelatedRecommendations,
                format!("recommended from {i}"),
            );
            for extra in 0..20 {
                push_related(
                    &mut map,
                    &HashSet::new(),
                    &seed,
                    &neighbor(20_000 + i * 20 + extra, &format!("pad-{i}-{extra}")),
                    RetrievalKind::RelatedRecommendations,
                    format!("recommended from {i}"),
                );
            }
        }
        let selected = select_fair_pool(map, 100);
        let mut seeds_kept = HashSet::new();
        for c in &selected {
            for s in &c.sources {
                if let Some(id) = s.seed_tmdb_id {
                    seeds_kept.insert(id);
                }
            }
        }
        assert_eq!(
            seeds_kept.len(),
            80,
            "fair cap must keep at least one neighbor per seed, kept {}",
            seeds_kept.len()
        );
    }

    #[test]
    fn fair_pool_puts_semantic_in_early_examination_window() {
        let mut map = HashMap::new();
        // Flood with multi-edge Related+Filmography rows that used to dominate @250.
        for i in 0..700i64 {
            let key = format!("rel-{i}");
            map.insert(
                key,
                Candidate {
                    tmdb_id: Some(50_000 + i),
                    title: format!("Related flood {i}"),
                    year: Some(2010),
                    poster: None,
                    genres: vec!["Drama".into()],
                    credits: vec![],
                    keywords: vec![],
                    runtime: Some(100),
                    vote_count: Some(10),
                    watchlist: false,
                    sources: vec![
                        RetrievalSource::new(
                            RetrievalKind::RelatedRecommendations,
                            format!("rec {i}"),
                            Some(i % 80),
                        ),
                        RetrievalSource::new(RetrievalKind::Filmography, "Actor", None),
                    ],
                    friend_affinity: 0.0,
                    tmdb_related: 1.0,
                    media_kind: MediaKind::Movie,
                },
            );
        }
        for i in 0..40i64 {
            map.insert(
                format!("sem-{i}"),
                Candidate {
                    tmdb_id: Some(90_000 + i),
                    title: format!("Semantic hit {i}"),
                    year: Some(2015),
                    poster: None,
                    genres: vec!["Drama".into()],
                    credits: vec![],
                    keywords: vec![],
                    runtime: Some(100),
                    vote_count: Some(10),
                    watchlist: false,
                    sources: vec![RetrievalSource::new(
                        RetrievalKind::SemanticProfile,
                        "profile:global",
                        None,
                    )
                    .with_similarity(0.9 - (i as f32) * 0.001, i as u32 + 1)],
                    friend_affinity: 0.0,
                    tmdb_related: 0.0,
                    media_kind: MediaKind::Movie,
                },
            );
        }
        let selected = select_fair_pool(map, 1000);
        let early_semantic = selected
            .iter()
            .take(250)
            .filter(|c| {
                c.sources
                    .iter()
                    .any(|s| s.kind == RetrievalKind::SemanticProfile)
            })
            .count();
        assert!(
            early_semantic >= 20,
            "semantic profile must occupy early examination slots, found {early_semantic} in @250"
        );
    }

    #[test]
    fn fair_pool_watchlist_does_not_consume_discovery_window() {
        let mut map = HashMap::new();
        for i in 0..300i64 {
            map.insert(
                format!("wl-{i}"),
                Candidate {
                    tmdb_id: Some(1_000 + i),
                    title: format!("Watch {i}"),
                    year: Some(2020),
                    poster: None,
                    genres: vec![],
                    credits: vec![],
                    keywords: vec![],
                    runtime: Some(100),
                    vote_count: Some(10),
                    watchlist: true,
                    sources: vec![RetrievalSource::new(RetrievalKind::Watchlist, "watchlist", None)],
                    friend_affinity: 0.0,
                    tmdb_related: 0.0,
                    media_kind: MediaKind::Movie,
                },
            );
        }
        for i in 0..30i64 {
            map.insert(
                format!("sem-{i}"),
                Candidate {
                    tmdb_id: Some(90_000 + i),
                    title: format!("Semantic {i}"),
                    year: Some(2015),
                    poster: None,
                    genres: vec![],
                    credits: vec![],
                    keywords: vec![],
                    runtime: Some(100),
                    vote_count: Some(10),
                    watchlist: false,
                    sources: vec![RetrievalSource::new(
                        RetrievalKind::SemanticFilmLocal,
                        "seed",
                        Some(1),
                    )
                    .with_similarity(0.85, i as u32 + 1)],
                    friend_affinity: 0.0,
                    tmdb_related: 0.0,
                    media_kind: MediaKind::Movie,
                },
            );
        }
        let selected = select_fair_pool(map, 1000);
        let early_sem = selected
            .iter()
            .take(250)
            .filter(|c| {
                c.sources
                    .iter()
                    .any(|s| s.kind == RetrievalKind::SemanticFilmLocal)
            })
            .count();
        assert!(
            early_sem >= 15,
            "watchlist prefix must not zero out semantic @250, found {early_sem}"
        );
    }

    #[test]
    fn filmography_keeps_only_the_affinity_job() {
        use crate::taste::features::FeatureFamily;
        assert!(keep_person_credit("Director", FeatureFamily::Director));
        assert!(!keep_person_credit("Producer", FeatureFamily::Director));
        assert!(!keep_person_credit("Writer", FeatureFamily::Director));
        assert!(keep_person_credit(
            "Director of Photography",
            FeatureFamily::Cinematographer
        ));
        assert!(!keep_person_credit(
            "Director",
            FeatureFamily::Cinematographer
        ));
        assert!(!keep_person_credit("Actor", FeatureFamily::Cinematographer));
    }

    #[test]
    fn retrieval_takes_neighbors_past_the_old_eight_cap() {
        use crate::storage::db::Database;
        use crate::taste::features::build_profile;
        let db = Database::in_memory().unwrap();
        let profile = build_profile(&[]);
        let mut seed = test_seed("Deep seed", 1, vec![]);
        seed.tmdb_id = Some(1);
        seed.recommendations = (1..=16)
            .map(|i| neighbor(10_000 + i, &format!("Rec {i}")))
            .collect();
        let out = retrieve(&db, &[seed], &profile, &HashSet::new(), false).unwrap();
        assert!(
            out.iter().any(|c| c.tmdb_id == Some(10_009)),
            "9th recommendation must enter the pool after widening retrieval"
        );
        assert!(
            out.iter().any(|c| c.tmdb_id == Some(10_016)),
            "deep stored recommendations must not be truncated at 8"
        );
    }

    #[test]
    fn shallow_legacy_related_lists_need_hydrate() {
        let mut deep = test_seed("Deep", 1, vec![]);
        deep.recommendations = (1..=20)
            .map(|i| neighbor(i, &format!("R{i}")))
            .collect();
        deep.similar = (1..=20)
            .map(|i| neighbor(100 + i, &format!("S{i}")))
            .collect();
        assert!(!needs_related_hydrate(&deep));

        let mut legacy = test_seed("Legacy", 1, vec![]);
        legacy.recommendations = (1..=12)
            .map(|i| neighbor(i, &format!("R{i}")))
            .collect();
        legacy.similar = (1..=12)
            .map(|i| neighbor(100 + i, &format!("S{i}")))
            .collect();
        assert!(
            needs_related_hydrate(&legacy),
            "exactly-12 legacy lists must refresh to the deeper store"
        );

        let mut empty = test_seed("Empty", 1, vec![]);
        empty.recommendations.clear();
        empty.similar.clear();
        assert!(needs_related_hydrate(&empty));
    }

    #[test]
    fn collection_siblings_enter_pool_with_collection_provenance() {
        use crate::storage::db::Database;
        use crate::taste::features::build_profile;
        let db = Database::in_memory().unwrap();
        let mut seed = test_seed("Kubo and the Two Strings", 1, vec![]);
        seed.tmdb_id = Some(407_887);
        seed.collection_name = Some("Laika Collection".into());
        seed.collection = vec![LibraryItem::catalog(
            "tmdb:503314".into(),
            "Missing Link".into(),
            Some(2019),
            None,
            None,
            None,
        )];
        let profile = build_profile(&[]);
        let seen = seen_keys(&[]);
        let result = retrieve_with_coverage(&db, &[seed], &profile, &seen, false).unwrap();
        let hit = result
            .candidates
            .iter()
            .find(|c| c.tmdb_id == Some(503_314))
            .expect("collection sibling should be retrieved");
        assert!(hit
            .sources
            .iter()
            .any(|s| s.kind == RetrievalKind::Collection));
    }

    #[test]
    fn examination_priority_uses_generator_families_not_edge_count() {
        let many_related = Candidate {
            tmdb_id: Some(1),
            title: "Many Related".into(),
            year: Some(2010),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: None,
            vote_count: None,
            watchlist: false,
            sources: vec![
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "a", Some(1)),
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "b", Some(2)),
                RetrievalSource::new(RetrievalKind::RelatedRecommendations, "c", Some(3)),
                RetrievalSource::new(RetrievalKind::RelatedSimilar, "d", Some(4)),
            ],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        };
        let craft_and_collection = Candidate {
            tmdb_id: Some(2),
            title: "Craft+Collection".into(),
            year: Some(2011),
            sources: vec![
                RetrievalSource::new(RetrievalKind::Filmography, "Pattinson", None),
                RetrievalSource::new(RetrievalKind::Collection, "same collection", Some(9)),
            ],
            ..many_related.clone()
        };
        assert!(
            candidate_priority(&craft_and_collection) > candidate_priority(&many_related),
            "distinct generator families should outrank repeated related edges"
        );
    }
}
