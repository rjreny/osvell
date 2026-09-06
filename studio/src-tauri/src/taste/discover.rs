use crate::catalog::tmdb::{self, MovieLookup};
use crate::storage::db::Database;
use crate::taste::features::{FeatureFamily, FeatureProfile};
use crate::taste::retrieve::{
    identity_key, Candidate, MediaKind, RetrievalKind, RetrievalSource,
};
use crate::taste::score::{score_candidate, ScoredCandidate};
use serde_json::Value;
use std::collections::HashSet;

pub const DISCOVERY_FLOOR: f32 = 0.08;
pub const MAX_DISCOVERIES: usize = 8;

pub fn parse_search_titles(raw: &Value) -> Vec<(String, Option<i32>)> {
    let mut out = Vec::new();
    if let Some(arr) = raw["titles"].as_array().or_else(|| raw["films"].as_array()) {
        for v in arr {
            let title = v["title"]
                .as_str()
                .or_else(|| v.as_str())
                .unwrap_or("")
                .trim();
            if title.is_empty() {
                continue;
            }
            let year = v["year"].as_i64().map(|y| y as i32);
            out.push((title.to_string(), year));
        }
    }
    out.truncate(8);
    out
}

pub fn materialize(
    db: &Database,
    titles: &[(String, Option<i32>)],
    query: &str,
    seen: &HashSet<String>,
    profile: &FeatureProfile,
) -> Vec<ScoredCandidate> {
    let mut out = Vec::new();
    for (title, year) in titles {
        let Ok(Some(hit)) = tmdb::lookup_movie(title, *year) else {
            continue;
        };
        if let Some(scored) = score_lookup(db, hit, query, seen, profile) {
            out.push(scored);
        }
        if out.len() >= MAX_DISCOVERIES {
            break;
        }
    }
    out
}

fn score_lookup(
    db: &Database,
    hit: MovieLookup,
    query: &str,
    seen: &HashSet<String>,
    profile: &FeatureProfile,
) -> Option<ScoredCandidate> {
    let key = identity_key(Some(hit.tmdb_id), &hit.title, hit.year);
    if seen.contains(&key) {
        return None;
    }
    let _ = tmdb::refresh_movie_catalog(db, hit.tmdb_id, false);
    let mut candidate = Candidate {
        tmdb_id: Some(hit.tmdb_id),
        title: hit.title,
        year: hit.year,
        poster: hit.poster,
        genres: Vec::new(),
        credits: Vec::new(),
        keywords: Vec::new(),
        runtime: None,
        vote_count: None,
        watchlist: false,
        sources: vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: query.to_string(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }],
        friend_affinity: 0.0,
        tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
    };
    let _ = crate::taste::retrieve::enrich_missing(db, std::slice::from_mut(&mut candidate), 1, false);
    let mut scored = score_candidate(profile, &candidate);
    if is_short_runtime(scored.candidate.runtime) {
        return None;
    }
    let sparse_facet = matches_sparse_facet(profile, &scored);
    // Web discovery is meant to escape the TMDB-neighbor pool. Do not require
    // the same craft-evidence grade as résumé filmography; overall fit + a
    // non-empty bridge is enough to enter the ranked pool.
    let has_bridge = !scored.matched_features.is_empty()
        || scored.score.semantic_coverage
        || sparse_facet;
    if (scored.score.total >= DISCOVERY_FLOOR && has_bridge) || sparse_facet {
        scored.eligibility.passed = true;
        Some(scored)
    } else {
        None
    }
}

fn is_short_runtime(runtime: Option<i32>) -> bool {
    matches!(runtime, Some(rt) if (1..crate::taste::score::FEATURE_RUNTIME_MIN).contains(&rt))
}

fn matches_sparse_facet(profile: &FeatureProfile, scored: &ScoredCandidate) -> bool {
    profile.affinities.iter().any(|a| {
        a.appearances <= 4
            && a.confidence > 0.35
            && a.recommendation_mean > 0.2
            && matches!(
                a.key.family,
                FeatureFamily::Keyword | FeatureFamily::Genre | FeatureFamily::Director
            )
            && (scored.positive_features.iter().any(|n| n == &a.key.name)
                || scored.candidate.genres.iter().any(|g| g == &a.key.name)
                || scored.candidate.directors.iter().any(|d| d == &a.key.name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_titles() {
        let v = json!({"titles":[{"title":"Zodiac","year":2007},{"title":"The Insider"}]});
        let t = parse_search_titles(&v);
        assert_eq!(t[0].0, "Zodiac");
        assert_eq!(t[0].1, Some(2007));
    }

    #[test]
    fn floor_constant() {
        assert!(DISCOVERY_FLOOR > 0.0);
        assert!(MAX_DISCOVERIES >= 8);
    }
}
