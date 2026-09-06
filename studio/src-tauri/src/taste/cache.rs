use crate::storage::db::Database;
use crate::taste::feedback;
use crate::taste::freeze::FROZEN_FORMULA_ID;
use crate::taste::retrieve::{Candidate, FilmRecord};
use crate::taste::score::ScoredCandidate;
use crate::taste::workspace::{self, ALGORITHM_VERSION, Workspace};
use chrono::{Duration, Utc};
use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const CATALOG_TTL_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteNarrative {
    pub title: String,
    pub summary: String,
    pub affinities: Vec<crate::taste::TasteAffinity>,
    pub aversions: Vec<crate::taste::TasteAffinity>,
    pub dimensions: Vec<crate::taste::TasteDimension>,
}

#[derive(Debug, Clone)]
pub struct Fingerprints {
    pub algorithm_version: String,
    pub profile_fingerprint: String,
    pub library_state_fingerprint: String,
    pub candidate_input_fingerprint: String,
    pub scoring_fingerprint: String,
    pub narrative_key: String,
}

pub fn scoring_fingerprint() -> String {
    hash_parts(&[ALGORITHM_VERSION, FROZEN_FORMULA_ID])
}

pub fn profile_fingerprint(films: &[FilmRecord]) -> String {
    let mut parts = vec![ALGORITHM_VERSION.to_string()];
    let mut rated: Vec<_> = films
        .iter()
        .filter(|f| f.rating.is_some())
        .collect();
    rated.sort_by(|a, b| {
        a.tmdb_id
            .unwrap_or(0)
            .cmp(&b.tmdb_id.unwrap_or(0))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.year.cmp(&b.year))
    });
    for f in rated {
        parts.push(format!(
            "{}:{}:{}:review:{}",
            f.tmdb_id.unwrap_or(0),
            f.rating.unwrap_or(0.0),
            credit_sig(f),
            f.review.as_deref().unwrap_or("")
        ));
    }
    hash_parts(&parts)
}

pub fn library_state_fingerprint(films: &[FilmRecord], friend_keys: &[String]) -> String {
    let mut parts = Vec::new();
    let mut rows: Vec<_> = films.iter().collect();
    rows.sort_by(|a, b| {
        a.tmdb_id
            .unwrap_or(0)
            .cmp(&b.tmdb_id.unwrap_or(0))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.year.cmp(&b.year))
    });
    for f in rows {
        parts.push(format!(
            "{}:w{}:l{}:r{}:review:{}",
            f.tmdb_id.unwrap_or(0),
            f.watched as u8,
            f.watchlist as u8,
            f.rating.is_some() as u8,
            f.review.as_deref().unwrap_or("")
        ));
    }
    let mut friends = friend_keys.to_vec();
    friends.sort();
    parts.extend(friends);
    hash_parts(&parts)
}

pub fn candidate_input_fingerprint(cands: &[Candidate]) -> String {
    let mut rows: Vec<_> = cands.iter().collect();
    rows.sort_by(|a, b| {
        a.tmdb_id
            .unwrap_or(i64::MAX)
            .cmp(&b.tmdb_id.unwrap_or(i64::MAX))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.year.cmp(&b.year))
    });
    let mut parts = Vec::new();
    for c in rows {
        let credits: Vec<_> = c
            .credits
            .iter()
            .map(|cr| format!("{}:{}", cr.job, cr.name))
            .collect();
        let keywords: Vec<_> = c.keywords.iter().map(|k| k.name.clone()).collect();
        let sources: Vec<_> = c
            .sources
            .iter()
            .map(|s| format!("{:?}:{}", s.kind, s.label))
            .collect();
        parts.push(format!(
            "{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}",
            c.tmdb_id.unwrap_or(0),
            credits.join(","),
            keywords.join(","),
            sources.join(","),
            c.watchlist,
            c.friend_affinity.to_bits(),
            c.tmdb_related.to_bits(),
            c.media_kind,
            c.runtime.unwrap_or(0),
            c.vote_count.unwrap_or(0)
        ));
    }
    hash_parts(&parts)
}

pub fn fingerprints(
    films: &[FilmRecord],
    cands: &[Candidate],
    friend_keys: &[String],
    model: &str,
    web: bool,
) -> Fingerprints {
    let profile = profile_fingerprint(films);
    let library = library_state_fingerprint(films, friend_keys);
    let candidate = candidate_input_fingerprint(cands);
    let scoring = scoring_fingerprint();
    let narrative_key = hash_parts(&[
        profile.as_str(),
        library.as_str(),
        candidate.as_str(),
        model,
        if web { "web1" } else { "web0" },
    ]);
    Fingerprints {
        algorithm_version: ALGORITHM_VERSION.into(),
        profile_fingerprint: profile,
        library_state_fingerprint: library,
        candidate_input_fingerprint: candidate,
        scoring_fingerprint: scoring,
        narrative_key,
    }
}

fn credit_sig(f: &FilmRecord) -> String {
    let mut credits: Vec<_> = f
        .credits
        .iter()
        .map(|c| format!("{}:{}", c.job, c.name))
        .collect();
    credits.sort();
    let mut kws: Vec<_> = f.keywords.iter().map(|k| k.name.clone()).collect();
    kws.sort();
    format!("{}|{}", credits.join(","), kws.join(","))
}

fn hash_parts(parts: &[impl AsRef<str>]) -> String {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.as_ref().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

#[derive(Debug, Clone)]
pub struct RunSnapshot {
    pub fingerprints: Fingerprints,
    pub catalog_valid_until: String,
    pub scored_pool: Vec<ScoredCandidate>,
    pub narrative: TasteNarrative,
}

pub fn load_snapshot(db: &Database) -> Result<Option<RunSnapshot>, String> {
    let row = db.conn().query_row(
        r#"
        SELECT algorithm_version, profile_fingerprint, library_state_fingerprint,
               candidate_input_fingerprint, scoring_fingerprint, narrative_key,
               catalog_valid_until, scored_pool_json, narrative_json
        FROM taste_run_snapshot WHERE id = 1
        "#,
        [],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
            ))
        },
    );
    let Ok((algo, profile, library, candidate, scoring, narrative_key, until, pool, nar)) = row else {
        return Ok(None);
    };
    let scored_pool: Vec<ScoredCandidate> =
        serde_json::from_str(&pool).map_err(|e| e.to_string())?;
    let narrative: TasteNarrative = serde_json::from_str(&nar).map_err(|e| e.to_string())?;
    Ok(Some(RunSnapshot {
        fingerprints: Fingerprints {
            algorithm_version: algo,
            profile_fingerprint: profile,
            library_state_fingerprint: library,
            candidate_input_fingerprint: candidate,
            scoring_fingerprint: scoring,
            narrative_key,
        },
        catalog_valid_until: until,
        scored_pool,
        narrative,
    }))
}

pub fn save_snapshot(db: &Database, snap: &RunSnapshot) -> Result<(), String> {
    let pool = serde_json::to_string(&snap.scored_pool).map_err(|e| e.to_string())?;
    let nar = serde_json::to_string(&snap.narrative).map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();
    db.conn()
        .execute(
            r#"
            INSERT INTO taste_run_snapshot (
              id, algorithm_version, profile_fingerprint, library_state_fingerprint,
              candidate_input_fingerprint, scoring_fingerprint, narrative_key,
              catalog_valid_until, scored_pool_json, narrative_json, created_at
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
              algorithm_version = excluded.algorithm_version,
              profile_fingerprint = excluded.profile_fingerprint,
              library_state_fingerprint = excluded.library_state_fingerprint,
              candidate_input_fingerprint = excluded.candidate_input_fingerprint,
              scoring_fingerprint = excluded.scoring_fingerprint,
              narrative_key = excluded.narrative_key,
              catalog_valid_until = excluded.catalog_valid_until,
              scored_pool_json = excluded.scored_pool_json,
              narrative_json = excluded.narrative_json,
              created_at = excluded.created_at
            "#,
            params![
                snap.fingerprints.algorithm_version,
                snap.fingerprints.profile_fingerprint,
                snap.fingerprints.library_state_fingerprint,
                snap.fingerprints.candidate_input_fingerprint,
                snap.fingerprints.scoring_fingerprint,
                snap.fingerprints.narrative_key,
                snap.catalog_valid_until,
                pool,
                nar,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn invalidate_snapshot(db: &Database) -> Result<(), String> {
    db.conn()
        .execute("DELETE FROM taste_run_snapshot WHERE id = 1", [])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn catalog_valid_until_from_now() -> String {
    (Utc::now() + Duration::days(CATALOG_TTL_DAYS)).to_rfc3339()
}

pub fn snapshot_usable(
    db: &Database,
    snap: &RunSnapshot,
    films: &[FilmRecord],
    friend_keys: &[String],
) -> Result<bool, String> {
    if snap.fingerprints.algorithm_version != ALGORITHM_VERSION
        || snap.fingerprints.profile_fingerprint != profile_fingerprint(films)
        || snap.fingerprints.library_state_fingerprint
            != library_state_fingerprint(films, friend_keys)
        || snap.fingerprints.scoring_fingerprint != scoring_fingerprint()
    {
        return Ok(false);
    }
    if Utc::now().to_rfc3339() >= snap.catalog_valid_until {
        return Ok(false);
    }
    let ids: Vec<i64> = snap
        .scored_pool
        .iter()
        .filter_map(|c| c.candidate.tmdb_id)
        .collect();
    if !catalog_fingerprint_matches(db, &snap.fingerprints.candidate_input_fingerprint, &ids) {
        return Ok(false);
    }
    catalog_rows_fresh(db, &snap.scored_pool)
}

pub fn catalog_payload_fingerprint(db: &Database, ids: &[i64]) -> String {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    let mut parts = Vec::new();
    for id in ids {
        let row: Option<(Option<String>, Option<String>)> = db
            .conn()
            .query_row(
                "SELECT credits_json, keywords_json FROM movies WHERE tmdb_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (credits, keywords) = row.unwrap_or((None, None));
        parts.push(format!(
            "{}:{}:{}",
            id,
            credits.unwrap_or_default(),
            keywords.unwrap_or_default()
        ));
    }
    hash_parts(&parts)
}

pub fn bind_catalog_fingerprint(db: &Database, retrieve_fp: &str, ids: &[i64]) -> String {
    format!("{}:{}", retrieve_fp, catalog_payload_fingerprint(db, ids))
}

pub fn catalog_fingerprint_matches(db: &Database, stored: &str, ids: &[i64]) -> bool {
    stored.ends_with(&format!(":{}", catalog_payload_fingerprint(db, ids)))
}

fn catalog_rows_fresh(db: &Database, pool: &[ScoredCandidate]) -> Result<bool, String> {
    for row in pool {
        let Some(id) = row.candidate.tmdb_id else {
            return Ok(false);
        };
        let fresh: Option<i64> = db
            .conn()
            .query_row(
                "SELECT CASE WHEN enriched_at IS NOT NULL
                              AND datetime(enriched_at) >= datetime('now', '-30 days')
                             THEN 1 ELSE 0 END
                 FROM movies WHERE tmdb_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if fresh != Some(1) {
            return Ok(false);
        }
    }
    let stale_people: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM person_credits
             WHERE datetime(fetched_at) < datetime('now', '-30 days')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(stale_people == 0)
}

pub fn workspace_from_pool(
    db: &Database,
    pool: &[ScoredCandidate],
) -> Result<Workspace, String> {
    let hide = feedback::hide_ids(&feedback::list_feedback(db)?);
    Ok(workspace::apply_feedback_filter(pool, &hide))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::MediaKind;

    fn cand(id: i64, name: &str) -> Candidate {
        Candidate {
            tmdb_id: Some(id),
            title: name.into(),
            year: Some(2000),
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
        }
    }

    #[test]
    fn mutating_credits_changes_candidate_input_fingerprint() {
        let a = vec![cand(1, "A")];
        let mut b = cand(1, "A");
        b.credits.push(crate::taste::features::Credit {
            id: Some(9),
            name: "Nolan".into(),
            job: "Director".into(),
        });
        assert_ne!(
            candidate_input_fingerprint(&a),
            candidate_input_fingerprint(&[b])
        );
    }

    #[test]
    fn ids_alone_are_not_the_fingerprint() {
        let a = cand(1, "A");
        let mut b = cand(1, "A");
        b.keywords.push(crate::taste::features::Keyword {
            id: None,
            name: "neo-noir".into(),
        });
        assert_ne!(
            candidate_input_fingerprint(&[a]),
            candidate_input_fingerprint(&[b])
        );
    }

    #[test]
    fn importing_a_review_changes_the_profile_fingerprint() {
        fn film(review: Option<&str>) -> FilmRecord {
            FilmRecord {
                key: "film".into(),
                title: "Film".into(),
                year: Some(2000),
                tmdb_id: Some(1),
                rating: Some(4.5),
                liked: true,
                watched: true,
                watchlist: false,
                viewings: 1,
                last_date: Some("2024-01-01".into()),
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
                review: review.map(str::to_string),
                signal: None,
                age_years: None,
            }
        }

        assert_ne!(
            profile_fingerprint(&[film(None)]),
            profile_fingerprint(&[film(Some("beautifully shot"))])
        );
    }

    fn scored_id(id: i64) -> ScoredCandidate {
        use crate::taste::explain::{EligibilityTrace, MatchedFeatureView};
        use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
        use crate::taste::score::{CandidateScore, CandidateView};
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(id),
                title: format!("Film {id}"),
                year: Some(2000),
                poster: None,
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Filmography,
                    label: "Nolan".into(),
                    seed_tmdb_id: None,
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                directors: vec!["Nolan".into()],
                genres: vec!["Drama".into()],
                modes: vec![],
                media_kind: MediaKind::Movie,
                runtime: Some(110),
                vote_count: Some(400),
                semantic_cluster: None,
            },
            score: CandidateScore {
                content: 0.5,
                tmdb_related: 0.0,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: 0.0,
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total: 0.5,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: vec!["Nolan".into()],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec!["Nolan".into()],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![
                MatchedFeatureView {
                    feature_key: String::new(),
                    name: "Nolan".into(),
                    family: "director".into(),
                    appearances: 8,
                    recommendation_mean: 0.6,
                    scoring_affinity: 0.5,
                    confidence: 0.9,
                    portability: 1.0,
                    citeable: true,
                    cited: true,
                },
                MatchedFeatureView {
                    feature_key: String::new(),
                    name: "neo-noir".into(),
                    family: "keyword".into(),
                    appearances: 7,
                    recommendation_mean: 0.4,
                    scoring_affinity: 0.4,
                    confidence: 0.8,
                    portability: 1.0,
                    citeable: true,
                    cited: true,
                },
            ],
            hidden_features: vec![],
            eligibility: EligibilityTrace {
                portable_evidence_required: false,
                passed: true,
                passed_because: vec!["craft".into()],
                candidate_fit: 1.0,
                evidence_grade: crate::taste::explain::EvidenceGrade::Medium,
                ..Default::default()
            },
        }
    }

    fn insert_movie(db: &Database, tmdb_id: i64, credits: &str, enriched_sql: &str) {
        db.conn()
            .execute(
                &format!(
                    "INSERT INTO movies (id, canonical_title, tmdb_id, poster_path, collection_json, credits_json, keywords_json, enriched_at)
                     VALUES (?1, 'Film', ?2, '/x.jpg', '[]', ?3, '[]', {enriched_sql})"
                ),
                rusqlite::params![format!("m{tmdb_id}"), tmdb_id, credits],
            )
            .unwrap();
    }

    fn base_snap(db: &Database, films: &[FilmRecord], pool: &[ScoredCandidate]) -> RunSnapshot {
        let ids: Vec<i64> = pool.iter().filter_map(|c| c.candidate.tmdb_id).collect();
        let retrieve_fp = candidate_input_fingerprint(&[]);
        RunSnapshot {
            fingerprints: Fingerprints {
                algorithm_version: ALGORITHM_VERSION.into(),
                profile_fingerprint: profile_fingerprint(films),
                library_state_fingerprint: library_state_fingerprint(films, &[]),
                candidate_input_fingerprint: bind_catalog_fingerprint(db, &retrieve_fp, &ids),
                scoring_fingerprint: scoring_fingerprint(),
                narrative_key: "n".into(),
            },
            catalog_valid_until: catalog_valid_until_from_now(),
            scored_pool: pool.to_vec(),
            narrative: TasteNarrative {
                title: "t".into(),
                summary: String::new(),
                affinities: vec![],
                aversions: vec![],
                dimensions: vec![],
            },
        }
    }

    #[test]
    fn old_person_identity_snapshot_is_not_usable() {
        let db = Database::in_memory().unwrap();
        let mut snap = base_snap(&db, &[], &[]);
        assert!(snapshot_usable(&db, &snap, &[], &[]).unwrap());
        snap.fingerprints.algorithm_version = "taste-workspace-23-effective-viewings".into();
        assert!(!snapshot_usable(&db, &snap, &[], &[]).unwrap());
    }

    #[test]
    fn mismatched_scoring_fingerprint_is_not_usable() {
        let db = Database::in_memory().unwrap();
        let films: Vec<FilmRecord> = vec![];
        let mut snap = base_snap(&db, &films, &[]);
        snap.fingerprints.scoring_fingerprint = "other".into();
        assert!(!snapshot_usable(&db, &snap, &films, &[]).unwrap());
    }

    #[test]
    fn expired_catalog_valid_until_refuses_warm_skip() {
        let db = Database::in_memory().unwrap();
        let films: Vec<FilmRecord> = vec![];
        let mut snap = base_snap(&db, &films, &[]);
        snap.catalog_valid_until = "2000-01-01T00:00:00Z".into();
        assert!(!snapshot_usable(&db, &snap, &films, &[]).unwrap());
    }

    #[test]
    fn mutated_cached_credits_break_catalog_fingerprint() {
        let db = Database::in_memory().unwrap();
        insert_movie(&db, 7, "[]", "datetime('now')");
        let ids = [7];
        let before = catalog_payload_fingerprint(&db, &ids);
        db.conn()
            .execute(
                "UPDATE movies SET credits_json = '[{\"name\":\"Nolan\"}]' WHERE tmdb_id = 7",
                [],
            )
            .unwrap();
        let after = catalog_payload_fingerprint(&db, &ids);
        assert_ne!(before, after);
        let stored = format!("retrieve:{before}");
        assert!(!catalog_fingerprint_matches(&db, &stored, &ids));
    }

    #[test]
    fn stale_enriched_at_refuses_warm_skip() {
        let db = Database::in_memory().unwrap();
        insert_movie(&db, 8, "[]", "datetime('now', '-40 days')");
        let films: Vec<FilmRecord> = vec![];
        let snap = base_snap(&db, &films, &[scored_id(8)]);
        assert!(!snapshot_usable(&db, &snap, &films, &[]).unwrap());
    }

    #[test]
    fn feedback_clear_restores_from_scored_pool() {
        let db = Database::in_memory().unwrap();
        crate::taste::feedback::set_feedback(&db, 1, crate::taste::feedback::ACTION_REJECTED, None)
            .unwrap();
        let pool = vec![scored_id(1), scored_id(2)];
        let hidden = workspace_from_pool(&db, &pool).unwrap();
        assert!(hidden.new_picks.iter().all(|c| c.candidate.tmdb_id != Some(1)));
        crate::taste::feedback::clear_feedback(&db, 1).unwrap();
        let restored = workspace_from_pool(&db, &pool).unwrap();
        assert!(restored.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(1)));
    }
}
