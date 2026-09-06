use crate::letterboxd::normalize::{normalize_title, parse_year};
use crate::letterboxd::posters::{
    backdrop_url, backfill_letterboxd_posters, cache_poster_for_siblings, full_poster_url,
    letterboxd_page_metadata, poster_url,
};
use crate::models::{ArtworkImage, EnrichReport, FilmArtwork, FilmTrailer, JobProgress, LibraryItem, SetArtworkInput, TmdbKeyStatus};
use crate::storage::db::Database;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use uuid::Uuid;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmdbMediaType {
    Movie,
    Tv,
}

impl TmdbMediaType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Tv => "tv",
        }
    }

    fn from_raw(value: Option<&str>) -> Self {
        match value {
            Some("tv") => Self::Tv,
            _ => Self::Movie,
        }
    }

    fn endpoint(self) -> &'static str {
        self.as_str()
    }

    fn alternate(self) -> Self {
        match self {
            Self::Movie => Self::Tv,
            Self::Tv => Self::Movie,
        }
    }
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    keyring::Entry::new("studio", "tmdb_api_key")
        .map_err(|e| e.to_string())?
        .set_password(key)
        .map_err(|e| e.to_string())?;
    match get_api_key()? {
        Some(stored) if stored == key => Ok(()),
        Some(_) => Err("Windows Credential Manager stored a different TMDB key than the one just saved".into()),
        None => Err(
            "Windows Credential Manager did not keep the TMDB key. Studio cannot match ZIP films without it."
                .into(),
        ),
    }
}

pub fn store_api_key(key: &str) -> Result<TmdbKeyStatus, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("Paste a TMDB API key first".into());
    }
    let status = probe_key(trimmed)?;
    if status.valid != Some(true) {
        let previous = get_api_key()?.is_some();
        let mut rejected = status;
        rejected.stored = previous;
        if previous {
            rejected.last_error = Some(format!(
                "{}. Previous key is still stored.",
                rejected
                    .last_error
                    .unwrap_or_else(|| "TMDB rejected this key".into())
            ));
        }
        return Ok(rejected);
    }
    set_api_key(trimmed)?;
    Ok(TmdbKeyStatus {
        stored: true,
        valid: Some(true),
        kind: status.kind,
        last_error: None,
    })
}

pub fn key_status() -> Result<TmdbKeyStatus, String> {
    match get_api_key()? {
        Some(key) => {
            let mut status = probe_key(&key)?;
            status.stored = true;
            Ok(status)
        }
        None => Ok(TmdbKeyStatus {
            stored: false,
            valid: None,
            kind: None,
            last_error: None,
        }),
    }
}

fn probe_key(key: &str) -> Result<TmdbKeyStatus, String> {
    let kind = if is_bearer_token(key) {
        "accessToken"
    } else {
        "apiKey"
    };
    match tmdb_get(key, "/configuration") {
        Ok(_) => Ok(TmdbKeyStatus {
            stored: false,
            valid: Some(true),
            kind: Some(kind.into()),
            last_error: None,
        }),
        Err(err) => {
            let invalid =
                err.contains("401") || err.contains("403") || err.contains("Unauthorized");
            Ok(TmdbKeyStatus {
                stored: false,
                valid: if invalid { Some(false) } else { None },
                kind: Some(kind.into()),
                last_error: Some(if invalid {
                    "TMDB rejected this key. Use the API Key (v3) from themoviedb.org/settings/api."
                        .into()
                } else {
                    format!("Could not reach TMDB: {err}")
                }),
            })
        }
    }
}

pub fn get_api_key() -> Result<Option<String>, String> {
    keyring::Entry::new("studio", "tmdb_api_key")
        .map_err(|e| e.to_string())?
        .get_password()
        .map(|k| {
            let trimmed = k.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .or_else(|e| {
            if e.to_string().contains("No matching entry") {
                Ok(None)
            } else {
                Err(e.to_string())
            }
        })
}

pub fn clear_api_key() -> Result<(), String> {
    keyring::Entry::new("studio", "tmdb_api_key")
        .map_err(|e| e.to_string())?
        .set_password("")
        .map_err(|e| e.to_string())
}

fn is_bearer_token(key: &str) -> bool {
    let k = key.trim();
    k.starts_with("eyJ") || (k.len() > 48 && k.bytes().filter(|b| *b == b'.').count() >= 2)
}

fn tmdb_get(key: &str, path_and_query: &str) -> Result<String, String> {
    let key = key.trim();
    let url = if is_bearer_token(key) {
        format!("{TMDB_BASE}{path_and_query}")
    } else {
        let sep = if path_and_query.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{TMDB_BASE}{path_and_query}{sep}api_key={key}")
    };
    let mut req = ureq::get(&url)
        .set("User-Agent", "Studio/0.1 (local film app)")
        .timeout(std::time::Duration::from_secs(8));
    if is_bearer_token(key) {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    let response = req
        .call()
        .map_err(|error| tmdb_request_error(path_and_query, &error.to_string()))?;
    response
        .into_string()
        .map_err(|_| format!("TMDB {}: response read failed", tmdb_path_label(path_and_query)))
}

/// Public wrapper for Taste universe harvest (discover pages, etc.).
pub fn tmdb_get_public(key: &str, path_and_query: &str) -> Result<String, String> {
    tmdb_get(key, path_and_query)
}

fn tmdb_path_label(path_and_query: &str) -> &str {
    path_and_query.split('?').next().unwrap_or(path_and_query)
}

fn tmdb_request_error(path_and_query: &str, error: &str) -> String {
    let status = error
        .split("status code ")
        .nth(1)
        .map(|suffix| suffix.chars().take_while(|character| character.is_ascii_digit()).collect::<String>())
        .filter(|status| !status.is_empty())
        .map(|status| format!("status code {status}"))
        .unwrap_or_else(|| "request failed".into());
    format!("TMDB {}: {status}", tmdb_path_label(path_and_query))
}

fn tmdb_not_found(error: &str) -> bool {
    error.contains("status code 404")
}

fn artwork_paths(primary: Option<&str>, images: &serde_json::Value, kind: &str) -> Vec<String> {
    let mut paths = primary
        .filter(|path| !path.trim().is_empty())
        .map(|path| vec![path.to_string()])
        .unwrap_or_default();
    let mut alternates = artwork_candidates(images, kind, false);
    for (_, _, _, path) in alternates.drain(..) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn artwork_picker_paths(images: &serde_json::Value, kind: &str) -> Vec<String> {
    artwork_candidates(images, kind, true)
        .into_iter()
        .map(|(_, _, _, path)| path)
        .collect()
}

fn artwork_candidates(
    images: &serde_json::Value,
    kind: &str,
    require_high_quality: bool,
) -> Vec<(u8, i64, i64, String)> {
    let mut candidates: Vec<(u8, i64, i64, String)> = images[kind]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let path = image["file_path"].as_str()?.trim();
            let width = image["width"].as_i64().unwrap_or(0);
            let height = image["height"].as_i64().unwrap_or(0);
            (!path.is_empty() && (!require_high_quality || picker_image_is_high_quality(kind, width, height))).then(|| {
                let language_rank = match image["iso_639_1"].as_str() {
                    Some("en") => 0,
                    None => 1,
                    _ => 2,
                };
                let votes = image["vote_count"].as_i64().unwrap_or(0);
                (language_rank, -(width * height), -votes, path.to_string())
            })
        })
        .collect();
    candidates.sort();
    candidates
}

fn picker_image_is_high_quality(kind: &str, width: i64, height: i64) -> bool {
    match kind {
        "posters" => width >= 500 && height >= 750 && height > width,
        "backdrops" => width >= 1280 && height >= 720 && width > height,
        _ => false,
    }
}

fn preferred_artwork_path(
    primary: Option<&str>,
    images: &serde_json::Value,
    kind: &str,
) -> Option<String> {
    artwork_paths(primary, images, kind).into_iter().next()
}

fn artwork_image(path: String, backdrop: bool) -> ArtworkImage {
    let url = if backdrop {
        backdrop_url(Some(path.clone()))
    } else {
        poster_url(Some(path.clone()))
    }
    .unwrap_or(path.clone());
    ArtworkImage { path, url }
}

fn resolve_artwork_movie(
    db: &Database,
    id: &str,
) -> Result<(String, i64, TmdbMediaType, Option<String>, Option<String>, Option<String>, Option<String>), String> {
    let row: (String, Option<i64>, String, Option<String>, Option<String>, Option<String>, Option<String>) = db.conn()
        .query_row(
            r#"
            SELECT m.id, m.tmdb_id, m.tmdb_media_type, m.poster_path, m.backdrop_path,
                   m.poster_override_url, m.backdrop_override_url
            FROM movies m
            LEFT JOIN movie_links ml ON ml.movie_id = m.id
            LEFT JOIN source_movie_records smr ON smr.id = ml.source_movie_record_id
            WHERE m.id = ?1
               OR CAST(m.tmdb_id AS TEXT) = ?1
               OR ('tmdb:' || CAST(m.tmdb_id AS TEXT)) = ?1
               OR smr.id = ?1
               OR (smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')) = ?1
            LIMIT 1
            "#,
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Artwork is available after this film has a TMDB match".to_string())?;
    let Some(tmdb_id) = row.1 else {
        return Err("Artwork is available after this film has a TMDB match".into());
    };
    Ok((
        row.0,
        tmdb_id,
        TmdbMediaType::from_raw(Some(&row.2)),
        row.3,
        row.4,
        row.5,
        row.6,
    ))
}

pub fn film_artwork(db: &Database, id: &str) -> Result<FilmArtwork, String> {
    let (_, tmdb_id, media_type, default_poster, default_backdrop, selected_poster, selected_backdrop) =
        resolve_artwork_movie(db, id)?;
    let api_key = get_api_key()?.ok_or("Add a TMDB key in Settings to browse artwork")?;
    let body = tmdb_get(
        &api_key,
        &format!("/{}/{tmdb_id}/images?include_image_language=en,null", media_type.endpoint()),
    )?;
    let images: serde_json::Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    Ok(FilmArtwork {
        posters: artwork_picker_paths(&images, "posters")
            .into_iter()
            .map(|path| artwork_image(path, false))
            .collect(),
        backdrops: artwork_picker_paths(&images, "backdrops")
            .into_iter()
            .map(|path| artwork_image(path, true))
            .collect(),
        selected_poster: selected_poster.or(default_poster.clone()),
        selected_backdrop: selected_backdrop.or(default_backdrop.clone()),
        default_poster,
        default_backdrop,
    })
}

fn normalize_artwork_override(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.starts_with('/') || value.starts_with("https://") {
        return Ok(Some(value));
    }
    Err("Artwork must be a TMDB image choice or an HTTPS image URL".into())
}

pub fn set_film_artwork(db: &Database, input: &SetArtworkInput) -> Result<(), String> {
    let (movie_id, _, _, _, _, _, _) = resolve_artwork_movie(db, &input.id)?;
    let poster = normalize_artwork_override(input.poster.clone())?;
    let backdrop = normalize_artwork_override(input.backdrop.clone())?;
    db.conn()
        .execute(
            "UPDATE movies SET poster_override_url = ?2, backdrop_override_url = ?3 WHERE id = ?1",
            params![movie_id, poster, backdrop],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Deserialize)]
struct TmdbSearchResponse {
    results: Vec<TmdbSearchResult>,
}

#[derive(Deserialize)]
struct TmdbTvSearchResponse {
    results: Vec<TmdbTvSearchResult>,
}

#[derive(Deserialize)]
struct TmdbSearchResult {
    id: i64,
    title: String,
    original_title: Option<String>,
    release_date: Option<String>,
    poster_path: Option<String>,
    #[serde(default)]
    vote_count: u32,
}

#[derive(Clone, Deserialize)]
struct TmdbTvSearchResult {
    id: i64,
    name: String,
    original_name: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MovieLookup {
    pub tmdb_id: i64,
    #[allow(dead_code)]
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
}

pub fn lookup_movie(title: &str, year: Option<i32>) -> Result<Option<MovieLookup>, String> {
    let Some(api_key) = get_api_key()? else {
        return Ok(None);
    };
    let normalized = normalize_title(title);
    let mut parsed = search_movies(&api_key, title, year)?;
    if pick_search_match(&parsed.results, &normalized, year).is_none() && year.is_some() {
        parsed = search_movies(&api_key, title, None)?;
    }
    let hit = pick_search_match(&parsed.results, &normalized, year);
    Ok(hit.map(|r| MovieLookup {
        tmdb_id: r.id,
        title: r.title.clone(),
        year: release_year(r),
        poster: poster_url(r.poster_path.clone()),
    }))
}

pub fn enrich_catalog(db: &Database) -> Result<EnrichReport, String> {
    enrich_catalog_with(db, &mut |_| {})
}

pub fn enrich_catalog_with(
    db: &Database,
    progress: &mut dyn FnMut(JobProgress),
) -> Result<EnrichReport, String> {
    let mut report = EnrichReport {
        has_key: false,
        key_valid: None,
        attempted: 0,
        matched: 0,
        posters: 0,
        remaining_unmatched: 0,
        remaining_without_poster: 0,
        errors: 0,
        last_error: None,
        log_path: None,
    };

    if let Some(api_key) = get_api_key()? {
        report.has_key = true;
        match tmdb_get(&api_key, "/configuration") {
            Ok(_) => report.key_valid = Some(true),
            Err(err) => {
                report.key_valid = Some(false);
                report.last_error = Some(err);
            }
        }
        if report.key_valid == Some(true) {
            let queue = list_unmatched(db)?;
            let total = queue.len() as u32;
            for (smr_id, raw, normalized, year, external_id) in queue {
                report.attempted += 1;
                progress(JobProgress {
                    job: "enrich".into(),
                    label: match_progress_label(report.attempted, total.max(1)),
                    current: report.attempted,
                    total: total.max(1),
                    posters: report.posters,
                    errors: report.errors,
                    done: false,
                    ..Default::default()
                });
                let had_poster = source_has_poster(db, &smr_id)?;
                match enrich_one_film(
                    db,
                    &api_key,
                    &smr_id,
                    &raw,
                    &normalized,
                    year,
                    external_id.as_deref(),
                ) {
                    Ok(true) => {
                        report.matched += 1;
                        let has_poster = source_has_poster(db, &smr_id)?;
                        if !had_poster && has_poster {
                            report.posters += 1;
                        }
                        if !has_poster {
                            mark_tmdb_checked(db, &smr_id)?;
                        }
                    }
                    Ok(false) => {
                        mark_tmdb_checked(db, &smr_id)?;
                        if !had_poster && source_has_poster(db, &smr_id)? {
                            report.posters += 1;
                        }
                    }
                    Err(err) => {
                        report.errors += 1;
                        if tmdb_not_found(&err) {
                            mark_tmdb_checked(db, &smr_id)?;
                        }
                        report.last_error = Some(err);
                    }
                }
            }

            // Poster recovery and metadata hydration have different stop
            // conditions. A trusted source cover can exist before its TMDB
            // detail record was filled, so repair those gaps independently.
            for (tmdb_id, media_type) in list_metadata_gaps(db)? {
                report.attempted += 1;
                progress(JobProgress {
                    job: "enrich".into(),
                    label: format!("Hydrating details · {}", report.attempted),
                    current: report.attempted,
                    total: report.attempted.max(1),
                    posters: report.posters,
                    errors: report.errors,
                    done: false,
                    ..Default::default()
                });
                if let Err(error) = upsert_tmdb(db, tmdb_id, media_type) {
                    report.errors += 1;
                    report.last_error = Some(error);
                }
            }
        }
    }

    let _ = db.conn().execute(
        "UPDATE source_movie_records SET poster_fetch_failed = 0
         WHERE cached_poster_url IS NULL OR TRIM(cached_poster_url) = ''",
        [],
    );

    for _ in 0..12 {
        match backfill_letterboxd_posters(db, 40) {
            Ok(batch) => {
                report.posters += batch.updated;
                progress(JobProgress {
                    job: "enrich".into(),
                    label: format!("Letterboxd posters · {} fetched", report.posters),
                    current: report.posters,
                    total: report.posters.max(1),
                    posters: report.posters,
                    errors: report.errors,
                    done: false,
                    ..Default::default()
                });
                if batch.attempted == 0 || batch.resolved == 0 {
                    break;
                }
            }
            Err(err) => {
                report.errors += 1;
                report.last_error = Some(err);
                break;
            }
        }
    }

    report.remaining_unmatched = count_unmatched(db)?;
    report.remaining_without_poster = count_without_poster(db)?;
    progress(JobProgress {
        job: "enrich".into(),
        label: "Catalog pass complete".into(),
        current: report.attempted,
        total: report.attempted.max(1),
        posters: report.posters,
        errors: report.errors,
        done: false,
        ..Default::default()
    });
    Ok(report)
}

pub fn enrich_unmatched(db: &Database) -> Result<EnrichReport, String> {
    enrich_catalog(db)
}

fn count_unmatched(db: &Database) -> Result<u32, String> {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM movie_links WHERE match_state IN ('unmatched', 'ambiguous')",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|e| e.to_string())
}

fn count_without_poster(db: &Database) -> Result<u32, String> {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM source_movie_records smr
             LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
             LEFT JOIN movies m ON m.id = ml.movie_id
             WHERE (smr.cached_poster_url IS NULL OR TRIM(smr.cached_poster_url) = '')
               AND (m.poster_override_url IS NULL OR TRIM(m.poster_override_url) = '')
               AND (m.poster_path IS NULL OR TRIM(m.poster_path) = '')
               AND (smr.source_type != 'letterboxd_rss'
                    OR LOWER(COALESCE(smr.external_id, '')) LIKE '%/film/%')",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|e| e.to_string())
}

fn source_has_poster(db: &Database, smr_id: &str) -> Result<bool, String> {
    db.conn()
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM source_movie_records smr
               LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
               LEFT JOIN movies m ON m.id = ml.movie_id
               WHERE smr.id = ?1
                 AND (
                   COALESCE(TRIM(smr.cached_poster_url), '') != ''
                   OR COALESCE(TRIM(m.poster_override_url), '') != ''
                   OR COALESCE(TRIM(m.poster_path), '') != ''
                 )
             )",
            params![smr_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .map_err(|error| error.to_string())
}

fn list_metadata_gaps(db: &Database) -> Result<Vec<(i64, TmdbMediaType)>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            // Immediate backfill when a row already has catalog detail but is
            // missing a newer column (e.g. videos_json). Keep the 7-day backoff
            // for deferred unavailable titles, which set enriched_at with no
            // detail payloads.
            "SELECT DISTINCT tmdb_id, tmdb_media_type
             FROM movies
             WHERE tmdb_id IS NOT NULL
               AND (
                 (
                   videos_json IS NULL
                   AND (genres_json IS NOT NULL OR credits_json IS NOT NULL)
                 )
                 OR (
                   (enriched_at IS NULL OR datetime(enriched_at) < datetime('now', '-7 days'))
                   AND (
                     enriched_at IS NULL
                     OR genres_json IS NULL
                     OR credits_json IS NULL
                     OR production_companies_json IS NULL
                     OR keywords_json IS NULL
                     OR similar_json IS NULL
                     OR videos_json IS NULL
                   )
                 )
               )",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|error| error.to_string())?
        .filter_map(|row| row.ok())
        .map(|(id, kind)| (id, TmdbMediaType::from_raw(Some(&kind))))
        .collect();
    Ok(rows)
}

fn mark_tmdb_checked(db: &Database, smr_id: &str) -> Result<(), String> {
    db.conn()
        .execute(
            "UPDATE movie_links SET tmdb_checked_at = datetime('now') WHERE source_movie_record_id = ?1",
            params![smr_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn match_progress_label(current: u32, total: u32) -> String {
    format!("Checking title matches · {current} of {total}")
}

fn list_unmatched(
    db: &Database,
) -> Result<Vec<(String, String, String, Option<i32>, Option<String>)>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT smr.id, smr.raw_identity, smr.normalized_title, smr.release_year, smr.external_id
             FROM source_movie_records smr
             JOIN movie_links ml ON ml.source_movie_record_id = smr.id
             LEFT JOIN movies m ON m.id = ml.movie_id
             WHERE (
                  (
                    ml.match_state IN ('unmatched', 'ambiguous')
                    OR (
                      m.tmdb_id IS NOT NULL
                      AND (m.poster_override_url IS NULL OR TRIM(m.poster_override_url) = '')
                      AND (m.poster_path IS NULL OR TRIM(m.poster_path) = '')
                      AND (smr.cached_poster_url IS NULL OR TRIM(smr.cached_poster_url) = '')
                    )
                  )
                  AND (ml.tmdb_checked_at IS NULL OR datetime(ml.tmdb_checked_at) < datetime('now', '-7 days'))
             )
               AND (smr.source_type != 'letterboxd_rss'
                    OR LOWER(COALESCE(smr.external_id, '')) LIKE '%/film/%')",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn enrich_one_film(
    db: &Database,
    api_key: &str,
    smr_id: &str,
    raw: &str,
    _normalized: &str,
    year: Option<i32>,
    external_id: Option<&str>,
) -> Result<bool, String> {
    if let Some((movie_id, tmdb_id, media_type)) = try_tmdb_id_from_raw(db, smr_id, raw)? {
        if let Some(poster) = cache_missing_tmdb_artwork(db, api_key, &movie_id, tmdb_id, media_type)? {
            cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
        }
        return Ok(true);
    }
    if let Some((tmdb_id, media_type)) = linked_tmdb(db, smr_id)? {
        let movie_id = upsert_tmdb(db, tmdb_id, media_type)?;
        confirm_link(db, smr_id, &movie_id, "tmdb_artwork_refresh")?;
        if let Some(poster) = cache_missing_tmdb_artwork(db, api_key, &movie_id, tmdb_id, media_type)? {
            cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
        }
        return Ok(true);
    }
    let (title, lookup_year) = title_and_embedded_year(&parse_raw_title(raw), year);
    let lookup_normalized = normalize_title(&title);
    if try_exact_match(db, smr_id, &lookup_normalized, lookup_year)? {
        return Ok(true);
    }
    if search_and_queue(db, api_key, smr_id, &title, &lookup_normalized, lookup_year)? > 0 {
        return Ok(true);
    }

    // Title matching is deliberately conservative. When it cannot identify a
    // Letterboxd film, its page's explicit TMDB link lets the normal TMDB
    // detail path hydrate cast, crew, genres, and artwork.
    if let Some(uri) = external_id {
        // Provider hiccups must not stop the conservative TMDB and TV fallbacks
        // below. They remain eligible for the no-key poster backfill afterwards.
        if let Ok(source_metadata) = letterboxd_page_metadata(uri) {
            if let Some(poster) = source_metadata.poster.as_deref() {
                cache_poster_for_siblings(db.conn(), smr_id, poster)?;
            }
            if let Some(tmdb_id) = source_metadata.tmdb_id {
                let media_type = TmdbMediaType::from_raw(source_metadata.tmdb_media_type.as_deref());
                let movie_id = upsert_tmdb(db, tmdb_id, media_type)?;
                confirm_link(db, smr_id, &movie_id, "letterboxd_tmdb_link")?;
                if let Some(poster) = cache_missing_tmdb_artwork(db, api_key, &movie_id, tmdb_id, media_type)? {
                    cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
                }
                return Ok(true);
            }
        }
    }

    if let Some(tv_match) = search_tv_fallback(api_key, &title, &lookup_normalized, lookup_year)? {
        match tv_match {
            TvSearchMatch::Exact(result) => {
                let movie_id = upsert_tmdb(db, result.id, TmdbMediaType::Tv)?;
                confirm_link(db, smr_id, &movie_id, "tmdb_tv_search")?;
                if let Some(poster) = cache_missing_tmdb_artwork(
                    db,
                    api_key,
                    &movie_id,
                    result.id,
                    TmdbMediaType::Tv,
                )? {
                    cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
                }
                return Ok(true);
            }
            TvSearchMatch::ParentSeries(result) => {
                let movie_id = upsert_tmdb(db, result.id, TmdbMediaType::Tv)?;
                confirm_link(db, smr_id, &movie_id, "tmdb_tv_parent_series")?;
                if let Some(poster) = cache_missing_tmdb_artwork(
                    db,
                    api_key,
                    &movie_id,
                    result.id,
                    TmdbMediaType::Tv,
                )? {
                    cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
                }
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn linked_tmdb(db: &Database, smr_id: &str) -> Result<Option<(i64, TmdbMediaType)>, String> {
    let row: Option<(Option<i64>, String)> = db.conn()
        .query_row(
            "SELECT m.tmdb_id, m.tmdb_media_type FROM movie_links ml
             JOIN movies m ON m.id = ml.movie_id
            WHERE ml.source_movie_record_id = ?1",
            params![smr_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(row.and_then(|(id, kind)| id.map(|id| (id, TmdbMediaType::from_raw(Some(&kind))))))
}

fn try_tmdb_id_from_raw(
    db: &Database,
    smr_id: &str,
    raw: &str,
) -> Result<Option<(String, i64, TmdbMediaType)>, String> {
    let Some(value) = serde_json::from_str::<serde_json::Value>(raw).ok() else {
        return Ok(None);
    };
    let Some(id) = value.get("tmdb_id").and_then(|x| x.as_i64())
        .filter(|&id| id > 0)
    else {
        return Ok(None);
    };
    let media_type = TmdbMediaType::from_raw(value.get("tmdb_media_type").and_then(|x| x.as_str()));
    let movie_id = upsert_tmdb(db, id, media_type)?;
    confirm_link(db, smr_id, &movie_id, "export_tmdb_id")?;
    Ok(Some((movie_id, id, media_type)))
}

fn confirm_link(db: &Database, smr_id: &str, movie_id: &str, method: &str) -> Result<(), String> {
    db.conn()
        .execute(
            "UPDATE movie_links SET movie_id = ?2, match_state = 'confirmed',
             match_method = ?3, confidence = 1.0, confirmed_at = datetime('now')
             WHERE source_movie_record_id = ?1",
            params![smr_id, movie_id, method],
        )
        .map_err(|e| e.to_string())?;
    propagate_movie_link(db, smr_id, movie_id)?;
    Ok(())
}

fn propagate_movie_link(db: &Database, smr_id: &str, movie_id: &str) -> Result<(), String> {
    let (normalized, year): (String, Option<i32>) = db
        .conn()
        .query_row(
            "SELECT normalized_title, release_year FROM source_movie_records WHERE id = ?1",
            params![smr_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;

    db.conn()
        .execute(
            "UPDATE movie_links SET movie_id = ?3, match_state = 'confirmed',
             match_method = 'propagated', confidence = 1.0, confirmed_at = datetime('now')
             WHERE source_movie_record_id IN (
               SELECT id FROM source_movie_records
               WHERE normalized_title = ?1 AND release_year IS ?2
             ) AND match_state != 'confirmed'",
            params![normalized, year, movie_id],
        )
        .map_err(|e| e.to_string())?;

    let poster_path = db
        .conn()
        .query_row(
            "SELECT poster_path FROM movies WHERE id = ?1",
            params![movie_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|path| path.flatten())
        .map_err(|e| e.to_string())?;

    if let Some(path) = poster_path.filter(|p| !p.is_empty()) {
        cache_poster_for_siblings(db.conn(), smr_id, &full_poster_url(&path))?;
    }
    Ok(())
}

/// The movie-details response occasionally has no primary poster even though
/// TMDB's dedicated artwork endpoint does. This performs that narrower fetch
/// only for a confirmed movie still lacking a poster, and leaves any existing
/// poster or custom override untouched.
fn cache_missing_tmdb_artwork(
    db: &Database,
    api_key: &str,
    movie_id: &str,
    tmdb_id: i64,
    media_type: TmdbMediaType,
) -> Result<Option<String>, String> {
    let media_type = db
        .conn()
        .query_row(
            "SELECT tmdb_media_type FROM movies WHERE id = ?1",
            params![movie_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .map(|kind| TmdbMediaType::from_raw(Some(&kind)))
        .unwrap_or(media_type);
    let has_poster: bool = db
        .conn()
        .query_row(
            "SELECT COALESCE(NULLIF(TRIM(COALESCE(poster_override_url, poster_path, '')), ''), '') != ''
             FROM movies WHERE id = ?1",
            params![movie_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if has_poster {
        return Ok(None);
    }

    let response = tmdb_get(
        api_key,
        &format!("/{}/{tmdb_id}/images?include_image_language=en,null", media_type.endpoint()),
    )?;
    let images: serde_json::Value =
        serde_json::from_str(&response).map_err(|error| error.to_string())?;
    let poster = preferred_artwork_path(None, &images, "posters");
    let backdrop = preferred_artwork_path(None, &images, "backdrops");
    if poster.is_none() && backdrop.is_none() {
        return Ok(None);
    }

    db.conn()
        .execute(
            "UPDATE movies
             SET poster_path = COALESCE(NULLIF(TRIM(poster_path), ''), ?2),
                 backdrop_path = COALESCE(NULLIF(TRIM(backdrop_path), ''), ?3)
             WHERE id = ?1",
            params![movie_id, poster, backdrop],
        )
        .map_err(|error| error.to_string())?;
    Ok(poster.map(|path| full_poster_url(&path)))
}

fn try_exact_match(
    db: &Database,
    smr_id: &str,
    normalized: &str,
    year: Option<i32>,
) -> Result<bool, String> {
    let movie_id: Option<String> = db
        .conn()
        .query_row(
            "SELECT movie_id FROM movie_aliases WHERE normalized_title = ?1 AND release_year IS ?2 LIMIT 1",
            params![normalized, year],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(mid) = movie_id {
        confirm_link(db, smr_id, &mid, "exact_title_year")?;
        return Ok(true);
    }
    Ok(false)
}

fn search_movies(
    api_key: &str,
    title: &str,
    year: Option<i32>,
) -> Result<TmdbSearchResponse, String> {
    let mut path = format!("/search/movie?query={}", percent_encode(title));
    if let Some(y) = year {
        path.push_str(&format!("&primary_release_year={y}"));
    }
    let response = tmdb_get(api_key, &path)?;
    serde_json::from_str(&response).map_err(|e| e.to_string())
}

fn search_tv(
    api_key: &str,
    title: &str,
    year: Option<i32>,
) -> Result<TmdbTvSearchResponse, String> {
    let mut path = format!("/search/tv?query={}", percent_encode(title));
    if let Some(y) = year {
        path.push_str(&format!("&first_air_date_year={y}"));
    }
    let response = tmdb_get(api_key, &path)?;
    serde_json::from_str(&response).map_err(|e| e.to_string())
}

enum TvSearchMatch {
    Exact(TmdbTvSearchResult),
    ParentSeries(TmdbTvSearchResult),
}

fn search_tv_fallback(
    api_key: &str,
    title: &str,
    normalized: &str,
    year: Option<i32>,
) -> Result<Option<TvSearchMatch>, String> {
    let mut parsed = search_tv(api_key, title, year)?;
    if pick_tv_match(&parsed.results, normalized, year).is_none() && year.is_some() {
        parsed = search_tv(api_key, title, None)?;
    }
    if let Some(result) = pick_tv_match(&parsed.results, normalized, year) {
        return Ok(Some(TvSearchMatch::Exact(result.clone())));
    }

    // TMDB does not index every TV special by its full Letterboxd title. For
    // likely specials only, search a few distinctive title words and retain a
    // result solely when the parent-series check below still proves that its
    // complete series title appears in the source title.
    let mut candidates = Vec::new();
    for query in tv_parent_search_queries(normalized) {
        let results = search_tv(api_key, &query, None)?;
        if let Some(result) = pick_tv_match(&results.results, normalized, year) {
            if !candidates
                .iter()
                .any(|candidate: &TmdbTvSearchResult| candidate.id == result.id)
            {
                candidates.push(result.clone());
            }
        }
    }
    Ok((candidates.len() == 1).then(|| TvSearchMatch::ParentSeries(candidates.remove(0))))
}

fn search_and_queue(
    db: &Database,
    api_key: &str,
    smr_id: &str,
    title: &str,
    normalized: &str,
    year: Option<i32>,
) -> Result<u32, String> {
    let mut parsed = search_movies(api_key, title, year)?;
    if pick_search_match(&parsed.results, normalized, year).is_none() && year.is_some() {
        parsed = search_movies(api_key, title, None)?;
    }

    if let Some(result) = pick_search_match(&parsed.results, normalized, year) {
        let movie_id = upsert_tmdb_movie(db, result.id)?;
        let search_poster = result
            .poster_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(full_poster_url);
        confirm_link(db, smr_id, &movie_id, "tmdb_search")?;
        let artwork_poster = cache_missing_tmdb_artwork(
            db,
            api_key,
            &movie_id,
            result.id,
            TmdbMediaType::Movie,
        )?;
        // Search results carry poster art even when the detail endpoint's
        // artwork payload is temporarily incomplete. Save that visual fallback
        // without weakening the title/year match that selected the movie.
        if let Some(poster) = search_poster.or(artwork_poster) {
            cache_poster_for_siblings(db.conn(), smr_id, &poster)?;
        }
        return Ok(1);
    }

    let state = if parsed.results.is_empty() {
        "unmatched"
    } else {
        "ambiguous"
    };
    db.conn()
        .execute(
            "UPDATE movie_links SET match_state = ?2, match_method = 'tmdb_search',
             confidence = NULL, confirmed_at = NULL WHERE source_movie_record_id = ?1",
            params![smr_id, state],
        )
        .map_err(|e| e.to_string())?;
    Ok(0)
}

fn pick_search_match<'a>(
    results: &'a [TmdbSearchResult],
    normalized: &str,
    year: Option<i32>,
) -> Option<&'a TmdbSearchResult> {
    let exact_year: Vec<_> = results
        .iter()
        .filter(|r| search_result_title_matches(r, normalized) && release_year(r) == year)
        .collect();
    if let Some(result) = preferred_search_match(&exact_year) {
        return Some(result);
    }

    let title_only: Vec<_> = results
        .iter()
        .filter(|r| search_result_title_matches(r, normalized))
        .collect();
    if let Some(result) = preferred_search_match(&title_only) {
        return Some(result);
    }

    if let Some(y) = year {
        let near: Vec<_> = results
            .iter()
            .filter(|r| {
                search_result_title_matches(r, normalized)
                    && release_year(r)
                        .map(|ry| (ry - y).abs() <= 1)
                        .unwrap_or(false)
            })
            .collect();
        if let Some(result) = preferred_search_match(&near) {
            return Some(result);
        }
    }

    None
}

/// TMDB sometimes returns duplicate records with the same title and release
/// year. When community validation produces a clear winner, prefer it over
/// leaving an otherwise exact film unresolved.
fn preferred_search_match<'a>(candidates: &[&'a TmdbSearchResult]) -> Option<&'a TmdbSearchResult> {
    if candidates.len() == 1 {
        return candidates.first().copied();
    }
    let mut ranked = candidates.to_vec();
    ranked.sort_by_key(|candidate| std::cmp::Reverse(candidate.vote_count));
    let top = ranked.first().copied()?;
    let runner_up = ranked.get(1).copied()?;
    (top.vote_count > runner_up.vote_count).then_some(top)
}

fn titles_match(candidate: &str, normalized: &str) -> bool {
    match_title_key(candidate) == match_title_key(normalized)
}

/// Comparison-only normalization for search results. Stored titles retain their
/// original punctuation, while matching treats punctuation and whitespace-only
/// variations as the same title.
fn match_title_key(title: &str) -> String {
    normalize_title(title)
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn search_result_title_matches(result: &TmdbSearchResult, normalized: &str) -> bool {
    title_matches_or_subtitled_variant(&result.title, normalized)
        || result
            .original_title
            .as_deref()
            .is_some_and(|title| title_matches_or_subtitled_variant(title, normalized))
}

// Some Letterboxd entries use a film's short title while TMDB records its
// official colon subtitle. Treat that as a match only when the base title is
// exact; this deliberately does not accept looser word-prefix matches.
fn title_matches_or_subtitled_variant(candidate: &str, normalized: &str) -> bool {
    let candidate_key = match_title_key(candidate);
    let normalized_key = match_title_key(normalized);
    candidate_key == normalized_key
        || candidate
            .trim()
            .to_lowercase()
            .strip_prefix(normalized.trim().to_lowercase().as_str())
            .is_some_and(|suffix| suffix.starts_with(':'))
}

fn pick_tv_match<'a>(
    results: &'a [TmdbTvSearchResult],
    normalized: &str,
    year: Option<i32>,
) -> Option<&'a TmdbTvSearchResult> {
    let exact_year: Vec<_> = results
        .iter()
        .filter(|result| tv_result_title_matches(result, normalized) && tv_release_year(result) == year)
        .collect();
    if exact_year.len() == 1 {
        return Some(exact_year[0]);
    }

    let title_only: Vec<_> = results
        .iter()
        .filter(|result| tv_result_title_matches(result, normalized))
        .collect();
    if title_only.len() == 1 {
        return Some(title_only[0]);
    }

    if let Some(y) = year {
        let near: Vec<_> = results
            .iter()
            .filter(|result| {
                tv_result_title_matches(result, normalized)
                    && tv_release_year(result)
                        .map(|ry| (ry - y).abs() <= 1)
                        .unwrap_or(false)
            })
            .collect();
        if near.len() == 1 {
            return Some(near[0]);
        }
    }

    // A Letterboxd entry can be a one-off special or episode while TMDB only
    // exposes artwork for its parent series. A unique, distinctive series-name
    // subset is safe to use as artwork-only fallback; it never creates a movie
    // link or changes the record's media type.
    let parent_series: Vec<_> = results
        .iter()
        .filter(|result| tv_series_title_is_in_special(result, normalized))
        .collect();
    if parent_series.len() == 1 {
        return Some(parent_series[0]);
    }
    None
}

fn tv_result_title_matches(result: &TmdbTvSearchResult, normalized: &str) -> bool {
    titles_match(&result.name, normalized)
        || result
            .original_name
            .as_deref()
            .is_some_and(|title| titles_match(title, normalized))
}

fn tv_series_title_is_in_special(result: &TmdbTvSearchResult, normalized: &str) -> bool {
    let source_tokens = distinctive_title_tokens(normalized);
    if source_tokens.len() < 2 {
        return false;
    }
    [Some(result.name.as_str()), result.original_name.as_deref()]
        .into_iter()
        .flatten()
        .map(distinctive_title_tokens)
        .any(|series_tokens| {
            series_tokens.len() >= 2
                && series_tokens
                    .iter()
                    .all(|token| source_tokens.contains(token))
        })
}

fn distinctive_title_tokens(title: &str) -> Vec<String> {
    match_title_key(title)
        .split_whitespace()
        .filter(|token| {
            token.len() > 1
                && !matches!(
                    *token,
                    "a" | "an" | "and" | "the" | "of" | "for" | "to" | "in" | "on" | "with"
                        | "movie" | "film" | "special" | "episode" | "part"
                )
        })
        .map(str::to_string)
        .collect()
}

fn tv_parent_search_queries(normalized: &str) -> Vec<String> {
    let title_key = match_title_key(normalized);
    if !title_key.split_whitespace().any(|token| matches!(token, "special" | "episode")) {
        return Vec::new();
    }

    let mut tokens = distinctive_title_tokens(&title_key);
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    let mut queries = Vec::new();
    for token in tokens {
        if !queries.contains(&token) {
            queries.push(token);
        }
        if queries.len() == 4 {
            break;
        }
    }
    queries
}

fn release_year(result: &TmdbSearchResult) -> Option<i32> {
    result
        .release_date
        .as_deref()
        .and_then(|d| d.get(0..4))
        .and_then(parse_year)
}

fn tv_release_year(result: &TmdbTvSearchResult) -> Option<i32> {
    result
        .first_air_date
        .as_deref()
        .and_then(|date| date.get(0..4))
        .and_then(parse_year)
}

fn upsert_tmdb_movie(db: &Database, tmdb_id: i64) -> Result<String, String> {
    refresh_movie_catalog(db, tmdb_id, false)
}

pub fn refresh_movie_catalog(db: &Database, tmdb_id: i64, force: bool) -> Result<String, String> {
    refresh_tmdb_catalog(db, tmdb_id, TmdbMediaType::Movie, force)
}

fn upsert_tmdb(db: &Database, tmdb_id: i64, media_type: TmdbMediaType) -> Result<String, String> {
    refresh_tmdb_catalog(db, tmdb_id, media_type, false)
}

fn refresh_tmdb_catalog(
    db: &Database,
    tmdb_id: i64,
    media_type: TmdbMediaType,
    force: bool,
) -> Result<String, String> {
    refresh_tmdb_catalog_with_fallback(db, tmdb_id, media_type, force, true)
}

fn refresh_tmdb_catalog_with_fallback(
    db: &Database,
    tmdb_id: i64,
    media_type: TmdbMediaType,
    force: bool,
    allow_alternate_type: bool,
) -> Result<String, String> {
    if !force {
        if let Some((
            existing_id,
            poster,
            collection_json,
            genres_json,
            credits_json,
            production_companies_json,
            keywords_json,
            similar_json,
            videos_json,
            fresh,
        )) = db
            .conn()
            .query_row(
                "SELECT id, COALESCE(poster_override_url, poster_path, ''), collection_json,
                        genres_json, credits_json, production_companies_json, keywords_json, similar_json,
                        videos_json,
                        CASE WHEN enriched_at IS NOT NULL
                              AND datetime(enriched_at) >= datetime('now', '-30 days')
                             THEN 1 ELSE 0 END
                 FROM movies WHERE tmdb_id = ?1 AND tmdb_media_type = ?2",
                params![tmdb_id, media_type.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            if !poster.is_empty()
                && collection_json.is_some()
                && genres_json.is_some()
                && credits_json.is_some()
                && keywords_json.is_some()
                && similar_json.is_some()
                && production_companies_json.is_some()
                && videos_json.is_some()
                && fresh == 1
            {
                return Ok(existing_id);
            }
        }
    }

    let api_key = get_api_key()?.ok_or("missing api key")?;
    let path = tmdb_details_path(tmdb_id, media_type);
    let body = match tmdb_get(&api_key, &path) {
        Ok(body) => body,
        Err(error) if allow_alternate_type && tmdb_not_found(&error) => {
            let alternate = media_type.alternate();
            match tmdb_get(&api_key, &tmdb_details_path(tmdb_id, alternate)) {
                Ok(_) => {
                    reclassify_tmdb_media_type(db, tmdb_id, media_type, alternate)?;
                    return refresh_tmdb_catalog_with_fallback(db, tmdb_id, alternate, force, false);
                }
                Err(alternate_error) => {
                    if tmdb_not_found(&alternate_error) {
                        defer_unavailable_tmdb_metadata(db, tmdb_id)?;
                    }
                    return Err(alternate_error);
                }
            }
        }
        Err(error) => {
            if tmdb_not_found(&error) {
                defer_unavailable_tmdb_metadata(db, tmdb_id)?;
            }
            return Err(error);
        }
    };
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let title = if media_type == TmdbMediaType::Tv {
        v["name"].as_str()
    } else {
        v["title"].as_str()
    }
    .unwrap_or("Unknown")
    .to_string();
    let year = if media_type == TmdbMediaType::Tv {
        v["first_air_date"].as_str()
    } else {
        v["release_date"].as_str()
    }
        .and_then(|d| d.get(0..4))
        .and_then(parse_year);
    let poster = preferred_artwork_path(v["poster_path"].as_str(), &v["images"], "posters")
        .unwrap_or_default();
    let tagline = v["tagline"]
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let (collection_name, collection_items) = if media_type == TmdbMediaType::Movie {
        collection_from_movie(&api_key, &v)
    } else {
        (None, Vec::new())
    };
    let (recommendations, similar) = related_lists(&v);
    let existing_id: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM movies WHERE tmdb_id = ?1 AND tmdb_media_type = ?2",
            params![tmdb_id, media_type.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let id = existing_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let poster_store = if poster.is_empty() {
        None::<String>
    } else {
        Some(poster)
    };
    let backdrop = preferred_artwork_path(v["backdrop_path"].as_str(), &v["images"], "backdrops");
    let overview = v["overview"].as_str().map(str::to_string);
    let runtime = v["runtime"].as_i64().or_else(|| {
        v["episode_run_time"]
            .as_array()
            .and_then(|runtimes| runtimes.first())
            .and_then(|runtime| runtime.as_i64())
    });
    let vote_average = v["vote_average"].as_f64();
    let vote_count = v["vote_count"].as_i64();
    let genres = serde_json::to_string(&genre_names(&v)).unwrap_or_else(|_| "[]".into());
    let cast = serde_json::to_string(&cast_names(&v)).unwrap_or_else(|_| "[]".into());
    let crew = serde_json::to_string(&crew_names(&v)).unwrap_or_else(|_| "[]".into());
    let similar_json = serde_json::to_string(&serde_json::json!({
        "recommendations": recommendations,
        "similar": similar,
    }))
    .unwrap_or_else(|_| r#"{"recommendations":[],"similar":[]}"#.into());
    let reviews = serde_json::to_string(&review_authors(&v)).unwrap_or_else(|_| "[]".into());
    let collection_json = serde_json::to_string(&collection_items).unwrap_or_else(|_| "[]".into());
    let keywords_json = serde_json::to_string(&keyword_entries(&v)).unwrap_or_else(|_| "[]".into());
    let credits_blob = serde_json::to_string(&credit_entries(&v)).unwrap_or_else(|_| "{}".into());
    let production_companies_json = serde_json::to_string(&production_companies(&v))
        .unwrap_or_else(|_| "[]".into());
    let videos_json = serde_json::to_string(&video_entries(&v)).unwrap_or_else(|_| "[]".into());

    if existing_id.is_some() {
        db.conn()
            .execute(
                "UPDATE movies SET canonical_title = ?2, release_year = ?3, tmdb_id = ?4, tmdb_media_type = ?5, poster_path = ?6,
                 backdrop_path = ?7, overview = ?8, runtime = ?9, vote_average = ?10, vote_count = ?11,
                 genres_json = ?12, cast_json = ?13, crew_json = ?14, similar_json = ?15,
                 reviews_json = ?16, tagline = ?17, collection_name = ?18, collection_json = ?19,
                 keywords_json = ?20, credits_json = ?21, production_companies_json = ?22,
                 videos_json = ?23,
                 enriched_at = datetime('now')
                 WHERE id = ?1",
                params![
                    id,
                    title,
                    year,
                    tmdb_id,
                    media_type.as_str(),
                    poster_store,
                    backdrop,
                    overview,
                    runtime,
                    vote_average,
                    vote_count,
                    genres,
                    cast,
                    crew,
                    similar_json,
                    reviews,
                    tagline,
                    collection_name,
                    collection_json,
                    keywords_json,
                    credits_blob,
                    production_companies_json,
                    videos_json,
                ],
            )
            .map_err(|e| e.to_string())?;
        return Ok(id);
    }

    db.conn()
        .execute(
            "INSERT INTO movies(id, canonical_title, release_year, tmdb_id, tmdb_media_type, poster_path, backdrop_path,
             overview, runtime, vote_average, vote_count, genres_json, cast_json, crew_json, similar_json,
             reviews_json, tagline, collection_name, collection_json, keywords_json, credits_json, production_companies_json,
             videos_json, enriched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, datetime('now'))",
            params![
                id,
                title,
                year,
                tmdb_id,
                media_type.as_str(),
                poster_store,
                backdrop,
                overview,
                runtime,
                vote_average,
                vote_count,
                genres,
                cast,
                crew,
                similar_json,
                reviews,
                tagline,
                collection_name,
                collection_json,
                keywords_json,
                credits_blob,
                production_companies_json,
                videos_json,
            ],
        )
        .map_err(|e| e.to_string())?;
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO movie_aliases(movie_id, normalized_title, release_year)
             VALUES (?1, ?2, ?3)",
            params![id, normalize_title(&title), year],
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

fn tmdb_details_path(tmdb_id: i64, media_type: TmdbMediaType) -> String {
    format!(
        "/{}/{tmdb_id}?append_to_response=credits,reviews,recommendations,similar,keywords,images,videos",
        media_type.endpoint()
    )
}

fn reclassify_tmdb_media_type(
    db: &Database,
    tmdb_id: i64,
    old_type: TmdbMediaType,
    new_type: TmdbMediaType,
) -> Result<(), String> {
    db.conn()
        .execute(
            "UPDATE movies SET tmdb_media_type = ?3, enriched_at = NULL
             WHERE tmdb_id = ?1 AND tmdb_media_type = ?2",
            params![tmdb_id, old_type.as_str(), new_type.as_str()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn defer_unavailable_tmdb_metadata(db: &Database, tmdb_id: i64) -> Result<(), String> {
    db.conn()
        .execute(
            "UPDATE movies SET enriched_at = datetime('now') WHERE tmdb_id = ?1",
            params![tmdb_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn parse_raw_title(raw: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(title) = v.get("title").and_then(|t| t.as_str()) {
            return title.to_string();
        }
    }
    raw.to_string()
}

/// A few import feeds put the release year directly in the title instead of
/// supplying a separate year field (for example, "Magnolia, 1999").  Remove
/// only an unambiguous trailing year delimiter for lookup; preserve the source
/// title exactly as imported and prefer an explicit year when it exists.
fn title_and_embedded_year(title: &str, explicit_year: Option<i32>) -> (String, Option<i32>) {
    let trimmed = title.trim();
    if trimmed.len() < 6 {
        return (trimmed.to_string(), explicit_year);
    }

    let split_at = trimmed.len() - 4;
    let (prefix, digits) = trimmed.split_at(split_at);
    let Some(embedded_year) = digits
        .chars()
        .all(|character| character.is_ascii_digit())
        .then(|| digits.parse::<i32>().ok())
        .flatten()
        .filter(|year| (1880..=2100).contains(year))
    else {
        return (trimmed.to_string(), explicit_year);
    };

    let prefix = prefix.trim_end();
    let Some(delimiter) = prefix.chars().last() else {
        return (trimmed.to_string(), explicit_year);
    };
    if !matches!(delimiter, ',' | '(' | '[') {
        return (trimmed.to_string(), explicit_year);
    }

    let clean_title = prefix
        .trim_end_matches(|character| matches!(character, ',' | '(' | '['))
        .trim_end();
    if clean_title.is_empty() {
        return (trimmed.to_string(), explicit_year);
    }
    (
        clean_title.to_string(),
        explicit_year.or(Some(embedded_year)),
    )
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn genre_names(v: &serde_json::Value) -> Vec<String> {
    v["genres"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn cast_names(v: &serde_json::Value) -> Vec<String> {
    v["credits"]["cast"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(32)
                .filter_map(|g| {
                    let name = g["name"].as_str()?;
                    let character = g["character"].as_str().unwrap_or("").trim();
                    if character.is_empty() {
                        Some(name.to_string())
                    } else {
                        Some(format!("{name} as {character}"))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

const CREW_JOBS: &[&str] = &[
    "Director",
    "Writer",
    "Screenplay",
    "Original Screenplay",
    "Story",
    "Novel",
    "Characters",
    "Director of Photography",
    "Cinematography",
    "Cinematographer",
    "Original Music Composer",
    "Music",
    "Editor",
    "Production Design",
    "Art Direction",
    "Costume Design",
    "Casting",
    "Sound Designer",
    "Sound Mixer",
    "Visual Effects Supervisor",
    "Animation",
    "Producer",
];

fn crew_names(v: &serde_json::Value) -> Vec<String> {
    v["credits"]["crew"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|g| {
                    let job = g["job"].as_str()?;
                    if CREW_JOBS.contains(&job) {
                        Some(format!("{} ({})", g["name"].as_str()?, job))
                    } else {
                        None
                    }
                })
                .take(48)
                .collect()
        })
        .unwrap_or_default()
}

fn keyword_entries(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v["keywords"]["keywords"]
        .as_array()
        .or_else(|| v["keywords"].as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|k| {
                    let name = k["name"].as_str()?.to_string();
                    Some(serde_json::json!({ "id": k["id"].as_i64(), "name": name }))
                })
                .take(24)
                .collect()
        })
        .unwrap_or_default()
}

fn credit_entries(v: &serde_json::Value) -> serde_json::Value {
    let cast: Vec<serde_json::Value> = v["credits"]["cast"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(64)
                .filter_map(|g| {
                    Some(serde_json::json!({
                        "tmdbId": g["id"].as_i64(),
                        "name": g["name"].as_str()?,
                        "profile": g["profile_path"].as_str(),
                        "character": g["character"].as_str().filter(|name| !name.trim().is_empty()),
                        "order": g["order"].as_i64(),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let crew: Vec<serde_json::Value> = v["credits"]["crew"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|g| {
                    let job = g["job"].as_str()?.trim();
                    if job.is_empty() {
                        return None;
                    }
                    Some(serde_json::json!({
                        "tmdbId": g["id"].as_i64(),
                        "name": g["name"].as_str()?,
                        "job": job,
                        "department": g["department"].as_str(),
                        "profile": g["profile_path"].as_str(),
                    }))
                })
                .take(120)
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({ "detailVersion": 1, "cast": cast, "crew": crew })
}

fn production_companies(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v["production_companies"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|company| {
                    Some(serde_json::json!({
                        "tmdbId": company["id"].as_i64(),
                        "name": company["name"].as_str()?,
                        "logo": company["logo_path"].as_str(),
                        "originCountry": company["origin_country"].as_str(),
                    }))
                })
                .take(24)
                .collect()
        })
        .unwrap_or_default()
}

fn video_type_rank(kind: &str) -> u8 {
    match kind {
        "Trailer" => 0,
        "Teaser" => 1,
        "Clip" => 2,
        _ => 3,
    }
}

fn video_entries(v: &serde_json::Value) -> Vec<FilmTrailer> {
    let mut videos: Vec<(u8, u8, u8, String, FilmTrailer)> = v["videos"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|video| {
            let site = video["site"].as_str()?.trim();
            if !site.eq_ignore_ascii_case("YouTube") {
                return None;
            }
            let key = video["key"].as_str()?.trim();
            if key.is_empty() {
                return None;
            }
            let kind = video["type"].as_str().unwrap_or("Trailer").trim();
            let name = video["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(kind)
                .to_string();
            let official = video["official"].as_bool().unwrap_or(false);
            let language_rank = match video["iso_639_1"].as_str() {
                Some("en") => 0,
                None => 1,
                _ => 2,
            };
            let official_rank = if official { 0 } else { 1 };
            Some((
                video_type_rank(kind),
                official_rank,
                language_rank,
                video["published_at"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                FilmTrailer {
                    key: key.to_string(),
                    name,
                    site: "YouTube".into(),
                    kind: kind.to_string(),
                    official,
                },
            ))
        })
        .collect();
    videos.sort_by(|a, b| {
        (a.0, a.1, a.2, std::cmp::Reverse(a.3.as_str()))
            .cmp(&(b.0, b.1, b.2, std::cmp::Reverse(b.3.as_str())))
    });
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (_, _, _, _, trailer) in videos {
        if seen.insert(trailer.key.clone()) {
            out.push(trailer);
        }
        if out.len() == 8 {
            break;
        }
    }
    out
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonCredit {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i32>,
    #[serde(default)]
    pub job: String,
    /// TMDB cast billing order when job is Actor. Lower is more prominent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

/// Cache envelope. Pre-v2 payloads were a bare `Vec<PersonCredit>` of crew only
/// and must be refetched so actor filmography can populate.
pub const PERSON_CREDITS_CACHE_VERSION: u32 = 2;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonCreditsCache {
    pub version: u32,
    pub credits: Vec<PersonCredit>,
}

fn read_person_credits_cache(raw: &str) -> Option<Vec<PersonCredit>> {
    if let Ok(wrapped) = serde_json::from_str::<PersonCreditsCache>(raw) {
        if wrapped.version == PERSON_CREDITS_CACHE_VERSION
            && wrapped.credits.iter().all(|i| !i.job.trim().is_empty())
        {
            return Some(wrapped.credits);
        }
        return None;
    }
    // Legacy bare array (crew-only era) — force a refresh.
    None
}

pub fn person_movie_credits(db: &Database, person_id: i64) -> Result<Vec<PersonCredit>, String> {
    person_movie_credits_with_force(db, person_id, false)
}

pub fn person_movie_credits_with_force(
    db: &Database,
    person_id: i64,
    force: bool,
) -> Result<Vec<PersonCredit>, String> {
    if !force {
        if let Some((raw, fresh)) = db
            .conn()
            .query_row(
                "SELECT credits_json,
                        CASE WHEN datetime(fetched_at) >= datetime('now', '-30 days')
                             THEN 1 ELSE 0 END
                 FROM person_credits WHERE person_id = ?1",
                params![person_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            if fresh == 1 {
                if let Some(items) = read_person_credits_cache(&raw) {
                    return Ok(items);
                }
            }
        }
    }
    let api_key = get_api_key()?.ok_or("missing api key")?;
    let body = tmdb_get(&api_key, &format!("/person/{person_id}/movie_credits"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(arr) = v["cast"].as_array() {
        for row in arr {
            let Some(lib) = library_item_from_movie_value(row) else {
                continue;
            };
            let Some(id) = parse_tmdb_ref(&lib.id) else {
                continue;
            };
            let job = "Actor";
            if !seen.insert((id, job.to_string())) {
                continue;
            }
            items.push(PersonCredit {
                tmdb_id: id,
                title: lib.title,
                year: lib.year,
                job: job.to_string(),
                order: row["order"].as_i64(),
            });
        }
    }
    if let Some(arr) = v["crew"].as_array() {
        for row in arr {
            let job = row["job"].as_str().unwrap_or("").trim();
            if crate::taste::features::family_for_job(job).is_none() {
                continue;
            }
            let Some(lib) = library_item_from_movie_value(row) else {
                continue;
            };
            let Some(id) = parse_tmdb_ref(&lib.id) else {
                continue;
            };
            if !seen.insert((id, job.to_string())) {
                continue;
            }
            items.push(PersonCredit {
                tmdb_id: id,
                title: lib.title,
                year: lib.year,
                job: job.to_string(),
                order: None,
            });
        }
    }
    // Prefer billed cast / known crew roles; keep a bounded filmography page.
    items.sort_by(|a, b| {
        let a_actor = a.job == "Actor";
        let b_actor = b.job == "Actor";
        match (a_actor, b_actor) {
            (true, true) => a
                .order
                .unwrap_or(i64::MAX)
                .cmp(&b.order.unwrap_or(i64::MAX))
                .then(a.tmdb_id.cmp(&b.tmdb_id)),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => a.tmdb_id.cmp(&b.tmdb_id),
        }
    });
    items.truncate(120);
    let wrapped = PersonCreditsCache {
        version: PERSON_CREDITS_CACHE_VERSION,
        credits: items.clone(),
    };
    let json = serde_json::to_string(&wrapped).unwrap_or_else(|_| "[]".into());
    let _ = db.conn().execute(
        "INSERT OR REPLACE INTO person_credits(person_id, credits_json, fetched_at)
         VALUES (?1, ?2, datetime('now'))",
        params![person_id, json],
    );
    Ok(items)
}

fn parse_tmdb_ref(id: &str) -> Option<i64> {
    id.strip_prefix("tmdb:")
        .and_then(|s| s.parse().ok())
        .or_else(|| id.parse().ok())
}

pub fn library_item_from_movie_value(v: &serde_json::Value) -> Option<LibraryItem> {
    classify_tmdb_value(v)
        .ok()
        .filter(|(kind, _)| *kind == crate::taste::retrieve::MediaKind::Movie)
        .map(|(_, item)| item)
}

pub fn classify_tmdb_value(
    v: &serde_json::Value,
) -> Result<(crate::taste::retrieve::MediaKind, LibraryItem), crate::taste::retrieve::MediaKind> {
    use crate::taste::retrieve::MediaKind;
    let Some(tmdb_id) = v["id"].as_i64() else {
        return Err(MediaKind::Ambiguous);
    };
    if let Some(media_type) = v.get("media_type").and_then(|t| t.as_str()) {
        match media_type {
            "movie" => {}
            "tv" => return Err(MediaKind::TvSeries),
            other if other.contains("episode") => return Err(MediaKind::TvEpisode),
            _ => return Err(MediaKind::Other),
        }
    }
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let name = v
        .get("name")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let release = v
        .get("release_date")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty());
    let first_air = v
        .get("first_air_date")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty());
    let Some(title) = title else {
        if name.is_some() && first_air.is_some() {
            return Err(MediaKind::TvSeries);
        }
        return Err(MediaKind::Ambiguous);
    };
    let year = release
        .and_then(|d| d.get(0..4))
        .and_then(|y| y.parse().ok());
    Ok((
        MediaKind::Movie,
        LibraryItem::catalog(
            format!("tmdb:{tmdb_id}"),
            title.to_string(),
            year,
            poster_url(v["poster_path"].as_str().map(str::to_string)),
            backdrop_url(v["backdrop_path"].as_str().map(str::to_string)),
            v["overview"].as_str().map(str::to_string),
        ),
    ))
}

pub fn library_item_from_tmdb_value(v: &serde_json::Value) -> Option<LibraryItem> {
    let tmdb_id = v["id"].as_i64()?;
    let title = v
        .get("title")
        .or_else(|| v.get("name"))
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let year = v
        .get("release_date")
        .or_else(|| v.get("first_air_date"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.get(0..4))
        .and_then(|y| y.parse().ok());
    Some(LibraryItem::catalog(
        format!("tmdb:{tmdb_id}"),
        title,
        year,
        poster_url(v["poster_path"].as_str().map(str::to_string)),
        backdrop_url(v["backdrop_path"].as_str().map(str::to_string)),
        v["overview"].as_str().map(str::to_string),
    ))
}

fn related_list(v: &serde_json::Value, key: &str, take: usize) -> Vec<LibraryItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(arr) = v[key]["results"].as_array() {
        for item in arr.iter().take(take) {
            if let Some(lib) = library_item_from_movie_value(item) {
                if seen.insert(lib.id.clone()) {
                    items.push(lib);
                }
            }
        }
    }
    items
}

/// How many TMDB recommendation/similar neighbors to persist per film.
/// Taste retrieval reads from this stored list; keeping it shallow caps discovery.
pub const RELATED_LIST_STORE: usize = 20;
/// Previous persist cap. Lists at this size are treated as shallow and refreshed.
pub const RELATED_LIST_LEGACY_CAP: usize = 12;

fn related_lists(v: &serde_json::Value) -> (Vec<LibraryItem>, Vec<LibraryItem>) {
    (
        related_list(v, "recommendations", RELATED_LIST_STORE),
        related_list(v, "similar", RELATED_LIST_STORE),
    )
}

fn collection_from_movie(
    api_key: &str,
    v: &serde_json::Value,
) -> (Option<String>, Vec<LibraryItem>) {
    let Some(col) = v.get("belongs_to_collection") else {
        return (None, Vec::new());
    };
    if col.is_null() {
        return (None, Vec::new());
    }
    let name = col["name"]
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let Some(collection_id) = col["id"].as_i64() else {
        return (name, Vec::new());
    };
    let Ok(body) = tmdb_get(api_key, &format!("/collection/{collection_id}")) else {
        return (name, Vec::new());
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
        return (name, Vec::new());
    };
    let parts = parsed["parts"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(library_item_from_movie_value)
                .collect()
        })
        .unwrap_or_default();
    (name, parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_jwt_read_token_as_bearer() {
        assert!(is_bearer_token(
            "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ0ZXN0In0.signature"
        ));
        assert!(!is_bearer_token("a1b2c3d4e5f678901234567890123456"));
    }

    #[test]
    fn percent_encodes_utf8_bytes() {
        assert_eq!(percent_encode("Amélie"), "Am%C3%A9lie");
        assert_eq!(percent_encode("The Matrix"), "The%20Matrix");
    }

    #[test]
    fn search_title_matching_ignores_punctuation_only_differences() {
        assert!(titles_match("Spider-Man: No Way Home", "spider man no way home"));
        assert!(titles_match("Amélie", "amélie"));
        assert!(!titles_match("The Piano Tuner", "tuner"));
    }

    #[test]
    fn tmdb_keyring_persists_beyond_the_entry() {
        use keyring::credential::{CredentialBuilderApi as _, CredentialPersistence};
        let persistence = keyring::default::default_credential_builder().persistence();
        assert!(
            !matches!(persistence, CredentialPersistence::EntryOnly),
            "keyring is using the in-memory mock store; enable windows-native so the TMDB key survives across Entry::new calls"
        );
    }

    #[test]
    fn match_progress_uses_library_total_not_batch_size() {
        assert_eq!(match_progress_label(2, 1097), "Checking title matches · 2 of 1097");
        assert_ne!(match_progress_label(2, 1097), "Checking title matches · 2 of 50");
    }

    #[test]
    fn movie_parser_rejects_tv_name_and_air_date() {
        let tv = serde_json::json!({
            "id": 1,
            "name": "Some Series",
            "first_air_date": "2020-01-01"
        });
        assert!(library_item_from_movie_value(&tv).is_none());
        assert!(matches!(
            classify_tmdb_value(&tv),
            Err(crate::taste::retrieve::MediaKind::TvSeries)
        ));
    }

    #[test]
    fn movie_parser_rejects_explicit_tv_media_type() {
        let tv = serde_json::json!({
            "id": 2,
            "title": "Looks Like A Movie",
            "release_date": "2021-01-01",
            "media_type": "tv"
        });
        assert!(library_item_from_movie_value(&tv).is_none());
    }

    #[test]
    fn movie_parser_accepts_title_without_media_type() {
        let movie = serde_json::json!({
            "id": 3,
            "title": "The Prestige",
            "release_date": "2006-10-20"
        });
        let item = library_item_from_movie_value(&movie).expect("movie");
        assert_eq!(item.title, "The Prestige");
        assert_eq!(item.year, Some(2006));
    }

    #[test]
    fn movie_parser_rejects_name_only_ambiguous() {
        let row = serde_json::json!({ "id": 4, "name": "Mystery" });
        assert!(matches!(
            classify_tmdb_value(&row),
            Err(crate::taste::retrieve::MediaKind::Ambiguous)
        ));
    }

    #[test]
    fn search_does_not_match_a_different_single_result() {
        let results = vec![TmdbSearchResult {
            id: 1571662,
            title: "The Piano Tuner".into(),
            original_title: None,
            release_date: Some("2025-01-01".into()),
            poster_path: None,
            vote_count: 100,
        }];
        assert!(pick_search_match(&results, "tuner", Some(2025)).is_none());
    }

    #[test]
    fn search_matches_a_film_by_its_original_title() {
        let results = vec![TmdbSearchResult {
            id: 1,
            title: "Localized title".into(),
            original_title: Some("Original Title".into()),
            release_date: Some("2020-01-01".into()),
            poster_path: None,
            vote_count: 100,
        }];
        assert_eq!(
            pick_search_match(&results, "original title", Some(2020)).map(|result| result.id),
            Some(1)
        );
    }

    #[test]
    fn search_matches_a_short_title_when_tmdb_only_adds_a_colon_subtitle() {
        let results = vec![TmdbSearchResult {
            id: 2,
            title: "Yellow: a LEGO Horror Movie".into(),
            original_title: None,
            release_date: Some("2026-07-02".into()),
            poster_path: Some("/yellow.jpg".into()),
            vote_count: 100,
        }];
        assert_eq!(
            pick_search_match(&results, "yellow", Some(2026)).map(|result| result.id),
            Some(2)
        );
    }

    #[test]
    fn search_selects_the_clearly_canonical_duplicate() {
        let results = vec![
            TmdbSearchResult {
                id: 10,
                title: "Companion".into(),
                original_title: None,
                release_date: Some("2025-01-31".into()),
                poster_path: Some("/canonical.jpg".into()),
                vote_count: 5000,
            },
            TmdbSearchResult {
                id: 11,
                title: "Companion".into(),
                original_title: None,
                release_date: Some("2025-01-31".into()),
                poster_path: None,
                vote_count: 1,
            },
        ];
        assert_eq!(
            pick_search_match(&results, "companion", Some(2025)).map(|result| result.id),
            Some(10)
        );
    }

    #[test]
    fn search_prefers_a_clear_title_match_when_the_import_year_is_off() {
        let results = vec![
            TmdbSearchResult {
                id: 10,
                title: "Talk to Me".into(),
                original_title: None,
                release_date: Some("2023-07-26".into()),
                poster_path: Some("/canonical.jpg".into()),
                vote_count: 5000,
            },
            TmdbSearchResult {
                id: 11,
                title: "Talk to Me".into(),
                original_title: None,
                release_date: Some("1984-01-01".into()),
                poster_path: None,
                vote_count: 1,
            },
        ];
        assert_eq!(
            pick_search_match(&results, "talk to me", Some(2022)).map(|result| result.id),
            Some(10)
        );
    }

    #[test]
    fn lookup_title_extracts_a_trailing_embedded_year() {
        assert_eq!(
            title_and_embedded_year("Magnolia, 1999", None),
            ("Magnolia".to_string(), Some(1999))
        );
        assert_eq!(
            title_and_embedded_year("The Phantom of the Opera (2004", Some(2004)),
            ("The Phantom of the Opera".to_string(), Some(2004))
        );
        assert_eq!(
            title_and_embedded_year("2001", None),
            ("2001".to_string(), None)
        );
    }

    #[test]
    fn parent_series_queries_only_expand_likely_tv_specials() {
        let queries = tv_parent_search_queries("a very solar holiday opposites special");
        assert!(queries.contains(&"opposites".to_string()));
        assert!(queries.contains(&"solar".to_string()));
        assert!(queries.len() <= 4);
        assert!(tv_parent_search_queries("the hunt for brown october").is_empty());
    }

    #[test]
    fn tv_artwork_fallback_requires_an_exact_series_title() {
        let results = vec![TmdbTvSearchResult {
            id: 1,
            name: "Localized series title".into(),
            original_name: Some("Pop Star Academy: KATSEYE".into()),
            first_air_date: Some("2024-08-21".into()),
            poster_path: Some("/katseye.jpg".into()),
        }];
        assert_eq!(
            pick_tv_match(
                &results,
                &normalize_title("Pop Star Academy: KATSEYE"),
                Some(2024),
            )
                .and_then(|result| result.poster_path.as_deref()),
            Some("/katseye.jpg")
        );
        assert!(pick_tv_match(&results, "katseye wild hearts", Some(2026)).is_none());
    }

    #[test]
    fn tv_artwork_fallback_can_use_a_unique_parent_series_for_a_special() {
        let results = vec![TmdbTvSearchResult {
            id: 2,
            name: "Solar Opposites".into(),
            original_name: None,
            first_air_date: Some("2020-05-08".into()),
            poster_path: Some("/solar-opposites.jpg".into()),
        }];
        assert_eq!(
            pick_tv_match(
                &results,
                "a very solar holiday opposites special",
                Some(2021),
            )
            .and_then(|result| result.poster_path.as_deref()),
            Some("/solar-opposites.jpg")
        );
    }

    #[test]
    fn community_images_fill_a_missing_default_poster() {
        let images = serde_json::json!({
            "posters": [
                { "file_path": "/foreign.jpg", "iso_639_1": "fr", "vote_count": 90 },
                { "file_path": "/english.jpg", "iso_639_1": "en", "vote_count": 2 }
            ]
        });
        assert_eq!(
            preferred_artwork_path(None, &images, "posters"),
            Some("/english.jpg".into())
        );
    }

    #[test]
    fn trailers_prefer_official_english_youtube_trailers() {
        let payload = serde_json::json!({
            "videos": {
                "results": [
                    {
                        "site": "Vimeo",
                        "key": "vimeo1",
                        "type": "Trailer",
                        "official": true,
                        "iso_639_1": "en",
                        "name": "Vimeo trailer",
                        "published_at": "2024-06-01T00:00:00.000Z"
                    },
                    {
                        "site": "YouTube",
                        "key": "teaser1",
                        "type": "Teaser",
                        "official": true,
                        "iso_639_1": "en",
                        "name": "Teaser",
                        "published_at": "2024-05-01T00:00:00.000Z"
                    },
                    {
                        "site": "YouTube",
                        "key": "fr-trailer",
                        "type": "Trailer",
                        "official": true,
                        "iso_639_1": "fr",
                        "name": "Bande-annonce",
                        "published_at": "2024-04-01T00:00:00.000Z"
                    },
                    {
                        "site": "YouTube",
                        "key": "official-en",
                        "type": "Trailer",
                        "official": true,
                        "iso_639_1": "en",
                        "name": "Official Trailer",
                        "published_at": "2024-03-01T00:00:00.000Z"
                    }
                ]
            }
        });
        let trailers = video_entries(&payload);
        assert_eq!(trailers.len(), 3);
        assert_eq!(trailers[0].key, "official-en");
        assert_eq!(trailers[0].kind, "Trailer");
        assert_eq!(trailers[1].key, "fr-trailer");
        assert_eq!(trailers[2].key, "teaser1");
    }

    #[test]
    fn artwork_picker_keeps_only_high_resolution_portrait_and_landscape_images() {
        let images = serde_json::json!({
            "posters": [
                { "file_path": "/tiny-poster.jpg", "width": 342, "height": 513, "vote_count": 100 },
                { "file_path": "/wide-poster.jpg", "width": 1000, "height": 600, "vote_count": 100 },
                { "file_path": "/sharp-poster.jpg", "width": 1000, "height": 1500, "vote_count": 2 }
            ],
            "backdrops": [
                { "file_path": "/tiny-backdrop.jpg", "width": 780, "height": 439, "vote_count": 50 },
                { "file_path": "/tall-backdrop.jpg", "width": 1280, "height": 1800, "vote_count": 50 },
                { "file_path": "/sharp-backdrop.jpg", "width": 1920, "height": 1080, "vote_count": 2 }
            ]
        });
        assert_eq!(artwork_picker_paths(&images, "posters"), vec!["/sharp-poster.jpg"]);
        assert_eq!(artwork_picker_paths(&images, "backdrops"), vec!["/sharp-backdrop.jpg"]);
    }

    #[test]
    fn artwork_gap_reuses_an_existing_tmdb_match() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, raw_identity, created_at)
                 VALUES ('source', 'letterboxd_export', 'source', 'gap film', '{\"title\":\"Gap Film\"}', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id) VALUES ('movie', 'Gap Film', 42)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, movie_id, match_state)
                 VALUES ('source', 'movie', 'confirmed')",
                [],
            )
            .unwrap();
        assert_eq!(
            linked_tmdb(&db, "source").unwrap(),
            Some((42, TmdbMediaType::Movie))
        );
    }

    #[test]
    fn cached_confirmed_cover_is_not_requeued_as_a_tmdb_artwork_gap() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, raw_identity, cached_poster_url, created_at)
                 VALUES ('cached', 'letterboxd_export', 'cached', 'cached film', '{}', 'https://cover.test/cached.jpg', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id) VALUES ('movie', 'Cached Film', 42)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, movie_id, match_state)
                 VALUES ('cached', 'movie', 'confirmed')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, raw_identity, cached_poster_url, created_at)
                 VALUES ('unmatched', 'letterboxd_export', 'unmatched', 'unmatched film', '{}', 'https://cover.test/unmatched.jpg', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, match_state)
                 VALUES ('unmatched', 'unmatched')",
                [],
            )
            .unwrap();

        let ids: Vec<_> = list_unmatched(&db)
            .unwrap()
            .into_iter()
            .map(|row| row.0)
            .collect();
        assert_eq!(ids, vec!["unmatched"]);
        assert!(source_has_poster(&db, "cached").unwrap());
    }

    #[test]
    fn metadata_gaps_are_hydrated_independently_of_poster_status() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id, tmdb_media_type, poster_path)
                 VALUES ('gap', 'A Series Special', 77, 'tv', '/already-has-a-cover.jpg')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(
                   id, canonical_title, tmdb_id, tmdb_media_type, enriched_at,
                   genres_json, credits_json, production_companies_json, keywords_json, similar_json, videos_json
                 ) VALUES ('complete', 'Complete', 78, 'movie', datetime('now'), '[]', '{}', '[]', '[]', '{}', '[]')",
                [],
            )
            .unwrap();
        assert_eq!(list_metadata_gaps(&db).unwrap(), vec![(77, TmdbMediaType::Tv)]);
    }

    #[test]
    fn missing_videos_backfill_immediately_on_enriched_rows() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(
                   id, canonical_title, tmdb_id, tmdb_media_type, enriched_at,
                   genres_json, credits_json, production_companies_json, keywords_json, similar_json
                 ) VALUES ('needs-trailers', 'Needs Trailers', 91, 'movie', datetime('now'), '[]', '{}', '[]', '[]', '{}')",
                [],
            )
            .unwrap();
        assert_eq!(
            list_metadata_gaps(&db).unwrap(),
            vec![(91, TmdbMediaType::Movie)]
        );
    }

    #[test]
    fn unavailable_tmdb_metadata_waits_before_retrying() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id, enriched_at)
                 VALUES ('gone', 'Removed Listing', 88, datetime('now'))",
                [],
            )
            .unwrap();
        assert!(list_metadata_gaps(&db).unwrap().is_empty());
        db.conn()
            .execute(
                "UPDATE movies SET enriched_at = datetime('now', '-8 days') WHERE id = 'gone'",
                [],
            )
            .unwrap();
        assert_eq!(list_metadata_gaps(&db).unwrap(), vec![(88, TmdbMediaType::Movie)]);
    }

    #[test]
    fn unmatched_records_back_off_after_a_completed_check() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, raw_identity, created_at)
                 VALUES ('unmatched', 'letterboxd_export', 'unmatched', 'not found', '{\"title\":\"Not Found\"}', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, match_state, tmdb_checked_at)
                 VALUES ('unmatched', 'unmatched', datetime('now'))",
                [],
            )
            .unwrap();
        assert!(list_unmatched(&db).unwrap().is_empty());
        db.conn()
            .execute(
                "UPDATE movie_links SET tmdb_checked_at = datetime('now', '-8 days') WHERE source_movie_record_id = 'unmatched'",
                [],
            )
            .unwrap();
        assert_eq!(list_unmatched(&db).unwrap().len(), 1);
    }

    #[test]
    fn tmdb_errors_hide_credentials_and_recognize_not_found() {
        let error = tmdb_request_error(
            "/movie/88?append_to_response=credits",
            "https://api.themoviedb.org/3/movie/88?api_key=secret: status code 404",
        );
        assert_eq!(error, "TMDB /movie/88: status code 404");
        assert!(tmdb_not_found(&error));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn reclassifying_a_stale_tmdb_type_updates_the_existing_record() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id, tmdb_media_type)
                 VALUES ('stale', 'A Special', 99, 'movie')",
                [],
            )
            .unwrap();
        reclassify_tmdb_media_type(&db, 99, TmdbMediaType::Movie, TmdbMediaType::Tv).unwrap();
        let kind: String = db
            .conn()
            .query_row("SELECT tmdb_media_type FROM movies WHERE id = 'stale'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kind, "tv");
    }

    #[test]
    fn propagating_a_movie_without_a_poster_does_not_fail() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, raw_identity, created_at)
                 VALUES ('source', 'letterboxd_export', 'source', 'posterless film', '{\"title\":\"Posterless Film\"}', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, tmdb_id, poster_path)
                 VALUES ('movie', 'Posterless Film', 43, NULL)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, movie_id, match_state)
                 VALUES ('source', 'movie', 'confirmed')",
                [],
            )
            .unwrap();

        assert!(propagate_movie_link(&db, "source", "movie").is_ok());
    }

    #[test]
    fn movie_parser_ignores_video_flag_and_keeps_shorts() {
        let row = serde_json::json!({
            "id": 5,
            "title": "A Short",
            "release_date": "2020-01-01",
            "video": true,
            "runtime": 8
        });
        let item = library_item_from_movie_value(&row).expect("movie");
        assert_eq!(item.title, "A Short");
    }

    #[test]
    fn force_refresh_bypasses_person_credit_cache() {
        let db = crate::storage::db::Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO person_credits(person_id, credits_json, fetched_at)
                 VALUES (1, '[{\"tmdb_id\":999001,\"title\":\"SENTINEL\",\"year\":1999}]', datetime('now'))",
                [],
            )
            .unwrap();
        let cached = person_movie_credits(&db, 1);
        match cached {
            Ok(items) => {
                assert!(
                    items.is_empty() || items.iter().any(|i| i.title != "SENTINEL"),
                    "legacy/jobless person-credit cache must not be reused"
                );
            }
            Err(_) => {}
        }
        let wrapped = serde_json::json!({
            "version": PERSON_CREDITS_CACHE_VERSION,
            "credits": [{
                "tmdb_id": 999001,
                "title": "SENTINEL",
                "year": 1999,
                "job": "Director"
            }]
        });
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO person_credits(person_id, credits_json, fetched_at)
                 VALUES (1, ?1, datetime('now'))",
                params![wrapped.to_string()],
            )
            .unwrap();
        let cached = person_movie_credits(&db, 1).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].title, "SENTINEL");
        assert_eq!(cached[0].job, "Director");
        match person_movie_credits_with_force(&db, 1, true) {
            Ok(items) => assert!(items.iter().all(|i| i.title != "SENTINEL")),
            Err(_) => {}
        }
    }

    #[test]
    fn person_credits_cache_rejects_legacy_crew_only_array() {
        let legacy = r#"[{"tmdb_id":1,"title":"Crew Only","year":2000,"job":"Writer"}]"#;
        assert!(
            read_person_credits_cache(legacy).is_none(),
            "pre-v2 cast-blind caches must miss so actors refetch"
        );
        let wrapped = PersonCreditsCache {
            version: PERSON_CREDITS_CACHE_VERSION,
            credits: vec![PersonCredit {
                tmdb_id: 1,
                title: "Both".into(),
                year: Some(2001),
                job: "Actor".into(),
                order: Some(0),
            }],
        };
        let raw = serde_json::to_string(&wrapped).unwrap();
        let hit = read_person_credits_cache(&raw).unwrap();
        assert_eq!(hit[0].job, "Actor");
        assert_eq!(hit[0].order, Some(0));
    }
}

fn review_authors(v: &serde_json::Value) -> Vec<String> {
    v["reviews"]["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(5)
                .filter_map(|r| {
                    let author = r["author"].as_str()?;
                    let content = r["content"].as_str()?.chars().take(120).collect::<String>();
                    Some(format!("{author}: {content}…"))
                })
                .collect()
        })
        .unwrap_or_default()
}
