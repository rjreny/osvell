use super::schema::{MIGRATION_SQL, SCHEMA_VERSION};
use crate::models::{AppSession, LibraryCoverage};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;
use std::time::Duration;

const FILM_KEY: &str =
    "COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), ''))";

fn apply_connection_pragmas(conn: &Connection) -> Result<(), String> {
    // RSS uses RFC 2822 while local timestamps use RFC 3339. SQLite cannot
    // parse RSS dates itself, and sorting their raw text is not chronological.
    conn.create_scalar_function(
        "event_timestamp",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value = ctx.get::<Option<String>>(0)?;
            Ok(value.and_then(|value| {
                chrono::DateTime::parse_from_rfc3339(value.trim())
                    .or_else(|_| chrono::DateTime::parse_from_rfc2822(value.trim()))
                    .ok()
                    .map(|date| date.timestamp_millis())
            }))
        },
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        apply_connection_pragmas(&conn)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        apply_connection_pragmas(&conn)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), String> {
        self.conn
            .execute_batch(MIGRATION_SQL)
            .map_err(|e| e.to_string())?;
        self.apply_schema_patches()?;
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())
    }

    fn apply_schema_patches(&self) -> Result<(), String> {
        let version: i32 = self
            .get_meta("schema_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        if version < 2 {
            let _ = self.conn.execute(
                "ALTER TABLE source_movie_records ADD COLUMN cached_poster_url TEXT",
                [],
            );
            let _ = self.conn.execute(
                "ALTER TABLE source_movie_records ADD COLUMN poster_fetch_failed INTEGER NOT NULL DEFAULT 0",
                [],
            );
            let _ = self
                .conn
                .execute("ALTER TABLE friend_activity ADD COLUMN poster_url TEXT", []);
        }
        if version < 3 {
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN tagline TEXT", []);
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN collection_name TEXT", []);
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN collection_json TEXT", []);
        }
        if version < 4 {
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN keywords_json TEXT", []);
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN credits_json TEXT", []);
            let _ = self.conn.execute(
                "CREATE TABLE IF NOT EXISTS person_credits (
                  person_id INTEGER PRIMARY KEY,
                  credits_json TEXT NOT NULL,
                  fetched_at TEXT NOT NULL
                )",
                [],
            );
        }
        if version < 5 {
            let _ = self.conn.execute(
                r#"CREATE TABLE IF NOT EXISTS taste_feedback (
                  content_key TEXT PRIMARY KEY,
                  tmdb_id INTEGER NOT NULL,
                  media_kind TEXT NOT NULL DEFAULT 'movie',
                  action TEXT NOT NULL,
                  reason TEXT,
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                )"#,
                [],
            );
        }
        if version < 6 {
            let _ = self.conn.execute(
                r#"CREATE TABLE IF NOT EXISTS taste_run_snapshot (
                  id INTEGER PRIMARY KEY CHECK (id = 1),
                  algorithm_version TEXT NOT NULL,
                  profile_fingerprint TEXT NOT NULL,
                  library_state_fingerprint TEXT NOT NULL,
                  candidate_input_fingerprint TEXT NOT NULL,
                  scoring_fingerprint TEXT NOT NULL,
                  narrative_key TEXT NOT NULL,
                  catalog_valid_until TEXT NOT NULL,
                  scored_pool_json TEXT NOT NULL,
                  narrative_json TEXT NOT NULL,
                  created_at TEXT NOT NULL
                )"#,
                [],
            );
        }
        if version < 7 {
            let _ = self.conn.execute(
                r#"CREATE TABLE IF NOT EXISTS taste_embeddings (
                  tmdb_id INTEGER NOT NULL,
                  model TEXT NOT NULL,
                  content_hash TEXT NOT NULL,
                  dimension INTEGER NOT NULL,
                  vector_json TEXT NOT NULL,
                  updated_at TEXT NOT NULL,
                  PRIMARY KEY (tmdb_id, model)
                )"#,
                [],
            );
            let _ = self.conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_taste_embeddings_model ON taste_embeddings(model)",
                [],
            );
        }
        if version < 9 {
            self.reconcile_duplicate_history()?;
        }
        if version < 10 {
            let _ = self.conn.execute(
                "ALTER TABLE taste_feedback ADD COLUMN suppressed_until TEXT",
                [],
            );
            self.conn
                .execute_batch(
                    r#"
                    CREATE TABLE IF NOT EXISTS taste_recommendation_exposures (
                      id TEXT PRIMARY KEY,
                      run_id TEXT NOT NULL,
                      tmdb_id INTEGER NOT NULL,
                      title TEXT NOT NULL,
                      snapshot_json TEXT NOT NULL,
                      prior_candidate_exposures INTEGER NOT NULL DEFAULT 0,
                      created_at TEXT NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS taste_exposure_features (
                      exposure_id TEXT NOT NULL REFERENCES taste_recommendation_exposures(id),
                      feature_key TEXT NOT NULL,
                      PRIMARY KEY (exposure_id, feature_key)
                    );
                    CREATE TABLE IF NOT EXISTS taste_feedback_events (
                      id TEXT PRIMARY KEY,
                      exposure_id TEXT NOT NULL REFERENCES taste_recommendation_exposures(id),
                      tmdb_id INTEGER NOT NULL,
                      action TEXT NOT NULL,
                      reason TEXT,
                      target_feature_key TEXT,
                      mood_scope TEXT,
                      mood_fallback INTEGER NOT NULL DEFAULT 0,
                      requested_adjustments_json TEXT NOT NULL DEFAULT '[]',
                      applied_adjustments_json TEXT NOT NULL DEFAULT '[]',
                      feature_snapshot_json TEXT NOT NULL DEFAULT '{}',
                      feedback_signal_version TEXT NOT NULL,
                      expires_at TEXT,
                      created_at TEXT NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS idx_taste_exposures_tmdb ON taste_recommendation_exposures(tmdb_id);
                    CREATE INDEX IF NOT EXISTS idx_taste_exposure_features_key ON taste_exposure_features(feature_key);
                    CREATE INDEX IF NOT EXISTS idx_taste_feedback_events_exposure ON taste_feedback_events(exposure_id);
                    "#,
                )
                .map_err(|e| e.to_string())?;
        }
        if version < 11 {
            let _ = self.conn.execute(
                "ALTER TABLE movies ADD COLUMN production_companies_json TEXT",
                [],
            );
        }
        if version < 12 {
            let _ = self.conn.execute(
                "ALTER TABLE movies ADD COLUMN poster_override_url TEXT",
                [],
            );
            let _ = self.conn.execute(
                "ALTER TABLE movies ADD COLUMN backdrop_override_url TEXT",
                [],
            );
        }
        if version < 13 {
            let _ = self.conn.execute(
                "ALTER TABLE movies ADD COLUMN tmdb_media_type TEXT NOT NULL DEFAULT 'movie'",
                [],
            );
        }
        if version < 14 {
            let _ = self.conn.execute(
                "ALTER TABLE movie_links ADD COLUMN tmdb_checked_at TEXT",
                [],
            );
        }
        if version < 15 {
            self.backfill_viewing_projections()?;
            self.conn
                .execute("DELETE FROM taste_run_snapshot WHERE id = 1", [])
                .map_err(|e| e.to_string())?;
            self.conn
                .execute("DELETE FROM app_meta WHERE key = 'taste_report'", [])
                .map_err(|e| e.to_string())?;
        }
        if version < 16 {
            let _ = self
                .conn
                .execute("ALTER TABLE movies ADD COLUMN videos_json TEXT", []);
        }
        if version < 17 {
            self.purge_letterboxd_list_exports()?;
        }
        Ok(())
    }

    /// Remove faux films that came from Letterboxd `lists.csv` / `lists/*`
    /// (list titles misclassified as diary) and list URLs from RSS.
    fn purge_letterboxd_list_exports(&self) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;

        // source_record_key = letterboxd_export|{dataset}|v1|{fp}[|rating]
        const LIST_KEY: &str = r#"
            source_record_key LIKE 'letterboxd_export|lists.csv|%'
            OR source_record_key LIKE 'letterboxd_export|lists/%'
            OR source_record_key LIKE 'letterboxd_export|%/lists.csv|%'
            OR source_record_key LIKE 'letterboxd_export|%/lists/%'
        "#;

        tx.execute(
            &format!("DELETE FROM rating_events WHERE {LIST_KEY}"),
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(&format!("DELETE FROM viewings WHERE {LIST_KEY}"), [])
            .map_err(|e| e.to_string())?;

        // List index rows store a /list/ URL in diary_entry_id; RSS list activity
        // used the same shape before it was filtered out.
        tx.execute(
            "DELETE FROM viewings
             WHERE diary_entry_id LIKE '%/list/%'
                OR raw_payload LIKE '%/list/%'",
            [],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM import_entries
             WHERE lower(replace(source_path, '\\', '/')) = 'lists.csv'
                OR lower(replace(source_path, '\\', '/')) LIKE '%/lists.csv'
                OR lower(replace(source_path, '\\', '/')) LIKE 'lists/%'
                OR lower(replace(source_path, '\\', '/')) LIKE '%/lists/%'",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Drop orphan source rows that only existed because of list imports.
        tx.execute(
            "DELETE FROM user_movie_state
             WHERE source_movie_record_id IN (
               SELECT smr.id FROM source_movie_records smr
               WHERE NOT EXISTS (
                 SELECT 1 FROM viewings v WHERE v.source_movie_record_id = smr.id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = smr.id
               )
               AND COALESCE(smr.on_watchlist, 0) = 0
               AND json_extract(smr.raw_identity, '$.review') IS NULL
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM movie_links
             WHERE source_movie_record_id IN (
               SELECT smr.id FROM source_movie_records smr
               WHERE NOT EXISTS (
                 SELECT 1 FROM viewings v WHERE v.source_movie_record_id = smr.id
               )
               AND NOT EXISTS (
                 SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = smr.id
               )
               AND COALESCE(smr.on_watchlist, 0) = 0
               AND json_extract(smr.raw_identity, '$.review') IS NULL
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM source_movie_records
             WHERE NOT EXISTS (
               SELECT 1 FROM viewings v WHERE v.source_movie_record_id = source_movie_records.id
             )
             AND NOT EXISTS (
               SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = source_movie_records.id
             )
             AND COALESCE(on_watchlist, 0) = 0
             AND json_extract(raw_identity, '$.review') IS NULL",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Any remaining list-URI rows (watchlist/review noise) go next.
        tx.execute(
            "DELETE FROM user_movie_state
             WHERE source_movie_record_id IN (
               SELECT id FROM source_movie_records WHERE raw_identity LIKE '%/list/%'
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM movie_links
             WHERE source_movie_record_id IN (
               SELECT id FROM source_movie_records WHERE raw_identity LIKE '%/list/%'
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM rating_events
             WHERE source_movie_record_id IN (
               SELECT id FROM source_movie_records WHERE raw_identity LIKE '%/list/%'
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM viewings
             WHERE source_movie_record_id IN (
               SELECT id FROM source_movie_records WHERE raw_identity LIKE '%/list/%'
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM source_movie_records WHERE raw_identity LIKE '%/list/%'",
            [],
        )
        .map_err(|e| e.to_string())?;

        Self::rebuild_projections(&tx)?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn reconcile_duplicate_history(&self) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        // Keep source evidence intact. Duplicate source representations are
        // classified by `viewing_projections` instead of being deleted.
        Self::rebuild_viewing_projections(&tx)?;
        tx.execute(
            r#"
            UPDATE source_movie_records
            SET cached_poster_url = NULL, poster_fetch_failed = 0
            WHERE id IN (
              SELECT smr.id
              FROM source_movie_records smr
              JOIN movie_links ml ON ml.source_movie_record_id = smr.id
              JOIN movies m ON m.id = ml.movie_id
              WHERE ml.match_method IN ('tmdb_search', 'propagated')
                AND smr.normalized_title != LOWER(TRIM(m.canonical_title))
            )
            "#,
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            r#"
            UPDATE movie_links
            SET movie_id = NULL, match_state = 'unmatched', match_method = NULL,
                confidence = NULL, confirmed_at = NULL
            WHERE match_method IN ('tmdb_search', 'propagated')
              AND EXISTS (
                SELECT 1
                FROM source_movie_records smr
                JOIN movies m ON m.id = movie_links.movie_id
                WHERE smr.id = movie_links.source_movie_record_id
                  AND smr.normalized_title != LOWER(TRIM(m.canonical_title))
              )
            "#,
            [],
        )
        .map_err(|e| e.to_string())?;
        Self::rebuild_projections(&tx)?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn backfill_viewing_projections(&self) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        Self::rebuild_viewing_projections(&tx)?;
        Self::refresh_last_watched_at(&tx)?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>, String> {
        self.conn.transaction().map_err(|e| e.to_string())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO app_meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT value FROM app_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn count_table(&self, table: &str) -> Result<u32, String> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        self.conn
            .query_row(&sql, [], |row| row.get::<_, i64>(0))
            .map(|n| n as u32)
            .map_err(|e| e.to_string())
    }

    pub fn rebuild_projections(tx: &Transaction<'_>) -> Result<(), String> {
        Self::rebuild_viewing_projections(tx)?;
        tx.execute("DELETE FROM user_movie_state", [])
            .map_err(|e| e.to_string())?;
        tx.execute(
            r#"
            INSERT INTO user_movie_state (
              source_movie_record_id, movie_id, watched, watchlist, liked,
              current_rating, last_watched_at, projection_updated_at
            )
            SELECT
              smr.id,
              ml.movie_id,
              CASE WHEN EXISTS (
                SELECT 1
                FROM viewings v
                LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
                WHERE v.source_movie_record_id = smr.id
                  AND COALESCE(vp.counted, 1) = 1
              ) OR EXISTS (
                SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = smr.id
              ) OR json_extract(smr.raw_identity, '$.review') IS NOT NULL THEN 1 ELSE 0 END,
              COALESCE(smr.on_watchlist, 0),
              0,
              (
                SELECT re.rating FROM rating_events re
                WHERE re.source_movie_record_id = smr.id
                ORDER BY COALESCE(re.occurred_at, re.observed_at) DESC
                LIMIT 1
              ),
              (
                SELECT v.occurred_at
                FROM viewings v
                LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
                WHERE v.source_movie_record_id = smr.id
                  AND COALESCE(vp.counted, 1) = 1
                ORDER BY COALESCE(v.occurred_at, v.observed_at) DESC
                LIMIT 1
              ),
              ?1
            FROM source_movie_records smr
            LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
            "#,
            params![Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn rebuild_viewing_projections(tx: &Transaction<'_>) -> Result<(), String> {
        let projected_at = Utc::now().to_rfc3339();
        tx.execute("DELETE FROM viewing_projections", [])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO viewing_projections(viewing_id, counted, duplicate_reason, projected_at)
             SELECT id, 1, NULL, ?1 FROM viewings",
            params![projected_at],
        )
        .map_err(|e| e.to_string())?;

        // The same diary date arriving from two transports is one viewing.
        // Prefer the full export, which has the most complete history.
        tx.execute(
            r#"
            UPDATE viewing_projections
            SET counted = 0, duplicate_reason = 'cross_source_same_day'
            WHERE viewing_id IN (
              SELECT duplicate.id
              FROM viewings duplicate
              JOIN source_movie_records duplicate_movie
                ON duplicate_movie.id = duplicate.source_movie_record_id
              JOIN viewings canonical
              JOIN source_movie_records canonical_movie
                ON canonical_movie.id = canonical.source_movie_record_id
              WHERE duplicate.id != canonical.id
                AND duplicate.source_type != canonical.source_type
                AND duplicate_movie.normalized_title = canonical_movie.normalized_title
                AND duplicate_movie.release_year IS canonical_movie.release_year
                AND DATE(COALESCE(NULLIF(duplicate.occurred_at, ''), NULLIF(duplicate.published_at, ''), duplicate.observed_at))
                    = DATE(COALESCE(NULLIF(canonical.occurred_at, ''), NULLIF(canonical.published_at, ''), canonical.observed_at))
                AND CASE duplicate.source_type
                      WHEN 'letterboxd_export' THEN 0
                      WHEN 'letterboxd_rss' THEN 1
                      WHEN 'legacy_json' THEN 2
                      ELSE 3
                    END
                    > CASE canonical.source_type
                        WHEN 'letterboxd_export' THEN 0
                        WHEN 'letterboxd_rss' THEN 1
                        WHEN 'legacy_json' THEN 2
                        ELSE 3
                      END
            )
            "#,
            [],
        )
        .map_err(|e| e.to_string())?;

        // Older Studio data could persist malformed payloads as duplicate
        // export rows, usually one calendar day apart. An unmarked adjacent
        // pair is ambiguous, so count it once unless the user explicitly
        // logged a rewatch. Valid source events are intentionally untouched.
        tx.execute(
            r#"
            UPDATE viewing_projections
            SET counted = 0, duplicate_reason = 'unconfirmed_adjacent_duplicate'
            WHERE viewing_id IN (
              SELECT duplicate.id
              FROM viewings duplicate
              JOIN source_movie_records duplicate_movie
                ON duplicate_movie.id = duplicate.source_movie_record_id
              JOIN viewing_projections duplicate_projection
                ON duplicate_projection.viewing_id = duplicate.id
              JOIN viewings canonical
              JOIN source_movie_records canonical_movie
                ON canonical_movie.id = canonical.source_movie_record_id
              JOIN viewing_projections canonical_projection
                ON canonical_projection.viewing_id = canonical.id
              WHERE duplicate_projection.counted = 1
                AND canonical_projection.counted = 1
                AND duplicate.rewatch = 0
                AND canonical.rewatch = 0
                AND json_valid(COALESCE(duplicate.raw_payload, '')) = 0
                AND json_valid(COALESCE(canonical.raw_payload, '')) = 0
                AND duplicate_movie.normalized_title = canonical_movie.normalized_title
                AND duplicate_movie.release_year IS canonical_movie.release_year
                AND DATE(COALESCE(NULLIF(duplicate.occurred_at, ''), NULLIF(duplicate.published_at, ''), duplicate.observed_at)) IS NOT NULL
                AND DATE(COALESCE(NULLIF(canonical.occurred_at, ''), NULLIF(canonical.published_at, ''), canonical.observed_at)) IS NOT NULL
                AND julianday(DATE(COALESCE(NULLIF(duplicate.occurred_at, ''), NULLIF(duplicate.published_at, ''), duplicate.observed_at)))
                    - julianday(DATE(COALESCE(NULLIF(canonical.occurred_at, ''), NULLIF(canonical.published_at, ''), canonical.observed_at)))
                    BETWEEN 0 AND 1
                AND (
                  DATE(COALESCE(NULLIF(canonical.occurred_at, ''), NULLIF(canonical.published_at, ''), canonical.observed_at))
                    < DATE(COALESCE(NULLIF(duplicate.occurred_at, ''), NULLIF(duplicate.published_at, ''), duplicate.observed_at))
                  OR (
                    DATE(COALESCE(NULLIF(canonical.occurred_at, ''), NULLIF(canonical.published_at, ''), canonical.observed_at))
                      = DATE(COALESCE(NULLIF(duplicate.occurred_at, ''), NULLIF(duplicate.published_at, ''), duplicate.observed_at))
                    AND canonical.id < duplicate.id
                  )
                )
            )
            "#,
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn refresh_last_watched_at(tx: &Transaction<'_>) -> Result<(), String> {
        tx.execute(
            r#"
            UPDATE user_movie_state
            SET last_watched_at = (
              SELECT v.occurred_at
              FROM viewings v
              LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
              WHERE v.source_movie_record_id = user_movie_state.source_movie_record_id
                AND COALESCE(vp.counted, 1) = 1
              ORDER BY COALESCE(v.occurred_at, v.observed_at) DESC
              LIMIT 1
            )
            "#,
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn compute_coverage(&self) -> Result<LibraryCoverage, String> {
        let unique_movies: u32 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(DISTINCT {FILM_KEY})
                 FROM source_movie_records smr
                 LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
                 WHERE EXISTS (
                   SELECT 1 FROM viewings v WHERE v.source_movie_record_id = smr.id
                 ) OR EXISTS (
                   SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = smr.id
                 ) OR json_extract(smr.raw_identity, '$.review') IS NOT NULL
                 "
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        let total_viewings: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM viewings v
                 LEFT JOIN viewing_projections vp ON vp.viewing_id = v.id
                 WHERE COALESCE(vp.counted, 1) = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let rating_events = self.count_table("rating_events")?;
        let unresolved_movies: u32 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(DISTINCT {FILM_KEY})
                 FROM source_movie_records smr
                 LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
                 WHERE (ml.match_state IS NULL OR ml.match_state IN ('unmatched', 'ambiguous'))
                 AND (
                   EXISTS (SELECT 1 FROM viewings v WHERE v.source_movie_record_id = smr.id)
                   OR EXISTS (SELECT 1 FROM rating_events re WHERE re.source_movie_record_id = smr.id)
                   OR json_extract(smr.raw_identity, '$.review') IS NOT NULL
                   OR smr.on_watchlist = 1
                 )"
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        let has_export: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM imports WHERE source_type = 'letterboxd_export' AND status = 'completed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())?;

        let has_rss: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM viewings WHERE source_type = 'letterboxd_rss'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())?;

        let source = match (has_export, has_rss) {
            (true, true) => "mixed",
            (true, false) => "export",
            (false, true) => "rss",
            (false, false) => "none",
        }
        .to_string();

        let last_full_import = self
            .conn
            .query_row(
                "SELECT imported_at FROM imports
                 WHERE source_type = 'letterboxd_export' AND status = 'completed'
                 ORDER BY imported_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let mut warnings = Vec::new();
        if has_rss && !has_export {
            warnings.push(
                "Only RSS data present — full history requires an official Letterboxd export."
                    .into(),
            );
        }

        let watchlist_movies: u32 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(DISTINCT {FILM_KEY})
                 FROM source_movie_records smr
                 LEFT JOIN movie_links ml ON ml.source_movie_record_id = smr.id
                 LEFT JOIN user_movie_state ums ON ums.source_movie_record_id = smr.id
                 WHERE ums.watchlist = 1 OR smr.on_watchlist = 1"
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        Ok(LibraryCoverage {
            unique_movies,
            watchlist_movies,
            total_viewings,
            rating_events,
            unresolved_movies,
            source,
            full_history_available: has_export,
            rss_window_limit: if has_rss { Some(50) } else { None },
            last_full_import,
            warnings,
        })
    }

    pub fn get_session(&self) -> Result<AppSession, String> {
        let coverage = self.compute_coverage()?;
        let self_username = self
            .get_meta("self_username")?
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        let friend_count = self.count_table("friends")?;
        let friend_activity = self.count_table("friend_activity")?;
        let imports = self.count_table("imports")?;
        let has_setup = self_username.is_some()
            || friend_count > 0
            || friend_activity > 0
            || imports > 0
            || coverage.total_viewings > 0
            || coverage.unique_movies > 0;
        Ok(AppSession {
            self_username,
            friend_count,
            has_setup,
            coverage,
            last_rss_sync_at: self.get_meta("last_rss_sync_at")?,
            rss_paused_until: self.get_meta("rss_backoff_until")?,
        })
    }

    pub fn remove_friend(&self, id: &str) -> Result<String, String> {
        let username: String = self
            .conn()
            .query_row(
                "SELECT username FROM friends WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Friend not found".to_string())?;
        self.conn()
            .execute(
                "DELETE FROM friend_activity WHERE friend_id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
        self.conn()
            .execute("DELETE FROM friends WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(username)
    }

    pub fn reset_all_data(&self) -> Result<(), String> {
        self.conn()
            .execute_batch(
                r#"
                DELETE FROM taste_run_snapshot;
                DELETE FROM taste_embeddings;
                DELETE FROM taste_feedback_events;
                DELETE FROM taste_exposure_features;
                DELETE FROM taste_recommendation_exposures;
                DELETE FROM taste_feedback;
                DELETE FROM person_credits;
                DELETE FROM friend_activity;
                DELETE FROM friends;
                DELETE FROM import_entries;
                DELETE FROM imports;
                DELETE FROM rating_events;
                DELETE FROM viewings;
                DELETE FROM user_movie_state;
                DELETE FROM movie_aliases;
                DELETE FROM movie_links;
                DELETE FROM movies;
                DELETE FROM source_movie_records;
                DELETE FROM app_meta;
                "#,
            )
            .map_err(|e| e.to_string())?;
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciliation_keeps_one_effective_viewing_and_unlinks_wrong_search_results() {
        let db = Database::in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO movies(id, canonical_title, release_year, tmdb_id) VALUES ('wrong', 'The Piano Tuner', 2025, 1571662)",
                [],
            )
            .unwrap();
        for (id, source, method, occurred_at, diary_entry_id) in [
            (
                "diary",
                "letterboxd_export",
                "tmdb_search",
                "2025-01-01",
                "https://boxd.it/fMUQ7R",
            ),
            (
                "rss",
                "letterboxd_rss",
                "propagated",
                "2025-01-01",
                "letterboxd-review-1446480335",
            ),
            (
                "summary",
                "letterboxd_export",
                "propagated",
                "2025-01-02",
                "https://boxd.it/icFU",
            ),
        ] {
            db.conn()
                .execute(
                    "INSERT INTO source_movie_records(
                       id, source_type, source_record_key, normalized_title, release_year,
                       raw_identity, cached_poster_url, created_at
                     ) VALUES (?1, ?2, ?1, 'tuner', 2025, '{\"title\":\"Tuner\"}', 'wrong-poster', '2025-01-01T00:00:00Z')",
                    params![id, source],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO movie_links(source_movie_record_id, movie_id, match_state, match_method)
                     VALUES (?1, 'wrong', 'confirmed', ?2)",
                    params![id, method],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO viewings(
                       id, source_movie_record_id, source_record_key, occurred_at, observed_at,
                       source_type, diary_entry_id
                     ) VALUES (?1, ?1, ?1, ?2, '2025-01-01T00:00:00Z', ?3, ?4)",
                    params![id, occurred_at, source, diary_entry_id],
                )
                .unwrap();
        }

        db.reconcile_duplicate_history().unwrap();

        assert_eq!(db.count_table("viewings").unwrap(), 3);
        let counted: u32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM viewing_projections WHERE counted = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(counted, 1);
        let unmatched: u32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM movie_links WHERE movie_id IS NULL AND match_state = 'unmatched'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmatched, 3);
        let stale_posters: u32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_movie_records WHERE cached_poster_url IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_posters, 0);
    }
    #[test]
    fn opens_in_memory() {
        let db = Database::in_memory().expect("db");
        assert_eq!(db.get_meta("schema_version").unwrap(), Some("17".into()));
    }

    #[test]
    fn projection_collapses_legacy_adjacent_duplicates_but_keeps_explicit_rewatches() {
        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        for (id, occurred_at, rewatch) in [
            ("first", "2026-02-28", 0),
            ("duplicate", "2026-03-01", 0),
            ("rewatch", "2026-03-01", 1),
        ] {
            tx.execute(
                "INSERT INTO source_movie_records(
                   id, source_type, source_record_key, normalized_title, release_year, raw_identity, created_at
                 ) VALUES (?1, 'letterboxd_export', ?1, 'source code', 2011, '{\"title\":\"Source Code\"}', '2026-03-01T00:00:00Z')",
                params![id],
            )
            .expect("source movie");
            tx.execute(
                "INSERT INTO viewings(
                   id, source_movie_record_id, source_record_key, occurred_at, observed_at, source_type, rewatch, raw_payload
                 ) VALUES (?1, ?1, ?1, ?2, ?2, 'letterboxd_export', ?3, 'legacy payload')",
                params![id, occurred_at, rewatch],
            )
            .expect("viewing");
        }
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        let (counted, duplicates): (u32, u32) = db
            .conn()
            .query_row(
                "SELECT
                   SUM(CASE WHEN counted = 1 THEN 1 ELSE 0 END),
                   SUM(CASE WHEN duplicate_reason = 'unconfirmed_adjacent_duplicate' THEN 1 ELSE 0 END)
                 FROM viewing_projections",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("projection counts");
        assert_eq!((counted, duplicates), (2, 1));
        assert_eq!(db.compute_coverage().expect("coverage").total_viewings, 2);
    }

    #[test]
    fn file_db_uses_wal() {
        let path = std::env::temp_dir().join(format!("studio-wal-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&path).expect("db");
        let mode: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal_mode");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_file_name(format!(
            "{}-wal",
            path.file_name().unwrap().to_string_lossy()
        )));
        let _ = std::fs::remove_file(path.with_file_name(format!(
            "{}-shm",
            path.file_name().unwrap().to_string_lossy()
        )));
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn remove_friend_drops_only_that_friend_activity() {
        let db = Database::in_memory().expect("db");
        db.conn()
            .execute(
                "INSERT INTO friends(id, username, enabled) VALUES ('f1', 'ada', 1), ('f2', 'bee', 1)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO friend_activity(id, friend_id, source_record_key, activity_type)
                 VALUES ('a1', 'f1', 'k1', 'diary'), ('a2', 'f2', 'k2', 'diary')",
                [],
            )
            .unwrap();
        assert_eq!(db.remove_friend("f1").unwrap(), "ada");
        let remaining: Vec<(String, String)> = {
            let mut stmt = db
                .conn()
                .prepare("SELECT id, username FROM friends ORDER BY username")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert_eq!(remaining, vec![("f2".into(), "bee".into())]);
        let activity: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM friend_activity", [], |row| row.get(0))
            .unwrap();
        let leftover_friend: String = db
            .conn()
            .query_row("SELECT friend_id FROM friend_activity", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(activity, 1);
        assert_eq!(leftover_friend, "f2");
        assert_eq!(db.remove_friend("missing").unwrap_err(), "Friend not found");
    }

    #[test]
    fn schema_v17_keeps_existing_tables_and_adds_viewing_projection() {
        let db = Database::in_memory().expect("db");
        assert_eq!(db.get_meta("schema_version").unwrap(), Some("17".into()));
        let feedback: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'taste_feedback'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let snap: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'taste_run_snapshot'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let embeddings: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'taste_embeddings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let exposures: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'taste_recommendation_exposures'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let events: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'taste_feedback_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(feedback, 1);
        assert_eq!(snap, 1);
        assert_eq!(embeddings, 1);
        assert_eq!(exposures, 1);
        assert_eq!(events, 1);
        let viewing_projection: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'viewing_projections'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(viewing_projection, 1);
        let company_column: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('movies') WHERE name = 'production_companies_json'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(company_column, 1);
        let artwork_columns: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('movies') WHERE name IN ('poster_override_url', 'backdrop_override_url')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(artwork_columns, 2);
        let media_type_column: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('movies') WHERE name = 'tmdb_media_type'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(media_type_column, 1);
        db.reset_all_data().unwrap();
        let leftover: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM taste_feedback", [], |row| row.get(0))
            .unwrap();
        assert_eq!(leftover, 0);
    }

    #[test]
    fn purge_removes_previously_imported_list_titles() {
        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        tx.execute(
            "INSERT INTO source_movie_records(
               id, source_type, source_record_key, normalized_title, release_year, raw_identity, created_at
             ) VALUES
             ('real', 'letterboxd_export', 'letterboxd_export|film|v1|real', 'inception', 2010, '{\"title\":\"Inception\",\"uri\":\"/film/inception/\"}', '2026-01-01T00:00:00Z'),
             ('list', 'letterboxd_export', 'letterboxd_export|film|v1|list', 'most excited for in 2026', NULL, '{\"title\":\"Most Excited For In 2026\",\"uri\":\"https://letterboxd.com/example/list/most-excited-for-in-2026/\"}', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("smr");
        tx.execute(
            "INSERT INTO viewings(
               id, source_movie_record_id, source_record_key, occurred_at, observed_at, source_type, diary_entry_id, raw_payload
             ) VALUES
             ('v-real', 'real', 'letterboxd_export|diary.csv|v1|real', '2020-01-01', '2026-01-01', 'letterboxd_export', '/film/inception/', '{}'),
             ('v-list', 'list', 'letterboxd_export|lists.csv|v1|list', '2024-01-01', '2026-01-01', 'letterboxd_export', 'https://letterboxd.com/example/list/most-excited-for-in-2026/', '{}')",
            [],
        )
        .expect("viewings");
        Database::rebuild_projections(&tx).expect("rebuild");
        tx.commit().expect("commit");

        db.purge_letterboxd_list_exports().expect("purge");

        let titles: Vec<String> = db
            .conn()
            .prepare("SELECT normalized_title FROM source_movie_records")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(titles, vec!["inception".to_string()]);
        let viewings: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM viewings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(viewings, 1);
    }
}
