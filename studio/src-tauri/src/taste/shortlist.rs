use crate::taste::score::ScoredCandidate;

const MMR_LAMBDA: f32 = 0.72;
const SHORTLIST_MIN: usize = 30;
const SHORTLIST_MAX: usize = 50;

fn similarity(a: &ScoredCandidate, b: &ScoredCandidate) -> f32 {
    let mut s = 0.0;
    if !a.candidate.directors.is_empty()
        && a.candidate
            .directors
            .iter()
            .any(|d| b.candidate.directors.contains(d))
    {
        s += 0.6;
    }
    if !a.candidate.genres.is_empty() {
        let shared = a
            .candidate
            .genres
            .iter()
            .filter(|g| b.candidate.genres.contains(g))
            .count();
        if shared > 0 {
            s += 0.25 * (shared as f32).min(2.0);
        }
    }
    let same_seed = a.candidate.sources.iter().any(|sa| {
        b.candidate.sources.iter().any(|sb| {
            sa.kind == sb.kind && sa.seed_tmdb_id.is_some() && sa.seed_tmdb_id == sb.seed_tmdb_id
        })
    });
    if same_seed {
        s += 0.2;
    }
    if !a.candidate.modes.is_empty() {
        let shared = a
            .candidate
            .modes
            .iter()
            .filter(|m| b.candidate.modes.contains(m))
            .count();
        if shared > 0 {
            s += 0.35;
        }
    }
    let shared_person = a
        .person_keys
        .iter()
        .filter(|f| b.person_keys.contains(f))
        .count();
    if shared_person > 0 {
        s += 0.55;
    }
    s.min(1.0)
}

fn repeat_person_count(cand: &ScoredCandidate, selected: &[ScoredCandidate]) -> usize {
    cand.person_keys
        .iter()
        .map(|f| {
            selected
                .iter()
                .filter(|s| s.person_keys.iter().any(|p| p == f))
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// Light MMR: keep cluster-mates. This is not the final list.
/// New neighbors stay in so the critic sees the same films the board will show.
pub fn llm_pool(ranked: &[ScoredCandidate]) -> Vec<ScoredCandidate> {
    ranked
        .iter()
        .filter(|c| {
            c.candidate.watchlist || crate::taste::confidence::occupies_new(c)
        })
        .cloned()
        .collect()
}

pub fn shortlist(ranked: &[ScoredCandidate]) -> Vec<ScoredCandidate> {
    let target = SHORTLIST_MAX.min(ranked.len()).max(SHORTLIST_MIN.min(ranked.len()));
    shortlist_n(ranked, target)
}

pub fn shortlist_n(ranked: &[ScoredCandidate], target: usize) -> Vec<ScoredCandidate> {
    if ranked.is_empty() || target == 0 {
        return Vec::new();
    }
    let pool: Vec<&ScoredCandidate> = ranked.iter().collect();
    let mut selected: Vec<ScoredCandidate> = Vec::new();
    let mut remaining: Vec<usize> = (0..pool.len()).collect();
    let target = target.min(pool.len());

    while selected.len() < target && !remaining.is_empty() {
        let mut best_i = 0;
        let mut best_score = f32::NEG_INFINITY;
        for (idx, &pi) in remaining.iter().enumerate() {
            let frozen = (pool[pi].score.total + 1.5) / 3.0;
            let fit = pool[pi].eligibility.candidate_fit.clamp(0.0, 1.0);
            let grade = crate::taste::score::evidence_grade(pool[pi]) as f32 / 3.0;
            let rel = 0.55 * frozen + 0.30 * fit + 0.15 * grade;
            let sim = selected
                .iter()
                .map(|s| similarity(pool[pi], s))
                .fold(0.0_f32, f32::max);
            let repeats = repeat_person_count(pool[pi], &selected) as f32;
            let mmr = MMR_LAMBDA * rel - (1.0 - MMR_LAMBDA) * sim - 0.14 * repeats;
            if mmr > best_score {
                best_score = mmr;
                best_i = idx;
            }
        }
        let pi = remaining.remove(best_i);
        selected.push(pool[pi].clone());
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn cand(title: &str, director: &str, total: f32) -> ScoredCandidate {
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(title.len() as i64),
                title: title.into(),
                year: Some(1995),
                poster: None,
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Related,
                    label: "x".into(),
                    seed_tmdb_id: Some(1),
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                directors: vec![director.into()],
                genres: vec!["Crime".into()],
                modes: vec![],
                media_kind: crate::taste::retrieve::MediaKind::Movie,
                runtime: Some(110),
                vote_count: Some(400),
                semantic_cluster: None,
            },
            score: CandidateScore {
                content: total,
                tmdb_related: 0.5,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: 0.0,
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total,
            },
            reasons: vec!["director".into()],
            evidence: vec!["Heat".into()],
            positive_features: vec![director.into()],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec![],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![],
            hidden_features: vec![],
            eligibility: crate::taste::explain::EligibilityTrace {
                portable_evidence_required: false,
                passed: false,
                passed_because: vec!["fixture".into()],
                candidate_fit: 1.0,
                evidence_grade: crate::taste::explain::EvidenceGrade::None,
                ..Default::default()
            },
        }
    }

    #[test]
    fn shortlist_keeps_more_than_twelve() {
        let ranked: Vec<_> = (0..80)
            .map(|i| cand(&format!("Film {i}"), if i < 40 { "Mann" } else { "Other" }, 0.9 - i as f32 * 0.005))
            .collect();
        let s = shortlist(&ranked);
        assert!(s.len() >= 30);
        assert!(s.len() <= 50);
        let mann = s.iter().filter(|c| c.candidate.directors.iter().any(|d| d == "Mann")).count();
        assert!(mann >= 2, "light MMR must not wipe a dense cluster");
        let ranked_powell: Vec<_> = (0..20)
            .map(|i| {
                let mut c = cand(&format!("Powell {i}"), &format!("Dir{i}"), 0.85);
                c.positive_features = vec!["John Powell".into()];
                c.person_keys = vec!["John Powell".into()];
                c
            })
            .chain((20..80).map(|i| cand(&format!("Alt {i}"), &format!("AltDir{i}"), 0.82)))
            .collect();
        let mixed = shortlist(&ranked_powell);
        let powell = mixed
            .iter()
            .filter(|c| c.positive_features.iter().any(|f| f == "John Powell"))
            .count();
        assert!(
            powell < 8,
            "shared person FeatureKey must keep filmography from filling the shortlist, got {powell}"
        );
    }

    #[test]
    fn llm_pool_keeps_neighbors_and_craft() {
        let related = cand("Reservoir Dogs", "Tarantino", 0.9);
        let mut craft = cand("Killing Them Softly", "Dominik", 0.85);
        craft.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Greig Fraser".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        craft.matched_features = vec![
            crate::taste::explain::MatchedFeatureView {
                feature_key: String::new(),
                name: "Greig Fraser".into(),
                family: "cinematographer".into(),
                appearances: 4,
                recommendation_mean: 0.45,
                scoring_affinity: 0.45,
                confidence: 0.8,
                portability: 1.0,
                citeable: true,
                cited: true,
            },
            crate::taste::explain::MatchedFeatureView {
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
        ];
        craft.eligibility.candidate_fit = 1.0;
        craft.eligibility.passed = true;
        craft.eligibility.evidence_grade = crate::taste::explain::EvidenceGrade::Medium;
        let mut neighbor = cand("Solo: A Star Wars Story", "Howard", 0.8);
        neighbor.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from Rogue One: A Star Wars Story".into(),
                seed_tmdb_id: Some(330_459),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from Avatar: Fire and Ash".into(),
                seed_tmdb_id: Some(835_33),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        neighbor.matched_features = vec![crate::taste::explain::MatchedFeatureView {
            feature_key: String::new(),
            name: "John Powell".into(),
            family: "composer".into(),
            appearances: 9,
            recommendation_mean: 0.5,
            scoring_affinity: 0.5,
            confidence: 0.8,
            portability: 1.0,
            citeable: true,
            cited: true,
        }];
        neighbor.eligibility.candidate_fit = 1.0;
        neighbor.eligibility.passed = true;
        neighbor.eligibility.evidence_grade = crate::taste::explain::EvidenceGrade::Medium;
        let pool = llm_pool(&[related, craft, neighbor]);
        assert!(pool.iter().all(|c| c.candidate.title != "Reservoir Dogs"));
        assert!(pool.iter().any(|c| c.candidate.title == "Killing Them Softly"));
        assert!(pool.iter().any(|c| c.candidate.title == "Solo: A Star Wars Story"));
    }
}
