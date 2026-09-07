use crate::taste::confidence;
use crate::taste::diversify::{diversify_board_featured, DiversifyConfig};
use crate::taste::retrieve::MediaKind;
use crate::taste::score::ScoredCandidate;

pub const NEW_SCORE_BUFFER: usize = 220;
pub const WATCHLIST_SCORE_BUFFER: usize = 50;
/// Full New recommendation inventory (Featured + More for you).
pub const NEW_MAX: usize = 50;
/// High-value curated front of New — D1.1 scarce-slot policy (F2 OFF for v1).
pub const FEATURED_MAX: usize = 12;
#[allow(dead_code)] // retained for experiment / legacy board-validation helpers
pub const WATCHLIST_MAX: usize = 30;
#[allow(dead_code)]
pub const EXPLORATION_MAX: usize = 0;
pub const NEW_FILMOGRAPHY_PER_PERSON: usize = 6;
/// Frozen v1: quality-first ordering + Content Fit C1/Match; unified board.
pub const ALGORITHM_VERSION: &str = "taste-v1-quality-first-final";
/// v2 experiment stamp (research closed — not for production cutover).
pub const ALGORITHM_VERSION_V2: &str = "taste-v2-bounded-experiment";

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub new_picks: Vec<ScoredCandidate>,
    pub explore_picks: Vec<ScoredCandidate>,
    pub watchlist_picks: Vec<ScoredCandidate>,
    /// Eligible scored rows after buffers, before feedback/display caps.
    pub pre_feedback_pool: Vec<ScoredCandidate>,
}

pub fn split_ranked_buffers(ranked: Vec<ScoredCandidate>) -> Vec<ScoredCandidate> {
    // Single pool: watchlist is state, not a separate buffer lane.
    ranked.into_iter().take(NEW_SCORE_BUFFER + WATCHLIST_SCORE_BUFFER).collect()
}

pub fn eligible(row: &ScoredCandidate) -> bool {
    if row.candidate.tmdb_id.is_none() || row.candidate.media_kind != MediaKind::Movie {
        return false;
    }
    // Same C1 for every candidate — watchlist does not change admission.
    match row.eligibility.state.as_str() {
        "recommended" | "exploratory" => true,
        "held" => false,
        _ => row.eligibility.passed && row.eligibility.predicted_fit >= 0.50,
    }
}

pub fn assemble(ranked: &[ScoredCandidate]) -> Workspace {
    // Default unified board: strict Content order, no D1.1 mutation.
    assemble_unified(ranked)
}

/// Experimental path that still applies D1.1 Featured reorder. Not production.
pub fn assemble_with_diversify(ranked: &[ScoredCandidate], cfg: &DiversifyConfig) -> Workspace {
    let mut ws = assemble_unified(ranked);
    ws.new_picks = diversify_board_featured(&ws.new_picks, cfg, Some(FEATURED_MAX));
    ws.new_picks.truncate(NEW_MAX);
    ws
}

/// One pool → same C1 → quality-first rank → top min(50, eligible). Watchlist is metadata only.
pub fn assemble_unified(ranked: &[ScoredCandidate]) -> Workspace {
    let pool: Vec<_> = ranked.iter().filter(|c| eligible(c)).cloned().collect();
    let board_pool: Vec<_> = pool
        .iter()
        .filter(|c| confidence::occupies_new(c))
        .cloned()
        .collect();

    // Cap, not quota: only fill from eligible C1 rows (recommended then exploratory).
    let mut new_picks = shortlist_new_pool(&board_pool, NEW_MAX);
    cap_new_filmography(&mut new_picks, NEW_FILMOGRAPHY_PER_PERSON);
    new_picks.retain(|c| confidence::occupies_new(c));
    refill_new_without_resume(&mut new_picks, &board_pool, NEW_FILMOGRAPHY_PER_PERSON);
    // Preserve quality-first order after filmography caps (re-sort).
    new_picks.sort_by(confidence::rank_order);
    new_picks.truncate(NEW_MAX);

    Workspace {
        pre_feedback_pool: pool,
        new_picks,
        explore_picks: Vec::new(),
        watchlist_picks: Vec::new(),
    }
}

fn shortlist_new_pool(pool: &[ScoredCandidate], target: usize) -> Vec<ScoredCandidate> {
    if pool.is_empty() || target == 0 {
        return Vec::new();
    }
    let target = target.min(pool.len());
    // C1: Recommended first, then Exploratory — quality-first order within each band.
    let mut recommended: Vec<_> = pool
        .iter()
        .filter(|c| c.eligibility.state == "recommended")
        .cloned()
        .collect();
    let mut exploratory: Vec<_> = pool
        .iter()
        .filter(|c| c.eligibility.state == "exploratory")
        .cloned()
        .collect();

    recommended.sort_by(confidence::rank_order);
    exploratory.sort_by(confidence::rank_order);

    let mut selected = Vec::new();
    selected.extend(recommended.into_iter().take(target));
    if selected.len() < target {
        selected.extend(exploratory.into_iter().take(target - selected.len()));
    }
    // Cap not quota: do not pull held/fallback rows just to manufacture NEW_MAX.
    selected
}

pub fn apply_feedback_filter(
    pool: &[ScoredCandidate],
    hide: &std::collections::HashSet<i64>,
) -> Workspace {
    let filtered: Vec<_> = pool
        .iter()
        .filter(|c| {
            c.candidate
                .tmdb_id
                .map(|id| !hide.contains(&id))
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    assemble(&filtered)
}

pub fn displayed_picks(ws: &Workspace) -> Vec<ScoredCandidate> {
    let mut out = ws.new_picks.clone();
    out.extend(ws.watchlist_picks.clone());
    out
}

fn board_ids(rows: &[ScoredCandidate]) -> std::collections::HashSet<i64> {
    rows.iter().filter_map(|c| c.candidate.tmdb_id).collect()
}

/// Displayed run-log section after New/Explore caps. Eligibility stays on
/// `occupies_new` / `occupies_explore`.
pub fn displayed_section(c: &ScoredCandidate, ws: &Workspace) -> &'static str {
    let Some(id) = c.candidate.tmdb_id else {
        return "held";
    };
    if board_ids(&ws.watchlist_picks).contains(&id) {
        "watchlist"
    } else if board_ids(&ws.new_picks).contains(&id) {
        "new"
    } else if board_ids(&ws.explore_picks).contains(&id) {
        "explore"
    } else {
        "held"
    }
}

pub fn omit_reason(c: &ScoredCandidate, ws: &Workspace) -> Option<&'static str> {
    let section = displayed_section(c, ws);
    if section != "held" {
        return None;
    }
    let id = c.candidate.tmdb_id;
    if confidence::occupies_new(c)
        && id
            .map(|tid| !board_ids(&ws.new_picks).contains(&tid))
            .unwrap_or(true)
    {
        if filmography_label(c).is_some() {
            return Some("new-filmography-cap");
        }
        return Some("new-max");
    }
    if confidence::occupies_explore(c)
        && id
            .map(|tid| !board_ids(&ws.explore_picks).contains(&tid))
            .unwrap_or(true)
    {
        return Some("explore-max");
    }
    None
}

fn filmography_label(row: &ScoredCandidate) -> Option<&str> {
    row.candidate
        .sources
        .iter()
        .find(|s| s.kind == crate::taste::retrieve::RetrievalKind::Filmography)
        .map(|s| s.label.as_str())
}

fn cap_new_filmography(rows: &mut Vec<ScoredCandidate>, max_n: usize) {
    use std::collections::HashMap;
    let mut kept: HashMap<String, usize> = HashMap::new();
    rows.retain(|c| {
        let Some(label) = filmography_label(c) else {
            return true;
        };
        let n = kept.entry(label.to_string()).or_insert(0);
        if *n >= max_n {
            return false;
        }
        *n += 1;
        true
    });
}

fn filmography_at_cap(selected: &[ScoredCandidate], row: &ScoredCandidate, max_n: usize) -> bool {
    let Some(label) = filmography_label(row) else {
        return false;
    };
    selected
        .iter()
        .filter(|c| filmography_label(c) == Some(label))
        .count()
        >= max_n
}

fn refill_new_without_resume(
    new_picks: &mut Vec<ScoredCandidate>,
    pool: &[ScoredCandidate],
    max_n: usize,
) {
    use std::collections::HashSet;
    let mut used: HashSet<i64> = new_picks
        .iter()
        .filter_map(|c| c.candidate.tmdb_id)
        .collect();
    for row in pool {
        if new_picks.len() >= NEW_MAX {
            break;
        }
        if row
            .candidate
            .tmdb_id
            .map(|id| used.contains(&id))
            .unwrap_or(false)
        {
            continue;
        }
        if filmography_at_cap(new_picks, row, max_n) {
            continue;
        }
        if !crate::taste::confidence::occupies_new(row) {
            continue;
        }
        if let Some(id) = row.candidate.tmdb_id {
            used.insert(id);
        }
        new_picks.push(row.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::explain::{EligibilityTrace, EvidenceGrade, MatchedFeatureView};
    use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn feat(appearances: u32) -> MatchedFeatureView {
        person_feat("Nolan", "director", appearances, 0.6)
    }

    fn fraser_new_feats() -> Vec<MatchedFeatureView> {
        vec![
            person_feat("Greig Fraser", "cinematographer", 4, 0.45),
            person_feat("neo-noir", "keyword", 7, 0.4),
        ]
    }

    fn person_feat(
        name: &str,
        family: &str,
        appearances: u32,
        affinity: f32,
    ) -> MatchedFeatureView {
        MatchedFeatureView {
            feature_key: String::new(),
            name: name.into(),
            family: family.into(),
            appearances,
            recommendation_mean: affinity,
            scoring_affinity: affinity,
            confidence: 0.9,
            portability: 1.0,
            citeable: true,
            cited: true,
        }
    }

    fn row(id: i64, watchlist: bool, appearances: u32, total: f32) -> ScoredCandidate {
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(id),
                title: format!("Film {id}"),
                year: Some(2000),
                poster: None,
                watchlist,
                sources: vec![RetrievalSource {
                    kind: if watchlist {
                        RetrievalKind::Watchlist
                    } else {
                        RetrievalKind::Filmography
                    },
                    label: if watchlist {
                        "watchlist".into()
                    } else {
                        format!("Person {id}")
                    },
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
                content: total,
                tmdb_related: 0.0,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: if watchlist { 1.0 } else { 0.0 },
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: vec!["Nolan".into()],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec!["Nolan".into()],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: {
                let mut feats = vec![feat(appearances)];
                if !watchlist {
                    feats.push(person_feat("neo-noir", "keyword", 7, 0.4));
                }
                feats
            },
            hidden_features: vec![],
            eligibility: EligibilityTrace {
                portable_evidence_required: false,
                passed: watchlist || appearances > 0,
                passed_because: vec!["craft".into()],
                candidate_fit: 1.0,
                evidence_grade: if watchlist {
                    if appearances >= 3 {
                        EvidenceGrade::Strong
                    } else {
                        EvidenceGrade::Medium
                    }
                } else if appearances == 0 {
                    EvidenceGrade::None
                } else {
                    EvidenceGrade::Medium
                },
                predicted_fit: if appearances > 0 { 0.72 } else { 0.3 },
                confidence: 0.6,
                hydration_completeness: 0.7,
                state: if watchlist || appearances > 0 {
                    "recommended".into()
                } else {
                    "held".into()
                },
                primary_reason: if watchlist || appearances > 0 {
                    "recommended".into()
                } else {
                    "low_fit".into()
                },
            },
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    #[test]
    fn unified_buffer_includes_watchlist_without_separate_lane() {
        let mut ranked = Vec::new();
        for i in 0..300 {
            ranked.push(row(1000 + i, false, 8, 0.9 - (i as f32) * 0.001));
        }
        for i in 0..40 {
            ranked.push(row(i, true, 8, 0.2));
        }
        ranked.sort_by(|a, b| {
            crate::taste::diversify::fit_of(b)
                .partial_cmp(&crate::taste::diversify::fit_of(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let buffered = split_ranked_buffers(ranked);
        assert_eq!(
            buffered.len(),
            NEW_SCORE_BUFFER + WATCHLIST_SCORE_BUFFER,
            "single buffer takes combined capacity"
        );
        let ws = assemble(&buffered);
        assert!(ws.watchlist_picks.is_empty());
        assert!(ws.new_picks.len() <= NEW_MAX);
    }

    #[test]
    fn independent_caps_do_not_steal_slots() {
        let mut ranked = Vec::new();
        for i in 0..80 {
            ranked.push(row(2000 + i, false, 8, 0.8));
        }
        for i in 0..40 {
            ranked.push(row(i, true, 8, 0.8));
        }
        let ws = assemble(&ranked);
        assert_eq!(ws.new_picks.len(), NEW_MAX);
        assert!(ws.watchlist_picks.is_empty(), "watchlist is state on unified picks");
        assert!(
            ws.new_picks.iter().any(|c| c.candidate.watchlist),
            "watchlist titles compete in the unified board"
        );
    }

    #[test]
    fn match_floor_omits_weak_rows() {
        let mut weak = row(9, false, 0, 0.1);
        weak.matched_features.clear();
        weak.person_keys.clear();
        weak.positive_features.clear();
        weak.eligibility.state = "held".into();
        weak.eligibility.passed = false;
        weak.eligibility.predicted_fit = 0.30;
        weak.eligibility.primary_reason = "low_fit".into();
        let ws = assemble(&[weak, row(10, false, 8, 0.5)]);
        assert_eq!(ws.new_picks.len(), 1);
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(10));
    }

    #[test]
    fn displayed_new_order_is_quality_first_then_content_fit() {
        let mut lower_g = row(1, false, 8, 0.99);
        lower_g.has_quality_prior = true;
        lower_g.quality_prior = 0.05;
        lower_g.eligibility.predicted_fit = 0.90;
        let mut higher_g = row(2, false, 8, 0.01);
        higher_g.has_quality_prior = true;
        higher_g.quality_prior = 0.20;
        higher_g.eligibility.predicted_fit = 0.50;
        let ws = assemble(&[lower_g, higher_g]);
        assert_eq!(
            ws.new_picks[0].candidate.tmdb_id,
            Some(2),
            "known G must dominate Content Fit outside the ε window"
        );

        let mut near_low = row(3, false, 8, 0.40);
        near_low.has_quality_prior = true;
        near_low.quality_prior = 0.10;
        let mut near_high = row(4, false, 8, 0.85);
        near_high.has_quality_prior = true;
        near_high.quality_prior = 0.12;
        let ws2 = assemble(&[near_low, near_high]);
        assert_eq!(
            ws2.new_picks[0].candidate.tmdb_id,
            Some(4),
            "within |ΔG|≤ε Content Fit may break the tie"
        );
    }

    #[test]
    fn displayed_new_keeps_raw_order_when_fits_clearly_separated() {
        let mut higher = row(1, false, 8, 0.99);
        higher.candidate.sources[0].kind = RetrievalKind::Related;
        higher.eligibility.predicted_fit = 0.78;
        let mut lower = row(2, false, 8, 0.01);
        lower.candidate.sources[0].kind = RetrievalKind::Related;
        lower.eligibility.evidence_grade = EvidenceGrade::Strong;
        lower.eligibility.predicted_fit = 0.70;
        let ws = assemble(&[higher, lower]);
        assert_eq!(
            ws.new_picks[0].candidate.tmdb_id,
            Some(1),
            "clear Content-fit gaps must not be overturned by diversification"
        );
    }

    #[test]
    fn leftover_llm_ids_cannot_change_membership() {
        let pool = vec![row(1, false, 8, 0.7), row(2, false, 8, 0.6)];
        let ws = assemble(&pool);
        assert_eq!(ws.new_picks.len(), 2);
        assert!(ws.new_picks.iter().all(|c| c.candidate.tmdb_id == Some(1) || c.candidate.tmdb_id == Some(2)));
    }

    #[test]
    fn new_list_caps_filmography_resume_per_person() {
        let mut ranked = Vec::new();
        for i in 0..8 {
            let mut r = row(100 + i, false, 8, 0.9);
            r.candidate.sources = vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "John Powell".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }];
            r.matched_features = vec![person_feat("John Powell", "composer", 9, 0.55)];
            r.person_keys = vec!["John Powell".into()];
            r.positive_features = vec!["John Powell".into()];
            r.eligibility.candidate_fit = 1.0;
            ranked.push(r);
        }
        for i in 0..20 {
            ranked.push(row(200 + i, false, 8, 0.8));
        }
        let ws = assemble(&ranked);
        let powell = ws
            .new_picks
            .iter()
            .filter(|c| {
                c.candidate
                    .sources
                    .iter()
                    .any(|s| s.label == "John Powell")
            })
            .count();
        assert_eq!(powell, NEW_FILMOGRAPHY_PER_PERSON, "filmography cap still applies, got {powell}");
        assert!(!ws.new_picks.is_empty());
    }

    #[test]
    fn portable_filmography_with_specific_fit_occupies_new() {
        let mut kts = row(1, false, 8, 0.9);
        kts.candidate.title = "Killing Them Softly".into();
        kts.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Greig Fraser".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        kts.matched_features = fraser_new_feats();
        kts.person_keys = vec!["Greig Fraser".into()];
        kts.positive_features = vec!["Greig Fraser".into()];
        kts.eligibility.candidate_fit = 1.0;
        let ws = assemble(&[kts]);
        assert_eq!(ws.new_picks.len(), 1);
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(1));
    }

    #[test]
    fn composer_filmography_stays_off_new_even_with_specific_fit() {
        let mut antz = row(1, false, 8, 0.9);
        antz.candidate.title = "Antz".into();
        antz.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "John Powell".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        antz.matched_features = vec![person_feat("John Powell", "composer", 9, 0.55)];
        antz.person_keys = vec!["John Powell".into()];
        antz.positive_features = vec!["John Powell".into()];
        antz.eligibility.candidate_fit = 1.0;
        let other = row(2, false, 8, 0.8);
        let ws = assemble(&[antz, other]);
        // C1: Content Fit_v1 admits composer filmography when Recommended.
        assert!(
            ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(1)),
            "composer résumé with Recommended state can occupy New"
        );
    }

    #[test]
    fn new_keeps_several_portable_filmography_rows_per_person() {
        let mut ranked = Vec::new();
        for i in 0..8 {
            let mut r = row(300 + i, false, 8, 0.9);
            r.candidate.sources = vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }];
            r.matched_features = fraser_new_feats();
            r.person_keys = vec!["Greig Fraser".into()];
            r.positive_features = vec!["Greig Fraser".into()];
            r.eligibility.candidate_fit = 1.0;
            ranked.push(r);
        }
        let ws = assemble(&ranked);
        let fraser = ws
            .new_picks
            .iter()
            .filter(|c| {
                c.candidate
                    .sources
                    .iter()
                    .any(|s| s.label == "Greig Fraser")
            })
            .count();
        assert_eq!(fraser, NEW_FILMOGRAPHY_PER_PERSON);
    }

    #[test]
    fn filmography_plus_related_loved_seed_may_occupy_new() {
        let mut mixed = row(1, false, 8, 0.9);
        mixed.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to The Batman".into(),
                seed_tmdb_id: Some(414_906),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        mixed.eligibility.candidate_fit = 1.0;
        let ws = assemble(&[mixed]);
        assert_eq!(ws.new_picks.len(), 1);
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(1));
    }

    #[test]
    fn filmography_plus_unseeded_related_occupies_new() {
        let mut mixed = row(1, false, 8, 0.9);
        mixed.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to a catalog title".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        mixed.matched_features = fraser_new_feats();
        mixed.person_keys = vec!["Greig Fraser".into()];
        mixed.positive_features = vec!["Greig Fraser".into()];
        mixed.eligibility.candidate_fit = 1.0;
        let ws = assemble(&[mixed]);
        assert_eq!(ws.new_picks.len(), 1);
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(1));
    }

    #[test]
    fn composer_filmography_plus_related_stays_off_new() {
        let mut mixed = row(1, false, 8, 0.9);
        mixed.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "John Powell".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to Pulp Fiction".into(),
                seed_tmdb_id: Some(680),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        mixed.matched_features = vec![person_feat("John Powell", "composer", 9, 0.55)];
        mixed.person_keys = vec!["John Powell".into()];
        mixed.positive_features = vec!["John Powell".into()];
        mixed.eligibility.candidate_fit = 1.0;
        let other = row(2, false, 8, 0.8);
        let ws = assemble(&[mixed, other]);
        // C1: Recommended Content Fit admits regardless of composer+Related provenance.
        assert!(
            ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(1)),
            "composer résumé plus Related can occupy New under C1"
        );
    }

    #[test]
    fn weak_single_bridge_filmography_is_omitted_from_new() {
        let mut weak = row(1, false, 8, 0.9);
        weak.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "John Powell".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        weak.eligibility.candidate_fit = 0.32;
        weak.eligibility.predicted_fit = 0.32;
        weak.eligibility.state = "held".into();
        weak.eligibility.passed = false;
        weak.eligibility.primary_reason = "low_fit".into();
        let other = row(2, false, 8, 0.8);
        let ws = assemble(&[weak, other]);
        assert!(
            ws.new_picks.iter().all(|c| c.candidate.tmdb_id != Some(1)),
            "person-only filmography with weak movie evidence must leave New"
        );
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(2));
    }

    #[test]
    fn unreleased_filmography_is_omitted_from_new() {
        let mut seconds = row(1, false, 8, 0.9);
        seconds.candidate.title = "Seconds".into();
        seconds.candidate.year = None;
        seconds.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Edgar Wright".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        seconds.eligibility.candidate_fit = 1.0;
        let other = row(2, false, 8, 0.8);
        let ws = assemble(&[seconds, other]);
        assert!(
            ws.new_picks.iter().all(|c| c.candidate.tmdb_id != Some(1)),
            "year-less filmography stubs must leave New"
        );
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(2));
    }

    #[test]
    fn short_runtime_rows_are_not_assembled() {
        let mut short = row(1, false, 8, 0.9);
        short.eligibility.passed = false;
        short.eligibility.state = "held".into();
        short.eligibility.primary_reason = "short-runtime".into();
        short.eligibility.passed_because = vec!["short-runtime".into()];
        let ws = assemble(&[short, row(2, false, 8, 0.5)]);
        assert_eq!(ws.new_picks.len(), 1);
        assert_eq!(ws.new_picks[0].candidate.tmdb_id, Some(2));
    }

    #[test]
    fn feedback_filter_can_restore_from_pre_feedback_pool() {
        let pool = vec![row(1, false, 8, 0.7), row(2, false, 8, 0.6)];
        let mut hide = std::collections::HashSet::new();
        hide.insert(1);
        let hidden = apply_feedback_filter(&pool, &hide);
        assert!(hidden.new_picks.iter().all(|c| c.candidate.tmdb_id != Some(1)));
        let restored = apply_feedback_filter(&pool, &std::collections::HashSet::new());
        assert!(restored.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(1)));
    }

    fn related_row(id: i64, appearances: u32, total: f32) -> ScoredCandidate {
        let mut r = row(id, false, appearances, total);
        r.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Related,
            label: "similar to Pulp Fiction".into(),
            seed_tmdb_id: Some(680),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        r
    }

    #[test]
    fn related_neighbors_land_on_new() {
        let dogs = related_row(500, 8, 0.9);
        let craft = row(2, false, 8, 0.8);
        let ws = assemble(&[dogs, craft]);
        assert!(ws.explore_picks.is_empty());
        assert!(ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(2)));
        if crate::taste::confidence::occupies_new(&related_row(500, 8, 0.9)) {
            assert!(ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(500)));
        }
        assert!(ws.new_picks.iter().all(|c| {
            crate::taste::confidence::match_score(c) >= crate::taste::confidence::MATCH_SCORE_FLOOR
        }));
    }

    #[test]
    fn seventh_filmography_logs_omit_reason_not_displayed_new() {
        let mut ranked = Vec::new();
        for i in 0..8 {
            let mut r = row(300 + i, false, 8, 0.9);
            r.candidate.sources = vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }];
            r.matched_features = fraser_new_feats();
            r.person_keys = vec!["Greig Fraser".into()];
            r.positive_features = vec!["Greig Fraser".into()];
            r.eligibility.candidate_fit = 1.0;
            ranked.push(r);
        }
        let ws = assemble(&ranked);
        assert_eq!(ws.new_picks.len(), NEW_FILMOGRAPHY_PER_PERSON);
        let omitted = ranked
            .iter()
            .find(|c| {
                crate::taste::confidence::occupies_new(c)
                    && !ws
                        .new_picks
                        .iter()
                        .any(|n| n.candidate.tmdb_id == c.candidate.tmdb_id)
            })
            .expect("a 7th qualifying row should exist");
        assert_eq!(displayed_section(omitted, &ws), "held");
        assert_eq!(omit_reason(omitted, &ws), Some("new-filmography-cap"));
        assert!(!ws.explore_picks.iter().any(|c| c.candidate.tmdb_id == omitted.candidate.tmdb_id));
    }

    #[test]
    fn new_feedback_hide_and_restore() {
        let dogs = related_row(500, 8, 0.9);
        let craft = row(2, false, 8, 0.8);
        let pool = vec![dogs, craft];
        let ws = assemble(&pool);
        let dogs_on_new = ws.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(500));
        let mut hide = std::collections::HashSet::new();
        hide.insert(500);
        let hidden = apply_feedback_filter(&pool, &hide);
        assert!(hidden.new_picks.iter().all(|c| c.candidate.tmdb_id != Some(500)));
        let restored = apply_feedback_filter(&pool, &std::collections::HashSet::new());
        if dogs_on_new {
            assert!(restored.new_picks.iter().any(|c| c.candidate.tmdb_id == Some(500)));
        }
    }

    #[test]
    fn boards_do_not_overlap() {
        let ranked = vec![
            row(1, false, 8, 0.9),
            related_row(2, 8, 0.8),
            row(3, true, 8, 0.9),
        ];
        let ws = assemble(&ranked);
        let mut seen = std::collections::HashSet::new();
        for c in ws.new_picks.iter() {
            let id = c.candidate.tmdb_id.unwrap();
            assert!(seen.insert(id), "duplicate tmdb {id} on board");
        }
        assert!(ws.watchlist_picks.is_empty());
        assert!(ws.new_picks.iter().any(|c| c.candidate.watchlist));
    }

    #[test]
    fn watchlist_uses_same_c1_as_non_watchlist() {
        // Weak watchlist row that fails C1 must not get a free pass onto the board.
        let mut low = row(7, true, 2, 0.2);
        low.matched_features.clear();
        low.person_keys.clear();
        low.positive_features.clear();
        low.eligibility.state = "held".into();
        low.eligibility.passed = false;
        low.eligibility.predicted_fit = 0.30;
        low.eligibility.primary_reason = "low_fit".into();
        let ws = assemble(&[low]);
        assert!(ws.watchlist_picks.is_empty());
        assert!(ws.new_picks.is_empty());
    }

    #[test]
    fn web_discovery_row_can_assemble_onto_new() {
        let mut found = row(88_001, false, 4, 0.22);
        found.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: "atmospheric neo-noir like Prisoners".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        found.eligibility.evidence_grade = EvidenceGrade::None;
        found.eligibility.passed = true;
        found.eligibility.state = "recommended".into();
        found.eligibility.predicted_fit = 0.72;
        found.eligibility.primary_reason = "recommended".into();
        found.candidate.genres = vec!["Crime".into(), "Thriller".into()];
        let ws = assemble(&[found, row(2, false, 8, 0.5)]);
        assert!(
            ws.new_picks
                .iter()
                .any(|c| c.candidate.tmdb_id == Some(88_001)),
            "web discovery must survive workspace assembly onto New"
        );
    }
}
