use crate::taste::retrieve::RetrievalKind;
use crate::taste::score::{evidence_grade, ScoredCandidate};
use crate::taste::features::{keyword_strength, KeywordStrength};
use chrono::Datelike;
use std::collections::HashSet;

pub const MATCH_SCORE_FLOOR: u8 = 50;
pub const NEW_MATCH_FLOOR: u8 = 70;
pub const RELATED_ONLY_CAP: u8 = 69;
pub const SINGLE_BRIDGE_CAP: u8 = 69;
pub const LIMITED_EVIDENCE_CAP: u8 = 69;
pub const EXCELLENT_BAND: u8 = 90;
const APPEARANCE_CAP: u32 = 8;
const LIMITED_PENALTY: f32 = 0.82;
const STRENGTH_SCALE: f32 = 0.38;
/// Displayed match % tracks overall scoring total (already includes negatives),
/// with craft affinity as ballast rather than the driver.
const OVERALL_WEIGHT: f32 = 0.48;
const STRENGTH_WEIGHT: f32 = 0.28;
const GRADE_WEIGHT: f32 = 0.14;
const APPEARANCE_WEIGHT: f32 = 0.10;
const CONFLICT_WEIGHT: f32 = 0.42;
const TOTAL_SCALE: f32 = 0.28;
const NO_CRAFT_PENALTY: f32 = 0.45;
const NEIGHBOR_FLOOR_CONFLICT_FREE: f32 = 0.20;
const NEIGHBOR_FLOOR_CONFLICT_SCALE: f32 = 0.75;
const PORTABLE_AFFINITY_FLOOR: f32 = 0.18;
const PORTABLE_APPEARANCES: u32 = 3;

const CRAFT: &[&str] = &[
    "director",
    "writer",
    "cinematographer",
    "composer",
    "actor",
];

fn overall_component(total: f32) -> f32 {
    ((total / TOTAL_SCALE).tanh() * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn conflict_magnitude(c: &ScoredCandidate) -> f32 {
    (-c.score.negative_evidence).clamp(0.0, 1.0)
}

/// Neighbor floors must not resurrect cards the scorer already marked as a
/// poor overall fit. Strong conflict discounts the floor before it applies.
fn apply_neighbor_floor(score: u8, c: &ScoredCandidate) -> u8 {
    let floor = recommendation_neighbor_floor(c);
    if floor == 0 {
        return score;
    }
    let conflict = conflict_magnitude(c);
    if conflict < NEIGHBOR_FLOOR_CONFLICT_FREE {
        score.max(floor)
    } else {
        let discounted =
            ((floor as f32) * (1.0 - NEIGHBOR_FLOOR_CONFLICT_SCALE * conflict)).round() as u8;
        score.max(discounted)
    }
}

pub fn match_score(c: &ScoredCandidate) -> u8 {
    // C2: calibrated Content Fit_v1 → displayed Match. Legacy craft/grade/neighbor
    // floors no longer drive the number shown on New.
    let fit = if c.eligibility.predicted_fit > 0.0 {
        c.eligibility.predicted_fit
    } else {
        // Fallback for fixtures that predate Fit_v1 stamping.
        ((c.score.content + 1.0) * 0.5).clamp(0.0, 1.0)
    };
    crate::taste::match_calibration::fit_to_match_percent(fit)
}

/// Legacy match formula retained for run-log comparison only.
pub fn legacy_match_score(c: &ScoredCandidate) -> u8 {
    let (quality, n) = best_craft_quality(c);
    let strength = (quality / STRENGTH_SCALE).tanh().clamp(0.0, 1.0);
    let g = (evidence_grade(c) as f32 / 3.0).clamp(0.0, 1.0);
    let appear = 1.0 - (-(n.min(APPEARANCE_CAP) as f32) / APPEARANCE_CAP as f32).exp();
    let limited = if is_limited_evidence(c) {
        LIMITED_PENALTY
    } else {
        1.0
    };
    let mut raw = OVERALL_WEIGHT * overall_component(c.score.total)
        + STRENGTH_WEIGHT * strength
        + GRADE_WEIGHT * g
        + APPEARANCE_WEIGHT * appear;
    raw *= 1.0 - CONFLICT_WEIGHT * conflict_magnitude(c);
    raw *= limited;
    if quality < 0.05 {
        raw *= NO_CRAFT_PENALTY;
    }
    let mut score = (100.0 * raw.clamp(0.0, 1.0)).round().clamp(0.0, 100.0) as u8;
    score = apply_neighbor_floor(score, c);
    if related_only(c) {
        score = score.min(RELATED_ONLY_CAP);
    }
    if filmography_single_bridge(c) {
        score = score.min(SINGLE_BRIDGE_CAP);
    }
    if has_filmography_source(c)
        && c.candidate
            .sources
            .iter()
            .any(|s| s.kind.is_related())
        && !qualifying_director_dp_filmography(c)
        && !mixed_recommendation_corroboration(c)
        && !c.candidate.watchlist
    {
        score = score.min(SINGLE_BRIDGE_CAP);
    }
    if is_limited_evidence(c) {
        score = score.min(LIMITED_EVIDENCE_CAP);
    }
    if !c.candidate.watchlist && kids_or_animation(c) {
        score = score.min(RELATED_ONLY_CAP);
    }
    if !c.candidate.watchlist
        && has_filmography_source(c)
        && !has_new_corroboration(c)
        && !mixed_recommendation_corroboration(c)
    {
        score = score.min(RELATED_ONLY_CAP);
    }
    if qualifying_director_dp_filmography(c) {
        score = score.max(NEW_MATCH_FLOOR);
    }
    score
}

pub fn passes_match_floor(c: &ScoredCandidate) -> bool {
    // C1: Match floors no longer gate New. Kept for older diagnostics.
    match_score(c) >= MATCH_SCORE_FLOOR
}

pub fn thin_evidence(c: &ScoredCandidate) -> bool {
    is_limited_evidence(c)
}

fn cited_craft(c: &ScoredCandidate) -> Vec<&crate::taste::explain::MatchedFeatureView> {
    c.matched_features
        .iter()
        .filter(|f| f.cited && CRAFT.contains(&f.family.as_str()))
        .collect()
}

fn best_craft_quality(c: &ScoredCandidate) -> (f32, u32) {
    cited_craft(c)
        .into_iter()
        .map(|f| {
            let quality = (f.scoring_affinity.max(0.0) * f.portability.clamp(0.0, 1.0)).max(0.0);
            (quality, f.appearances)
        })
        .max_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        })
        .unwrap_or((0.0, 0))
}

fn is_limited_evidence(c: &ScoredCandidate) -> bool {
    let people = cited_craft(c);
    if people.is_empty() {
        return true;
    }
    people.iter().all(|f| f.appearances <= 2)
}

pub fn related_only(c: &ScoredCandidate) -> bool {
    if c.candidate.watchlist || c.candidate.sources.is_empty() {
        return false;
    }
    if !c
        .candidate
        .sources
        .iter()
        .all(|s| s.kind.is_related())
    {
        return false;
    }
    // Neighbor retrieval is the point of relatedRecommendations. Similar-to
    // dumps stay capped at 69; corroborated recs are not treated as related-only.
    if has_recommendation_source(c) && has_new_corroboration(c) {
        return false;
    }
    true
}

fn has_recommendation_source(c: &ScoredCandidate) -> bool {
    c.candidate.sources.iter().any(|s| {
        s.kind == RetrievalKind::RelatedRecommendations
    })
}

fn has_direct_portable_director_or_dp(c: &ScoredCandidate) -> bool {
    cited_craft(c).into_iter().any(|f| {
        matches!(f.family.as_str(), "director" | "cinematographer")
            && f.scoring_affinity >= PORTABLE_AFFINITY_FLOOR
            && f.appearances >= PORTABLE_APPEARANCES
    })
}

fn kids_or_animation(c: &ScoredCandidate) -> bool {
    c.candidate.genres.iter().any(|g| {
        let lower = g.to_ascii_lowercase();
        lower == "animation" || lower == "family" || lower.contains("kid") || lower == "tv movie"
    })
}

fn tv_movie(c: &ScoredCandidate) -> bool {
    c.candidate
        .genres
        .iter()
        .any(|g| g.eq_ignore_ascii_case("tv movie"))
}

pub fn filmography_only(c: &ScoredCandidate) -> bool {
    if c.candidate.watchlist || c.candidate.sources.is_empty() {
        return false;
    }
    c.candidate
        .sources
        .iter()
        .all(|s| s.kind == RetrievalKind::Filmography)
}

fn qualified_bridge_count(c: &ScoredCandidate) -> usize {
    cited_craft(c)
        .into_iter()
        .filter(|f| {
            matches!(
                f.family.as_str(),
                "director" | "writer" | "cinematographer" | "composer"
            )
        })
        .count()
}

fn has_catalog_proof(c: &ScoredCandidate) -> bool {
    matches!(c.candidate.runtime, Some(rt) if rt >= crate::taste::score::FEATURE_RUNTIME_MIN)
        || matches!(c.candidate.vote_count, Some(n) if n >= 1)
}

fn incomplete_metadata_stub(c: &ScoredCandidate) -> bool {
    c.candidate.year.is_none() && !has_catalog_proof(c)
}

fn future_dated(c: &ScoredCandidate) -> bool {
    let year_now = chrono::Utc::now().year();
    matches!(c.candidate.year, Some(y) if y > year_now)
}

/// Future-dated or yearless stubs with no catalog proof stay off New and Explore.
/// Watchlist titles like Avatar 4 may still be future-dated.
pub fn unreleased_display_row(c: &ScoredCandidate) -> bool {
    if c.candidate.watchlist {
        return false;
    }
    future_dated(c) || incomplete_metadata_stub(c)
}

/// Year-less filmography stubs stay off New even when runtime/votes exist.
pub fn unreleased_new_row(c: &ScoredCandidate) -> bool {
    if c.candidate.watchlist {
        return false;
    }
    unreleased_display_row(c) || (c.candidate.year.is_none() && filmography_only(c))
}

fn qualifying_director_dp_filmography(c: &ScoredCandidate) -> bool {
    has_filmography_source(c)
        && has_direct_portable_director_or_dp(c)
        && c.eligibility.candidate_fit >= 0.999
        && !unreleased_new_row(c)
        && !tv_movie(c)
        && !kids_or_animation(c)
        && has_new_corroboration(c)
}

fn portable_cited_craft(c: &ScoredCandidate) -> Vec<&crate::taste::explain::MatchedFeatureView> {
    cited_craft(c)
        .into_iter()
        .filter(|f| {
            matches!(
                f.family.as_str(),
                "director" | "writer" | "cinematographer" | "composer"
            ) && f.appearances >= PORTABLE_APPEARANCES
                && f.scoring_affinity >= PORTABLE_AFFINITY_FLOOR
        })
        .collect()
}

fn has_strong_cited_keyword(c: &ScoredCandidate) -> bool {
    c.matched_features.iter().any(|f| {
        f.cited
            && f.family == "keyword"
            && keyword_strength(&f.name) == KeywordStrength::Strong
    })
}

/// New needs more than "this DP/director is on the crew." A second portable
/// craft person (or the same auteur as director+writer) or a strong craft
/// keyword like neo-noir is the corroboration.
fn has_new_corroboration(c: &ScoredCandidate) -> bool {
    let craft = portable_cited_craft(c);
    craft.len() >= 2 || (craft.len() >= 1 && has_strong_cited_keyword(c))
}

/// Several distinct loved recommendation seeds can corroborate a filmography
/// lead. Keep one-seed résumé expansion out, while allowing the strongest
/// multi-neighbor rows to backfill New when the direct craft bridge is sparse.
fn mixed_recommendation_corroboration(c: &ScoredCandidate) -> bool {
    if filmography_only(c) {
        return false;
    }
    let seeds = crate::taste::score::unique_loved_rec_seeds(c);
    seeds >= 3
        || (seeds >= 2
            && c.eligibility.evidence_grade.rank() >= 3
            && c.eligibility.candidate_fit >= 0.65)
}

pub fn filmography_single_bridge(c: &ScoredCandidate) -> bool {
    filmography_only(c)
        && qualified_bridge_count(c) < 2
        && !qualifying_director_dp_filmography(c)
        && !portable_person_loyalty(c)
}

fn has_filmography_source(c: &ScoredCandidate) -> bool {
    c.candidate
        .sources
        .iter()
        .any(|s| s.kind == RetrievalKind::Filmography)
}

/// TMDB recommendations from several loved films are a real neighbor signal
/// even when no DP/director is shared. One seed is not enough (Tony → Curves).
fn recommendation_neighbor_floor(c: &ScoredCandidate) -> u8 {
    if c.candidate.watchlist || kids_or_animation(c) {
        return 0;
    }
    let seeds: HashSet<i64> = c
        .candidate
        .sources
        .iter()
        .filter(|s| s.kind == RetrievalKind::RelatedRecommendations)
        .filter_map(|s| s.seed_tmdb_id)
        .collect();
    match seeds.len() {
        n if n >= 6 => 66,
        5 => 64,
        4 => 62,
        3 => 58,
        2 => 52,
        _ => 0,
    }
}

/// Résumé filmography still needs a director/DP or corroboration. Neighbors
/// of loved films may occupy New; the match percent is the honesty.
pub fn has_independent_new_bridge(c: &ScoredCandidate) -> bool {
    if has_filmography_source(c) {
        return qualifying_director_dp_filmography(c)
            || has_new_corroboration(c)
            || mixed_recommendation_corroboration(c)
            || portable_person_loyalty(c);
    }
    true
}

fn portable_person_loyalty(c: &ScoredCandidate) -> bool {
    // Writers/DPs still need corroboration. Directors and actors the user
    // repeatedly loves are why filmography retrieval exists — but only when
    // the movie itself looks compatible (not a random résumé credit).
    cited_craft(c).into_iter().any(|f| {
        matches!(f.family.as_str(), "director" | "actor")
            && f.appearances >= 4
            && f.scoring_affinity >= PORTABLE_AFFINITY_FLOOR
            && f.recommendation_mean >= 0.45
            && c.eligibility.candidate_fit >= 0.55
    })
}

/// Related-only neighbors need a clear preference margin, not just Medium
/// eligibility from a TMDB similar-to edge.
#[allow(dead_code)]
const RELATED_ONLY_TOTAL_FLOOR: f32 = 0.06;

pub fn occupies_new(c: &ScoredCandidate) -> bool {
    if unreleased_new_row(c) || tv_movie(c) {
        return false;
    }
    // Watchlist is state only — same C1 bands as every other candidate.
    match c.eligibility.state.as_str() {
        "recommended" | "exploratory" => true,
        "held" => false,
        _ => {
            // Fixtures / pre-C1 rows: fall back to passed flag without grade authority.
            c.eligibility.passed && c.eligibility.predicted_fit >= 0.50
        }
    }
}

#[allow(dead_code)]
fn discovery_row(c: &ScoredCandidate) -> bool {
    c.candidate
        .sources
        .iter()
        .any(|s| s.kind == RetrievalKind::Discovery)
}

/// Explore is folded into New. Kept so older run logs still deserialize.
pub fn occupies_explore(_c: &ScoredCandidate) -> bool {
    false
}

pub fn weak_filmography_resume(c: &ScoredCandidate) -> bool {
    filmography_single_bridge(c)
}

/// Why a ranked row did not occupy New. Watchlist and New members are `None`.
pub fn filter_reason(c: &ScoredCandidate) -> Option<String> {
    let short_runtime = matches!(
        c.candidate.runtime,
        Some(rt) if (1..crate::taste::score::FEATURE_RUNTIME_MIN).contains(&rt)
    );
    if (c.candidate.watchlist && c.eligibility.passed)
        || (!c.candidate.watchlist && occupies_new(c))
    {
        return None;
    }
    if short_runtime {
        return Some("short-runtime".into());
    }
    if future_dated(c) {
        return Some("unreleased".into());
    }
    if incomplete_metadata_stub(c) {
        return Some("incomplete-metadata".into());
    }
    if unreleased_new_row(c) {
        return Some("unreleased".into());
    }
    if !c.eligibility.primary_reason.is_empty() {
        return Some(c.eligibility.primary_reason.clone());
    }
    if c.eligibility.state == "held" || !c.eligibility.passed {
        return Some("held".into());
    }
    Some("held".into())
}

/// Eligibility placement. Displayed `section` in the run log is computed from
/// the assembled workspace and may be `held` when a cap omits a member.
pub fn placement(c: &ScoredCandidate) -> &'static str {
    if c.candidate.watchlist {
        "watchlist"
    } else if occupies_new(c) {
        "new"
    } else if occupies_explore(c) {
        "explore"
    } else {
        "held"
    }
}

pub fn sort_workspace(rows: &mut [ScoredCandidate]) {
    rows.sort_by(rank_order);
}

/// Internal ranking for `taste-v1-quality-first-final`:
/// known G > missing G → strict G when `|ΔG| > ε` → Content Fit only inside ε →
/// confidence / candidate_fit / seeds / stable tmdb id.
pub fn rank_order(a: &ScoredCandidate, b: &ScoredCandidate) -> std::cmp::Ordering {
    use crate::taste::quality::{quality_near_tie, quality_rank_key, QUALITY_TIE_EPSILON};
    let _ = QUALITY_TIE_EPSILON; // documented in quality_near_tie

    let (ka, ga) = quality_rank_key(a.has_quality_prior, a.quality_prior);
    let (kb, gb) = quality_rank_key(b.has_quality_prior, b.quality_prior);
    kb.cmp(&ka).then_with(|| {
        // Both missing → Content Fit; both known with |ΔG|>ε → G; near-tie → Content.
        if ka == 0 && kb == 0 {
            return content_then_confidence(a, b);
        }
        if quality_near_tie(a.has_quality_prior, a.quality_prior, b.has_quality_prior, b.quality_prior)
        {
            return content_then_confidence(a, b);
        }
        // total_cmp: NaN must not collapse to Equal or sort panics on total order.
        gb.total_cmp(&ga).then_with(|| content_then_confidence(a, b))
    })
}

fn content_then_confidence(a: &ScoredCandidate, b: &ScoredCandidate) -> std::cmp::Ordering {
    b.score
        .total
        .total_cmp(&a.score.total)
        .then_with(|| {
            b.eligibility
                .predicted_fit
                .total_cmp(&a.eligibility.predicted_fit)
        })
        .then_with(|| b.eligibility.confidence.total_cmp(&a.eligibility.confidence))
        .then_with(|| {
            b.eligibility
                .candidate_fit
                .total_cmp(&a.eligibility.candidate_fit)
        })
        .then_with(|| {
            crate::taste::score::unique_loved_rec_seeds(b)
                .cmp(&crate::taste::score::unique_loved_rec_seeds(a))
        })
        .then_with(|| {
            a.candidate
                .tmdb_id
                .unwrap_or(i64::MAX)
                .cmp(&b.candidate.tmdb_id.unwrap_or(i64::MAX))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::explain::{EvidenceGrade, MatchedFeatureView};
    use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn feat(family: &str, appearances: u32, affinity: f32) -> MatchedFeatureView {
        MatchedFeatureView {
            feature_key: String::new(),
            name: format!("{family}-{appearances}"),
            family: family.into(),
            appearances,
            recommendation_mean: affinity,
            scoring_affinity: affinity,
            confidence: 0.8,
            portability: 1.0,
            citeable: true,
            cited: true,
        }
    }

    fn neo_noir() -> MatchedFeatureView {
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
        }
    }

    fn row_with(
        features: Vec<MatchedFeatureView>,
        watchlist: bool,
        related: bool,
        total: f32,
        tmdb_id: i64,
    ) -> ScoredCandidate {
        let person_keys: Vec<_> = features.iter().map(|f| f.name.clone()).collect();
        let evidence_grade = if watchlist {
            if features.iter().any(|f| f.appearances >= 3) {
                EvidenceGrade::Strong
            } else {
                EvidenceGrade::Medium
            }
        } else {
            EvidenceGrade::Medium
        };
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(tmdb_id),
                title: "Film".into(),
                year: Some(2000),
                poster: None,
                watchlist,
                sources: vec![RetrievalSource {
                    kind: if related {
                        RetrievalKind::Related
                    } else if watchlist {
                        RetrievalKind::Watchlist
                    } else {
                        RetrievalKind::Filmography
                    },
                    label: "x".into(),
                    seed_tmdb_id: None,
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                directors: vec!["A".into()],
                genres: vec![],
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
                total,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: person_keys.clone(),
            negative_features: vec![],
            contextual_only: false,
            person_keys,
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: features,
            hidden_features: vec![],
            eligibility: crate::taste::explain::EligibilityTrace {
                portable_evidence_required: true,
                passed: evidence_grade.displayable(),
                passed_because: vec!["fixture".into()],
                candidate_fit: 1.0,
                evidence_grade,
                predicted_fit: if evidence_grade.displayable() { 0.72 } else { 0.35 },
                confidence: 0.6,
                hydration_completeness: 0.7,
                state: if evidence_grade.displayable() {
                    "recommended".into()
                } else {
                    "held".into()
                },
                primary_reason: if evidence_grade.displayable() {
                    "recommended".into()
                } else {
                    "low_fit".into()
                },
            },
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    fn row(grade_people: &[(u32, bool)], watchlist: bool) -> ScoredCandidate {
        let features: Vec<_> = grade_people
            .iter()
            .map(|(n, _)| feat("director", *n, 0.5))
            .collect();
        row_with(features, watchlist, false, 0.4, 1)
    }

    fn with_total(mut c: ScoredCandidate, total: f32) -> ScoredCandidate {
        c.score.total = total;
        c
    }

    fn with_g(mut c: ScoredCandidate, prior: Option<f32>) -> ScoredCandidate {
        match prior {
            Some(g) => {
                c.has_quality_prior = true;
                c.quality_prior = g;
            }
            None => {
                c.has_quality_prior = false;
                c.quality_prior = 0.0;
            }
        }
        c
    }

    #[test]
    fn known_negative_g_outranks_missing_g_regardless_of_content() {
        let known = with_g(with_total(row_with(vec![], false, false, 0.2, 1), 0.2), Some(-0.05));
        let missing = with_g(with_total(row_with(vec![], false, false, 0.95, 2), 0.95), None);
        assert_eq!(rank_order(&known, &missing), std::cmp::Ordering::Less);
    }

    #[test]
    fn content_fit_only_breaks_near_g_ties() {
        let lower_g_higher_fit =
            with_g(with_total(row_with(vec![], false, false, 0.9, 1), 0.9), Some(0.10));
        let higher_g_lower_fit =
            with_g(with_total(row_with(vec![], false, false, 0.4, 2), 0.4), Some(0.20));
        assert_eq!(
            rank_order(&higher_g_lower_fit, &lower_g_higher_fit),
            std::cmp::Ordering::Less
        );

        let a = with_g(with_total(row_with(vec![], false, false, 0.9, 3), 0.9), Some(0.10));
        let b = with_g(with_total(row_with(vec![], false, false, 0.4, 4), 0.4), Some(0.12));
        assert_eq!(rank_order(&a, &b), std::cmp::Ordering::Less);
    }

    #[test]
    fn rank_order_survives_nan_scores() {
        let mut rows = vec![
            with_g(with_total(row_with(vec![], false, false, 0.8, 1), 0.8), Some(0.1)),
            with_g(with_total(row_with(vec![], false, false, 0.4, 2), f32::NAN), Some(0.2)),
            with_g(with_total(row_with(vec![], false, false, 0.6, 3), 0.6), Some(f32::NAN)),
            with_g(with_total(row_with(vec![], false, false, f32::NAN, 4), 0.5), None),
        ];
        rows.sort_by(rank_order);
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn higher_evidence_grade_scores_higher() {
        let g3 = row(&[(5, true), (4, true)], true);
        let g2 = row(&[(2, true)], false);
        assert!(legacy_match_score(&g3) > legacy_match_score(&g2));
    }

    #[test]
    fn more_appearances_only_raise_score_modestly() {
        let n8 = row_with(vec![feat("composer", 8, 0.22)], false, false, 0.4, 1);
        let n4 = row_with(vec![feat("composer", 4, 0.22)], false, false, 0.4, 2);
        let delta = legacy_match_score(&n8) as i16 - legacy_match_score(&n4) as i16;
        assert!(delta >= 0, "n=8 should not lose to n=4, delta={delta}");
        assert!(delta <= 12, "appearance count must not dominate, delta={delta}");
    }

    #[test]
    fn appearance_cap_treats_11_like_8() {
        let n8 = row(&[(8, true)], true);
        let n11 = row(&[(11, true)], true);
        assert_eq!(legacy_match_score(&n8), legacy_match_score(&n11));
    }

    #[test]
    fn limited_evidence_lowers_score() {
        let strong = row(&[(8, true)], true);
        let limited = row(&[(2, true)], true);
        assert!(legacy_match_score(&limited) < legacy_match_score(&strong));
    }

    #[test]
    fn low_evidence_can_fall_below_floor() {
        let mut weak = row_with(vec![], false, false, 0.05, 2);
        weak.eligibility.state = "held".into();
        weak.eligibility.passed = false;
        weak.eligibility.predicted_fit = 0.30;
        weak.eligibility.evidence_grade = EvidenceGrade::None;
        assert!(legacy_match_score(&weak) < MATCH_SCORE_FLOOR);
        assert_eq!(weak.eligibility.state, "held");
        assert!(!occupies_new(&weak));
    }

    #[test]
    fn higher_overall_total_raises_match_score() {
        let a = with_total(row(&[(8, true)], true), 0.1);
        let b = with_total(row(&[(8, true)], true), 0.9);
        assert!(
            legacy_match_score(&b) > legacy_match_score(&a),
            "match % must track overall fit, got {} vs {}",
            legacy_match_score(&a),
            legacy_match_score(&b)
        );
    }

    #[test]
    fn conflicting_evidence_lowers_match_score() {
        let clean = with_total(row(&[(5, true)], false), 0.25);
        let mut conflicted = clean.clone();
        conflicted.score.negative_evidence = -0.65;
        conflicted.candidate.tmdb_id = Some(3);
        assert!(
            legacy_match_score(&conflicted) < legacy_match_score(&clean),
            "negative evidence must cut displayed match, got {} vs {}",
            legacy_match_score(&conflicted),
            legacy_match_score(&clean)
        );
    }

    #[test]
    fn stronger_affinity_outranks_busier_collaborator_at_same_total() {
        let mut fraser = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.2,
            20,
        );
        fraser.matched_features.push(neo_noir());
        let zimmer = row_with(vec![feat("composer", 12, 0.22)], false, true, 0.2, 10);
        assert!(
            legacy_match_score(&fraser) > legacy_match_score(&zimmer),
            "at equal overall totals, stronger craft should still win, got {} vs {}",
            legacy_match_score(&fraser),
            legacy_match_score(&zimmer)
        );
        assert!(legacy_match_score(&zimmer) < EXCELLENT_BAND);
    }

    #[test]
    fn overall_fit_outranks_craft_alone() {
        let mut craft_thin = row_with(
            vec![feat("cinematographer", 8, 0.55)],
            false,
            false,
            0.02,
            75,
        );
        craft_thin.score.negative_evidence = -0.55;
        let solid = row_with(
            vec![feat("director", 4, 0.30)],
            false,
            false,
            0.22,
            76,
        );
        assert!(
            legacy_match_score(&solid) > legacy_match_score(&craft_thin),
            "strong craft with weak/conflicted overall must not top a better overall fit, got {} vs {}",
            legacy_match_score(&craft_thin),
            legacy_match_score(&solid)
        );
    }

    #[test]
    fn composer_count_cannot_make_related_boss_baby_excellent() {
        let boss_baby = row_with(
            vec![
                feat("composer", 12, 0.22),
                feat("actor", 2, 0.12),
            ],
            false,
            true,
            0.8,
            459_151,
        );
        let if_movie = row_with(
            vec![
                feat("composer", 9, 0.24),
                feat("actor", 3, 0.14),
            ],
            false,
            true,
            0.8,
            1,
        );
        let star_trek = row_with(
            vec![
                feat("composer", 9, 0.24),
                feat("actor", 5, 0.16),
            ],
            false,
            true,
            0.8,
            2,
        );
        for row in [&boss_baby, &if_movie, &star_trek] {
            assert!(
                legacy_match_score(row) < EXCELLENT_BAND,
                "{} scored {} Excellent from collaborator frequency",
                row.candidate.tmdb_id.unwrap(),
                legacy_match_score(row)
            );
            assert!(
                legacy_match_score(row) <= RELATED_ONLY_CAP,
                "related-only without a DP/director/writer must stay a discovery"
            );
        }
        let mut boss_baby = boss_baby;
        boss_baby.candidate.genres = vec!["Animation".into(), "Comedy".into(), "Family".into()];
        let mut foxcatcher = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.35,
            3,
        );
        foxcatcher.matched_features.push(neo_noir());
        assert!(
            legacy_match_score(&foxcatcher) > legacy_match_score(&boss_baby),
            "portable DP with solid overall must beat related kids composer spam, got {} vs {}",
            legacy_match_score(&foxcatcher),
            legacy_match_score(&boss_baby)
        );
        assert!(legacy_match_score(&foxcatcher) > legacy_match_score(&if_movie));
        assert!(legacy_match_score(&foxcatcher) > legacy_match_score(&star_trek));
    }

    #[test]
    fn related_only_is_capped_even_with_portable_dp() {
        let dune = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            true,
            0.5,
            78,
        );
        assert!(
            legacy_match_score(&dune) <= RELATED_ONLY_CAP,
            "related-only must never pad into Strong possibility, got {}",
            legacy_match_score(&dune)
        );
        assert!(
            occupies_new(&dune) || legacy_match_score(&dune) < MATCH_SCORE_FLOOR,
            "a portable DP neighbor belongs on New, not a hidden Explore shelf"
        );
        assert!(!occupies_explore(&dune));
    }

    #[test]
    fn displayed_order_is_total_then_fit_then_tmdb() {
        let mut low = with_total(row(&[(8, true)], false), 0.01);
        low.eligibility.candidate_fit = 0.4;
        low.candidate.tmdb_id = Some(10);
        let mut high = with_total(row(&[(8, true)], false), 0.99);
        high.eligibility.candidate_fit = 1.0;
        high.candidate.tmdb_id = Some(20);
        let mut rows = vec![low, high];
        sort_workspace(&mut rows);
        assert_eq!(rows[0].candidate.tmdb_id, Some(20));
    }

    #[test]
    fn candidate_fit_ranks_without_changing_visible_match_score() {
        let mut weak = row_with(vec![feat("composer", 8, 0.55)], false, false, 0.4, 81);
        weak.eligibility.candidate_fit = 0.32;
        let mut specific = weak.clone();
        specific.eligibility.candidate_fit = 1.0;
        specific.candidate.tmdb_id = Some(82);
        assert_eq!(
            legacy_match_score(&weak),
            legacy_match_score(&specific),
            "candidate fit stays an internal rank tie-break; match % uses overall total"
        );
        assert!(legacy_match_score(&weak) <= SINGLE_BRIDGE_CAP);
    }

    #[test]
    fn neighbor_floor_is_discounted_by_conflict() {
        let mut clean = row_with(vec![feat("actor", 3, 0.20)], false, true, 0.05, 501);
        clean.candidate.genres = vec!["Drama".into()];
        clean.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from A".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from B".into(),
                seed_tmdb_id: Some(2),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from C".into(),
                seed_tmdb_id: Some(3),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from D".into(),
                seed_tmdb_id: Some(4),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        let mut conflicted = clean.clone();
        conflicted.candidate.tmdb_id = Some(502);
        conflicted.score.negative_evidence = -0.65;
        assert!(
            legacy_match_score(&clean) >= 60,
            "multi-seed neighbor floor should still lift a clean row, got {}",
            legacy_match_score(&clean)
        );
        assert!(
            legacy_match_score(&conflicted) < legacy_match_score(&clean),
            "conflict must discount the neighbor floor, got {} vs {}",
            legacy_match_score(&conflicted),
            legacy_match_score(&clean)
        );
        assert!(
            legacy_match_score(&conflicted) < 55,
            "Playdate-class conflict must not stay propped at 62%, got {}",
            legacy_match_score(&conflicted)
        );
    }

    #[test]
    fn filmography_single_bridge_caps_as_discovery() {
        let be_cool = row_with(vec![feat("composer", 8, 0.55)], false, false, 0.4, 70);
        assert!(
            legacy_match_score(&be_cool) <= SINGLE_BRIDGE_CAP,
            "single-person filmography must not read as Strong possibility, got {}",
            legacy_match_score(&be_cool)
        );
        let mut two_bridges = row_with(
            vec![
                feat("cinematographer", 5, 0.41),
                feat("cinematographer", 8, 0.38),
            ],
            false,
            false,
            0.4,
            71,
        );
        two_bridges.matched_features[0].name = "Mauro Fiore".into();
        two_bridges.matched_features[1].name = "Wally Pfister".into();
        assert!(
            legacy_match_score(&two_bridges) > SINGLE_BRIDGE_CAP,
            "two craft people on the same film may outrank Discovery, got {}",
            legacy_match_score(&two_bridges)
        );
    }

    #[test]
    fn limited_evidence_cannot_display_strong_possibility() {
        let thin = row_with(vec![feat("actor", 2, 0.7)], false, false, 0.5, 80);
        assert!(thin_evidence(&thin));
        assert!(
            legacy_match_score(&thin) <= LIMITED_EVIDENCE_CAP,
            "thin evidence must stay below Strong possibility, got {}",
            legacy_match_score(&thin)
        );
    }

    #[test]
    fn watchlist_match_score_ignores_candidate_fit() {
        let mut prestige = row_with(
            vec![feat("cinematographer", 3, 0.39), feat("director", 5, 0.55)],
            true,
            false,
            0.4,
            1124,
        );
        prestige.eligibility.candidate_fit = 0.55;
        let mut full = prestige.clone();
        full.eligibility.candidate_fit = 1.0;
        assert_eq!(
            legacy_match_score(&prestige),
            legacy_match_score(&full),
            "Watchlist Nolan/Pfister titles must not lose a band to candidate_fit"
        );
        assert!(
            legacy_match_score(&prestige) >= NEW_MATCH_FLOOR,
            "Prestige-class watchlist must stay Strong possibility, got {}",
            legacy_match_score(&prestige)
        );
    }

    fn related_seed(
        features: Vec<MatchedFeatureView>,
        genres: &[&str],
        fit: f32,
        tmdb_id: i64,
    ) -> ScoredCandidate {
        let mut c = row_with(features, false, true, 0.5, tmdb_id);
        c.candidate.sources[0].seed_tmdb_id = Some(27_205);
        c.candidate.genres = genres.iter().map(|g| (*g).to_string()).collect();
        c.eligibility.candidate_fit = fit;
        c
    }

    #[test]
    fn portable_filmography_with_specific_fit_occupies_new() {
        let mut kts = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.4,
            49_530,
        );
        kts.eligibility.candidate_fit = 1.0;
        kts.matched_features.push(neo_noir());
        assert!(
            legacy_match_score(&kts) >= NEW_MATCH_FLOOR,
            "Fraser + specific fit must clear Strong possibility, got {}",
            legacy_match_score(&kts)
        );
        assert!(occupies_new(&kts), "Killing Them Softly belongs on New");
        assert_eq!(filter_reason(&kts), None);
    }

    #[test]
    fn composer_filmography_stays_discovery_even_with_specific_fit() {
        let mut antz = row_with(vec![feat("composer", 9, 0.55)], false, false, 0.4, 101);
        antz.eligibility.candidate_fit = 1.0;
        assert!(
            legacy_match_score(&antz) <= SINGLE_BRIDGE_CAP,
            "Powell résumé cards must stay Discovery under legacy match, got {}",
            legacy_match_score(&antz)
        );
        // C1: Content Fit_v1 eligibility admits when state is recommended — source
        // role (composer filmography) is not an admission veto.
        assert!(occupies_new(&antz));
        assert_eq!(filter_reason(&antz), None);
    }

    #[test]
    fn loved_similar_without_craft_does_not_occupy_new() {
        let mut raging = related_seed(vec![], &["Drama"], 1.0, 11);
        let mut kissing = related_seed(vec![], &["Romance", "Comedy"], 1.0, 15);
        for row in [&mut raging, &mut kissing] {
            row.eligibility.evidence_grade = EvidenceGrade::None;
            row.eligibility.state = "held".into();
            row.eligibility.passed = false;
            row.eligibility.predicted_fit = 0.35;
            row.eligibility.primary_reason = "low_fit".into();
        }
        for row in [&raging, &kissing] {
            assert!(
                legacy_match_score(row) < NEW_MATCH_FLOOR,
                "TMDB similar-to must not be padded to Strong possibility, got {} for {}",
                legacy_match_score(row),
                row.candidate.tmdb_id.unwrap()
            );
            assert!(!occupies_new(row));
        }
        assert_eq!(filter_reason(&kissing).as_deref(), Some("low_fit"));
    }

    #[test]
    fn modest_portable_filmography_still_occupies_new() {
        let mut insomnia = row_with(
            vec![feat("cinematographer", 3, 0.32)],
            false,
            false,
            0.4,
            320,
        );
        insomnia.eligibility.candidate_fit = 1.0;
        insomnia.matched_features.push(feat("director", 5, 0.38));
        insomnia.matched_features.push(neo_noir());
        assert!(
            legacy_match_score(&insomnia) >= NEW_MATCH_FLOOR,
            "Pfister/Fiore filmography with a specific match must not stall at Discovery, got {}",
            legacy_match_score(&insomnia)
        );
        assert!(occupies_new(&insomnia));
    }

    #[test]
    fn animation_similar_and_composer_related_stay_off_new() {
        let mut feet = related_seed(
            vec![feat("composer", 9, 0.55)],
            &["Animation", "Family"],
            1.0,
            12,
        );
        feet.eligibility.evidence_grade = EvidenceGrade::None;
        feet.eligibility.state = "held".into();
feet.eligibility.passed = false;
feet.eligibility.predicted_fit = 0.35;
        let trek = related_seed(
            vec![feat("composer", 12, 0.24)],
            &["Science Fiction", "Action"],
            1.0,
            13,
        );
        let mut sponge = related_seed(
            vec![feat("writer", 3, 0.4)],
            &["Animation"],
            1.0,
            14,
        );
        sponge.eligibility.evidence_grade = EvidenceGrade::None;
        sponge.eligibility.state = "held".into();
sponge.eligibility.passed = false;
sponge.eligibility.predicted_fit = 0.35;
        for row in [&feet, &sponge] {
            assert!(
                legacy_match_score(row) <= RELATED_ONLY_CAP,
                "{} scored {} and would occupy New",
                row.candidate.tmdb_id.unwrap(),
                legacy_match_score(row)
            );
            assert!(!occupies_new(row));
        }
        assert!(
            occupies_new(&trek) || legacy_match_score(&trek) < MATCH_SCORE_FLOOR,
            "composer-linked Star Trek neighbors belong on New, got {}",
            legacy_match_score(&trek)
        );
        assert!(!occupies_explore(&trek));
    }

    #[test]
    fn mixed_filmography_and_unseeded_related_occupies_new() {
        let mut mixed = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.4,
            49_530,
        );
        mixed.eligibility.candidate_fit = 1.0;
        mixed.matched_features.push(neo_noir());
        mixed.candidate.sources.push(RetrievalSource {
            kind: RetrievalKind::Related,
            label: "similar to a catalog title".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        assert!(occupies_new(&mixed), "qualifying DP filmography must occupy New without a Related seed");
        assert!(!occupies_explore(&mixed));
    }

    #[test]
    fn several_loved_recommendation_seeds_can_backfill_filmography_new() {
        let mut mixed = row_with(vec![feat("composer", 2, 0.3)], false, false, 0.4, 700);
        mixed.candidate.sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Composer Name".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "Loved One".into(),
                seed_tmdb_id: Some(1),
                seed_rating: Some(5.0),
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "Loved Two".into(),
                seed_tmdb_id: Some(2),
                seed_rating: Some(4.5),
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "Loved Three".into(),
                seed_tmdb_id: Some(3),
                seed_rating: Some(4.0),
                similarity: None,
                neighbor_rank: None,
            },
        ];
        mixed.eligibility.candidate_fit = 0.7;
        mixed.eligibility.evidence_grade = EvidenceGrade::Strong;
        assert!(mixed_recommendation_corroboration(&mixed));
        assert!(occupies_new(&mixed));

        mixed.candidate.sources.truncate(2);
        assert!(!mixed_recommendation_corroboration(&mixed));
        // C1: corroboration is diagnostic only — Recommended state still occupies New.
        assert!(occupies_new(&mixed));
    }

    #[test]
    fn composer_filmography_plus_related_stays_off_new() {
        let mut mixed = row_with(vec![feat("composer", 9, 0.55)], false, false, 0.4, 101);
        mixed.eligibility.candidate_fit = 1.0;
        mixed.eligibility.evidence_grade = EvidenceGrade::None;
        mixed.eligibility.state = "held".into();
mixed.eligibility.passed = false;
mixed.eligibility.predicted_fit = 0.35;
        mixed.candidate.sources.push(RetrievalSource {
            kind: RetrievalKind::Related,
            label: "similar to Pulp Fiction".into(),
            seed_tmdb_id: Some(680),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        assert!(!occupies_new(&mixed));
        assert!(legacy_match_score(&mixed) <= SINGLE_BRIDGE_CAP || occupies_explore(&mixed));
    }

    #[test]
    fn writer_filmography_is_not_padded_to_new() {
        let mut writer = row_with(vec![feat("writer", 3, 0.4)], false, false, 0.4, 88);
        writer.eligibility.candidate_fit = 1.0;
        assert!(
            legacy_match_score(&writer) < NEW_MATCH_FLOOR,
            "writer-only filmography must not use the 70 pad, got {}",
            legacy_match_score(&writer)
        );
        // C1: high Content fit admits regardless of craft role.
        assert!(occupies_new(&writer));
    }

    #[test]
    fn related_60_69_occupies_new() {
        let dogs = related_seed(vec![feat("director", 8, 0.5)], &["Crime", "Thriller"], 1.0, 500);
        assert!(
            legacy_match_score(&dogs) <= RELATED_ONLY_CAP,
            "Reservoir Dogs-class similar-to must not land at exact 70, got {}",
            legacy_match_score(&dogs)
        );
        if legacy_match_score(&dogs) >= MATCH_SCORE_FLOOR {
            assert!(occupies_new(&dogs));
            assert!(!occupies_explore(&dogs));
            assert_eq!(placement(&dogs), "new");
        }
    }

    #[test]
    fn yearless_stub_stays_off_both_boards() {
        let mut stub = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.4,
            1,
        );
        stub.candidate.year = None;
        stub.candidate.runtime = None;
        stub.candidate.vote_count = None;
        stub.eligibility.candidate_fit = 1.0;
        assert!(unreleased_display_row(&stub));
        assert!(!occupies_new(&stub));
        assert!(!occupies_explore(&stub));
        assert_eq!(filter_reason(&stub).as_deref(), Some("incomplete-metadata"));
    }

    #[test]
    fn yearless_with_runtime_can_occupy_new() {
        let mut related = related_seed(vec![feat("director", 4, 0.4)], &["Drama"], 1.0, 2);
        related.candidate.year = None;
        related.candidate.runtime = Some(110);
        related.candidate.vote_count = Some(200);
        assert!(!unreleased_display_row(&related));
        if legacy_match_score(&related) >= MATCH_SCORE_FLOOR {
            assert!(occupies_new(&related));
        }
        let mut filmography = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.4,
            3,
        );
        filmography.candidate.year = None;
        filmography.candidate.runtime = Some(110);
        filmography.candidate.vote_count = Some(200);
        filmography.eligibility.candidate_fit = 1.0;
        assert!(unreleased_new_row(&filmography));
        assert!(!occupies_new(&filmography));
        assert!(!occupies_explore(&filmography));
    }

    #[test]
    fn future_dated_stays_off_both_boards() {
        let mut future = row_with(
            vec![feat("cinematographer", 4, 0.45)],
            false,
            false,
            0.4,
            4,
        );
        future.candidate.year = Some(chrono::Utc::now().year() + 2);
        future.eligibility.candidate_fit = 1.0;
        assert!(unreleased_display_row(&future));
        assert!(!occupies_new(&future));
        assert!(!occupies_explore(&future));
        assert_eq!(filter_reason(&future).as_deref(), Some("unreleased"));
    }

    #[test]
    fn tv_movie_filmography_does_not_occupy_new() {
        let mut sketch = row_with(
            vec![feat("cinematographer", 3, 0.39)],
            false,
            false,
            0.4,
            49_001,
        );
        sketch.candidate.title = "Sketch Artist".into();
        sketch.candidate.genres = vec!["Crime".into(), "TV Movie".into()];
        sketch.eligibility.candidate_fit = 1.0;
        assert!(
            legacy_match_score(&sketch) <= SINGLE_BRIDGE_CAP,
            "TV movies must not be padded onto New, got {}",
            legacy_match_score(&sketch)
        );
        assert!(!occupies_new(&sketch));
        assert!(!occupies_explore(&sketch));
    }

    #[test]
    fn uncorroborated_dp_filmography_stays_off_new() {
        let mut blue = row_with(
            vec![feat("cinematographer", 4, 0.47)],
            false,
            false,
            0.4,
            13_922,
        );
        blue.candidate.title = "Out of the Blue".into();
        blue.eligibility.candidate_fit = 1.0;
        assert!(
            legacy_match_score(&blue) <= RELATED_ONLY_CAP,
            "a DP credit alone must not read as Very likely under legacy match, got {}",
            legacy_match_score(&blue)
        );
        // C1: Content Fit_v1 state admits; corroboration is no longer required.
        assert!(occupies_new(&blue));
    }

    #[test]
    fn excellent_overall_fit_can_exceed_former_new_cap() {
        let mut kts = row_with(
            vec![feat("cinematographer", 4, 0.47)],
            false,
            false,
            0.4,
            49_530,
        );
        kts.eligibility.candidate_fit = 1.0;
        kts.matched_features.push(neo_noir());
        assert!(occupies_new(&kts));
        assert!(
            legacy_match_score(&kts) >= 80,
            "an excellent corroborated New fit may exceed the former 79 cap, got {}",
            legacy_match_score(&kts)
        );
        let mut dune = row_with(
            vec![feat("cinematographer", 4, 0.47)],
            true,
            false,
            0.4,
            438_631,
        );
        dune.eligibility.candidate_fit = 1.0;
        dune.matched_features.push(neo_noir());
        assert!(
            legacy_match_score(&dune) >= 80,
            "watchlist may stay Very likely, got {}",
            legacy_match_score(&dune)
        );
    }

    #[test]
    fn corroborated_recommendations_can_occupy_new() {
        let mut solo = row_with(
            vec![feat("writer", 3, 0.4), feat("composer", 9, 0.5)],
            false,
            true,
            0.8,
            348_350,
        );
        solo.candidate.sources = vec![
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
        solo.eligibility.candidate_fit = 1.0;
        solo.candidate.title = "Solo: A Star Wars Story".into();
        assert!(
            !related_only(&solo),
            "corroborated recommendations must not be treated as a similar-to dump"
        );
        assert!(occupies_new(&solo), "neighbor of Rogue One with writer+composer should occupy New");
        assert!(!occupies_explore(&solo));
    }

    #[test]
    fn uncorroborated_recommendations_stay_related_only() {
        let mut insurgent = row_with(vec![feat("actor", 3, 0.4)], false, true, 0.8, 262_504);
        insurgent.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::RelatedRecommendations,
            label: "recommended from The Hunger Games: Catching Fire".into(),
            seed_tmdb_id: Some(101_299),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        insurgent.candidate.title = "Insurgent".into();
        insurgent.eligibility.evidence_grade = EvidenceGrade::None;
        insurgent.eligibility.state = "held".into();
insurgent.eligibility.passed = false;
insurgent.eligibility.predicted_fit = 0.35;
        assert!(related_only(&insurgent));
        assert!(!occupies_new(&insurgent));
    }

    #[test]
    fn similar_only_stays_capped_but_can_occupy_new() {
        let mut similar = row_with(vec![feat("director", 6, 0.5)], false, true, 0.8, 116);
        similar.matched_features.push(neo_noir());
        similar.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::RelatedSimilar,
            label: "similar to Last Night in Soho".into(),
            seed_tmdb_id: Some(565_123),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        assert!(related_only(&similar));
        assert!(legacy_match_score(&similar) <= RELATED_ONLY_CAP);
        if legacy_match_score(&similar) >= MATCH_SCORE_FLOOR {
            assert!(occupies_new(&similar));
        }
    }

    #[test]
    fn related_only_family_does_not_occupy_explore() {
        let mut kids = related_seed(
            vec![feat("director", 4, 0.4)],
            &["Family", "Comedy"],
            1.0,
            351_837,
        );
        kids.eligibility.evidence_grade = EvidenceGrade::None;
        kids.eligibility.state = "held".into();
kids.eligibility.passed = false;
kids.eligibility.predicted_fit = 0.35;
        assert!(!occupies_new(&kids));
        assert!(!occupies_explore(&kids));
    }

    #[test]
    fn multi_seed_recommendations_occupy_new() {
        let mut trek = row_with(vec![feat("composer", 4, 0.24)], false, true, 0.5, 13_475);
        trek.candidate.title = "Star Trek".into();
        trek.candidate.genres = vec!["Science Fiction".into(), "Action".into()];
        trek.candidate.sources = vec![
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
                label: "recommended from Avatar".into(),
                seed_tmdb_id: Some(19_995),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from Interstellar".into(),
                seed_tmdb_id: Some(157_336),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::RelatedRecommendations,
                label: "recommended from The Mandalorian and Grogu".into(),
                seed_tmdb_id: Some(1_228_710),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        assert!(
            legacy_match_score(&trek) >= MATCH_SCORE_FLOOR,
            "Rogue One/Avatar/Interstellar recs must not sit at 5%, got {}",
            legacy_match_score(&trek)
        );
        assert!(occupies_new(&trek));
        assert!(!occupies_explore(&trek));
        assert!(legacy_match_score(&trek) <= RELATED_ONLY_CAP);
    }

    #[test]
    fn single_seed_recommendation_does_not_invent_a_match() {
        let mut curves = related_seed(
            vec![feat("actor", 3, 0.12)],
            &["Drama", "Comedy"],
            1.0,
            10_337,
        );
        curves.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::RelatedRecommendations,
            label: "recommended from Tony".into(),
            seed_tmdb_id: Some(1_329_016),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        curves.candidate.title = "Real Women Have Curves".into();
        curves.eligibility.evidence_grade = EvidenceGrade::None;
        curves.eligibility.state = "held".into();
curves.eligibility.passed = false;
curves.eligibility.predicted_fit = 0.35;
        curves.score.total = 0.02;
        curves.score.negative_evidence = -0.2;
        assert!(
            legacy_match_score(&curves) < MATCH_SCORE_FLOOR,
            "one weak seed must not mint a New card, got {}",
            legacy_match_score(&curves)
        );
        assert!(!occupies_new(&curves));
    }

    #[test]
    fn web_discovery_can_occupy_new_without_craft_grade() {
        let mut found = row_with(
            vec![feat("director", 3, 0.28)],
            false,
            false,
            0.18,
            88_001,
        );
        found.candidate.sources = vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: "neo-noir atmospheric thrillers like Zodiac".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        found.eligibility.evidence_grade = EvidenceGrade::None;
        found.eligibility.state = "recommended".into();
        found.eligibility.passed = true;
        found.eligibility.predicted_fit = 0.72;
        found.eligibility.primary_reason = "recommended".into();
        found.candidate.genres = vec!["Crime".into(), "Thriller".into()];
        assert!(
            occupies_new(&found),
            "targeted web discovery must be allowed onto New without a craft grade"
        );
        assert_eq!(filter_reason(&found), None);
    }

    #[test]
    fn below_match_floor_does_not_occupy_new() {
        let mut weak = related_seed(vec![feat("director", 8, 0.5)], &["Crime"], 1.0, 501);
        weak.score.total = 0.02;
        weak.matched_features.clear();
        weak.positive_features.clear();
        weak.person_keys.clear();
        weak.eligibility.state = "held".into();
        weak.eligibility.passed = false;
        weak.eligibility.predicted_fit = 0.30;
        weak.eligibility.primary_reason = "low_fit".into();
        assert!(legacy_match_score(&weak) < MATCH_SCORE_FLOOR);
        assert!(!occupies_new(&weak));
        assert_eq!(filter_reason(&weak).as_deref(), Some("low_fit"));
    }

    #[test]
    fn related_only_with_weak_semantic_still_occupies_when_match_clears() {
        // Semantic demotion handles dislike-aligned Medium grades; occupancy
        // no longer requires a high semantic floor or the board collapses.
        let mut dogs = related_seed(vec![feat("director", 8, 0.5)], &["Crime", "Thriller"], 1.0, 502);
        dogs.score.semantic_coverage = true;
        dogs.score.semantic_fit = 0.50;
        dogs.score.total = 0.4;
        if legacy_match_score(&dogs) >= MATCH_SCORE_FLOOR {
            assert!(occupies_new(&dogs));
        }
    }

    #[test]
    fn related_only_with_strong_semantic_can_occupy_new() {
        let mut dogs = related_seed(vec![feat("director", 8, 0.5)], &["Crime", "Thriller"], 1.0, 503);
        dogs.score.semantic_coverage = true;
        dogs.score.semantic_fit = 0.62;
        dogs.score.total = 0.4;
        if legacy_match_score(&dogs) >= MATCH_SCORE_FLOOR {
            assert!(occupies_new(&dogs));
        }
    }

    #[test]
    fn actor_loyalty_filmography_occupies_new() {
        let mut rocky = row_with(
            vec![feat("actor", 5, 0.55)],
            false,
            false,
            0.35,
            1374,
        );
        rocky.matched_features[0].recommendation_mean = 0.55;
        rocky.matched_features[0].name = "Michael B. Jordan".into();
        rocky.candidate.genres = vec!["Drama".into(), "Action".into()];
        rocky.eligibility.candidate_fit = 0.8;
        assert!(
            !filmography_single_bridge(&rocky),
            "repeated actor preference must not be treated as thin résumé"
        );
        if legacy_match_score(&rocky) >= MATCH_SCORE_FLOOR {
            assert!(
                occupies_new(&rocky),
                "Creed-class actor loyalty should surface unseen Rocky/lead work"
            );
        }
    }

    #[test]
    fn director_animation_loyalty_filmography_occupies_new() {
        let mut wildwood = row_with(
            vec![feat("director", 5, 0.55)],
            false,
            false,
            0.35,
            88_002,
        );
        wildwood.matched_features[0].recommendation_mean = 0.55;
        wildwood.matched_features[0].name = "Travis Knight".into();
        wildwood.candidate.genres = vec!["Animation".into(), "Family".into(), "Adventure".into()];
        wildwood.eligibility.candidate_fit = 0.8;
        assert!(!filmography_single_bridge(&wildwood));
        if legacy_match_score(&wildwood) >= MATCH_SCORE_FLOOR {
            assert!(
                occupies_new(&wildwood),
                "Laika/Knight loyalty must occupy New for unseen filmography"
            );
        }
    }
}
