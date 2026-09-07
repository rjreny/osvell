use crate::taste::reason::ReasonerPick;
use crate::taste::retrieve::identity_key;
use crate::taste::score::ScoredCandidate;
use crate::taste::workspace;
use std::collections::{HashMap, HashSet};

pub const MIN_SHORTLIST_PICKS: usize = 8;
pub const MAX_DISCOVERY_PICKS: usize = 3;

#[derive(Debug, Clone)]
pub struct DiversityWarning {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub picks: Vec<ScoredCandidate>,
    pub dropped: Vec<String>,
    pub warnings: Vec<DiversityWarning>,
    pub narrow_profile: bool,
    pub workspace: workspace::Workspace,
}

pub fn hard_validate(
    _picks: &[ReasonerPick],
    shortlist: &[ScoredCandidate],
    discoveries: &[ScoredCandidate],
    seen: &HashSet<String>,
    profile_mode_count: usize,
) -> ValidationResult {
    let mut dropped = Vec::new();
    let mut ranked = Vec::new();
    let mut used = HashSet::new();
    for c in shortlist.iter().chain(discoveries.iter()) {
        let key = identity_key(
            c.candidate.tmdb_id,
            &c.candidate.title,
            c.candidate.year,
        );
        if seen.contains(&key) {
            dropped.push(format!("seen: {}", c.candidate.title));
            continue;
        }
        if !c.eligibility.passed || !c.eligibility.evidence_grade.displayable() {
            dropped.push(format!("contextual-only: {}", c.candidate.title));
            continue;
        }
        if c.candidate.tmdb_id.is_none() {
            dropped.push(format!("invalid identity: {}", c.candidate.title));
            continue;
        }
        if !used.insert(key) {
            continue;
        }
        ranked.push(c.clone());
    }
    let ws = workspace::assemble(&ranked);
    let kept = workspace::displayed_picks(&ws);
    let warnings = diversity_warnings(&kept);
    let narrow = profile_mode_count <= 2 || is_narrow(&kept);
    ValidationResult {
        picks: kept,
        dropped,
        warnings,
        narrow_profile: narrow,
        workspace: ws,
    }
}

pub fn diversity_warnings(picks: &[ScoredCandidate]) -> Vec<DiversityWarning> {
    let mut dirs: HashMap<String, u32> = HashMap::new();
    let mut genres: HashMap<String, u32> = HashMap::new();
    let mut modes: HashMap<String, u32> = HashMap::new();
    let mut people: HashMap<String, u32> = HashMap::new();
    for p in picks {
        for d in &p.candidate.directors {
            *dirs.entry(d.clone()).or_insert(0) += 1;
        }
        if let Some(g) = p.candidate.genres.first() {
            *genres.entry(g.clone()).or_insert(0) += 1;
        }
        if let Some(m) = p.candidate.modes.first() {
            *modes.entry(m.clone()).or_insert(0) += 1;
        }
        for f in &p.person_keys {
            *people.entry(f.clone()).or_insert(0) += 1;
        }
    }
    let mut out = Vec::new();
    for (d, n) in dirs {
        if n > 3 {
            out.push(DiversityWarning {
                message: format!("{n} films share director {d}"),
            });
        }
    }
    for (g, n) in genres {
        if n > 4 {
            out.push(DiversityWarning {
                message: format!("{n} films share primary genre {g}"),
            });
        }
    }
    for (m, n) in modes {
        if n > 5 {
            out.push(DiversityWarning {
                message: format!("{n} films share taste mode {m}"),
            });
        }
    }
    for (person, n) in people {
        if n > 3 {
            out.push(DiversityWarning {
                message: format!("{n} films share person {person}"),
            });
        }
    }
    out
}

fn is_narrow(picks: &[ScoredCandidate]) -> bool {
    if picks.len() < 6 {
        return false;
    }
    let mut dirs: HashMap<String, u32> = HashMap::new();
    for p in picks {
        for d in &p.candidate.directors {
            *dirs.entry(d.clone()).or_insert(0) += 1;
        }
    }
    dirs.values().any(|n| *n as usize * 2 >= picks.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn scored(id: i64, title: &str, director: &str) -> ScoredCandidate {
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(id),
                title: title.into(),
                year: Some(1999),
                poster: None,
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Filmography,
                    label: format!("{director} {id}"),
                    seed_tmdb_id: None,
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
                content: 0.5,
                tmdb_related: 0.4,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: 0.0,
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total: 0.5,
            },
            reasons: vec!["r".into()],
            evidence: vec!["Heat".into()],
            positive_features: vec![director.to_string()],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec![director.to_string()],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![
                crate::taste::explain::MatchedFeatureView {
                    feature_key: String::new(),
                    name: director.into(),
                    family: "director".into(),
                    appearances: 8,
                    recommendation_mean: 0.6,
                    scoring_affinity: 0.5,
                    confidence: 0.9,
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
            ],
            hidden_features: vec![],
            eligibility: crate::taste::explain::EligibilityTrace {
                portable_evidence_required: false,
                passed: true,
                passed_because: vec!["craft".into()],
                candidate_fit: 1.0,
                evidence_grade: crate::taste::explain::EvidenceGrade::Medium,
                ..Default::default()
            },
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    fn pick(id: i64, title: &str) -> ReasonerPick {
        ReasonerPick {
            id: format!("tmdb:{id}"),
            title: title.into(),
            year: Some(1999),
            why: "because".into(),
            mode: "core".into(),
            rhymes_with: vec![],
        }
    }

    #[test]
    fn drops_seen_and_ignores_llm_picks() {
        let short = vec![scored(1, "Heat", "Mann"), scored(2, "Thief", "Mann")];
        let picks = vec![
            pick(1, "Heat"),
            pick(99, "Invented"),
        ];
        let mut seen = HashSet::new();
        seen.insert("tmdb:1".into());
        let result = hard_validate(&picks, &short, &[], &seen, 4);
        assert!(result.dropped.iter().any(|d| d.contains("seen")));
        assert!(result.picks.iter().any(|p| p.candidate.tmdb_id == Some(2)));
        assert!(result.picks.iter().all(|p| p.candidate.tmdb_id != Some(1)));
        assert!(result.picks.iter().all(|p| p.candidate.tmdb_id != Some(99)));
    }

    #[test]
    fn scorer_owns_membership_not_call2() {
        let short: Vec<_> = (1..20)
            .map(|i| scored(i, &format!("Film {i}"), "Mann"))
            .collect();
        let picks: Vec<_> = (1..13).map(|i| pick(i, &format!("Film {i}"))).collect();
        let result = hard_validate(&picks, &short, &[], &HashSet::new(), 4);
        assert_eq!(result.picks.len(), 19);
        assert!(result.picks.iter().any(|p| p.candidate.tmdb_id == Some(19)));
    }

    #[test]
    fn drops_contextual_only_core_picks() {
        let mut dirty = scored(3, "Dirty", "Other");
        dirty.contextual_only = true;
        dirty.eligibility.passed = false;
        dirty.eligibility.evidence_grade = crate::taste::explain::EvidenceGrade::None;
        dirty.reasons = vec!["2000s affinity (0.25)".into()];
        let short = vec![scored(1, "Heat", "Mann"), scored(2, "Thief", "Mann"), dirty];
        let picks = vec![pick(3, "Dirty"), pick(1, "Heat")];
        let result = hard_validate(&picks, &short, &[], &HashSet::new(), 4);
        assert!(result.dropped.iter().any(|d| d.contains("contextual-only")));
        assert!(result.picks.iter().all(|p| p.candidate.tmdb_id != Some(3)));
        assert!(result.picks.iter().any(|p| p.candidate.tmdb_id == Some(1)));
    }

    fn shallow_keyword(id: i64, title: &str) -> ScoredCandidate {
        let mut c = scored(id, title, "Other");
        c.positive_features = vec!["drugs".into()];
        c.reasons = vec!["drugs affinity (0.22)".into()];
        c.scoring_reasons = vec!["drugs affinity (0.22)".into()];
        c.display_reasons = vec!["drugs · 4 films".into()];
        c.score.tmdb_related = 1.0;
        c.score.total = 0.40;
        c
    }

    fn prestige_style(id: i64) -> ScoredCandidate {
        let mut c = scored(id, "The Prestige", "Christopher Nolan");
        c.candidate.watchlist = true;
        c.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Christopher Nolan".into(),
                seed_tmdb_id: Some(155),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Watchlist,
                label: "watchlist".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        c.person_keys = vec!["Christopher Nolan".into(), "Wally Pfister".into()];
        c.positive_features = vec!["Christopher Nolan".into(), "Wally Pfister".into()];
        c.matched_features = vec![
            crate::taste::explain::MatchedFeatureView {
                feature_key: String::new(),
                name: "Christopher Nolan".into(),
                family: "director".into(),
                appearances: 4,
                recommendation_mean: 0.71,
                scoring_affinity: 0.45,
                confidence: 0.63,
                portability: 1.0,
                citeable: true,
                cited: true,
            },
            crate::taste::explain::MatchedFeatureView {
                feature_key: String::new(),
                name: "Wally Pfister".into(),
                family: "cinematographer".into(),
                appearances: 3,
                recommendation_mean: 0.69,
                scoring_affinity: 0.40,
                confidence: 0.70,
                portability: 1.0,
                citeable: true,
                cited: true,
            },
        ];
        c.display_reasons = vec!["Christopher Nolan · 4 films".into(), "Wally Pfister · 3 films".into()];
        c.score.total = 0.23;
        c
    }

    #[test]
    fn stronger_evidence_survives_without_call2_selection() {
        let mut short: Vec<_> = (1..13).map(|i| shallow_keyword(i, &format!("Shallow {i}"))).collect();
        short.push(prestige_style(13));
        let picks: Vec<_> = (1..13).map(|i| pick(i, &format!("Shallow {i}"))).collect();
        let result = hard_validate(&picks, &short, &[], &HashSet::new(), 4);
        assert!(
            result.picks.iter().any(|p| p.candidate.title == "The Prestige"),
            "stronger multi-evidence candidate must appear, got {:?}",
            result.picks.iter().map(|p| &p.candidate.title).collect::<Vec<_>>()
        );
        assert!(result.picks.len() <= 100);
    }

    #[test]
    fn warns_when_one_person_dominates_the_final_12() {
        let mut picks = Vec::new();
        for i in 1..13 {
            let mut c = scored(i, &format!("Film {i}"), "Other");
            c.positive_features = vec!["John Powell".into()];
            c.person_keys = vec!["John Powell".into()];
            picks.push(c);
        }
        let warnings = diversity_warnings(&picks);
        assert!(
            warnings.iter().any(|w| w.message.contains("person John Powell")),
            "{:?}",
            warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
        );
    }
}
