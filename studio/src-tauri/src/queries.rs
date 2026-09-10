use crate::catalog::tmdb::library_item_from_tmdb_value;
use crate::letterboxd::posters::{backdrop_url, poster_from_rss_body, poster_url, tmdb_image_url};
use crate::letterboxd::rss::parse_activity_payload;
use crate::models::{
    ConnectionFilm, FilmCastMember, FilmConnection, FilmCrewMember, FilmDetail, FilmTrailer,
    FriendActivityItem, HomeViewModel, LibraryItem, LibraryPage, LibraryQuery,
    ProductionCompany, StatsBucket, StatsSnapshot, ViewingHistoryItem,
};
use crate::storage::db::Database;
use chrono::{Datelike, Utc};
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde_json;
use std::collections::HashMap;

const FILM_KEY: &str =
    "COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), ''))";

fn library_filter_clause(filter: Option<&str>) -> &'static str {
    match filter {
        Some("watchlist") => " AND (ums.watchlist = 1 OR smr.on_watchlist = 1) ",
        Some("watched") => " AND ums.watched = 1 ",
        Some("unresolved") => {
            " AND COALESCE(ml.match_state, 'unmatched') NOT IN ('confirmed', 'catalog') "
        }
        _ => "",
    }
}

const LIBRARY_MEMBERSHIP: &str = "(ums.watched = 1 OR ums.watchlist = 1 OR ums.current_rating IS NOT NULL OR smr.on_watchlist = 1)";

pub fn get_library(db: &Database, query: &LibraryQuery) -> Result<LibraryPage, String> {
    let search = query.search.as_deref().unwrap_or("").trim().to_lowercase();
    let limit = query.limit.unwrap_or(10_000).max(1) as i64;
    let offset = query.offset.unwrap_or(0) as i64;
    let sort = query.sort.as_deref().unwrap_or("recent");
    let filter_clause = library_filter_clause(query.filter.as_deref());

    let order = match sort {
        "title" => "title_raw ASC",
        "rating" => "(current_rating IS NULL), current_rating DESC, title_raw ASC",
        "year" => "(year IS NULL), year DESC, title_raw ASC",
        _ => "(last_watched_at IS NULL), last_watched_at DESC, p.last_watched_sequence DESC, title_raw ASC, p.film_key ASC",
    };

    let search_clause = if search.is_empty() {
        String::new()
    } else {
        " AND smr.normalized_title LIKE ?1 ".to_string()
    };

    let limit_clause = if search.is_empty() {
        format!(" ORDER BY {order} LIMIT ?1 OFFSET ?2 ")
    } else {
        format!(" ORDER BY {order} LIMIT ?2 OFFSET ?3 ")
    };

    let sql = format!(
        r#"
        WITH film_entries AS (
          SELECT
            {film_key} AS film_key,
            smr.id AS source_id,
            CASE WHEN m.tmdb_media_type = 'tv' THEN smr.raw_identity ELSE COALESCE(m.canonical_title, smr.raw_identity) END AS title_raw,
            CASE WHEN m.tmdb_media_type = 'tv' THEN smr.release_year ELSE COALESCE(m.release_year, smr.release_year) END AS year,
            ums.current_rating,
            COALESCE(m.poster_override_url, m.poster_path) AS poster,
            COALESCE(m.backdrop_override_url, m.backdrop_path) AS backdrop,
            m.overview AS overview,
            smr.cached_poster_url,
            json_extract(smr.raw_identity, '$.poster') AS identity_poster,
            COALESCE(ums.watched, 0) AS watched,
            COALESCE(ums.watchlist, smr.on_watchlist, 0) AS watchlist,
            COALESCE(ums.liked, 0) AS liked,
            (SELECT COUNT(*)
             FROM viewings v
             LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
             WHERE v.source_movie_record_id = smr.id
               AND COALESCE(vp.counted, 1) = 1) AS source_viewing_count,
            COALESCE(ml.match_state, 'unmatched') AS match_state,
            smr.source_type,
            ums.last_watched_at,
            (SELECT COALESCE(event_timestamp(v.published_at), event_timestamp(v.observed_at))
             FROM viewings v
             LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
             WHERE v.source_movie_record_id = smr.id
               AND COALESCE(vp.counted, 1) = 1
               AND v.occurred_at IS ums.last_watched_at
             ORDER BY COALESCE(event_timestamp(v.published_at), event_timestamp(v.observed_at)) DESC
             LIMIT 1) AS last_watched_sequence
          FROM source_movie_records smr
          LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
          LEFT JOIN movies m ON m.id = ml.movie_id
          LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
          WHERE {membership}
          {search_clause}
          {filter_clause}
        ),
        aggregated AS (
          SELECT
            film_key,
            SUM(source_viewing_count) AS viewing_count,
            MAX(watched) AS watched,
            MAX(watchlist) AS watchlist,
            MAX(liked) AS liked
          FROM film_entries
          GROUP BY film_key
        ),
        primary_row AS (
          SELECT
            fe.*,
            (
              SELECT COALESCE(m3.poster_override_url, m3.poster_path, smr3.cached_poster_url, json_extract(smr3.raw_identity, '$.poster'))
              FROM source_movie_records smr3
              LEFT JOIN movie_links ml3 ON ml3.source_movie_record_id = smr3.id
              LEFT JOIN movies m3 ON m3.id = ml3.movie_id
              WHERE COALESCE(ml3.movie_id, smr3.normalized_title || ':' || IFNULL(CAST(smr3.release_year AS TEXT), '')) = fe.film_key
                AND COALESCE(m3.poster_override_url, m3.poster_path, smr3.cached_poster_url, json_extract(smr3.raw_identity, '$.poster')) IS NOT NULL
                AND TRIM(COALESCE(m3.poster_override_url, m3.poster_path, smr3.cached_poster_url, json_extract(smr3.raw_identity, '$.poster'), '')) != ''
              LIMIT 1
            ) AS resolved_poster,
            ROW_NUMBER() OVER (
              PARTITION BY fe.film_key
              ORDER BY fe.last_watched_at DESC, fe.last_watched_sequence DESC, fe.source_id
            ) AS rn
          FROM film_entries fe
        )
        SELECT
          p.film_key AS id,
          p.title_raw,
          p.year,
          p.current_rating,
          COALESCE(p.resolved_poster, p.poster, p.cached_poster_url, p.identity_poster) AS poster,
          a.watched,
          a.watchlist,
          a.liked,
          a.viewing_count,
          p.match_state,
          p.source_type,
          p.last_watched_at,
          p.backdrop,
          p.overview
        FROM primary_row p
        JOIN aggregated a ON a.film_key = p.film_key
        WHERE p.rn = 1
        {limit_clause}
        "#,
        film_key = FILM_KEY,
        membership = LIBRARY_MEMBERSHIP,
        filter_clause = filter_clause,
    );

    let mut stmt = db.conn().prepare(&sql).map_err(|e| e.to_string())?;
    let rows = if search.is_empty() {
        stmt.query_map(params![limit, offset], map_library_item)
    } else {
        stmt.query_map(
            params![format!("%{search}%"), limit, offset],
            map_library_item,
        )
    }
    .map_err(|e| e.to_string())?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| e.to_string())?);
    }

    let count_sql = format!(
        "SELECT COUNT(DISTINCT {FILM_KEY})
         FROM source_movie_records smr
         LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
         LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
         WHERE {LIBRARY_MEMBERSHIP}
         {search_clause}
         {filter_clause}"
    );
    let total: u32 = if search.is_empty() {
        db.conn()
            .query_row(&count_sql, [], |row| row.get(0))
            .map_err(|e| e.to_string())?
    } else {
        db.conn()
            .query_row(&count_sql, params![format!("%{search}%")], |row| row.get(0))
            .map_err(|e| e.to_string())?
    };

    Ok(LibraryPage {
        items,
        total,
        coverage: db.compute_coverage()?,
    })
}

pub fn get_stats(db: &Database) -> Result<StatsSnapshot, String> {
    let mut monthly_stmt = db
        .conn()
        .prepare(
            r#"
            SELECT strftime('%Y-%m', COALESCE(v.occurred_at, v.observed_at)) AS month, COUNT(*)
            FROM viewings v
            LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
            WHERE COALESCE(vp.counted, 1) = 1
              AND strftime('%Y-%m', COALESCE(v.occurred_at, v.observed_at)) IS NOT NULL
            GROUP BY month
            "#,
        )
        .map_err(|e| e.to_string())?;
    let monthly_counts: HashMap<String, u32> = monthly_stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let viewing_months = recent_month_labels(24)
        .into_iter()
        .map(|label| StatsBucket {
            count: monthly_counts.get(&label).copied().unwrap_or(0),
            label,
            average_rating: None,
        })
        .collect();

    let mut genre_stmt = db
        .conn()
        .prepare(
            r#"
            WITH watched_movies AS (
              SELECT DISTINCT m.id, ums.current_rating
              FROM viewings v
              LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
              JOIN source_movie_records smr ON smr.id = v.source_movie_record_id
              JOIN movie_links ml ON ml.source_movie_record_id = smr.id
              JOIN movies m ON m.id = ml.movie_id
              LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
              WHERE COALESCE(vp.counted, 1) = 1
                AND m.genres_json IS NOT NULL
            )
            SELECT genre.value, COUNT(*) AS film_count, AVG(current_rating)
            FROM watched_movies wm
            JOIN movies m ON m.id = wm.id
            JOIN json_each(m.genres_json) genre
            WHERE TRIM(genre.value) != ''
            GROUP BY genre.value
            ORDER BY film_count DESC, genre.value COLLATE NOCASE ASC
            LIMIT 8
            "#,
        )
        .map_err(|e| e.to_string())?;
    let genres = genre_stmt
        .query_map([], |row| {
            Ok(StatsBucket {
                label: row.get(0)?,
                count: row.get::<_, i64>(1)? as u32,
                average_rating: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let rewatch_count: u32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM viewings v
             LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
             WHERE v.rewatch = 1 AND COALESCE(vp.counted, 1) = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let (total_runtime_minutes, runtime_viewings): (u32, u32) = db
        .conn()
        .query_row(
            r#"
            SELECT COALESCE(SUM(m.runtime), 0), COUNT(*)
            FROM viewings v
            LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
            JOIN source_movie_records smr ON smr.id = v.source_movie_record_id
            JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            JOIN movies m ON m.id = ml.movie_id
            WHERE COALESCE(vp.counted, 1) = 1
              AND m.runtime IS NOT NULL AND m.runtime > 0
            "#,
            [],
            |row| Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32)),
        )
        .map_err(|e| e.to_string())?;

    let metadata_movies: u32 = db
        .conn()
        .query_row(
            r#"
            SELECT COUNT(DISTINCT m.id)
            FROM viewings v
            LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
            JOIN source_movie_records smr ON smr.id = v.source_movie_record_id
            JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            JOIN movies m ON m.id = ml.movie_id
            WHERE COALESCE(vp.counted, 1) = 1
              AND m.genres_json IS NOT NULL
            "#,
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(StatsSnapshot {
        viewing_months,
        genres,
        rewatch_count,
        total_runtime_minutes,
        runtime_viewings,
        metadata_movies,
    })
}

fn recent_month_labels(month_count: usize) -> Vec<String> {
    let now = Utc::now();
    let mut year = now.year();
    let mut month = now.month() as i32;
    let mut labels = Vec::with_capacity(month_count);

    for _ in 0..month_count {
        labels.push(format!("{year}-{month:02}"));
        month -= 1;
        if month == 0 {
            month = 12;
            year -= 1;
        }
    }
    labels.reverse();
    labels
}

pub fn parse_tmdb_ref(id: &str) -> Option<i64> {
    id.strip_prefix("tmdb:")?.parse().ok().filter(|n| *n > 0)
}

fn map_library_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryItem> {
    let raw_title: String = row.get(1)?;
    let title = parse_raw_title(&raw_title);
    Ok(LibraryItem {
        id: row.get(0)?,
        title,
        year: row.get(2)?,
        current_rating: row.get(3)?,
        poster: poster_url(row.get(4)?),
        watched: row.get::<_, i32>(5)? == 1,
        watchlist: row.get::<_, i32>(6)? == 1,
        liked: row.get::<_, i32>(7)? == 1,
        viewing_count: row.get::<_, i64>(8)? as u32,
        match_state: row.get(9)?,
        source_type: row.get(10)?,
        last_watched_at: row.get(11)?,
        backdrop: backdrop_url(row.get(12)?),
        overview: row.get(13)?,
    })
}

fn parse_raw_title(raw: &str) -> String {
    parse_activity_payload(raw).0
}

pub fn get_film(db: &Database, id: &str) -> Result<FilmDetail, String> {
    if let Some(detail) = get_library_film(db, id)? {
        return Ok(detail);
    }
    get_catalog_film(db, id)?.ok_or_else(|| "Film not found".to_string())
}

pub fn resolve_source_movie_ids(db: &Database, id: &str) -> Result<Vec<String>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT smr.id
            FROM source_movie_records smr
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            LEFT JOIN movies m ON m.id = ml.movie_id
            WHERE smr.id = ?1
               OR ml.movie_id = ?1
               OR (smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')) = ?1
               OR CAST(m.tmdb_id AS TEXT) = ?1
               OR ('tmdb:' || CAST(m.tmdb_id AS TEXT)) = ?1
            "#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![id], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn get_library_film(db: &Database, id: &str) -> Result<Option<FilmDetail>, String> {
    let row = db
        .conn()
        .query_row(
            r#"
            SELECT
              COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')),
              CASE WHEN m.tmdb_media_type = 'tv' THEN smr.raw_identity ELSE COALESCE(m.canonical_title, smr.raw_identity) END,
              CASE WHEN m.tmdb_media_type = 'tv' THEN smr.release_year ELSE COALESCE(m.release_year, smr.release_year) END,
              ums.current_rating,
              COALESCE(m.poster_override_url, m.poster_path, smr.cached_poster_url, json_extract(smr.raw_identity, '$.poster')),
              COALESCE(m.backdrop_override_url, m.backdrop_path),
              m.overview,
              m.runtime,
              m.genres_json,
              COALESCE(ml.match_state, 'unmatched'),
              smr.source_type,
              m.vote_average,
              m.vote_count,
              m.reviews_json,
              m.cast_json,
              m.crew_json,
              m.similar_json,
              m.tmdb_id,
              m.tagline,
              m.collection_name,
              m.collection_json,
              m.credits_json,
              m.production_companies_json,
              m.keywords_json,
              m.videos_json
            FROM source_movie_records smr
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            LEFT JOIN movies m ON m.id = ml.movie_id
            LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
            WHERE smr.id = ?1
               OR ml.movie_id = ?1
               OR (smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')) = ?1
               OR CAST(m.tmdb_id AS TEXT) = ?1
               OR ('tmdb:' || CAST(m.tmdb_id AS TEXT)) = ?1
            ORDER BY CASE WHEN ml.match_state = 'confirmed' THEN 0 ELSE 1 END, ums.last_watched_at DESC
            LIMIT 1
            "#,
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<i32>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<f64>>(11)?,
                    row.get::<_, Option<i32>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<i64>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, Option<String>>(21)?,
                    row.get::<_, Option<String>>(22)?,
                    row.get::<_, Option<String>>(23)?,
                    row.get::<_, Option<String>>(24)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some(row) = row else {
        return Ok(None);
    };

    let source_ids = resolve_source_movie_ids(db, id)?;
    let title = parse_raw_title(&row.1);
    let your_history = viewing_history(db, &source_ids)?;
    let legacy_crew = json_vec(row.15.as_deref());
    let (cast, crew) = structured_credits(row.21.as_deref(), row.14.as_deref(), row.15.as_deref());
    let companies = production_companies(row.22.as_deref());
    let connections = film_connections(db, &cast, &crew, &companies, row.17)?;
    let similar = relink_catalog_items(db, parse_related(row.16.as_deref()));
    let collection = relink_catalog_items(db, parse_related(row.20.as_deref()));
    let trailers = film_trailers(row.24.as_deref());

    Ok(Some(FilmDetail {
        id: row.0,
        title: title.clone(),
        year: row.2,
        current_rating: row.3,
        poster: poster_url(row.4.clone()),
        backdrop: backdrop_url(row.5.clone()),
        overview: row.6,
        runtime: row.7,
        genres: json_vec(row.8.as_deref()),
        match_state: row.9,
        source_identity: row.10,
        your_history,
        friends: get_friend_activity_for_movie(db, &title, row.2)?,
        tmdb_id: row.17,
        tmdb_vote_average: row.11,
        tmdb_vote_count: row.12,
        tmdb_reviews: json_vec(row.13.as_deref()),
        tagline: row.18.filter(|s| !s.is_empty()),
        directors: directors_from_people(&crew, &legacy_crew),
        cast: cast.clone(),
        crew,
        companies: companies.clone(),
        keywords: keyword_names(row.23.as_deref()),
        connections,
        collection_name: row.19.filter(|s| !s.is_empty()),
        collection,
        similar,
        trailers,
        collection_hydrated: row.20.is_some(),
        detail_metadata_hydrated: structured_metadata_hydrated(
            row.21.as_deref(),
            row.22.as_deref(),
            row.24.as_deref(),
        ),
    }))
}

fn get_catalog_film(db: &Database, id: &str) -> Result<Option<FilmDetail>, String> {
    let tmdb_id = parse_tmdb_ref(id);
    let row = db
        .conn()
        .query_row(
            r#"
            SELECT id, canonical_title, release_year,
                   COALESCE(poster_override_url, poster_path),
                   COALESCE(backdrop_override_url, backdrop_path), overview, runtime,
                   genres_json, vote_average, vote_count, reviews_json, cast_json, crew_json,
                   similar_json, tmdb_id, tagline, collection_name, collection_json, credits_json,
                   production_companies_json, keywords_json, videos_json
            FROM movies
            WHERE id = ?1
               OR CAST(tmdb_id AS TEXT) = ?1
               OR ('tmdb:' || CAST(tmdb_id AS TEXT)) = ?1
            LIMIT 1
            "#,
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i32>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<f64>>(8)?,
                    row.get::<_, Option<i32>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, Option<String>>(21)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some(row) = row else {
        return Ok(None);
    };

    let legacy_crew = json_vec(row.12.as_deref());
    let (cast, crew) = structured_credits(row.18.as_deref(), row.11.as_deref(), row.12.as_deref());
    let companies = production_companies(row.19.as_deref());
    let connections = film_connections(db, &cast, &crew, &companies, row.14)?;
    let catalog_id = tmdb_id
        .or(row.14)
        .map(|n| format!("tmdb:{n}"))
        .unwrap_or(row.0);
    Ok(Some(FilmDetail {
        id: catalog_id,
        title: row.1,
        year: row.2,
        current_rating: None,
        poster: poster_url(row.3.clone()),
        backdrop: backdrop_url(row.4.clone()),
        overview: row.5,
        runtime: row.6,
        genres: json_vec(row.7.as_deref()),
        match_state: "catalog".into(),
        source_identity: "tmdb".into(),
        your_history: Vec::new(),
        friends: Vec::new(),
        tmdb_id: row.14,
        tmdb_vote_average: row.8,
        tmdb_vote_count: row.9,
        tmdb_reviews: json_vec(row.10.as_deref()),
        tagline: row.15.filter(|s| !s.is_empty()),
        directors: directors_from_people(&crew, &legacy_crew),
        cast: cast.clone(),
        crew,
        companies: companies.clone(),
        keywords: keyword_names(row.20.as_deref()),
        connections,
        collection_name: row.16.filter(|s| !s.is_empty()),
        collection: parse_related(row.17.as_deref()),
        similar: parse_related(row.13.as_deref()),
        trailers: film_trailers(row.21.as_deref()),
        collection_hydrated: row.17.is_some(),
        detail_metadata_hydrated: structured_metadata_hydrated(
            row.18.as_deref(),
            row.19.as_deref(),
            row.21.as_deref(),
        ),
    }))
}

fn viewing_history(db: &Database, source_ids: &[String]) -> Result<Vec<ViewingHistoryItem>, String> {
    if source_ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = source_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, occurred_at, published_at, rewatch,
              (SELECT rating FROM rating_events re WHERE re.source_movie_record_id = v.source_movie_record_id
               AND COALESCE(re.occurred_at, re.observed_at) = COALESCE(v.occurred_at, v.observed_at) LIMIT 1),
              source_type
         FROM viewings v
         LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
         WHERE v.source_movie_record_id IN ({marks})
           AND COALESCE(vp.counted, 1) = 1
         ORDER BY COALESCE(occurred_at, observed_at) DESC,
                  COALESCE(event_timestamp(published_at), event_timestamp(observed_at)) DESC, v.id"
    );
    let mut stmt = db.conn().prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_from_iter(source_ids.iter()), |row| {
            Ok(ViewingHistoryItem {
                id: row.get(0)?,
                occurred_at: row.get(1)?,
                published_at: row.get(2)?,
                rewatch: row.get::<_, i32>(3)? == 1,
                rating: row.get(4)?,
                source: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn directors_from_crew(crew: &[String]) -> Vec<String> {
    crew.iter()
        .filter_map(|entry| entry.strip_suffix(" (Director)").map(str::to_string))
        .collect()
}

fn directors_from_people(crew: &[FilmCrewMember], legacy_crew: &[String]) -> Vec<String> {
    let mut directors: Vec<String> = crew
        .iter()
        .filter(|member| member.job.eq_ignore_ascii_case("director"))
        .map(|member| member.name.clone())
        .collect();
    directors.sort();
    directors.dedup();
    if directors.is_empty() {
        directors_from_crew(legacy_crew)
    } else {
        directors
    }
}

fn structured_credits(
    raw: Option<&str>,
    legacy_cast: Option<&str>,
    legacy_crew: Option<&str>,
) -> (Vec<FilmCastMember>, Vec<FilmCrewMember>) {
    if let Some(raw) = raw {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            let cast = serde_json::from_value::<Vec<FilmCastMember>>(value["cast"].clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|member| !member.name.trim().is_empty())
                .map(|mut member| {
                    member.profile = tmdb_image_url(member.profile, "w185");
                    member
                })
                .collect::<Vec<_>>();
            let crew = serde_json::from_value::<Vec<FilmCrewMember>>(value["crew"].clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|member| !member.name.trim().is_empty() && !member.job.trim().is_empty())
                .map(|mut member| {
                    member.profile = tmdb_image_url(member.profile, "w185");
                    member
                })
                .collect::<Vec<_>>();
            if !cast.is_empty() || !crew.is_empty() {
                return (cast, crew);
            }
        }
    }

    let cast = json_vec(legacy_cast)
        .into_iter()
        .enumerate()
        .filter_map(|(order, entry)| {
            let (name, character) = entry
                .split_once(" as ")
                .map(|(name, character)| (name.trim(), Some(character.trim().to_string())))
                .unwrap_or((entry.trim(), None));
            (!name.is_empty()).then(|| FilmCastMember {
                tmdb_id: None,
                name: name.to_string(),
                profile: None,
                character: character.filter(|value| !value.is_empty()),
                order: Some(order as i32),
            })
        })
        .collect();
    let crew = json_vec(legacy_crew)
        .into_iter()
        .filter_map(|entry| {
            let (name, job) = entry.rsplit_once(" (")?;
            let job = job.trim_end_matches(')').trim();
            (!name.trim().is_empty() && !job.is_empty()).then(|| FilmCrewMember {
                tmdb_id: None,
                name: name.trim().to_string(),
                profile: None,
                department: None,
                job: job.to_string(),
            })
        })
        .collect();
    (cast, crew)
}

fn production_companies(raw: Option<&str>) -> Vec<ProductionCompany> {
    raw.and_then(|value| serde_json::from_str::<Vec<ProductionCompany>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|company| !company.name.trim().is_empty())
        .map(|mut company| {
            company.logo = tmdb_image_url(company.logo, "w185");
            company
        })
        .collect()
}

fn keyword_names(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|value| serde_json::from_str::<Vec<serde_json::Value>>(value).ok())
        .map(|keywords| {
            keywords
                .into_iter()
                .filter_map(|keyword| keyword["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn structured_metadata_hydrated(
    credits: Option<&str>,
    companies: Option<&str>,
    videos: Option<&str>,
) -> bool {
    credits
        .map(|value| value.contains("\"detailVersion\":1"))
        .unwrap_or(false)
        && companies.is_some()
        && videos.is_some()
}

fn film_trailers(raw: Option<&str>) -> Vec<FilmTrailer> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<FilmTrailer>>(raw).unwrap_or_default()
}

#[derive(Clone)]
struct ConnectionEntity {
    kind: &'static str,
    id: String,
    name: String,
    roles: Vec<String>,
}

fn entity_key(kind: &str, tmdb_id: Option<i64>, name: &str) -> String {
    match tmdb_id {
        Some(id) => format!("{kind}:{id}"),
        None => format!("{kind}:{}", name.trim().to_ascii_lowercase()),
    }
}

fn film_connections(
    db: &Database,
    cast: &[FilmCastMember],
    crew: &[FilmCrewMember],
    companies: &[ProductionCompany],
    detail_tmdb_id: Option<i64>,
) -> Result<Vec<FilmConnection>, String> {
    let mut entities: HashMap<String, ConnectionEntity> = HashMap::new();
    for member in cast {
        let key = entity_key("person", member.tmdb_id, &member.name);
        let entry = entities.entry(key.clone()).or_insert_with(|| ConnectionEntity {
            kind: "person",
            id: key,
            name: member.name.clone(),
            roles: Vec::new(),
        });
        if !entry.roles.iter().any(|role| role == "Cast") {
            entry.roles.push("Cast".into());
        }
    }
    for member in crew {
        let key = entity_key("person", member.tmdb_id, &member.name);
        let entry = entities.entry(key.clone()).or_insert_with(|| ConnectionEntity {
            kind: "person",
            id: key,
            name: member.name.clone(),
            roles: Vec::new(),
        });
        if !entry.roles.iter().any(|role| role == &member.job) {
            entry.roles.push(member.job.clone());
        }
    }
    for company in companies {
        let key = entity_key("company", company.tmdb_id, &company.name);
        entities.entry(key.clone()).or_insert_with(|| ConnectionEntity {
            kind: "company",
            id: key,
            name: company.name.clone(),
            roles: vec!["Production company".into()],
        });
    }
    if entities.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT
              COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), '')),
              CASE WHEN m.tmdb_media_type = 'tv' THEN smr.raw_identity ELSE COALESCE(m.canonical_title, smr.raw_identity) END,
              ums.current_rating,
              COALESCE(m.poster_override_url, m.poster_path),
              m.tmdb_id,
              m.credits_json,
              m.production_companies_json
            FROM source_movie_records smr
            JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            LEFT JOIN movies m ON m.id = ml.movie_id
            WHERE ums.current_rating IS NOT NULL
            "#,
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| error.to_string())?;

    let rows = rows.filter_map(|row| row.ok()).collect::<Vec<_>>();
    let ratings = rows.iter().map(|row| row.2).collect::<Vec<_>>();
    if ratings.is_empty() {
        return Ok(Vec::new());
    }
    let baseline = ratings.iter().sum::<f64>() / ratings.len() as f64;
    let mut evidence: HashMap<String, Vec<ConnectionFilm>> = HashMap::new();
    for (id, raw_title, rating, poster, tmdb_id, credits, stored_companies) in rows {
        if detail_tmdb_id.is_some() && detail_tmdb_id == tmdb_id {
            continue;
        }
        let (local_cast, local_crew) = structured_credits(credits.as_deref(), None, None);
        let local_companies = production_companies(stored_companies.as_deref());
        let mut matched = std::collections::HashSet::new();
        for member in local_cast {
            matched.insert(entity_key("person", member.tmdb_id, &member.name));
        }
        for member in local_crew {
            matched.insert(entity_key("person", member.tmdb_id, &member.name));
        }
        for company in local_companies {
            matched.insert(entity_key("company", company.tmdb_id, &company.name));
        }
        let item = ConnectionFilm {
            id,
            title: parse_raw_title(&raw_title),
            rating,
            poster: poster_url(poster),
        };
        for key in matched {
            if entities.contains_key(&key) {
                let values = evidence.entry(key).or_default();
                if !values.iter().any(|value| value.id == item.id) {
                    values.push(item.clone());
                }
            }
        }
    }

    let mut connections = evidence
        .into_iter()
        .filter_map(|(key, mut films)| {
            let entity = entities.get(&key)?;
            films.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
            let shared_count = films.len() as u32;
            let average_rating = films.iter().map(|film| film.rating).sum::<f64>() / shared_count as f64;
            let tone = if shared_count >= 3 && average_rating >= baseline + 0.35 {
                "positive"
            } else if shared_count >= 3 && average_rating <= baseline - 0.35 {
                "negative"
            } else if shared_count >= 3 {
                "mixed"
            } else {
                "unknown"
            };
            let confidence = match shared_count {
                0 => "No shared films",
                1 | 2 => "Limited evidence",
                3 | 4 => "Some history",
                _ => "Strong sample",
            };
            Some(FilmConnection {
                entity_kind: entity.kind.into(),
                entity_id: entity.id.clone(),
                name: entity.name.clone(),
                roles: entity.roles.clone(),
                shared_count,
                average_rating: (average_rating * 10.0).round() / 10.0,
                confidence: confidence.into(),
                tone: tone.into(),
                evidence: films,
            })
        })
        .collect::<Vec<_>>();
    let tone_rank = |tone: &str| match tone {
        "positive" => 0,
        "negative" => 1,
        "mixed" => 2,
        _ => 3,
    };
    connections.sort_by(|a, b| {
        tone_rank(&a.tone)
            .cmp(&tone_rank(&b.tone))
            .then_with(|| b.shared_count.cmp(&a.shared_count))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(connections)
}

fn parse_related(raw: Option<&str>) -> Vec<LibraryItem> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return Vec::new();
    };
    if let Ok(items) = serde_json::from_str::<Vec<LibraryItem>>(raw) {
        if items.iter().any(|item| !item.title.is_empty()) {
            return items;
        }
    }
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .ok()
        .map(|vals| {
            vals.iter()
                .filter_map(library_item_from_tmdb_value)
                .collect()
        })
        .unwrap_or_default()
}

fn relink_catalog_items(db: &Database, items: Vec<LibraryItem>) -> Vec<LibraryItem> {
    items
        .into_iter()
        .map(|mut item| {
            if let Some(tmdb_id) = parse_tmdb_ref(&item.id) {
                if let Ok(Some(key)) = library_key_for_tmdb(db, tmdb_id) {
                    item.id = key;
                }
            }
            item
        })
        .collect()
}

fn library_key_for_tmdb(db: &Database, tmdb_id: i64) -> Result<Option<String>, String> {
    db.conn()
        .query_row(
            &format!(
                "SELECT {FILM_KEY}
                 FROM source_movie_records smr
                 JOIN movie_links ml ON ml.source_movie_record_id = smr.id
                 JOIN movies m ON m.id = ml.movie_id
                 WHERE m.tmdb_id = ?1
                 LIMIT 1"
            ),
            params![tmdb_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
}

fn json_vec(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

fn get_friend_activity_for_movie(
    db: &Database,
    title: &str,
    year: Option<i32>,
) -> Result<Vec<FriendActivityItem>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT f.username, fa.raw_payload, fa.rating, fa.review, fa.watched_at, fa.published_at
            FROM friend_activity fa
            JOIN friends f ON f.id = fa.friend_id
            WHERE LOWER(fa.raw_payload) LIKE ?1
            ORDER BY COALESCE(fa.watched_at, fa.published_at) DESC
            LIMIT 20
            "#,
        )
        .map_err(|e| e.to_string())?;
    let needle = format!("%{}%", title.to_lowercase());
    let rows = stmt
        .query_map(params![needle], |row| {
            Ok(FriendActivityItem {
                username: row.get(0)?,
                title: title.to_string(),
                year,
                rating: row.get(2)?,
                review: row.get(3)?,
                watched_at: row.get(4)?,
                published_at: row.get(5)?,
                poster: None,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn get_home(db: &Database) -> Result<HomeViewModel, String> {
    let page = get_library(
        db,
        &LibraryQuery {
            search: None,
            sort: Some("recent".into()),
            filter: None,
            limit: Some(36),
            offset: Some(0),
        },
    )?;
    let top = get_library(
        db,
        &LibraryQuery {
            search: None,
            sort: Some("rating".into()),
            filter: None,
            limit: Some(12),
            offset: Some(0),
        },
    )?;
    Ok(HomeViewModel {
        coverage: db.compute_coverage()?,
        recent: page.items,
        top_rated: top.items,
        friend_feed: get_friend_feed(db, 30)?,
    })
}

pub fn get_friend_feed(db: &Database, limit: u32) -> Result<Vec<FriendActivityItem>, String> {
    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT f.username, fa.raw_payload, fa.rating, fa.review, fa.watched_at, fa.published_at, fa.poster_url
            FROM friend_activity fa
            JOIN friends f ON f.id = fa.friend_id
            ORDER BY COALESCE(fa.watched_at, fa.published_at) DESC
            LIMIT ?1
            "#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit], |row| {
            let raw: String = row.get(1)?;
            let stored_poster: Option<String> = row.get(6)?;
            let (title, year) = parse_activity_payload(&raw);
            Ok(FriendActivityItem {
                username: row.get(0)?,
                title,
                year,
                rating: row.get(2)?,
                review: row.get(3)?,
                watched_at: row.get(4)?,
                published_at: row.get(5)?,
                poster: stored_poster.or_else(|| poster_from_rss_body(&raw)),
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LibraryQuery;
    use crate::storage::db::Database;

    #[test]
    fn library_query_without_search_binds_limit_offset() {
        let db = Database::in_memory().expect("db");
        let page = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: Some("recent".into()),
                filter: None,
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("library page");
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 0);
    }

    #[test]
    fn structured_credits_keep_person_fields_and_fall_back_to_legacy_data() {
        let raw = r#"{
          "detailVersion": 1,
          "cast": [{"tmdbId": 7, "name": "Ada Actor", "profile": "/ada.jpg", "character": "Lead", "order": 0}],
          "crew": [{"tmdbId": 8, "name": "Dee Director", "profile": "/dee.jpg", "department": "Directing", "job": "Director"}]
        }"#;
        let (cast, crew) = structured_credits(Some(raw), None, None);
        assert_eq!(cast[0].tmdb_id, Some(7));
        assert_eq!(cast[0].character.as_deref(), Some("Lead"));
        assert!(cast[0].profile.as_deref().unwrap_or_default().contains("/w185/ada.jpg"));
        assert_eq!(crew[0].department.as_deref(), Some("Directing"));

        let (legacy_cast, legacy_crew) = structured_credits(
            None,
            Some(r#"["Ada Actor as Lead"]"#),
            Some(r#"["Dee Director (Director)"]"#),
        );
        assert_eq!(legacy_cast[0].name, "Ada Actor");
        assert_eq!(legacy_crew[0].job, "Director");
    }

    #[test]
    fn stats_snapshot_uses_viewings_and_enriched_movie_metadata() {
        use rusqlite::params;

        let db = Database::in_memory().expect("db");
        db.conn()
            .execute(
                "INSERT INTO movies (id, canonical_title, runtime, genres_json) VALUES (?1, ?2, ?3, ?4)",
                params!["movie-1", "Example Film", 100, r#"["Adventure","Drama"]"#],
            )
            .expect("movie");
        db.conn()
            .execute(
                "INSERT INTO source_movie_records (id, source_type, source_record_key, normalized_title, raw_identity, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params!["source-1", "letterboxd_export", "film|example", "example film", "{}", "2026-01-01T00:00:00Z"],
            )
            .expect("source movie");
        db.conn()
            .execute(
                "INSERT INTO movie_links (source_movie_record_id, movie_id, match_state)
                 VALUES (?1, ?2, 'confirmed')",
                params!["source-1", "movie-1"],
            )
            .expect("link");
        db.conn()
            .execute(
                "INSERT INTO user_movie_state (source_movie_record_id, movie_id, watched, current_rating, projection_updated_at)
                 VALUES (?1, ?2, 1, ?3, ?4)",
                params!["source-1", "movie-1", 4.5, "2026-01-01T00:00:00Z"],
            )
            .expect("state");

        let current_month = Utc::now().format("%Y-%m").to_string();
        let previous_month = (Utc::now() - chrono::Duration::days(31))
            .format("%Y-%m")
            .to_string();
        for (id, record_key, occurred_at, rewatch) in [
            ("viewing-1", "viewing|1", format!("{previous_month}-15"), 0),
            ("viewing-2", "viewing|2", format!("{current_month}-15"), 1),
        ] {
            db.conn()
                .execute(
                    "INSERT INTO viewings (id, source_movie_record_id, source_record_key, occurred_at, observed_at, source_type, rewatch)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![id, "source-1", record_key, occurred_at, occurred_at, "letterboxd_export", rewatch],
                )
                .expect("viewing");
        }

        let stats = get_stats(&db).expect("stats");
        assert_eq!(stats.rewatch_count, 1);
        assert_eq!(stats.total_runtime_minutes, 200);
        assert_eq!(stats.runtime_viewings, 2);
        assert_eq!(stats.metadata_movies, 1);
        assert_eq!(stats.viewing_months.len(), 24);
        assert_eq!(stats.viewing_months.iter().map(|bucket| bucket.count).sum::<u32>(), 2);
        assert_eq!(stats.genres.len(), 2);
        assert!(stats.genres.iter().all(|genre| genre.count == 1));
        assert!(stats
            .genres
            .iter()
            .all(|genre| genre.average_rating == Some(4.5)));
    }

    #[test]
    fn recent_library_uses_publication_time_within_watched_day_before_pagination() {
        use crate::letterboxd::import::upsert_source_movie;
        use crate::letterboxd::posters::SourceMovieMeta;

        let mut db = Database::in_memory().unwrap();
        let tx = db.transaction().unwrap();
        // Deliberately insert out of order, with a shared observation time.
        // The two Grinch entries cross a UTC date boundary in their RSS timezone.
        for (title, day, published) in [
            ("Halloween is Grinch Night", "2026-09-06", Some("Mon, 7 Sep 2026 16:35:03 +1200")),
            ("Mayday", "2026-09-06", Some("Sun, 6 Sep 2026 19:15:15 -0700")),
            ("Onslaught", "2026-09-06", None),
            ("The Grinch Grinches the Cat in the Hat", "2026-09-06", Some("2026-09-07T05:01:06Z")),
            ("Speed Racer", "2026-09-06", Some("Mon, 7 Sep 2026 13:00:24 +1200")),
            ("Earlier watched, later logged", "2026-09-05", Some("Tue, 8 Sep 2026 12:00:00 +1200")),
        ] {
            let id = upsert_source_movie(&tx, "letterboxd_rss", title, title, Some(2026), "", &SourceMovieMeta::default()).unwrap();
            tx.execute(
                "INSERT INTO viewings (id, source_movie_record_id, source_record_key, occurred_at, published_at, observed_at, source_type, rewatch)
                 VALUES (?1, ?2, ?1, ?3, ?4, '2026-09-07T00:43:35Z', 'letterboxd_rss', 0)",
                params![title, id, day, published],
            ).unwrap();
        }
        Database::rebuild_projections(&tx).unwrap();
        tx.commit().unwrap();
        let mut query = LibraryQuery { search: None, sort: Some("recent".into()), filter: None, limit: Some(100), offset: Some(0) };
        let page = get_library(&db, &query).unwrap();
        let titles: Vec<_> = page.items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, vec!["The Grinch Grinches the Cat in the Hat", "Halloween is Grinch Night", "Mayday", "Speed Racer", "Onslaught", "Earlier watched, later logged"]);
        query.limit = Some(2);
        query.offset = Some(1);
        let page = get_library(&db, &query).unwrap();
        assert_eq!(page.items[0].title, "Halloween is Grinch Night");
        assert_eq!(page.items[1].title, "Mayday");
        assert_eq!(page.total, 6);
    }

    #[test]
    fn library_collapses_same_day_linked_source_records_to_one_effective_viewing() {
        use crate::letterboxd::import::upsert_source_movie;
        use crate::letterboxd::posters::SourceMovieMeta;
        use rusqlite::params;
        use uuid::Uuid;

        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        let movie_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO movies (id, canonical_title, release_year) VALUES (?1, ?2, ?3)",
            params![movie_id, "Ant-Man", 2015],
        )
        .expect("movie");

        let smr_a = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|ant-man",
            "Ant-Man",
            Some(2015),
            "https://letterboxd.com/film/ant-man/",
            &SourceMovieMeta::default(),
        )
        .expect("smr a");
        let smr_b = upsert_source_movie(
            &tx,
            "letterboxd_rss",
            "rss|ant-man",
            "Ant-Man",
            Some(2015),
            "https://letterboxd.com/film/ant-man/",
            &SourceMovieMeta::default(),
        )
        .expect("smr b");

        for smr_id in [&smr_a, &smr_b] {
            tx.execute(
                "UPDATE movie_links SET movie_id = ?2, match_state = 'confirmed'
                 WHERE source_movie_record_id = ?1",
                params![smr_id, movie_id],
            )
            .expect("link");
            tx.execute(
                "INSERT INTO viewings (
                  id, source_movie_record_id, source_record_key, observed_at, source_type, rewatch
                ) VALUES (?1, ?2, ?3, ?4, 'letterboxd_export', 0)",
                params![
                    Uuid::new_v4().to_string(),
                    smr_id,
                    format!("viewing-{smr_id}"),
                    "2024-01-01T00:00:00Z"
                ],
            )
            .expect("viewing");
        }

        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let page = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: None,
                filter: None,
                limit: Some(100),
                offset: Some(0),
            },
        )
        .expect("library");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].viewing_count, 1);
    }

    #[test]
    fn get_film_opens_matched_library_id() {
        use crate::letterboxd::import::upsert_source_movie;
        use crate::letterboxd::posters::SourceMovieMeta;
        use rusqlite::params;
        use uuid::Uuid;

        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        let movie_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO movies (id, canonical_title, release_year, tmdb_id, poster_path, backdrop_path, similar_json, collection_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                movie_id,
                "Ant-Man",
                2015,
                102899,
                "/poster.jpg",
                "/backdrop.jpg",
                r#"[{"id":27205,"title":"Inception","release_date":"2010-07-16","poster_path":"/inception.jpg"}]"#,
                "[]"
            ],
        )
        .expect("movie");
        let smr = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|ant-man",
            "Ant-Man",
            Some(2015),
            "https://letterboxd.com/film/ant-man/",
            &SourceMovieMeta::default(),
        )
        .expect("smr");
        tx.execute(
            "UPDATE movie_links SET movie_id = ?2, match_state = 'confirmed' WHERE source_movie_record_id = ?1",
            params![smr, movie_id],
        )
        .expect("link");
        tx.execute(
            "INSERT INTO viewings (
              id, source_movie_record_id, source_record_key, observed_at, source_type, rewatch
            ) VALUES (?1, ?2, ?3, ?4, 'letterboxd_export', 0)",
            params![
                Uuid::new_v4().to_string(),
                smr,
                format!("viewing-{smr}"),
                "2024-01-01T00:00:00Z"
            ],
        )
        .expect("viewing");
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let page = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: None,
                filter: None,
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("library");
        assert_eq!(page.items.len(), 1);
        let detail = get_film(&db, &page.items[0].id).expect("film");
        assert_eq!(detail.title, "Ant-Man");
        assert!(detail.backdrop.unwrap().contains("/original/"));
        assert_eq!(detail.similar.len(), 1);
        assert_eq!(detail.similar[0].title, "Inception");
    }

    #[test]
    fn tv_special_keeps_the_source_title_while_using_series_metadata() {
        let db = Database::in_memory().expect("db");
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, release_year, tmdb_id, tmdb_media_type, overview, genres_json, credits_json, production_companies_json, keywords_json, similar_json, collection_json)
                 VALUES ('series', 'Solar Opposites', 2020, 97680, 'tv', 'Series details', '[]', '{}', '[]', '[]', '{}', '[]')",
                [],
            )
            .expect("series");
        db.conn()
            .execute(
                "INSERT INTO source_movie_records(id, source_type, source_record_key, normalized_title, release_year, raw_identity, created_at)
                 VALUES ('special', 'letterboxd_export', 'special', 'solar special', 2025, '{\"title\":\"An Earth Shatteringly Romantic Solar Valentines Day Opposites Special\"}', datetime('now'))",
                [],
            )
            .expect("special");
        db.conn()
            .execute(
                "INSERT INTO movie_links(source_movie_record_id, movie_id, match_state)
                 VALUES ('special', 'series', 'confirmed')",
                [],
            )
            .expect("link");

        let detail = get_film(&db, "special").expect("detail");
        assert_eq!(detail.title, "An Earth Shatteringly Romantic Solar Valentines Day Opposites Special");
        assert_eq!(detail.year, Some(2025));
        assert_eq!(detail.overview.as_deref(), Some("Series details"));
    }

    #[test]
    fn get_film_opens_unmatched_title_year_key() {
        use crate::letterboxd::import::upsert_source_movie;
        use crate::letterboxd::posters::SourceMovieMeta;
        use rusqlite::params;
        use uuid::Uuid;

        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        let smr = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|mystery",
            "Mystery Film",
            Some(1999),
            "https://letterboxd.com/film/mystery-film/",
            &SourceMovieMeta::default(),
        )
        .expect("smr");
        tx.execute(
            "INSERT INTO viewings (
              id, source_movie_record_id, source_record_key, observed_at, source_type, rewatch
            ) VALUES (?1, ?2, ?3, ?4, 'letterboxd_export', 0)",
            params![
                Uuid::new_v4().to_string(),
                smr,
                format!("viewing-{smr}"),
                "2024-01-01T00:00:00Z"
            ],
        )
        .expect("viewing");
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let page = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: None,
                filter: None,
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("library");
        assert_eq!(page.items[0].id.contains(':'), true);
        let detail = get_film(&db, &page.items[0].id).expect("unmatched film");
        assert_eq!(detail.title, "Mystery Film");
        assert_eq!(detail.your_history.len(), 1);
    }

    #[test]
    fn watchlist_filter_returns_unwatched_titles_beyond_recent_limit() {
        use crate::letterboxd::import::upsert_source_movie;
        use crate::letterboxd::posters::SourceMovieMeta;
        use rusqlite::params;
        use uuid::Uuid;

        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");

        for i in 0..20 {
            let title = format!("Watched {i}");
            let smr = upsert_source_movie(
                &tx,
                "letterboxd_export",
                &format!("export|watched-{i}"),
                &title,
                Some(2000 + i),
                &format!("https://letterboxd.com/film/watched-{i}/"),
                &SourceMovieMeta::default(),
            )
            .expect("watched smr");
            tx.execute(
                "INSERT INTO viewings (
                  id, source_movie_record_id, source_record_key, observed_at, source_type, rewatch
                ) VALUES (?1, ?2, ?3, ?4, 'letterboxd_export', 0)",
                params![
                    Uuid::new_v4().to_string(),
                    smr,
                    format!("viewing-{smr}"),
                    format!("2024-01-{:02}T00:00:00Z", i + 1)
                ],
            )
            .expect("viewing");
        }

        let watchlist = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|watchlist-only",
            "Unwatched Watchlist Film",
            Some(1994),
            "https://letterboxd.com/film/unwatched-watchlist-film/",
            &SourceMovieMeta::default(),
        )
        .expect("watchlist smr");
        tx.execute(
            "UPDATE source_movie_records SET on_watchlist = 1 WHERE id = ?1",
            params![watchlist],
        )
        .expect("flag");
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let recent = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: Some("year".into()),
                filter: None,
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("recent page");
        assert_eq!(recent.items.len(), 10);
        assert!(!recent.items.iter().any(|f| f.title == "Unwatched Watchlist Film"));

        let page = get_library(
            &db,
            &LibraryQuery {
                search: None,
                sort: Some("recent".into()),
                filter: Some("watchlist".into()),
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("watchlist page");
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Unwatched Watchlist Film");
        assert!(page.items[0].watchlist);
        assert!(!page.items[0].watched);
    }
}
