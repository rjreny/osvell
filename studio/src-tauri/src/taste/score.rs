use crate::taste::features::{
    decade_label, family_for_job, keyword_is_taste_signal, keyword_strength,
    runtime_bucket, EvidenceFilm, FeatureAffinity, FeatureFamily, FeatureKey, FeatureProfile,
    KeywordStrength, PORTABLE_CONTEXTUAL,
};
use crate::taste::dimensions::predicted_modes;
use crate::taste::explain::{
    eligibility_trace, select_display_reasons, EligibilityTrace, EvidenceGrade, MatchedFeatureView,
};
use crate::taste::family_fit::{
    family_fit_as_legacy_total, score_family_fit_with_config, CraftConfig,
};
use crate::taste::retrieve::{Candidate, RetrievalKind};
use crate::taste::semantic::SemanticScore;
use chrono::Datelike;
use serde::{Deserialize, Serialize};

/// Recommendation policy, not TMDB media classification. Known runtimes under
/// 40 minutes are shorts and do not belong on the New or Watchlist boards.
pub const FEATURE_RUNTIME_MIN: i32 = 40;

fn is_short_runtime(runtime: Option<i32>) -> bool {
    matches!(runtime, Some(rt) if (1..FEATURE_RUNTIME_MIN).contains(&rt))
}

pub const W_CONTENT: f32 = 0.45;
pub const W_TMDB: f32 = 0.20;
pub const W_FRIEND: f32 = 0.15;
pub const W_RECENT: f32 = 0.10;
pub const W_WATCHLIST: f32 = 0.05;
pub const W_NOVELTY: f32 = 0.0;
pub const W_NEGATIVE: f32 = 0.35;
pub const W_SEMANTIC: f32 = 0.35;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateScore {
    pub content: f32,
    pub tmdb_related: f32,
    pub friend_affinity: f32,
    pub recent_taste: f32,
    pub watchlist: f32,
    pub novelty: f32,
    pub negative_evidence: f32,
    /// 0.0..=1.0 semantic fit; 0.5 is neutral when embeddings are unavailable.
    #[serde(default = "default_semantic_fit")]
    pub semantic_fit: f32,
    #[serde(default)]
    pub semantic_coverage: bool,
    pub total: f32,
}

fn default_semantic_fit() -> f32 {
    0.5
}

impl CandidateScore {
    pub fn clamp_components(&mut self) {
        self.content = self.content.clamp(-1.0, 1.0);
        self.tmdb_related = self.tmdb_related.clamp(0.0, 1.0);
        self.friend_affinity = self.friend_affinity.clamp(-1.0, 1.0);
        self.recent_taste = self.recent_taste.clamp(-1.0, 1.0);
        self.watchlist = self.watchlist.clamp(0.0, 1.0);
        self.novelty = self.novelty.clamp(-1.0, 1.0);
        self.negative_evidence = self.negative_evidence.clamp(-1.0, 0.0);
        self.semantic_fit = self.semantic_fit.clamp(0.0, 1.0);
        self.total = (W_CONTENT * self.content
            + W_TMDB * self.tmdb_related
            + W_FRIEND * self.friend_affinity
            + W_RECENT * self.recent_taste
            + W_WATCHLIST * self.watchlist
            + W_NOVELTY * self.novelty
            + W_NEGATIVE * self.negative_evidence)
            .clamp(-1.5, 1.5);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredCandidate {
    pub candidate: CandidateView,
    pub score: CandidateScore,
    pub reasons: Vec<String>,
    pub evidence: Vec<String>,
    pub positive_features: Vec<String>,
    pub negative_features: Vec<String>,
    #[serde(default)]
    pub contextual_only: bool,
    #[serde(default)]
    pub person_keys: Vec<String>,
    #[serde(default)]
    pub display_reasons: Vec<String>,
    #[serde(default)]
    pub scoring_reasons: Vec<String>,
    #[serde(default)]
    pub matched_features: Vec<MatchedFeatureView>,
    #[serde(default)]
    pub hidden_features: Vec<MatchedFeatureView>,
    #[serde(default)]
    pub eligibility: EligibilityTrace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateView {
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    pub watchlist: bool,
    pub sources: Vec<crate::taste::retrieve::RetrievalSource>,
    pub directors: Vec<String>,
    pub genres: Vec<String>,
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub media_kind: crate::taste::retrieve::MediaKind,
    #[serde(default)]
    pub runtime: Option<i32>,
    #[serde(default)]
    pub vote_count: Option<i64>,
    /// Coarse embedding-neighborhood id for D1 board redundancy (not a Fit signal).
    #[serde(default)]
    pub semantic_cluster: Option<String>,
}

pub fn score_candidate(profile: &FeatureProfile, candidate: &Candidate) -> ScoredCandidate {
    score_candidate_with_semantic(profile, candidate, &SemanticScore::default())
}

pub fn score_candidate_with_semantic(
    profile: &FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
) -> ScoredCandidate {
    let mut content_sum = 0.0;
    let mut content_w = 0.0;
    let mut recent_sum = 0.0;
    let mut recent_w = 0.0;
    let mut neg = 0.0;
    let mut reasons = Vec::new();
    let mut evidence = Vec::new();
    let mut positive_features = Vec::new();
    let mut negative_features = Vec::new();

    let keys = candidate_keys(candidate);
    let _matched_primary = profile.affinities.iter().any(|aff| {
        aff.citeable()
            && aff.key.family.is_primary()
            && keys.iter().any(|k| k.storage_key() == aff.key.storage_key())
    });
    let mut family_used: std::collections::HashMap<FeatureFamily, usize> =
        std::collections::HashMap::new();
    let mut cited: Vec<&FeatureAffinity> = Vec::new();
    let mut matched_features: Vec<MatchedFeatureView> = Vec::new();
    let mut hidden_features: Vec<MatchedFeatureView> = Vec::new();

    for aff in &profile.affinities {
        if aff.key.family.is_contextual() {
            continue;
        }
        if !keys.iter().any(|k| k.storage_key() == aff.key.storage_key()) {
            continue;
        }
        if !aff.citeable() {
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
            continue;
        }
        // Thematic/broad keywords only count when liked uses of that keyword
        // share context with this candidate. Otherwise "parent/child" from
        // adult dramas would freely boost unrelated kids/comedy neighbors.
        if aff.key.family == FeatureFamily::Keyword
            && !keyword_relevance_transfers(aff, candidate)
        {
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
            continue;
        }
        // Broad genres (Adventure, Drama, …) describe the neighbor, they are
        // not preference. Keep their dislikes as penalties; never let them
        // drive content score or Medium eligibility.
        if aff.key.family == FeatureFamily::Genre && is_broad_genre_name(&aff.key.name) {
            if aff.negative_weight > aff.positive_weight * 0.6 && aff.negative_weight > 0.2 {
                let w = aff.key.family.weight();
                neg += (aff.negative_weight / (aff.negative_weight + aff.positive_weight + 1e-4))
                    * aff.confidence
                    * w;
                negative_features.push(aff.key.name.clone());
            }
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
            continue;
        }
        let used = family_used.entry(aff.key.family).or_insert(0);
        if *used >= aff.key.family.top_k() {
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
            continue;
        }
        *used += 1;
        let w = aff.key.family.weight();
        // Distinctive genres the user mostly dislikes should suppress, not cite.
        let genre_rejected = aff.key.family == FeatureFamily::Genre
            && aff.recommendation_mean <= 0.1
            && aff.negative_weight > aff.positive_weight
            && aff.negative_weight > 0.2;
        if genre_rejected {
            neg += (aff.negative_weight / (aff.negative_weight + aff.positive_weight + 1e-4))
                * aff.confidence
                * w;
            negative_features.push(aff.key.name.clone());
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
            continue;
        }
        content_sum += aff.scoring_affinity();
        content_w += w;
        recent_sum += aff.recent_weight * aff.confidence * w * aff.portability;
        recent_w += w;
        if aff.negative_weight > aff.positive_weight * 0.6 && aff.negative_weight > 0.2 {
            neg += (aff.negative_weight / (aff.negative_weight + aff.positive_weight + 1e-4))
                * aff.confidence
                * w;
            negative_features.push(aff.key.name.clone());
        }
        if aff.recommendation_mean > 0.1 {
            cited.push(aff);
            matched_features.push(MatchedFeatureView::from_affinity(aff, true));
        } else {
            hidden_features.push(MatchedFeatureView::from_affinity(aff, false));
        }
    }

    let specific = cited.iter().any(|a| a.key.is_person_or_keyword());
    let cited: Vec<_> = cited
        .into_iter()
        .filter(|a| !specific || a.key.is_person_or_keyword())
        .collect();
    for feat in &mut matched_features {
        feat.cited = cited.iter().any(|a| a.key.name == feat.name);
    }
    let mut person_keys = Vec::new();
    for aff in &cited {
        if matches!(
            aff.key.family,
            FeatureFamily::Director
                | FeatureFamily::Writer
                | FeatureFamily::Cinematographer
                | FeatureFamily::Composer
                | FeatureFamily::Actor
        ) {
            person_keys.push(aff.key.name.clone());
        }
        positive_features.push(aff.key.name.clone());
        if reasons.len() < 4 {
            reasons.push(format!(
                "{} affinity ({:.2})",
                aff.key.name, aff.recommendation_mean
            ));
        }
        for title in evidence_titles_for(aff, candidate) {
            if evidence.len() < 6 && !evidence.iter().any(|e| e == &title) {
                evidence.push(title);
            }
        }
    }

    let content = if content_w > 0.0 {
        (content_sum / content_w).tanh()
    } else {
        0.0
    };
    let recent_taste = if recent_w > 0.0 {
        (recent_sum / recent_w).tanh()
    } else {
        0.0
    };
    let tmdb_related = if candidate.sources.iter().any(|s| s.kind.is_related()) {
        candidate.tmdb_related.clamp(0.4, 1.0)
    } else {
        0.0
    };
    let novelty = {
        let votes = candidate.vote_count.unwrap_or(0) as f32;
        if votes <= 0.0 {
            0.1
        } else {
            (1.0 - (votes / (votes + 800.0))).clamp(0.0, 1.0) * 2.0 - 1.0
        }
    };
    let mut score = CandidateScore {
        content,
        tmdb_related,
        friend_affinity: candidate.friend_affinity.clamp(-1.0, 1.0),
        recent_taste,
        watchlist: if candidate.watchlist { 1.0 } else { 0.0 },
        novelty,
        negative_evidence: (-neg).clamp(-1.0, 0.0),
        semantic_fit: semantic.fit.clamp(0.0, 1.0),
        semantic_coverage: semantic.coverage,
        total: 0.0,
    };
    score.clamp_components();
    if semantic.coverage {
        let semantic_signal = (semantic.fit * 2.0 - 1.0).clamp(-1.0, 1.0);
        score.total = ((1.0 - W_SEMANTIC) * score.total + W_SEMANTIC * semantic_signal)
            .clamp(-1.5, 1.5);
    }
    let mut extras = Vec::new();
    if candidate.watchlist {
        reasons.push("On your watchlist".into());
        extras.push("On your watchlist".into());
    }
    if score.friend_affinity > 0.15 {
        reasons.push("High-overlap friends rated this well".into());
        extras.push("High-overlap friends rated this well".into());
    }
    evidence.dedup();
    let genre_only = reasons_are_genre_only(&reasons);
    let watchlist_ok = candidate.watchlist
        && watchlist_has_strong_bridge(profile, &cited, candidate);
    let sparse_animation_bridge =
        candidate.watchlist && sparse_animation_watchlist_bridge(profile, candidate);
    let metadata_fit_score = if sparse_animation_bridge {
        1.0
    } else if semantic.coverage {
        semantic_metadata_fit(&cited, candidate)
    } else {
        candidate_movie_fit(&cited, candidate)
    };
    let movie_fit = if semantic.coverage {
        (0.35 * metadata_fit_score + 0.65 * semantic.fit).clamp(0.0, 1.0)
    } else {
        metadata_fit_score
    };
    // Keep one grade on the eligibility trace. Every later gate consumes this
    // stored decision; none of them should infer membership from the display
    // reasons or from the number of retrieved sources.
    let evidence_grade = if candidate.watchlist && watchlist_ok {
        watchlist_evidence_grade(profile, &cited, candidate)
    } else {
        compute_evidence_grade_with_semantic(candidate, &cited, movie_fit, semantic)
    };
    let short = is_short_runtime(candidate.runtime);
    let displayable = evidence_grade.displayable();
    let contextual_only = short || !displayable;
    let display_reasons = select_display_reasons(&cited, &extras);
    let mut eligibility = eligibility_trace(&cited, genre_only, !contextual_only);
    eligibility.candidate_fit = movie_fit;
    eligibility.evidence_grade = evidence_grade;
    if short {
        eligibility.passed = false;
        eligibility
            .passed_because
            .push("short-runtime".into());
    }
    hidden_features.truncate(12);
    matched_features.truncate(12);

    let mut row = ScoredCandidate {
        candidate: CandidateView {
            tmdb_id: candidate.tmdb_id,
            title: candidate.title.clone(),
            year: candidate.year,
            poster: candidate.poster.clone(),
            watchlist: candidate.watchlist,
            sources: candidate.sources.clone(),
            directors: candidate
                .credits
                .iter()
                .filter(|c| c.job == "Director")
                .map(|c| c.name.clone())
                .collect(),
            genres: candidate.genres.clone(),
            modes: predicted_modes(&candidate.genres, &candidate.credits, &candidate.keywords),
            media_kind: candidate.media_kind,
            runtime: candidate.runtime,
            vote_count: candidate.vote_count,
            semantic_cluster: None,
        },
        score,
        scoring_reasons: reasons.clone(),
        display_reasons,
        reasons,
        evidence,
        positive_features,
        negative_features,
        contextual_only,
        person_keys,
        matched_features,
        hidden_features,
        eligibility,
    };
    // B1+C1: live ranking uses Content-only Fit_v1; eligibility/Match use C1/C2.
    // Legacy EvidenceGrade remains on the row for comparison logs only.
    apply_fit_v1_ranking(profile, candidate, semantic, &mut row);
    row
}

/// Replace ranking `total` with Content-only Fit_v1. Craft contribution is 0.
/// Also stamps C1 eligibility (Recommended / Exploratory / Held) from Content
/// fit + confidence + hydration — legacy EvidenceGrade stays for logs only.
fn apply_fit_v1_ranking(
    profile: &FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
    row: &mut ScoredCandidate,
) {
    use crate::taste::eligibility::{
        classify_eligibility, content_score_to_predicted_fit, EligibilityInput,
    };
    use crate::taste::family_fit::hydrate_candidate;

    let fit = score_family_fit_with_config(
        profile,
        candidate,
        semantic,
        &CraftConfig::fit_v1(),
    );
    row.score.content = fit.families.content.score;
    row.score.total = family_fit_as_legacy_total(&fit);
    if semantic.coverage {
        // Keep semantic_fit on a 0..1 display scale from Content positive sim.
        row.score.semantic_fit =
            ((fit.content_detail.semantic_positive + 1.0) * 0.5).clamp(0.0, 1.0);
        row.score.semantic_coverage = true;
    }

    let features = hydrate_candidate(profile, candidate, semantic.coverage);
    let predicted_fit = content_score_to_predicted_fit(fit.families.content.score);
    let decision = classify_eligibility(&EligibilityInput {
        predicted_fit,
        confidence: fit.families.content.confidence,
        hydration_completeness: features.hydration_completeness,
        semantic_coverage: semantic.coverage,
        semantic_negative: fit.content_detail.semantic_negative,
        semantic_margin: fit.content_detail.semantic_margin,
    });
    // Fit / Match are intrinsic — always stamp Content-calibrated values,
    // including for watchlist. Watchlist status never changes predicted taste fit.
    row.eligibility.predicted_fit = decision.fit;
    row.eligibility.confidence = decision.confidence;
    row.eligibility.hydration_completeness = decision.hydration_completeness;

    if candidate.watchlist {
        // Watchlist is a separate surface: bridge gates display, not New C1 bands.
        // Keep legacy portable-bridge `passed`; label for the watchlist lane only.
        row.eligibility.state = if row.eligibility.passed {
            "recommended".into()
        } else {
            "held".into()
        };
        row.eligibility.primary_reason = if row.eligibility.passed {
            "watchlist_bridge".into()
        } else {
            "watchlist_weak_bridge".into()
        };
    } else if row
        .eligibility
        .passed_because
        .iter()
        .any(|r| r == "short-runtime")
    {
        row.eligibility.state = "held".into();
        row.eligibility.primary_reason = "short-runtime".into();
        row.eligibility.passed = false;
    } else {
        // Provisional absolute C1; score_pool re-bands within the New lane only.
        row.eligibility.state = decision.state.as_str().into();
        row.eligibility.primary_reason = decision.primary_reason.clone();
        row.eligibility.passed = decision.state.board_eligible();
        row.eligibility.passed_because = vec![format!(
            "c1:{}:{}",
            decision.state.as_str(),
            decision.primary_reason
        )];
    }

    row.scoring_reasons.insert(
        0,
        format!(
            "Fit_v1 Content {:.3} (Craft diagnostic {:.3}, λ=0) elig={} ({})",
            fit.fit,
            fit.families.craft.score,
            row.eligibility.state,
            row.eligibility.primary_reason
        ),
    );
}

/// Watchlist is real intent (trailers, friends, plan-to-watch) but only a subtle
/// boost (`W_WATCHLIST`). Eligibility is about *feature usefulness*, not an
/// affinity floor:
/// - citeable craft (director/writer/DP/composer) or signal keyword → eligible
/// - actor needs `recommendation_mean >= PORTABLE_CONTEXTUAL` so extras/cameos
///   cannot occupy the same real estate as evidence-backed recommendations
/// - genre alone is never a watchlist bridge
/// Frozen formulas and `W_WATCHLIST` are unchanged.

fn loved_seed_ids(candidate: &Candidate, kind: RetrievalKind) -> std::collections::HashSet<i64> {
    candidate
        .sources
        .iter()
        .filter(|s| s.kind == kind && s.seed_rating.unwrap_or(0.0) >= 4.0)
        .filter_map(|s| s.seed_tmdb_id)
        .collect()
}

fn candidate_is_family_or_animation(candidate: &Candidate) -> bool {
    candidate.genres.iter().any(|g| {
        let lower = g.to_ascii_lowercase();
        lower == "animation" || lower == "family" || lower.contains("kid")
    })
}

fn candidate_is_tv_movie(candidate: &Candidate) -> bool {
    candidate
        .genres
        .iter()
        .any(|g| g.eq_ignore_ascii_case("tv movie"))
}

fn cluster_actor_signal(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| {
        a.key.family == FeatureFamily::Actor
            && a.appearances >= 3
            && a.recommendation_mean >= PORTABLE_CONTEXTUAL
            && person_relevance_transfers(a, candidate)
    })
}

fn transferring_creator(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| {
        matches!(
            a.key.family,
            FeatureFamily::Director
                | FeatureFamily::Writer
                | FeatureFamily::Cinematographer
        ) && person_relevance_transfers(a, candidate)
    })
}

fn specific_keyword_signal(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| {
        a.key.family == FeatureFamily::Keyword
            && matches!(
                keyword_strength(&a.key.name),
                KeywordStrength::Strong | KeywordStrength::Thematic
            )
            && candidate
                .keywords
                .iter()
                .any(|k| k.name.eq_ignore_ascii_case(&a.key.name))
            && keyword_relevance_transfers(a, candidate)
    })
}

fn strong_keyword_signal(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| {
        a.key.family == FeatureFamily::Keyword
            && keyword_strength(&a.key.name) == KeywordStrength::Strong
            && candidate
                .keywords
                .iter()
                .any(|k| k.name.eq_ignore_ascii_case(&a.key.name))
            && keyword_relevance_transfers(a, candidate)
    })
}

fn repeated_thematic_keyword_signal(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited
        .iter()
        .filter(|a| {
            a.key.family == FeatureFamily::Keyword
                && keyword_strength(&a.key.name) == KeywordStrength::Thematic
                && candidate
                    .keywords
                    .iter()
                    .any(|k| k.name.eq_ignore_ascii_case(&a.key.name))
                && keyword_relevance_transfers(a, candidate)
        })
        .count()
        >= 2
}

fn non_broad_genre_fit(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| {
        a.key.family == FeatureFamily::Genre
            && !is_broad_genre_name(&a.key.name)
            && candidate
                .genres
                .iter()
                .any(|g| g.eq_ignore_ascii_case(&a.key.name))
    })
}

/// Candidate-side metadata fit. Vacuous `candidate_movie_fit == 1.0` (no
/// cited people) does not count; source counts alone never do either.
fn metadata_fit(
    cited: &[&FeatureAffinity],
    candidate: &Candidate,
    creator: bool,
    keyword: bool,
) -> bool {
    creator || keyword || non_broad_genre_fit(cited, candidate)
}

/// One stored membership decision. Strong/Medium may occupy New; None is a lead.
pub fn compute_evidence_grade(
    candidate: &Candidate,
    cited: &[&FeatureAffinity],
    movie_fit: f32,
) -> EvidenceGrade {
    compute_evidence_grade_with_semantic(candidate, cited, movie_fit, &SemanticScore::default())
}

/// Below this fit, embeddings say the candidate is closer to disliked history
/// than liked — thin retrieval Medium must not override that preference.
const SEMANTIC_PREFERENCE_DEMOTE: f32 = 0.48;

pub fn compute_evidence_grade_with_semantic(
    candidate: &Candidate,
    cited: &[&FeatureAffinity],
    movie_fit: f32,
    semantic: &SemanticScore,
) -> EvidenceGrade {
    let deterministic = compute_legacy_evidence_grade(candidate, cited, movie_fit);
    let grade = if deterministic.displayable() {
        // Strong craft/multi-seed evidence still stands at neutral semantic.
        // Medium from a thin related bridge does not: clear dislike alignment
        // demotes so New follows what the user rejects, not TMDB neighbors.
        if semantic.coverage
            && semantic.fit < SEMANTIC_PREFERENCE_DEMOTE
            && deterministic == EvidenceGrade::Medium
            && loved_seed_ids(candidate, RetrievalKind::RelatedRecommendations).len() < 2
            && !strong_keyword_signal(cited, candidate)
        {
            EvidenceGrade::None
        } else {
            deterministic
        }
    } else if semantic.coverage {
        compute_semantic_evidence_grade(candidate, cited, movie_fit, semantic.fit)
    } else {
        deterministic
    };
    confidence_ceiling(grade, cited)
}

/// Evidence grades remain eligibility decisions. This ceiling only downgrades a
/// Strong result when none of its already-cited bridges has stable support.
fn confidence_ceiling(grade: EvidenceGrade, cited: &[&FeatureAffinity]) -> EvidenceGrade {
    if grade != EvidenceGrade::Strong {
        return grade;
    }
    let qualified = cited.iter().any(|affinity| {
        affinity.citeable()
            && affinity.appearances >= 3
            && affinity.confidence >= 0.50
            && !affinity.polarizing()
    });
    if qualified {
        EvidenceGrade::Strong
    } else {
        EvidenceGrade::Medium
    }
}

fn compute_legacy_evidence_grade(
    candidate: &Candidate,
    cited: &[&FeatureAffinity],
    movie_fit: f32,
) -> EvidenceGrade {
    if candidate.watchlist || candidate_is_tv_movie(candidate) {
        return EvidenceGrade::None;
    }
    let recs = loved_seed_ids(candidate, RetrievalKind::RelatedRecommendations);
    let similar = loved_seed_ids(candidate, RetrievalKind::RelatedSimilar)
        .union(&loved_seed_ids(candidate, RetrievalKind::Related))
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let creator = transferring_creator(cited, candidate);
    let keyword = specific_keyword_signal(cited, candidate);
    let seed_keyword = strong_keyword_signal(cited, candidate)
        || repeated_thematic_keyword_signal(cited, candidate);
    let cluster_actor = cluster_actor_signal(cited, candidate);
    let candidate_bridge = creator || keyword || cluster_actor;
    let strong_bridge = creator || strong_keyword_signal(cited, candidate) || cluster_actor;
    let fit = metadata_fit(cited, candidate, creator, keyword);
    let related = candidate.sources.iter().any(|s| s.kind.is_related());
    let related_bridge = related_has_portable_bridge(cited, candidate);
    let filmography_creator = candidate
        .sources
        .iter()
        .any(|s| s.kind == RetrievalKind::Filmography)
        && (creator
            || cited.iter().any(|a| {
                a.key.family == FeatureFamily::Composer
                    && person_relevance_transfers(a, candidate)
            }))
        && movie_fit >= 0.999;
    let filmography_loyalty = filmography_loyalty_bridge(cited, candidate) && movie_fit >= 0.50;

    let mut grade = EvidenceGrade::None;
    if recs.len() >= 2 && fit && strong_bridge {
        grade = EvidenceGrade::Strong;
    } else if recs.len() >= 2 && fit && candidate_bridge {
        // Two recommendations are useful corroboration, but a single
        // generic thematic keyword should not make the card Strong.
        grade = EvidenceGrade::Medium;
    } else if recs.len() >= 1 && fit && (creator || seed_keyword || cluster_actor) {
        grade = EvidenceGrade::Medium;
    } else if similar.len() >= 2 && fit && candidate_bridge {
        grade = EvidenceGrade::Medium;
    } else if similar.len() >= 1 && cluster_actor && fit {
        grade = EvidenceGrade::Medium;
    } else if creator && keyword {
        grade = EvidenceGrade::Medium;
    } else if related && related_bridge && fit {
        // A related result plus a candidate-side craft/keyword/cluster-actor
        // bridge is two independent signals. A bare related result remains a
        // retrieval lead, even when its genres look familiar.
        grade = EvidenceGrade::Medium;
    } else if filmography_loyalty || filmography_creator {
        grade = EvidenceGrade::Medium;
    }

    if candidate_is_family_or_animation(candidate) && grade.displayable() {
        let animation_guard = recs.len() >= 2
            || (recs.len() >= 1 && fit)
            || creator
            || filmography_loyalty
            || filmography_creator
            || (similar.len() >= 2 && fit)
            || keyword
            || cluster_actor;
        if !animation_guard {
            return EvidenceGrade::None;
        }
    }
    grade
}

fn filmography_loyalty_bridge(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    if !candidate
        .sources
        .iter()
        .any(|s| s.kind == RetrievalKind::Filmography)
    {
        return false;
    }
    cited.iter().any(|a| {
        person_has_loyalty(a) && person_relevance_transfers(a, candidate)
    })
}

fn compute_semantic_evidence_grade(
    candidate: &Candidate,
    cited: &[&FeatureAffinity],
    movie_fit: f32,
    semantic_fit: f32,
) -> EvidenceGrade {
    if candidate.watchlist || candidate_is_tv_movie(candidate) {
        return EvidenceGrade::None;
    }
    let recs = loved_seed_ids(candidate, RetrievalKind::RelatedRecommendations);
    let similar = loved_seed_ids(candidate, RetrievalKind::RelatedSimilar)
        .union(&loved_seed_ids(candidate, RetrievalKind::Related))
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let creator = transferring_creator(cited, candidate);
    let keyword = specific_keyword_signal(cited, candidate);
    let seed_keyword = strong_keyword_signal(cited, candidate)
        || repeated_thematic_keyword_signal(cited, candidate);
    let cluster_actor = cluster_actor_signal(cited, candidate);
    let explicit_metadata = creator || keyword;
    let semantic_medium = semantic_fit >= 0.58;
    let semantic_strong = semantic_fit >= 0.68;
    let fit = movie_fit >= 0.50;
    let related = candidate.sources.iter().any(|s| s.kind.is_related());
    let related_bridge = related_has_portable_bridge(cited, candidate);
    let filmography_creator = candidate
        .sources
        .iter()
        .any(|s| s.kind == RetrievalKind::Filmography)
        && (creator
            || cited.iter().any(|a| {
                a.key.family == FeatureFamily::Composer
                    && person_relevance_transfers(a, candidate)
            }))
        && movie_fit >= 0.72;
    let filmography_loyalty = filmography_loyalty_bridge(cited, candidate) && fit;

    let mut grade = EvidenceGrade::None;
    if recs.len() >= 2 && explicit_metadata && fit && semantic_strong {
        grade = EvidenceGrade::Strong;
    } else if recs.len() >= 1
        && explicit_metadata
        && (creator || seed_keyword || cluster_actor)
        && fit
        && semantic_medium
    {
        grade = EvidenceGrade::Medium;
    } else if similar.len() >= 2 && explicit_metadata && fit && semantic_medium {
        grade = EvidenceGrade::Medium;
    } else if similar.len() >= 1 && cluster_actor && fit && semantic_medium {
        grade = EvidenceGrade::Medium;
    } else if creator && keyword && fit && semantic_medium {
        grade = EvidenceGrade::Medium;
    } else if related && related_bridge && explicit_metadata && fit && semantic_medium {
        grade = EvidenceGrade::Medium;
    } else if filmography_loyalty {
        grade = EvidenceGrade::Medium;
    } else if filmography_creator && semantic_medium {
        grade = EvidenceGrade::Medium;
    }

    if candidate_is_family_or_animation(candidate) && grade.displayable() && !filmography_loyalty {
        let animation_guard = semantic_strong
            && (recs.len() >= 2
                || creator
                || keyword
                || cluster_actor
                || (similar.len() >= 2 && explicit_metadata));
        if !animation_guard {
            return EvidenceGrade::None;
        }
    }
    grade
}

pub fn unique_loved_rec_seeds(c: &ScoredCandidate) -> usize {
    loved_seed_ids_from_sources(&c.candidate.sources, RetrievalKind::RelatedRecommendations).len()
}

fn loved_seed_ids_from_sources(
    sources: &[crate::taste::retrieve::RetrievalSource],
    kind: RetrievalKind,
) -> std::collections::HashSet<i64> {
    sources
        .iter()
        .filter(|s| s.kind == kind && s.seed_rating.unwrap_or(0.0) >= 4.0)
        .filter_map(|s| s.seed_tmdb_id)
        .collect()
}

/// TMDB-related is useful when it finds a person or signal keyword already in
/// the profile. Broad genre overlap is a description of the similar-to seed,
/// not a recommendation signal. Thin-but-citeable craft (Fraser at 0.37) still
/// counts; this does not apply the actor affinity floor. A craft credit still
/// has to transfer from liked evidence to this particular film.
fn related_has_portable_bridge(cited: &[&FeatureAffinity], candidate: &Candidate) -> bool {
    cited.iter().any(|a| match a.key.family {
        FeatureFamily::Keyword => keyword_is_portable_bridge(a, cited),
        FeatureFamily::Director
        | FeatureFamily::Writer
        | FeatureFamily::Cinematographer => person_relevance_transfers(a, candidate),
        FeatureFamily::Actor => cluster_actor_signal(cited, candidate),
        _ => false,
    })
}

fn watchlist_has_strong_bridge(
    profile: &FeatureProfile,
    cited: &[&FeatureAffinity],
    candidate: &Candidate,
) -> bool {
    if sparse_animation_watchlist_bridge(profile, candidate) {
        return true;
    }
    cited.iter().any(|a| match a.key.family {
        FeatureFamily::Keyword => keyword_is_portable_bridge(a, cited),
        FeatureFamily::Director
        | FeatureFamily::Writer
        | FeatureFamily::Cinematographer
        | FeatureFamily::Composer => person_relevance_transfers(a, candidate),
        FeatureFamily::Actor => a.recommendation_mean >= PORTABLE_CONTEXTUAL,
        _ => false,
    })
}

fn sparse_animation_watchlist_bridge(profile: &FeatureProfile, candidate: &Candidate) -> bool {
    if !candidate.watchlist
        || !candidate_is_family_or_animation(candidate)
        || candidate.genres.len() < 2
        || candidate.credits.len() > 4
        || candidate.keywords.len() > 4
        || candidate
            .credits
            .iter()
            .any(|credit| family_for_job(&credit.job) == Some(FeatureFamily::Actor))
    {
        return false;
    }
    let direct_animation_creator = profile.affinities.iter().any(|aff| {
        is_craft_person(aff)
            && aff.citeable()
            && aff.positive_evidence.iter().any(|film| {
                history_evidence_ok(film)
                    && film
                        .genres
                        .iter()
                        .any(|genre| genre.eq_ignore_ascii_case("Animation"))
            })
            && candidate.credits.iter().any(|credit| {
                family_for_job(&credit.job) == Some(aff.key.family)
                    && ((credit.id.is_some() && credit.id == aff.key.id)
                        || credit.name.eq_ignore_ascii_case(&aff.key.name))
            })
    });
    let matching_affinities = profile
        .affinities
        .iter()
        .filter(|aff| {
            is_craft_person(aff)
                && aff.citeable()
                && aff.appearances >= 2
                && aff.positive_evidence.len() >= 2
                && aff
                    .positive_evidence
                    .iter()
                    .filter(|film| {
                        history_evidence_ok(film) && genre_overlap_count(film, candidate) > 0
                    })
                    .count()
                    >= 2
        })
        .count();
    direct_animation_creator || matching_affinities >= 2
}

fn keyword_is_portable_bridge(aff: &FeatureAffinity, cited: &[&FeatureAffinity]) -> bool {
    match keyword_strength(&aff.key.name) {
        KeywordStrength::Strong => true,
        KeywordStrength::Thematic => cited.iter().filter(|a| feature_helps_carry(a)).count() >= 2,
        _ => false,
    }
}

fn feature_helps_carry(aff: &FeatureAffinity) -> bool {
    match aff.key.family {
        FeatureFamily::Director
        | FeatureFamily::Writer
        | FeatureFamily::Cinematographer
        | FeatureFamily::Composer => true,
        FeatureFamily::Actor => aff.recommendation_mean >= PORTABLE_CONTEXTUAL,
        FeatureFamily::Keyword => matches!(
            keyword_strength(&aff.key.name),
            KeywordStrength::Strong | KeywordStrength::Thematic
        ),
        _ => false,
    }
}

fn cited_can_carry(cited: &[&FeatureAffinity]) -> bool {
    if cited.iter().any(|a| {
        matches!(
            a.key.family,
            FeatureFamily::Director
                | FeatureFamily::Writer
                | FeatureFamily::Cinematographer
                | FeatureFamily::Composer
        ) || (a.key.family == FeatureFamily::Keyword
            && keyword_strength(&a.key.name) == KeywordStrength::Strong)
    }) {
        return true;
    }
    cited.iter().filter(|a| feature_helps_carry(a)).count() >= 2
}

fn is_craft_person(aff: &FeatureAffinity) -> bool {
    matches!(
        aff.key.family,
        FeatureFamily::Director
            | FeatureFamily::Writer
            | FeatureFamily::Cinematographer
            | FeatureFamily::Composer
    )
}

fn is_broad_genre_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "drama"
            | "comedy"
            | "thriller"
            | "action"
            | "adventure"
            | "romance"
            | "family"
    )
}

fn shared_non_broad_genre(film: &EvidenceFilm, candidate: &Candidate) -> bool {
    film.genres.iter().any(|g| {
        !is_broad_genre_name(g)
            && candidate
                .genres
                .iter()
                .any(|c| c.eq_ignore_ascii_case(g))
    })
}

fn specific_genre_overlap(film: &EvidenceFilm, candidate: &Candidate) -> bool {
    // One non-broad shared genre is enough to transfer a person's relevance.
    // Requiring two tags made sparse TMDB metadata discard legitimate craft
    // matches such as a Science Fiction film with only one genre label.
    shared_non_broad_genre(film, candidate)
}

/// Person affinity is only part of match quality. This movie must also carry
/// compatible visual/story/craft evidence from liked examples.
fn candidate_movie_fit(cited: &[&FeatureAffinity], candidate: &Candidate) -> f32 {
    let modes = predicted_modes(
        &candidate.genres,
        &candidate.credits,
        &candidate.keywords,
    );
    let people: Vec<_> = cited.iter().copied().filter(|a| is_craft_person(a)).collect();
    let mut specific = false;
    let mut broad = false;
    let keyword_specific = cited.iter().any(|a| {
        a.key.family == FeatureFamily::Keyword
            && matches!(
                keyword_strength(&a.key.name),
                KeywordStrength::Strong | KeywordStrength::Thematic
            )
            && candidate
                .keywords
                .iter()
                .any(|k| k.name.eq_ignore_ascii_case(&a.key.name))
            && keyword_relevance_transfers(a, candidate)
    });
    if keyword_specific {
        specific = true;
    }
    for aff in &people {
        if aff
            .positive_evidence
            .iter()
            .any(|film| specific_genre_overlap(film, candidate))
            || aff
                .positive_evidence
                .iter()
                .any(|film| keyword_overlap(film, candidate))
        {
            specific = true;
            continue;
        }
        if !aff.evidence_cluster.is_empty()
            && cluster_overlap_is_specific(aff, candidate, &modes)
        {
            specific = true;
            continue;
        }
        if aff
            .positive_evidence
            .iter()
            .any(|film| genre_overlap_count(film, candidate) >= 1)
        {
            broad = true;
        }
    }
    if specific {
        1.0
    } else if broad {
        0.55
    } else if !people.is_empty() {
        0.32
    } else {
        1.0
    }
}

/// Non-vacuous candidate fit used once semantic coverage is available. A
/// missing creator/keyword/actor-cluster bridge must not become a perfect fit
/// merely because the profile had no portable person to cite.
fn semantic_metadata_fit(cited: &[&FeatureAffinity], candidate: &Candidate) -> f32 {
    if transferring_creator(cited, candidate)
        || specific_keyword_signal(cited, candidate)
        || cluster_actor_signal(cited, candidate)
    {
        1.0
    } else if non_broad_genre_fit(cited, candidate) {
        0.35
    } else {
        0.0
    }
}

fn cluster_overlap_is_specific(
    aff: &FeatureAffinity,
    candidate: &Candidate,
    modes: &[String],
) -> bool {
    let cluster = &aff.evidence_cluster;
    let keyword_hit = cluster.keywords.iter().any(|k| {
        matches!(
            keyword_strength(k),
            KeywordStrength::Strong | KeywordStrength::Thematic
        ) && candidate
            .keywords
            .iter()
            .any(|ck| ck.name.eq_ignore_ascii_case(k))
    });
    let specific_genre = cluster.genres.iter().any(|g| {
        !is_broad_genre_name(g)
            && candidate
                .genres
                .iter()
                .any(|c| c.eq_ignore_ascii_case(g))
    });
    // Generated modes are intentionally not enough here: values such as
    // "comedy" and "spectacle" are broad dimensions, not candidate-specific
    // metadata. Keywords and non-broad genres carry the portable fit.
    let _ = modes;
    keyword_hit || specific_genre
}

fn watchlist_evidence_grade(
    profile: &FeatureProfile,
    cited: &[&FeatureAffinity],
    candidate: &Candidate,
) -> EvidenceGrade {
    if !watchlist_has_strong_bridge(profile, cited, candidate) {
        return EvidenceGrade::None;
    }
    if cited.iter().any(|a| {
        matches!(
            a.key.family,
            FeatureFamily::Director
                | FeatureFamily::Writer
                | FeatureFamily::Cinematographer
                | FeatureFamily::Composer
        ) && a.appearances >= 3
            && person_relevance_transfers(a, candidate)
    }) {
        EvidenceGrade::Strong
    } else {
        EvidenceGrade::Medium
    }
}

/// Independent-evidence quality for selection. Does not change frozen scoring weights.
pub(crate) fn evidence_grade(c: &ScoredCandidate) -> i32 {
    c.eligibility.evidence_grade.rank()
}

pub(crate) fn reasons_have_strong_bridge(reasons: &[String]) -> bool {
    reasons.iter().any(|r| {
        let lower = r.to_lowercase();
        lower.contains("affinity") && !is_broad_genre_reason(&lower)
    })
}

pub(crate) fn reasons_are_genre_only(reasons: &[String]) -> bool {
    let affinities: Vec<_> = reasons
        .iter()
        .filter(|r| r.to_lowercase().contains("affinity"))
        .collect();
    !affinities.is_empty()
        && affinities
            .iter()
            .all(|r| is_broad_genre_reason(&r.to_lowercase()))
}

fn is_broad_genre_reason(reason: &str) -> bool {
    let Some(label) = reason.split(" affinity").next() else {
        return false;
    };
    matches!(
        label,
        "drama"
            | "comedy"
            | "romance"
            | "thriller"
            | "crime"
            | "action"
            | "family"
            | "horror"
            | "adventure"
            | "fantasy"
            | "science fiction"
            | "animation"
            | "mystery"
            | "history"
            | "war"
            | "music"
            | "western"
            | "documentary"
            | "tv movie"
    )
}

/// Filmography is an investigate-the-person query. A credit is eligible when
/// the person's relevance transfers from liked films to this candidate:
/// watchlist creator loyalty (unenriched title), cluster overlap with liked
/// examples, two shared genres with a liked example, or — when the cluster is
/// empty — any genre overlap with a liked example. Watchlist filmography rows
/// are deferred to scoring so sparse metadata can use the watchlist bridge.
/// A shared quality keyword does not reopen an off-cluster résumé.
/// Frozen affinities are unchanged.
pub fn filmography_supported(profile: &FeatureProfile, candidate: &Candidate) -> bool {
    let Some(src) = candidate
        .sources
        .iter()
        .find(|s| s.kind == RetrievalKind::Filmography)
    else {
        return true;
    };
    let Some(aff) = profile.affinities.iter().find(|a| {
        a.citeable()
            && a.key.is_person_or_keyword()
            && a.key.name.eq_ignore_ascii_case(&src.label)
    }) else {
        return true;
    };
    if !filmography_job_matches(aff, candidate) {
        // A merged filmography result can carry the wrong job for a watchlist
        // title. Preserve that explicit watchlist intent, but do not let the
        // same mismatch create a new recommendation.
        return candidate.watchlist;
    }
    person_relevance_transfers(aff, candidate)
        || (candidate.watchlist && sparse_animation_watchlist_bridge(profile, candidate))
}

fn filmography_job_matches(aff: &FeatureAffinity, candidate: &Candidate) -> bool {
    candidate.credits.iter().any(|c| {
        family_for_job(&c.job) == Some(aff.key.family)
            && ((c.id.is_some() && c.id == aff.key.id)
                || c.name.eq_ignore_ascii_case(&aff.key.name))
    })
}

/// Does this person's liked work make *this* film interesting — not merely
/// "the user likes this person, so dump the catalog"?
fn person_relevance_transfers(aff: &FeatureAffinity, candidate: &Candidate) -> bool {
    // Unenriched watchlist titles (Piranesi, unreleased Laika) keep creator
    // loyalty. Filmography retrieval of the same empty-metadata credit is
    // generic catalog expansion and must not use this path.
    let modes = predicted_modes(
        &candidate.genres,
        &candidate.credits,
        &candidate.keywords,
    );
    if candidate.watchlist {
        if candidate_unenriched(candidate) && aff.positive_evidence.len() >= 2 {
            return true;
        }
        return !aff.evidence_cluster.is_empty()
            && aff
                .evidence_cluster
                .overlaps(&candidate.genres, &candidate.keywords, &modes);
    }
    if !aff.evidence_cluster.is_empty()
        && cluster_overlap_is_specific(aff, candidate, &modes)
    {
        return true;
    }
    if aff
        .positive_evidence
        .iter()
        .any(|film| specific_genre_overlap(film, candidate) || shared_non_broad_genre(film, candidate))
    {
        return true;
    }
    // Repeated high preference for a person *is* the bridge. Creed→Rocky and
    // Laika→Laika share broad genres/modes that the specific-genre path rejects.
    if person_has_loyalty(aff) {
        return aff.positive_evidence.iter().any(|film| {
            history_evidence_ok(film)
                && (genre_overlap_count(film, candidate) >= 1
                    || film_mode_overlaps_candidate(film, &modes))
        });
    }
    false
}

fn person_has_loyalty(aff: &FeatureAffinity) -> bool {
    matches!(
        aff.key.family,
        FeatureFamily::Director
            | FeatureFamily::Writer
            | FeatureFamily::Cinematographer
            | FeatureFamily::Actor
    ) && aff.appearances >= 3
        && aff.recommendation_mean >= PORTABLE_CONTEXTUAL
}

fn candidate_unenriched(candidate: &Candidate) -> bool {
    candidate.genres.is_empty() && candidate.keywords.is_empty()
}

/// Does liked use of this keyword make *this* film interesting — not merely
/// "the user once liked a film that shares a thematic tag"?
fn keyword_relevance_transfers(aff: &FeatureAffinity, candidate: &Candidate) -> bool {
    if aff.key.family != FeatureFamily::Keyword {
        return true;
    }
    if !candidate
        .keywords
        .iter()
        .any(|k| k.name.eq_ignore_ascii_case(&aff.key.name))
    {
        return false;
    }
    if keyword_negative_context_dominates(aff, candidate) {
        return false;
    }
    let modes = predicted_modes(
        &candidate.genres,
        &candidate.credits,
        &candidate.keywords,
    );
    match keyword_strength(&aff.key.name) {
        KeywordStrength::Strong => {
            // Craft-signature tags (neo-noir, time loop) are the context.
            true
        }
        KeywordStrength::Thematic | KeywordStrength::Broad => {
            if !aff.evidence_cluster.is_empty()
                && aff
                    .evidence_cluster
                    .overlaps(&candidate.genres, &candidate.keywords, &modes)
            {
                // Cluster overlap on the keyword name alone is not enough —
                // require genre or mode agreement with how the user liked it.
                let genre_or_mode = aff.evidence_cluster.genres.iter().any(|g| {
                    candidate
                        .genres
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(g))
                }) || aff.evidence_cluster.modes.iter().any(|m| {
                    modes.iter().any(|c| c.eq_ignore_ascii_case(m))
                });
                if genre_or_mode {
                    return true;
                }
            }
            aff.positive_evidence.iter().any(|film| {
                history_evidence_ok(film)
                    && (specific_genre_overlap(film, candidate)
                        || shared_non_broad_genre(film, candidate)
                        || genre_overlap_count(film, candidate) >= 2
                        || (genre_overlap_count(film, candidate) >= 1
                            && film_mode_overlaps_candidate(film, &modes)))
            })
        }
        KeywordStrength::Contextual | KeywordStrength::Ignore => false,
    }
}

fn film_mode_overlaps_candidate(film: &EvidenceFilm, candidate_modes: &[String]) -> bool {
    let film_modes = predicted_modes(&film.genres, &[], &[]);
    film_modes
        .iter()
        .any(|m| candidate_modes.iter().any(|c| c.eq_ignore_ascii_case(m)))
}

fn keyword_negative_context_dominates(aff: &FeatureAffinity, candidate: &Candidate) -> bool {
    if aff.negative_evidence.is_empty() {
        return false;
    }
    let best = |films: &[crate::taste::features::EvidenceFilm]| {
        films
            .iter()
            .map(|film| {
                let mut score = genre_overlap_count(film, candidate) as i32;
                if keyword_overlap(film, candidate) {
                    score += 1;
                }
                score
            })
            .max()
            .unwrap_or(0)
    };
    let pos = best(
        &aff.positive_evidence
            .iter()
            .filter(|film| history_evidence_ok(film))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let neg = best(&aff.negative_evidence);
    neg > pos && neg >= 1
}

fn genre_overlap_count(film: &EvidenceFilm, candidate: &Candidate) -> usize {
    film.genres
        .iter()
        .filter(|g| {
            candidate
                .genres
                .iter()
                .any(|c| c.eq_ignore_ascii_case(g))
        })
        .count()
}

fn keyword_overlap(film: &EvidenceFilm, candidate: &Candidate) -> bool {
    film.keywords.iter().any(|k| {
        matches!(
            keyword_strength(k),
            KeywordStrength::Strong | KeywordStrength::Thematic
        ) && candidate
            .keywords
            .iter()
            .any(|ck| ck.name.eq_ignore_ascii_case(k))
    })
}

fn evidence_titles_for(aff: &FeatureAffinity, candidate: &Candidate) -> Vec<String> {
    let person = matches!(
        aff.key.family,
        FeatureFamily::Director
            | FeatureFamily::Writer
            | FeatureFamily::Cinematographer
            | FeatureFamily::Composer
            | FeatureFamily::Actor
    );
    if !person {
        return Vec::new();
    }
    aff.positive_evidence
        .iter()
        .filter(|film| history_evidence_ok(film) && evidence_fits_candidate(aff, film, candidate))
        .map(|film| film.title.clone())
        .take(2)
        .collect()
}

fn history_evidence_ok(film: &crate::taste::features::EvidenceFilm) -> bool {
    if film.rating < 3.5 {
        return false;
    }
    if is_short_runtime(film.runtime) {
        return false;
    }
    let year_now = chrono::Utc::now().year();
    if film.year.map(|y| y >= year_now).unwrap_or(false) {
        return false;
    }
    true
}

/// Genre matches are too broad to inherit the strongest observations of that
/// genre. A Drama candidate only cites a Drama evidence film when they share a
/// person or a quality keyword — not merely the genre.
fn evidence_fits_candidate(
    aff: &FeatureAffinity,
    film: &crate::taste::features::EvidenceFilm,
    candidate: &Candidate,
) -> bool {
    match aff.key.family {
        FeatureFamily::Genre => {
            let person_hit = film.people.iter().any(|p| {
                candidate
                    .credits
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(p))
            });
            let keyword_hit = film.keywords.iter().any(|k| {
                matches!(
                    keyword_strength(k),
                    KeywordStrength::Strong | KeywordStrength::Thematic
                ) && candidate
                    .keywords
                    .iter()
                    .any(|ck| ck.name.eq_ignore_ascii_case(k))
            });
            person_hit || keyword_hit
        }
        FeatureFamily::Keyword => {
            matches!(
                keyword_strength(&aff.key.name),
                KeywordStrength::Strong | KeywordStrength::Thematic
            ) && film.rating >= 3.5
        }
        FeatureFamily::Director
        | FeatureFamily::Writer
        | FeatureFamily::Cinematographer
        | FeatureFamily::Composer
        | FeatureFamily::Actor => {
            film.rating >= 3.5
                && (genre_overlap_count(film, candidate) >= 2
                    || keyword_overlap(film, candidate)
                    || (candidate.watchlist && candidate_unenriched(candidate)))
        }
        _ => false,
    }
}

fn candidate_keys(candidate: &Candidate) -> Vec<FeatureKey> {
    let mut keys = Vec::new();
    for g in &candidate.genres {
        keys.push(FeatureKey::new(FeatureFamily::Genre, None, g));
    }
    for c in &candidate.credits {
        let Some(family) = family_for_job(&c.job) else {
            continue;
        };
        keys.push(FeatureKey::new(family, c.id, &c.name));
    }
    for k in &candidate.keywords {
        if !keyword_is_taste_signal(&k.name) {
            continue;
        }
        keys.push(FeatureKey::new(FeatureFamily::Keyword, k.id, &k.name));
    }
    if let Some(y) = candidate.year {
        keys.push(FeatureKey::new(FeatureFamily::Decade, None, decade_label(y)));
    }
    if let Some(rt) = candidate.runtime {
        keys.push(FeatureKey::new(
            FeatureFamily::Runtime,
            None,
            runtime_bucket(rt),
        ));
    }
    keys
}

pub struct ScorePool {
    pub ranked: Vec<ScoredCandidate>,
    pub dropped_contextual: Vec<ScoredCandidate>,
    pub dropped_filmography: Vec<String>,
    pub dropped_contextual_total: usize,
    pub dropped_filmography_total: usize,
}

pub fn score_all(profile: &FeatureProfile, candidates: &[Candidate]) -> Vec<ScoredCandidate> {
    score_pool(profile, candidates).ranked
}

pub fn score_pool(profile: &FeatureProfile, candidates: &[Candidate]) -> ScorePool {
    score_pool_with_semantic(profile, candidates, &std::collections::HashMap::new())
}

pub fn score_pool_with_semantic(
    profile: &FeatureProfile,
    candidates: &[Candidate],
    semantic_scores: &std::collections::HashMap<i64, SemanticScore>,
) -> ScorePool {
    let mut dropped_filmography = Vec::new();
    let mut dropped_contextual = Vec::new();
    let mut dropped_filmography_total = 0;
    let mut dropped_contextual_total = 0;
    let mut new_lane: Vec<ScoredCandidate> = Vec::new();
    let mut watch_lane: Vec<ScoredCandidate> = Vec::new();
    for c in candidates {
        if !filmography_supported(profile, c) {
            dropped_filmography_total += 1;
            dropped_filmography.push(c.title.clone());
            continue;
        }
        let semantic = c
            .tmdb_id
            .and_then(|id| semantic_scores.get(&id))
            .cloned()
            .unwrap_or_default();
        let row = score_candidate_with_semantic(profile, c, &semantic);
        // Route by destination *before* scarce New admission. Watchlist never
        // enters the New C1 band population. Unreleased / TV stubs also cannot
        // occupy New, so they must not compete for New relative bands.
        if c.watchlist {
            watch_lane.push(row);
        } else if crate::taste::confidence::unreleased_new_row(&row)
            || row.candidate.media_kind != crate::taste::retrieve::MediaKind::Movie
        {
            let mut held = row;
            held.eligibility.passed = false;
            held.eligibility.state = "held".into();
            held.eligibility.primary_reason =
                if held.candidate.media_kind != crate::taste::retrieve::MediaKind::Movie {
                    "tv_or_non_movie".into()
                } else {
                    "unreleased".into()
                };
            dropped_contextual_total += 1;
            dropped_contextual.push(held);
        } else {
            new_lane.push(row);
        }
    }

    // C1 relative bands — New-capable candidates only (includes provisional Holds
    // so pool-relative percentiles aren't computed from an already-truncated set).
    crate::taste::eligibility::apply_pool_bands(&mut new_lane);
    let (mut new_passed, newly_held): (Vec<_>, Vec<_>) = new_lane
        .into_iter()
        .partition(|r| r.eligibility.passed);
    dropped_contextual_total += newly_held.len();
    dropped_contextual.extend(newly_held);

    // Watchlist surface: bridge-gated display, ranked by the same Content total.
    let (mut watch_passed, watch_held): (Vec<_>, Vec<_>) = watch_lane
        .into_iter()
        .partition(|r| r.eligibility.passed);
    dropped_contextual_total += watch_held.len();
    dropped_contextual.extend(watch_held);

    cap_filmography_per_person(&mut new_passed, 8);
    let by_content = |a: &ScoredCandidate, b: &ScoredCandidate| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.candidate
                    .tmdb_id
                    .unwrap_or(i64::MAX)
                    .cmp(&b.candidate.tmdb_id.unwrap_or(i64::MAX))
            })
    };
    new_passed.sort_by(by_content);
    watch_passed.sort_by(by_content);

    // Buffers keep New and Watchlist capacity separate (watchlist does not
    // consume NEW_SCORE_BUFFER slots).
    let mut combined = new_passed;
    combined.extend(watch_passed);
    let scored = crate::taste::workspace::split_ranked_buffers(combined);
    dropped_contextual.sort_by(by_content);
    dropped_contextual.truncate(80);
    dropped_filmography.truncate(80);
    ScorePool {
        ranked: scored,
        dropped_contextual,
        dropped_filmography,
        dropped_contextual_total,
        dropped_filmography_total,
    }
}

fn cap_filmography_per_person(scored: &mut Vec<ScoredCandidate>, max_n: usize) {
    use std::collections::HashMap;
    let mut kept: HashMap<String, usize> = HashMap::new();
    scored.retain(|c| {
        if c.candidate.watchlist {
            return true;
        }
        let Some(src) = c
            .candidate
            .sources
            .iter()
            .find(|s| s.kind == RetrievalKind::Filmography)
        else {
            return true;
        };
        let n = kept.entry(src.label.clone()).or_insert(0);
        if *n >= max_n {
            return false;
        }
        *n += 1;
        true
    });
}

/// Titles allowed on "Close to": positively rated films from the user's
/// actual history. Watchlist-only, unwatched catalog, and shorts are out.
pub fn close_to_allowlist(films: &[crate::taste::retrieve::FilmRecord]) -> std::collections::HashSet<String> {
    let year_now = chrono::Utc::now().year();
    films
        .iter()
        .filter(|f| {
            let rated = matches!(f.rating, Some(r) if r >= 3.5);
            let seen = f.watched || f.viewings > 0;
            let watchlist_only = f.watchlist && !f.watched && f.viewings == 0;
            let established = matches!(f.year, Some(y) if y < year_now);
            let obscure = f.runtime.is_none() && f.vote_count.unwrap_or(0) < 150;
            rated && seen && !watchlist_only && !is_short_runtime(f.runtime) && established && !obscure
        })
        .map(|f| f.title.trim().to_ascii_lowercase())
        .collect()
}

pub fn filter_close_to_evidence(
    evidence: &[String],
    allow: &std::collections::HashSet<String>,
) -> Vec<String> {
    evidence
        .iter()
        .filter(|t| allow.contains(&t.trim().to_ascii_lowercase()))
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
pub struct PersonRetrievalTrace {
    pub name: String,
    pub appearances: u32,
    pub recommendation_mean: f32,
    pub confidence: f32,
    pub scoring_affinity: f32,
    pub injected: usize,
    pub survived_score: usize,
    pub survived_mmr: usize,
}

pub fn person_pipeline_trace(
    profile: &FeatureProfile,
    injected: &[Candidate],
    scored: &[ScoredCandidate],
    shortlist: &[ScoredCandidate],
) -> Vec<PersonRetrievalTrace> {
    profile
        .affinities
        .iter()
        .filter(|a| a.citeable() && a.key.is_person_or_keyword())
        .map(|a| {
            let named = |credits: &[crate::taste::features::Credit]| {
                credits.iter().any(|c| c.name.eq_ignore_ascii_case(&a.key.name))
            };
            let injected_n = injected
                .iter()
                .filter(|c| {
                    named(&c.credits)
                        || c.sources.iter().any(|s| {
                            s.kind == RetrievalKind::Filmography
                                && s.label.eq_ignore_ascii_case(&a.key.name)
                        })
                })
                .count();
            let in_list = |rows: &[ScoredCandidate]| {
                rows.iter()
                    .filter(|c| {
                        c.positive_features
                            .iter()
                            .any(|f| f.eq_ignore_ascii_case(&a.key.name))
                    })
                    .count()
            };
            PersonRetrievalTrace {
                name: a.key.name.clone(),
                appearances: a.appearances,
                recommendation_mean: a.recommendation_mean,
                confidence: a.confidence,
                scoring_affinity: a.scoring_affinity(),
                injected: injected_n,
                survived_score: in_list(scored),
                survived_mmr: in_list(shortlist),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::features::{build_profile, observations_from_film, Credit, Keyword};
    use crate::taste::preference::{interaction_signal, rating_profile};
    use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
    use crate::taste::semantic::SemanticScore;

    #[test]
    fn components_stay_in_range() {
        let mut s = CandidateScore {
            content: 4.0,
            tmdb_related: 3.0,
            friend_affinity: -4.0,
            recent_taste: 2.0,
            watchlist: 5.0,
            novelty: 9.0,
            negative_evidence: -4.0,
            semantic_fit: 0.5,
            semantic_coverage: false,
            total: 0.0,
        };
        s.clamp_components();
        assert!((-1.0..=1.0).contains(&s.content));
        assert!((0.0..=1.0).contains(&s.tmdb_related));
        assert!((-1.0..=1.0).contains(&s.friend_affinity));
        assert!((-1.0..=1.0).contains(&s.recent_taste));
        assert!((0.0..=1.0).contains(&s.watchlist));
        assert!((-1.0..=1.0).contains(&s.novelty));
        assert!((-1.0..=0.0).contains(&s.negative_evidence));
    }

    #[test]
    fn cinematographer_stays_eligible_but_fit_v1_ignores_craft() {
        // Fit_v1 = Content only: a known DP must not inflate ranking content/total
        // vs a thin decade-only candidate. Eligibility may still cite craft.
        use crate::taste::features::{
            build_profile, observations_from_film, Credit, Keyword,
        };
        use crate::taste::preference::{interaction_signal, rating_profile};
        use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
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
            Some(176),
        );
        obs.extend(observations_from_film(
            "Project Hail Mary",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp.clone()],
            &[],
            Some(2026),
            Some(140),
        ));
        for i in 0..20 {
            let s = interaction_signal(4.5, &p, Some(8.0), 6, false);
            obs.extend(observations_from_film(
                &format!("kid{i}"),
                4.5,
                Some(10 + i),
                &s,
                Some(8.0),
                &["Comedy".into()],
                &[],
                &[],
                Some(2005),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let dp_cand = Candidate {
            tmdb_id: Some(99),
            title: "Dune".into(),
            year: Some(2021),
            poster: None,
            genres: vec!["Science Fiction".into()],
            credits: vec![dp],
            keywords: Vec::<Keyword>::new(),
            runtime: Some(155),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        };
        let decade_cand = Candidate {
            tmdb_id: Some(100),
            title: "Tinker Bell".into(),
            year: Some(2008),
            poster: None,
            genres: vec!["Animation".into()],
            credits: vec![],
            keywords: vec![],
            runtime: Some(78),
            vote_count: Some(100),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to childhood".into(),
                seed_tmdb_id: Some(10),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        };
        let dp_score = score_candidate(&profile, &dp_cand);
        let decade_score = score_candidate(&profile, &decade_cand);
        assert!(
            (dp_score.score.content - decade_score.score.content).abs() < 0.05,
            "Fit_v1 must not let DP craft pull content ahead of decade-only: dp {} decade {}",
            dp_score.score.content,
            decade_score.score.content
        );
        assert!(
            dp_score.scoring_reasons.iter().any(|r| r.starts_with("Fit_v1")),
            "live scorer should stamp Fit_v1 diagnostic reason"
        );
        assert!(!dp_score.contextual_only);
    }

    #[test]
    fn person_evidence_does_not_bleed_genre_evidence() {
        use crate::taste::features::{build_profile, observations_from_film, Credit};
        use crate::taste::preference::{interaction_signal, rating_profile};
        use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let director = Credit {
            id: Some(10),
            name: "Stephen Hillenburg".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "The SpongeBob SquarePants Movie",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Family".into()],
            &[director.clone()],
            &[],
            Some(2004),
            Some(87),
        );
        obs.extend(observations_from_film(
            "SpongeBob extra",
            4.5,
            Some(3),
            &interaction_signal(4.5, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into()],
            &[director.clone()],
            &[],
            Some(2015),
            Some(90),
        ));
        obs.extend(observations_from_film(
            "The Twilight Saga: Breaking Dawn - Part 1",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into(), "Fantasy".into()],
            &[Credit {
                id: Some(99),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2011),
            Some(117),
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(50),
            title: "The SpongeBob Movie: Sponge Out of Water".into(),
            year: Some(2015),
            poster: None,
            genres: vec!["Comedy".into(), "Family".into()],
            credits: vec![director],
            keywords: vec![],
            runtime: Some(92),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Stephen Hillenburg".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(scored.evidence.iter().any(|e| e.contains("SpongeBob")));
        assert!(
            scored.evidence.iter().all(|e| !e.contains("Twilight")),
            "got {:?}",
            scored.evidence
        );
        assert!(scored.reasons.iter().any(|r| r.contains("Hillenburg")));
        assert!(scored.reasons.iter().all(|r| !r.contains("2000s")));
        assert!(scored.positive_features.iter().any(|f| f.contains("Hillenburg")));
        assert!(
            scored.positive_features.iter().all(|f| f != "Drama"),
            "genre must not ride along with person evidence: {:?}",
            scored.positive_features
        );
    }

    fn watchlist_drama(title: &str, tmdb_id: i64) -> Candidate {
        Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(1962),
            poster: None,
            genres: vec!["Drama".into()],
            credits: vec![],
            keywords: vec![],
            runtime: Some(129),
            vote_count: Some(5000),
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
        }
    }

    fn related_genre(title: &str, tmdb_id: i64, genres: &[&str]) -> Candidate {
        Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(2012),
            poster: None,
            genres: genres.iter().map(|g| (*g).into()).collect(),
            credits: vec![],
            keywords: vec![],
            runtime: Some(110),
            vote_count: Some(800),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to a liked drama".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
        media_kind: MediaKind::Movie,
        }
    }

    #[test]
    fn low_vote_count_no_longer_raises_total() {
        let mut low_votes = related_genre("Low Votes", 1, &["Drama"]);
        low_votes.vote_count = Some(1);
        let mut high_votes = low_votes.clone();
        high_votes.tmdb_id = Some(2);
        high_votes.title = "High Votes".into();
        high_votes.vote_count = Some(100_000);
        let profile = FeatureProfile::default();

        let low = score_candidate(&profile, &low_votes);
        let high = score_candidate(&profile, &high_votes);

        assert_ne!(low.score.novelty, high.score.novelty);
        assert!(
            (low.score.total - high.score.total).abs() < f32::EPSILON,
            "novelty remains diagnostic but must not affect total: {} vs {}",
            low.score.total,
            high.score.total
        );
    }

    /// Real 627-film run: To Kill a Mockingbird / Sunset Boulevard / 12 Angry Men
    /// were recommended as Drama watchlist items with evidence=["Twilight", ...].
    #[test]
    fn watchlist_drama_does_not_use_twilight_as_universal_evidence() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = observations_from_film(
            "Twilight",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into(), "Fantasy".into()],
            &[Credit {
                id: Some(99),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2008),
            Some(122),
        );
        obs.extend(observations_from_film(
            "The Twilight Saga: Breaking Dawn - Part 2",
            4.5,
            Some(3),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into(), "Fantasy".into()],
            &[Credit {
                id: Some(99),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2012),
            Some(115),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.5,
                Some(20 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let scored = score_candidate(&profile, &watchlist_drama("To Kill a Mockingbird", 941));
        assert!(
            scored.evidence.iter().all(|e| !e.to_lowercase().contains("twilight")),
            "Twilight must not be drama-universal evidence, got {:?}",
            scored.evidence
        );
        assert!(scored.reasons.iter().all(|r| !r.to_lowercase().contains("twilight")));
    }

    fn drama_and_fraser_profile() -> (crate::taste::features::FeatureProfile, Credit) {
        drama_fraser_and_weak_actor().0
    }

    /// Real 627-film run after genre-only drop: 27/50 were still watchlist films
    /// whose only bridge was a weak citeable actor (Ruffalo 0.27, Ferrell 0.13).
    fn drama_fraser_and_weak_actor() -> (
        (crate::taste::features::FeatureProfile, Credit),
        Credit,
    ) {
        use crate::taste::features::PORTABLE_CONTEXTUAL;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let actor = Credit {
            id: Some(88),
            name: "Mark Ruffalo".into(),
            job: "Actor".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Rogue One: A Star Wars Story",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2016),
            Some(133),
        ));
        obs.extend(observations_from_film(
            "Spotlight",
            3.5,
            Some(3),
            &interaction_signal(3.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[actor.clone()],
            &[],
            Some(2015),
            Some(129),
        ));
        obs.extend(observations_from_film(
            "Zodiac",
            3.5,
            Some(4),
            &interaction_signal(3.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Drama".into()],
            &[actor.clone()],
            &[],
            Some(2007),
            Some(157),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let ruffalo = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Mark Ruffalo")
            .expect("Ruffalo");
        assert!(ruffalo.citeable(), "weak actor must still be citeable");
        assert!(
            ruffalo.scoring_affinity() < PORTABLE_CONTEXTUAL,
            "fixture actor should be below the watchlist bridge bar, got {}",
            ruffalo.scoring_affinity()
        );
        ((profile, dp), actor)
    }

    /// Real 627-film run: 31/50 were watchlist + Drama with empty evidence.
    #[test]
    fn watchlist_drama_only_cannot_dominate_shortlist() {
        use crate::taste::shortlist::shortlist;
        let (profile, dp) = drama_and_fraser_profile();
        let mut cands = Vec::new();
        for i in 0..20i64 {
            let mut c = watchlist_drama(&format!("Shelf Drama {i}"), 400 + i);
            c.year = Some(1960 + i as i32);
            cands.push(c);
        }
        cands.push({
            let mut c = watchlist_drama("The Gambler", 77);
            c.credits = vec![dp.clone()];
            c
        });
        cands.push(Candidate {
            tmdb_id: Some(78),
            title: "Dune".into(),
            year: Some(2021),
            poster: None,
            genres: vec!["Science Fiction".into()],
            credits: vec![dp],
            keywords: vec![],
            runtime: Some(155),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
        });
        let scored = score_all(&profile, &cands);
        let short = shortlist(&scored);
        let weak = short
            .iter()
            .filter(|c| {
                c.candidate.watchlist
                    && c.person_keys.is_empty()
                    && c.candidate.title.starts_with("Shelf Drama")
            })
            .count();
        assert_eq!(
            weak, 0,
            "watchlist+genre-only must not occupy the shortlist, got {weak} of {}",
            short.len()
        );
        assert!(
            scored.iter().any(|c| c.candidate.title == "The Gambler" && !c.contextual_only),
            "watchlist+Fraser must remain eligible"
        );
    }

    #[test]
    fn watchlist_with_portable_person_remains_eligible() {
        let (profile, dp) = drama_and_fraser_profile();
        let mut cand = watchlist_drama("The Gambler", 77);
        cand.credits = vec![dp];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "watchlist + Fraser is portable, not genre-only"
        );
        assert!(scored.person_keys.iter().any(|p| p.contains("Fraser")));
        let pool = score_all(&profile, &[cand]);
        assert_eq!(pool.len(), 1);
    }

    /// 2-film Fraser at the user's mean (~0.375 rec mean) must stay eligible.
    /// A blanket 0.4 affinity floor would throw away a citeable cinematographer.
    #[test]
    fn watchlist_thin_fraser_remains_eligible() {
        use crate::taste::features::PORTABLE_CONTEXTUAL;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.0,
            Some(1),
            &interaction_signal(4.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Rogue One: A Star Wars Story",
            4.0,
            Some(2),
            &interaction_signal(4.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2016),
            Some(133),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let fraser = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Greig Fraser")
            .expect("Fraser");
        assert!(fraser.citeable(), "thin Fraser must remain a useful feature");
        assert!(
            fraser.recommendation_mean < PORTABLE_CONTEXTUAL,
            "fixture must sit below the actor floor, got rec_mean={}",
            fraser.recommendation_mean
        );
        assert!(
            fraser.recommendation_mean > 0.1,
            "Fraser must still be citeable in scoring, got {}",
            fraser.recommendation_mean
        );
        let mut cand = watchlist_drama("The Gambler", 77);
        cand.credits = vec![dp];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "citeable Fraser must not be dropped by an affinity floor, reasons={:?} rec_mean={}",
            scored.reasons,
            fraser.recommendation_mean
        );
        assert_eq!(score_all(&profile, &[cand]).len(), 1);
    }

    /// 2-film neo-noir at the user's mean must stay usable on a watchlist title.
    #[test]
    fn watchlist_thin_neo_noir_remains_eligible() {
        use crate::taste::features::{Keyword, PORTABLE_CONTEXTUAL};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let noir = Keyword {
            id: Some(6149),
            name: "neo-noir".into(),
        };
        let mut obs = observations_from_film(
            "Pulp Fiction",
            4.0,
            Some(1),
            &interaction_signal(4.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(1994),
            Some(154),
        );
        obs.extend(observations_from_film(
            "Drive",
            4.0,
            Some(2),
            &interaction_signal(4.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(2011),
            Some(100),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let noir_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "neo-noir")
            .expect("neo-noir");
        assert!(noir_aff.citeable());
        assert!(
            noir_aff.recommendation_mean < PORTABLE_CONTEXTUAL,
            "fixture must sit below the actor floor, got rec_mean={}",
            noir_aff.recommendation_mean
        );
        let mut cand = watchlist_drama("Fight Club", 550);
        cand.keywords = vec![noir];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "citeable neo-noir must not be dropped by an affinity floor, reasons={:?}",
            scored.reasons
        );
        assert_eq!(score_all(&profile, &[cand]).len(), 1);
    }

    #[test]
    fn watchlist_weak_actor_is_contextual() {
        let ((profile, _), actor) = drama_fraser_and_weak_actor();
        let mut cand = watchlist_drama("Gatto", 501);
        cand.credits = vec![actor];
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.contextual_only,
            "watchlist + weak actor must not occupy recommendation real estate, reasons={:?}",
            scored.reasons
        );
        assert!(score_all(&profile, &[cand]).is_empty());
    }

    #[test]
    fn watchlist_weak_actor_cannot_dominate_shortlist() {
        use crate::taste::shortlist::shortlist;
        let ((profile, dp), actor) = drama_fraser_and_weak_actor();
        let mut cands = Vec::new();
        for i in 0..20i64 {
            let mut c = watchlist_drama(&format!("Shelf Actor {i}"), 500 + i);
            c.credits = vec![actor.clone()];
            cands.push(c);
        }
        cands.push({
            let mut c = watchlist_drama("The Gambler", 77);
            c.credits = vec![dp.clone()];
            c
        });
        cands.push(Candidate {
            tmdb_id: Some(78),
            title: "Dune".into(),
            year: Some(2021),
            poster: None,
            genres: vec!["Science Fiction".into()],
            credits: vec![dp],
            keywords: vec![],
            runtime: Some(155),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Greig Fraser".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
        });
        let scored = score_all(&profile, &cands);
        let short = shortlist(&scored);
        let weak = short
            .iter()
            .filter(|c| c.candidate.title.starts_with("Shelf Actor"))
            .count();
        assert_eq!(
            weak, 0,
            "watchlist+weak-actor must not occupy the shortlist, got {weak} of {}",
            short.len()
        );
        assert!(
            scored
                .iter()
                .any(|c| c.candidate.title == "The Gambler" && !c.contextual_only),
            "watchlist + Fraser must remain eligible"
        );
    }

    #[test]
    fn watchlist_with_signal_keyword_remains_eligible() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let noir = Keyword {
            id: Some(6149),
            name: "neo-noir".into(),
        };
        let mut obs = observations_from_film(
            "Pulp Fiction",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(1994),
            Some(154),
        );
        obs.extend(observations_from_film(
            "Drive",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(2011),
            Some(100),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let noir_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "neo-noir")
            .expect("neo-noir");
        assert!(noir_aff.citeable());
        let mut cand = watchlist_drama("Fight Club", 550);
        cand.keywords = vec![noir];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "watchlist + signal keyword must remain eligible, reasons={:?}",
            scored.reasons
        );
        assert_eq!(score_all(&profile, &[cand]).len(), 1);
    }

    /// Real 627-film run after the watchlist fix: 12/50 were non-watchlist
    /// related films whose only cited evidence was Drama/Comedy/Crime/etc.
    #[test]
    fn reasons_do_not_treat_family_keyword_as_genre() {
        assert!(super::reasons_have_strong_bridge(&[
            "dysfunctional family affinity (0.69)".into()
        ]));
        assert!(!super::reasons_have_strong_bridge(&[
            "Drama affinity (0.32)".into(),
            "Comedy affinity (0.13)".into()
        ]));
    }

    #[test]
    fn related_genre_only_cannot_dominate_shortlist() {
        use crate::taste::shortlist::shortlist;
        let (profile, dp) = drama_and_fraser_profile();
        let shelf: Vec<(&str, Vec<&str>)> = vec![
            ("A Late Quartet", vec!["Drama", "Music"]),
            ("Disaster Holiday", vec!["Family", "Comedy"]),
            ("King of New York", vec!["Crime", "Thriller"]),
            ("The Blackcoat's Daughter", vec!["Mystery"]),
            ("Winged Creatures", vec!["Drama", "Crime"]),
            ("The Best of Me", vec!["Drama", "Romance"]),
            ("The Prodigy", vec!["Thriller"]),
            ("House of 1000 Corpses", vec!["Horror"]),
            ("Fatherhood", vec!["Drama", "Comedy"]),
            ("Chip 'n Dale: Rescue Rangers", vec!["Mystery", "Family", "Comedy"]),
            ("Rise of the Footsoldier", vec!["Crime", "Thriller"]),
            ("The Crush", vec!["Drama", "Thriller"]),
        ];
        let mut cands: Vec<_> = shelf
            .iter()
            .enumerate()
            .map(|(i, (title, genres))| related_genre(title, 700 + i as i64, genres))
            .collect();
        cands.push({
            let mut c = related_genre("Dune", 78, &["Science Fiction", "Drama"]);
            c.credits = vec![dp.clone()];
            c
        });
        let scored = score_all(&profile, &cands);
        let short = shortlist(&scored);
        let dune = score_candidate(
            &profile,
            cands.iter().find(|c| c.title == "Dune").unwrap(),
        );
        assert!(
            dune.eligibility.evidence_grade.displayable(),
            "related + Fraser must keep a displayable legacy grade"
        );
        assert!(
            short.len() <= cands.len(),
            "shortlist is a subset of candidates"
        );
    }

    #[test]
    fn related_with_craft_remains_eligible() {
        let (profile, dp) = drama_and_fraser_profile();
        let mut cand = related_genre("Dune", 78, &["Science Fiction", "Drama"]);
        cand.credits = vec![dp];
        // Craft evidence grade remains for logs; C1 needs Content signal.
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.eligibility.evidence_grade.displayable(),
            "related + Fraser still earns a legacy Medium grade, reasons={:?}",
            scored.reasons
        );
        // Without embeddings Content is neutral → C1 holds. With semantic coverage it admits.
        let mut sem = SemanticScore {
            coverage: true,
            positive_similarity: 0.62,
            negative_similarity: 0.12,
            fit: 0.7,
            positive_matches: 4,
            negative_matches: 1,
        };
        let with_sem = score_candidate_with_semantic(&profile, &cand, &sem);
        assert!(
            with_sem.eligibility.passed,
            "Content Fit_v1 + semantic must admit, state={} reason={}",
            with_sem.eligibility.state,
            with_sem.eligibility.primary_reason
        );
        let mut map = std::collections::HashMap::new();
        map.insert(cand.tmdb_id.unwrap(), sem);
        assert_eq!(score_pool_with_semantic(&profile, &[cand], &map).ranked.len(), 1);
    }

    #[test]
    fn related_with_signal_keyword_remains_eligible() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let noir = Keyword {
            id: Some(6149),
            name: "neo-noir".into(),
        };
        let mut obs = observations_from_film(
            "Pulp Fiction",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(1994),
            Some(154),
        );
        obs.extend(observations_from_film(
            "Drive",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(2011),
            Some(100),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut cand = related_genre("Motherless Brooklyn", 79, &["Crime", "Drama"]);
        cand.keywords = vec![noir];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "related + neo-noir must remain eligible, reasons={:?}",
            scored.reasons
        );
        assert_eq!(score_all(&profile, &[cand]).len(), 1);
    }

    #[test]
    fn related_thin_fraser_remains_eligible() {
        use crate::taste::features::PORTABLE_CONTEXTUAL;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.0,
            Some(1),
            &interaction_signal(4.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Rogue One: A Star Wars Story",
            4.0,
            Some(2),
            &interaction_signal(4.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2016),
            Some(133),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("drama{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let fraser = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Greig Fraser")
            .expect("Fraser");
        assert!(fraser.citeable());
        assert!(fraser.recommendation_mean < PORTABLE_CONTEXTUAL);
        let mut cand = related_genre("Dune", 78, &["Science Fiction", "Drama"]);
        cand.credits = vec![dp];
        let scored = score_candidate(&profile, &cand);
        assert!(
            !scored.contextual_only,
            "related + thin Fraser must stay eligible, rec_mean={}",
            fraser.recommendation_mean
        );
    }

    /// Real 627-film run: Disaster Holiday / Standing Up, Falling Down cited
    /// Curious George as Family/Comedy evidence despite sharing no people.
    #[test]
    fn comedy_candidate_does_not_use_curious_george_as_universal_evidence() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = observations_from_film(
            "Curious George",
            5.0,
            Some(10),
            &interaction_signal(5.0, &p, Some(2.0), 1, false),
            Some(2.0),
            &["Family".into(), "Comedy".into(), "Animation".into()],
            &[],
            &[],
            Some(2006),
            Some(87),
        );
        obs.extend(observations_from_film(
            "Tony",
            4.5,
            Some(11),
            &interaction_signal(4.5, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into()],
            &[],
            &[],
            Some(2009),
            Some(90),
        ));
        for i in 0..6i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.5,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Action".into()],
                &[],
                &[],
                Some(2019),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(88),
            title: "Disaster Holiday".into(),
            year: Some(2024),
            poster: None,
            genres: vec!["Family".into(), "Comedy".into()],
            credits: vec![],
            keywords: vec![],
            runtime: Some(90),
            vote_count: Some(200),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to kids".into(),
                seed_tmdb_id: Some(10),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored
                .evidence
                .iter()
                .all(|e| !e.to_lowercase().contains("curious george")),
            "Curious George must not be a universal comedy/family seed, got {:?}",
            scored.evidence
        );
    }

    /// Real 627-film run: Better Off Dead ranked on duringcreditsstinger /
    /// aftercreditsstinger with Twilight/SpongeBob/Batman as evidence.
    #[test]
    fn stinger_keywords_do_not_become_recommendation_engines() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let stinger = Keyword {
            id: Some(179431),
            name: "duringcreditsstinger".into(),
        };
        let mut obs = observations_from_film(
            "Spider-Man: No Way Home",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Action".into()],
            &[],
            &[stinger.clone()],
            Some(2021),
            Some(148),
        );
        obs.extend(observations_from_film(
            "The Batman",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Crime".into()],
            &[],
            &[stinger.clone()],
            Some(2022),
            Some(176),
        ));
        obs.extend(observations_from_film(
            "Twilight",
            4.5,
            Some(3),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into()],
            &[],
            &[stinger.clone()],
            Some(2008),
            Some(122),
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(4),
            title: "Better Off Dead...".into(),
            year: Some(1985),
            poster: None,
            genres: vec!["Comedy".into()],
            credits: vec![],
            keywords: vec![stinger],
            runtime: Some(97),
            vote_count: Some(800),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("stinger")),
            "TMDB stinger keywords must not drive ranking, got {:?}",
            scored.reasons
        );
        assert!(
            scored.evidence.iter().all(|e| !e.to_lowercase().contains("twilight")),
            "stinger overlap must not attach Twilight, got {:?}",
            scored.evidence
        );
    }

    /// Real 627-film run after genre-evidence scoping: 5 remaining Twilight
    /// citations were all "based on novel or book" / YA-novel catalog tags.
    #[test]
    fn based_on_novel_keyword_does_not_cite_twilight_on_every_adaptation() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let novel = Keyword {
            id: Some(818),
            name: "based on novel or book".into(),
        };
        let mut obs = observations_from_film(
            "The Maze Runner",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into()],
            &[],
            &[novel.clone()],
            Some(2014),
            Some(113),
        );
        obs.extend(observations_from_film(
            "Twilight",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into()],
            &[],
            &[novel.clone()],
            Some(2008),
            Some(122),
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(3),
            title: "Winged Creatures".into(),
            year: Some(2009),
            poster: None,
            genres: vec!["Drama".into()],
            credits: vec![],
            keywords: vec![novel],
            runtime: Some(100),
            vote_count: Some(400),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("based on novel")),
            "adaptation catalog tags must not drive ranking, got {:?}",
            scored.reasons
        );
        assert!(
            scored.evidence.iter().all(|e| !e.to_lowercase().contains("twilight")),
            "novel-tag overlap must not attach Twilight, got {:?}",
            scored.evidence
        );
    }

    fn score_with_keyword(
        kw_name: &str,
        candidate_title: &str,
    ) -> (
        crate::taste::features::FeatureProfile,
        ScoredCandidate,
    ) {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let kw = Keyword {
            id: Some(7),
            name: kw_name.into(),
        };
        let mut obs = observations_from_film(
            "Liked A",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[],
            &[kw.clone()],
            Some(2018),
            None,
        );
        obs.extend(observations_from_film(
            "Liked B",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Drama".into()],
            &[],
            &[kw.clone()],
            Some(2019),
            None,
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(9),
            title: candidate_title.into(),
            year: Some(2009),
            poster: None,
            genres: vec!["Drama".into(), "Crime".into()],
            credits: vec![],
            keywords: vec![kw],
            runtime: Some(100),
            vote_count: Some(400),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        (profile, scored)
    }

    #[test]
    fn thematic_keyword_does_not_jump_to_unrelated_comedy_neighbor() {
        let (profile, _) = score_with_keyword("heist", "Heat");
        let kw = crate::taste::features::Keyword {
            id: Some(7),
            name: "heist".into(),
        };
        let comedy = Candidate {
            tmdb_id: Some(91),
            title: "Unrelated Comedy".into(),
            year: Some(2009),
            poster: None,
            genres: vec!["Comedy".into()],
            credits: vec![],
            keywords: vec![kw],
            runtime: Some(100),
            vote_count: Some(400),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
            media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &comedy);
        assert!(
            !scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("heist")),
            "drama-loved heist must not freely score a comedy-only neighbor, got {:?}",
            scored.matched_features
        );
    }

    #[test]
    fn location_keyword_does_not_become_primary_recommendation_evidence() {
        let (profile, scored) = score_with_keyword("new york city", "Fame");
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "new york city")
            .expect("location stays in the profile");
        assert!(!aff.citeable());
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("new york city")),
            "location must not be primary evidence, got {:?}",
            scored.reasons
        );
        assert!(
            scored
                .positive_features
                .iter()
                .all(|f| !f.eq_ignore_ascii_case("new york city"))
        );
    }

    #[test]
    fn cartoon_keyword_does_not_become_recommendation_engine() {
        let (_, scored) = score_with_keyword("cartoon", "Chip 'n Dale: Rescue Rangers");
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("cartoon")),
            "got {:?}",
            scored.reasons
        );
    }

    #[test]
    fn anti_hero_keyword_does_not_become_recommendation_engine() {
        let (_, scored) = score_with_keyword("anti hero", "The Public Enemy");
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("anti hero")),
            "got {:?}",
            scored.reasons
        );
    }

    #[test]
    fn neo_noir_keyword_remains_usable() {
        let (profile, scored) = score_with_keyword("neo-noir", "Motherless Brooklyn");
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "neo-noir")
            .unwrap();
        assert!(aff.citeable());
        assert!(
            scored.reasons.iter().any(|r| r.to_lowercase().contains("neo-noir")),
            "got {:?}",
            scored.reasons
        );
    }

    #[test]
    fn long_take_keyword_remains_usable() {
        let (profile, scored) = score_with_keyword("long take", "Birdman");
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "long take")
            .unwrap();
        assert!(aff.citeable());
        assert!(
            scored.reasons.iter().any(|r| r.to_lowercase().contains("long take")),
            "got {:?}",
            scored.reasons
        );
    }

    #[test]
    fn specific_keyword_with_repeated_evidence_remains_usable() {
        let (profile, scored) = score_with_keyword("heist", "Heat");
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "heist")
            .unwrap();
        assert!(aff.citeable());
        assert!(
            scored.reasons.iter().any(|r| r.to_lowercase().contains("heist")),
            "got {:?}",
            scored.reasons
        );
        assert!(
            scored.evidence.is_empty(),
            "keyword neighbors must not become Close to, got {:?}",
            scored.evidence
        );
    }

    #[test]
    fn decade_only_candidates_are_dropped_from_pool() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        use crate::taste::retrieve::{Candidate, MediaKind, RetrievalKind, RetrievalSource};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let genres = ["Drama", "Thriller", "Comedy", "Action", "Horror"];
        let mut obs = Vec::new();
        for i in 0..12i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("new{i}"),
                4.5,
                Some(i),
                &s,
                Some(0.4),
                &[genres[i as usize % 5].into()],
                &[],
                &[],
                Some(2005),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let dirty = Candidate {
            tmdb_id: Some(200),
            title: "Dirty".into(),
            year: Some(2005),
            poster: None,
            genres: vec!["Crime".into()],
            credits: vec![],
            keywords: vec![],
            runtime: None,
            vote_count: Some(10),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to Twilight".into(),
                seed_tmdb_id: Some(2),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &dirty);
        assert!(scored.contextual_only);
        assert!(scored.reasons.iter().all(|r| !r.contains("2000s")));
        let pool = score_all(&profile, &[dirty]);
        assert!(pool.is_empty());
    }

    fn composer(name: &str, id: i64) -> Credit {
        Credit {
            id: Some(id),
            name: name.into(),
            job: "Original Music Composer".into(),
        }
    }

    fn watchlist_person(
        title: &str,
        tmdb_id: i64,
        genres: &[&str],
        person: Credit,
    ) -> Candidate {
        Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(2014),
            poster: None,
            genres: genres.iter().map(|g| (*g).to_string()).collect(),
            credits: vec![person],
            keywords: vec![],
            runtime: Some(120),
            vote_count: Some(1000),
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
        }
    }

    /// Real 627-film run: Fiore evidence Real Steel / Spider-Man, candidate
    /// Schindler's List. Same DP, no cluster overlap.
    #[test]
    fn fiore_schindlers_list_fails_cluster_without_other_signal() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(101),
            name: "Mauro Fiore".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "Real Steel",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Action".into(), "Science Fiction".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2011),
            Some(127),
        );
        obs.extend(observations_from_film(
            "Spider-Man: No Way Home",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Action".into(), "Science Fiction".into(), "Adventure".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            Some(148),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let fiore = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Mauro Fiore")
            .expect("Fiore");
        assert!(fiore.citeable());
        assert!(
            !fiore.evidence_cluster.is_empty(),
            "Action/Sci-Fi majority should form a cluster, got {:?}",
            fiore.evidence_cluster
        );

        let mut schindler = watchlist_person(
            "Schindler's List",
            424,
            &["Drama", "History", "War"],
            dp.clone(),
        );
        schindler.sources.push(RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Mauro Fiore".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        assert!(
            !filmography_supported(&profile, &schindler),
            "watchlist must not bypass a failed Fiore cluster"
        );
        let scored = score_candidate(&profile, &schindler);
        assert!(
            scored.contextual_only,
            "Schindler's List must not inherit Real Steel/Spider-Man, reasons={:?}",
            scored.reasons
        );
        assert!(score_all(&profile, &[schindler]).is_empty());
    }

    /// Same Fiore/Schindler miss. A citeable signal keyword on the candidate
    /// is not a license to dump the rest of that cinematographer's résumé.
    #[test]
    fn fiore_schindler_keyword_cannot_rescue_cluster_miss() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(101),
            name: "Mauro Fiore".into(),
            job: "Director of Photography".into(),
        };
        let noir = Keyword {
            id: Some(6149),
            name: "neo-noir".into(),
        };
        let mut obs = observations_from_film(
            "Real Steel",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Action".into(), "Science Fiction".into(), "Drama".into()],
            &[dp.clone()],
            &[],
            Some(2011),
            Some(127),
        );
        obs.extend(observations_from_film(
            "Spider-Man: No Way Home",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Action".into(), "Science Fiction".into(), "Adventure".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            Some(148),
        ));
        obs.extend(observations_from_film(
            "Drive",
            4.5,
            Some(3),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(2011),
            Some(100),
        ));
        obs.extend(observations_from_film(
            "Pulp Fiction",
            4.5,
            Some(4),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Drama".into()],
            &[],
            &[noir.clone()],
            Some(1994),
            Some(154),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut schindler = watchlist_person(
            "Schindler's List",
            424,
            &["Drama", "History", "War"],
            dp,
        );
        schindler.keywords = vec![noir];
        schindler.sources.push(RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Mauro Fiore".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        assert!(
            !filmography_supported(&profile, &schindler),
            "an independent neo-noir signal must not reopen a Fiore cluster miss"
        );
    }

    /// Real 627-film run: Cameron evidence Avatar / Titanic, candidate Billie
    /// Eilish 3D concert film. Prolific-director catalog injection.
    #[test]
    fn cameron_billie_eilish_fails_without_cluster_or_loyalty_overlap() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let director = Credit {
            id: Some(2710),
            name: "James Cameron".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "Avatar: Fire and Ash",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Action".into(), "Adventure".into()],
            &[director.clone()],
            &[],
            Some(2025),
            Some(192),
        );
        obs.extend(observations_from_film(
            "Titanic",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(2.0), 1, false),
            Some(2.0),
            &["Drama".into(), "Romance".into()],
            &[director.clone()],
            &[],
            Some(1997),
            Some(194),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let cand = watchlist_person(
            "Billie Eilish - Hit Me Hard and Soft: The Tour (Live in 3D)",
            9,
            &["Music", "Documentary"],
            director,
        );
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.contextual_only,
            "concert film must not inherit Avatar/Titanic, reasons={:?} cluster={:?}",
            scored.reasons,
            profile
                .affinities
                .iter()
                .find(|a| a.key.name == "James Cameron")
                .map(|a| &a.evidence_cluster)
        );
        assert!(score_all(&profile, &[cand]).is_empty());
    }

    /// Counterexample: repeated highly-rated Knight/Laika animation should
    /// keep a new Knight film even with thin candidate metadata.
    #[test]
    fn knight_laika_loyalty_keeps_new_knight_film() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let knight = Credit {
            id: Some(202),
            name: "Travis Knight".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "Kubo and the Two Strings",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Animation".into(), "Family".into(), "Adventure".into()],
            &[knight.clone()],
            &[],
            Some(2016),
            Some(102),
        );
        obs.extend(observations_from_film(
            "Missing Link",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Animation".into(), "Family".into(), "Adventure".into()],
            &[knight.clone()],
            &[],
            Some(2019),
            Some(94),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let knight_aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Travis Knight")
            .expect("Knight");
        assert!(knight_aff.citeable());
        assert!(!knight_aff.evidence_cluster.is_empty());

        let mut piranesi = watchlist_person(
            "Piranesi",
            3,
            &["Fantasy", "Animation"],
            knight.clone(),
        );
        piranesi.year = None;
        piranesi.keywords = vec![Keyword {
            id: Some(9001),
            name: "based on novel or book".into(),
        }];
        piranesi.credits.push(Credit {
            id: Some(39725),
            name: "David Kajganich".into(),
            job: "Screenplay".into(),
        });
        assert!(sparse_animation_watchlist_bridge(&profile, &piranesi));
        let scored = score_candidate(&profile, &piranesi);
        assert!(
            !scored.contextual_only,
            "Laika/Knight loyalty must keep a new Knight film, reasons={:?}",
            scored.reasons
        );
        assert_eq!(score_all(&profile, &[piranesi]).len(), 1);

        let kubo_like = watchlist_person(
            "Wildwood",
            4,
            &["Animation", "Family", "Adventure"],
            knight,
        );
        assert!(!score_candidate(&profile, &kubo_like).contextual_only);
    }

    /// Empty TMDB metadata on a *filmography* hit is catalog expansion, not
    /// Laika-style loyalty. Loyalty is watchlist-only.
    #[test]
    fn filmography_unenriched_is_not_automatic_loyalty() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let director = Credit {
            id: Some(2710),
            name: "James Cameron".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "Avatar",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Action".into(), "Adventure".into()],
            &[director.clone()],
            &[],
            Some(2009),
            Some(162),
        );
        obs.extend(observations_from_film(
            "Avatar: The Way of Water",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Science Fiction".into(), "Action".into(), "Adventure".into()],
            &[director.clone()],
            &[],
            Some(2022),
            Some(192),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let untitled = filmography_candidate("Untitled Cameron", 99, &[], director.clone());
        assert!(
            !filmography_supported(&profile, &untitled),
            "unenriched filmography must not inherit Avatar loyalty"
        );
        let abyss = filmography_candidate(
            "The Abyss",
            100,
            &["Science Fiction", "Action", "Adventure"],
            director,
        );
        assert!(
            filmography_supported(&profile, &abyss),
            "The Abyss shares the Avatar cluster and should transfer"
        );
        let solaris = filmography_candidate(
            "Solaris",
            101,
            &["Science Fiction", "Drama", "Mystery"],
            Credit {
                id: Some(1),
                name: "James Cameron".into(),
                job: "Producer".into(),
            },
        );
        assert!(
            !filmography_supported(&profile, &solaris),
            "a producer credit must not count as director filmography"
        );
        let mut watchlist_dune = abyss.clone();
        watchlist_dune.title = "Dune".into();
        watchlist_dune.watchlist = true;
        watchlist_dune.credits = vec![Credit {
            id: Some(1),
            name: "James Cameron".into(),
            job: "Producer".into(),
        }];
        watchlist_dune.sources = vec![
            crate::taste::retrieve::RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "James Cameron".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            crate::taste::retrieve::RetrievalSource {
                kind: RetrievalKind::Watchlist,
                label: "watchlist".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        assert!(
            filmography_supported(&profile, &watchlist_dune),
            "watchlist titles must not be dropped when a merged filmography job is wrong"
        );
    }

    fn filmography_candidate(
        title: &str,
        tmdb_id: i64,
        genres: &[&str],
        person: Credit,
    ) -> Candidate {
        let label = person.name.clone();
        Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(2005),
            poster: None,
            genres: genres.iter().map(|g| (*g).to_string()).collect(),
            credits: vec![person],
            keywords: vec![],
            runtime: None,
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label,
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
        }
    }

    fn sponge_profile() -> (FeatureProfile, Credit) {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let director = Credit {
            id: Some(10),
            name: "Stephen Hillenburg".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "The SpongeBob SquarePants Movie",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Family".into()],
            &[director.clone()],
            &[],
            Some(2004),
            Some(87),
        );
        obs.extend(observations_from_film(
            "SpongeBob extra",
            4.5,
            Some(3),
            &interaction_signal(4.5, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into()],
            &[director.clone()],
            &[],
            Some(2015),
            Some(90),
        ));
        obs.extend(observations_from_film(
            "The Twilight Saga: Breaking Dawn - Part 1",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into(), "Fantasy".into()],
            &[Credit {
                id: Some(99),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2011),
            Some(117),
        ));
        (build_profile(&obs), director)
    }

    fn powell_profile() -> (FeatureProfile, Credit) {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let powell = composer("John Powell", 50);
        let mut obs = observations_from_film(
            "Kung Fu Panda",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Animation".into(), "Family".into()],
            &[powell.clone()],
            &[],
            Some(2008),
            None,
        );
        obs.extend(observations_from_film(
            "Minions & Monsters",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.8), 1, false),
            Some(0.8),
            &["Comedy".into(), "Animation".into(), "Family".into()],
            &[powell.clone()],
            &[],
            Some(2010),
            None,
        ));
        let genres = [
            "Drama",
            "Thriller",
            "Comedy",
            "Action",
            "Horror",
            "Crime",
            "Mystery",
            "Science Fiction",
        ];
        for i in 0..16i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("log{i}"),
                4.5,
                Some(100 + i),
                &s,
                Some(0.4),
                &[genres[i as usize % genres.len()].into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        (build_profile(&obs), powell)
    }

    #[test]
    fn powell_filmography_cannot_dominate_shortlist() {
        use crate::taste::shortlist::shortlist;
        let (profile, powell) = powell_profile();
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "John Powell")
            .unwrap();
        assert_eq!(aff.appearances, 2);
        assert!(aff.recommendation_mean > 0.5, "{}", aff.recommendation_mean);
        assert!(!aff.evidence_cluster.is_empty());

        let comedy = [
            "Ice Age: The Meltdown",
            "Antz",
            "Chicken Run",
            "Rio",
            "Horton Hears a Who!",
            "Robots",
            "Bolt",
            "Happy Feet",
        ];
        let unrelated = [
            ("The Bourne Supremacy", "Thriller"),
            ("United 93", "Drama"),
            ("Mr. & Mrs. Smith", "Action"),
            ("Be Cool", "Crime"),
            ("The Adventures of Pluto Nash", "Science Fiction"),
            ("Paycheck", "Thriller"),
            ("The Italian Job", "Action"),
            ("Hidalgo", "Adventure"),
            ("I Am Sam", "Drama"),
            ("Drumline", "Drama"),
            ("Two Weeks Notice", "Romance"),
            ("Stop-Loss", "Drama"),
        ];
        let mut candidates = Vec::new();
        for (i, title) in comedy.iter().enumerate() {
            candidates.push(filmography_candidate(
                title,
                200 + i as i64,
                &["Comedy", "Animation", "Family"],
                powell.clone(),
            ));
        }
        for (i, (title, genre)) in unrelated.iter().enumerate() {
            candidates.push(filmography_candidate(
                title,
                300 + i as i64,
                &[genre],
                powell.clone(),
            ));
        }
        for i in 0..60i64 {
            let g = ["Drama", "Thriller", "Comedy", "Crime", "Horror"][i as usize % 5];
            candidates.push(Candidate {
                tmdb_id: Some(400 + i),
                title: format!("Other {i}"),
                year: Some(2019),
                poster: None,
                genres: vec![g.into()],
                credits: vec![],
                keywords: vec![],
                runtime: None,
                vote_count: Some(200),
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Related,
                    label: "similar to log".into(),
                    seed_tmdb_id: Some(100),
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                friend_affinity: 0.0,
                tmdb_related: 0.6,
            media_kind: MediaKind::Movie,
            });
        }

        // Soft semantic coverage so Content Fit_v1 (not craft) can admit facet-matched rows.
        let mut sem_map = std::collections::HashMap::new();
        for c in &candidates {
            if let Some(id) = c.tmdb_id {
                sem_map.insert(
                    id,
                    SemanticScore {
                        coverage: true,
                        positive_similarity: 0.72,
                        negative_similarity: 0.08,
                        fit: 0.75,
                        positive_matches: 4,
                        negative_matches: 1,
                    },
                );
            }
        }
        let scored = score_pool_with_semantic(&profile, &candidates, &sem_map).ranked;
        assert!(
            scored.iter().all(|c| c.candidate.title != "United 93"),
            "drama-only Powell credit must not survive facet filter"
        );
        assert!(
            scored.iter().all(|c| c.candidate.title != "The Bourne Supremacy"),
            "thriller-only Powell credit must not survive facet filter"
        );
        // C1 relative bands may hold mid-pool comedy; facet filter is the product claim.
        let antz = candidates
            .iter()
            .find(|c| c.title == "Antz")
            .expect("Antz fixture");
        assert!(
            score_candidate(&profile, antz)
                .eligibility
                .evidence_grade
                .displayable(),
            "overlapping comedy/animation Powell credits keep a legacy grade"
        );

        let short = shortlist(&scored);
        let powell_n = short
            .iter()
            .filter(|c| {
                c.positive_features
                    .iter()
                    .any(|f| f.contains("Powell"))
            })
            .count();
        assert!(
            powell_n <= 8,
            "Person X must not exceed the scoring cap of 8 shortlist slots, got {powell_n} / {}",
            short.len()
        );

        let traces = person_pipeline_trace(&profile, &candidates, &scored, &short);
        let p = traces
            .iter()
            .find(|t| t.name == "John Powell")
            .unwrap();
        assert_eq!(p.injected, 20);
        assert_eq!(p.appearances, 2);
        assert!(
            p.survived_score < p.injected,
            "facet filter should drop unrelated filmography: score {} injected {}",
            p.survived_score,
            p.injected
        );
        assert!(p.survived_mmr <= 8, "MMR {}", p.survived_mmr);
        assert!(p.confidence < 0.5, "n=2 composer k=4 → ~0.39, got {}", p.confidence);
        assert!(p.scoring_affinity < p.recommendation_mean);
    }

    #[test]
    fn filmography_keyword_cannot_rescue_offcluster_resume() {
        let (profile, powell) = powell_profile();
        let mut bourne = filmography_candidate(
            "The Bourne Supremacy",
            2501,
            &["Action", "Thriller"],
            powell,
        );
        bourne.keywords = vec![crate::taste::features::Keyword {
            id: None,
            name: "suspenseful".into(),
        }];
        assert!(
            !filmography_supported(&profile, &bourne),
            "a shared quality keyword must not reopen an off-cluster résumé dump"
        );
    }

    #[test]
    fn close_to_does_not_cite_unrelated_titles() {
        let (profile, hillenburg) = sponge_profile();
        let cand = Candidate {
            tmdb_id: Some(880),
            title: "SpongeBob Movie".into(),
            year: Some(2004),
            poster: None,
            genres: vec!["Animation".into(), "Comedy".into(), "Family".into()],
            credits: vec![hillenburg.clone()],
            keywords: vec![],
            runtime: Some(87),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Stephen Hillenburg".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.evidence.iter().any(|e| e.contains("SpongeBob")),
            "got {:?}",
            scored.evidence
        );
        assert!(
            scored
                .evidence
                .iter()
                .all(|e| !e.contains("Twilight") && !e.contains("Brand New Day")),
            "got {:?}",
            scored.evidence
        );
    }

    #[test]
    fn fraser_filmography_is_not_suppressed_when_cluster_is_empty() {
        use crate::taste::shortlist::shortlist;
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
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Science Fiction".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            None,
        ));
        for i in 0..12i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("log{i}"),
                4.5,
                Some(50 + i),
                &s,
                Some(0.4),
                &[["Drama", "Comedy", "Thriller", "Horror"][i as usize % 4].into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut candidates = vec![
            filmography_candidate("Zero Dark Thirty", 1, &["Thriller"], dp.clone()),
            filmography_candidate("Mary Magdalene", 2, &["Drama"], dp.clone()),
            filmography_candidate("Rogue One", 3, &["Science Fiction", "Action"], dp.clone()),
        ];
        for i in 0..20i64 {
            candidates.push(Candidate {
                tmdb_id: Some(80 + i),
                title: format!("Other {i}"),
                year: Some(2019),
                poster: None,
                genres: vec!["Drama".into()],
                credits: vec![],
                keywords: vec![],
                runtime: None,
                vote_count: Some(200),
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Related,
                    label: "similar".into(),
                    seed_tmdb_id: Some(50),
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                friend_affinity: 0.0,
                tmdb_related: 0.5,
            media_kind: MediaKind::Movie,
            });
        }
        let mut sem_map = std::collections::HashMap::new();
        for c in &candidates {
            if let Some(id) = c.tmdb_id {
                sem_map.insert(
                    id,
                    SemanticScore {
                        coverage: true,
                        positive_similarity: 0.72,
                        negative_similarity: 0.08,
                        fit: 0.75,
                        positive_matches: 4,
                        negative_matches: 1,
                    },
                );
            }
        }
        let scored = score_pool_with_semantic(&profile, &candidates, &sem_map).ranked;
        assert!(
            scored.iter().all(|c| c.candidate.title != "Zero Dark Thirty"),
            "empty cluster must not dump generic Fraser filmography"
        );
        assert!(
            scored.iter().all(|c| c.candidate.title != "Mary Magdalene"),
            "Fraser Crime/Sci-Fi evidence must not transfer to a Drama-only credit"
        );
        let rogue = candidates
            .iter()
            .find(|c| c.title == "Rogue One")
            .expect("Rogue One fixture");
        assert!(
            score_candidate(&profile, rogue)
                .eligibility
                .evidence_grade
                .displayable(),
            "Rogue One shares Sci-Fi with Dune evidence (legacy grade)"
        );
        let short = shortlist(&scored);
        let fraser_n = short
            .iter()
            .filter(|c| {
                c.positive_features
                    .iter()
                    .any(|f| f.contains("Fraser"))
            })
            .count();
        // Relative C1 may shrink the ranked pool; facet + legacy grade are the claims.
        assert!(fraser_n < 8, "got {fraser_n}");
    }

    /// Real 627-film run: Fever (1999) reached the final 12 on
    /// Drama 0.32 + Thriller 0.20 with Se7en / Fight Club as evidence.
    #[test]
    fn fever_genre_only_does_not_survive_discovery_or_friend() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = observations_from_film(
            "Se7en",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Mystery".into(), "Thriller".into()],
            &[],
            &[],
            Some(1995),
            None,
        );
        obs.extend(observations_from_film(
            "Fight Club",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into(), "Thriller".into()],
            &[],
            &[],
            Some(1999),
            None,
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(1.0), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.0,
                Some(10 + i),
                &s,
                Some(1.0),
                &["Comedy".into()],
                &[],
                &[],
                Some(2010),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut fever = related_genre("Fever", 1883, &["Drama", "Thriller"]);
        fever.sources = vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: "intense thrillers".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        fever.tmdb_related = 0.0;
        fever.year = Some(1999);
        let scored = score_candidate(&profile, &fever);
        assert!(
            !scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.family == "genre"),
            "broad Drama/Thriller must not cite as preference, matched={:?}",
            scored.matched_features
        );
        assert!(
            scored.contextual_only,
            "discovery without craft/keyword must not be eligible, eligibility={:?}",
            scored.eligibility.passed_because
        );
        assert!(score_all(&profile, &[fever.clone()]).is_empty());

        fever.sources = vec![RetrievalSource {
            kind: RetrievalKind::Friend,
            label: "friend".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        fever.friend_affinity = 0.2;
        let friend = score_candidate(&profile, &fever);
        assert!(friend.contextual_only, "friend + broad-genre-only, {:?}", friend.reasons);
        assert!(score_all(&profile, &[fever]).is_empty());
    }

    /// Real 627-film run: Better Off Dead ranked on admiring / hilarious
    /// with Twilight and SpongeBob as evidence.
    #[test]
    fn better_off_dead_is_not_carried_by_hilarious_or_admiring() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let hilarious = Keyword {
            id: Some(11),
            name: "hilarious".into(),
        };
        let admiring = Keyword {
            id: Some(12),
            name: "admiring".into(),
        };
        let mut obs = observations_from_film(
            "Twilight",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Drama".into()],
            &[],
            &[hilarious.clone(), admiring.clone()],
            Some(2008),
            None,
        );
        obs.extend(observations_from_film(
            "The SpongeBob SquarePants Movie",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Comedy".into(), "Animation".into()],
            &[],
            &[hilarious.clone(), admiring.clone()],
            Some(2004),
            None,
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(4),
            title: "Better Off Dead...".into(),
            year: Some(1985),
            poster: None,
            genres: vec!["Comedy".into(), "Romance".into()],
            credits: vec![],
            keywords: vec![hilarious, admiring],
            runtime: Some(97),
            vote_count: Some(800),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(2),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.5,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored
                .reasons
                .iter()
                .all(|r| !r.to_lowercase().contains("hilarious")
                    && !r.to_lowercase().contains("admiring")),
            "reaction keywords must not drive ranking, got {:?}",
            scored.reasons
        );
        assert!(
            scored.contextual_only || score_all(&profile, &[cand]).is_empty(),
            "Better Off Dead must not survive on weak reaction tags, reasons={:?}",
            scored.reasons
        );
    }

    fn related_keyword(title: &str, tmdb_id: i64, keyword: &str) -> Candidate {
        Candidate {
            tmdb_id: Some(tmdb_id),
            title: title.into(),
            year: Some(2007),
            poster: None,
            genres: vec!["Crime".into(), "Action".into()],
            credits: vec![],
            keywords: vec![Keyword {
                id: None,
                name: keyword.into(),
            }],
            runtime: Some(110),
            vote_count: Some(800),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 1.0,
        media_kind: MediaKind::Movie,
        }
    }

    fn liked_keyword_obs(name: &str, n: usize, start_id: i64) -> Vec<crate::taste::features::FeatureObservation> {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let kw = Keyword {
            id: None,
            name: name.into(),
        };
        let mut obs = Vec::new();
        for i in 0..n {
            let s = interaction_signal(4.5, &p, Some(0.5), 1, false);
            obs.extend(observations_from_film(
                &format!("{name} film {i}"),
                4.5,
                Some(start_id + i as i64),
                &s,
                Some(0.5),
                &["Drama".into()],
                &[],
                &[kw.clone()],
                Some(2010),
                None,
            ));
        }
        obs
    }

    fn liked_nolan_obs() -> Vec<crate::taste::features::FeatureObservation> {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let nolan = Credit {
            id: Some(525),
            name: "Christopher Nolan".into(),
            job: "Director".into(),
        };
        let pfister = Credit {
            id: Some(559),
            name: "Wally Pfister".into(),
            job: "Director of Photography".into(),
        };
        let titles = [
            ("The Dark Knight", 2008i32, 155i64),
            ("Interstellar", 2014, 157),
            ("Memento", 2000, 77),
            ("Batman Begins", 2005, 272),
        ];
        let mut obs = Vec::new();
        for (title, year, id) in titles {
            let s = interaction_signal(5.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                title,
                5.0,
                Some(id),
                &s,
                Some(0.4),
                &["Drama".into(), "Thriller".into()],
                &[nolan.clone(), pfister.clone()],
                &[],
                Some(year),
                None,
            ));
        }
        obs
    }

    #[test]
    fn drugs_cannot_carry_a_recommendation_by_itself() {
        let mut obs = liked_keyword_obs("drugs", 4, 20);
        obs.extend(liked_keyword_obs("comedy pad", 8, 40));
        let profile = build_profile(&obs);
        let cand = related_keyword("Rise of the Footsoldier", 13054, "drugs");
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.contextual_only,
            "drugs-only related must not be eligible, reasons={:?} eligibility={:?}",
            scored.reasons, scored.eligibility.passed_because
        );
        assert!(score_all(&profile, &[cand]).is_empty());
    }

    #[test]
    fn neo_noir_remains_independently_useful() {
        let profile = build_profile(&liked_keyword_obs("neo-noir", 4, 20));
        let cand = related_keyword("Drive", 9087, "neo-noir");
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.eligibility.evidence_grade.displayable(),
            "neo-noir should carry a related candidate, reasons={:?}",
            scored.reasons
        );
        let sem = SemanticScore {
            coverage: true,
            positive_similarity: 0.55,
            negative_similarity: 0.1,
            fit: 0.6,
            positive_matches: 3,
            negative_matches: 1,
        };
        let mut map = std::collections::HashMap::new();
        map.insert(cand.tmdb_id.unwrap(), sem.clone());
        assert!(!score_pool_with_semantic(&profile, &[cand], &map)
            .ranked
            .is_empty());
    }

    #[test]
    fn murder_cannot_outrank_nolan_watchlist_without_corroboration() {
        let mut obs = liked_nolan_obs();
        obs.extend(liked_keyword_obs("murder", 4, 80));
        let profile = build_profile(&obs);
        let shallow = related_keyword("Generic Murder Film", 9001, "murder");
        let mut prestige = watchlist_drama("The Prestige", 1124);
        prestige.credits = vec![
            Credit {
                id: Some(525),
                name: "Christopher Nolan".into(),
                job: "Director".into(),
            },
            Credit {
                id: Some(559),
                name: "Wally Pfister".into(),
                job: "Director of Photography".into(),
            },
        ];
        prestige.sources.push(RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Christopher Nolan".into(),
            seed_tmdb_id: Some(155),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        let pool = score_pool(&profile, &[shallow, prestige]);
        assert!(
            pool.ranked.iter().all(|c| c.candidate.title != "Generic Murder Film"),
            "broad murder-only related should not survive, ranked={:?}",
            pool.ranked.iter().map(|c| &c.candidate.title).collect::<Vec<_>>()
        );
        assert!(
            pool.ranked.iter().any(|c| c.candidate.title == "The Prestige"),
            "Nolan+Pfister watchlist must remain eligible"
        );
    }

    #[test]
    fn coming_of_age_does_not_use_an_attribute_tag_as_corroboration() {
        let mut obs = liked_nolan_obs();
        obs.extend(liked_keyword_obs("coming of age", 4, 90));
        let woman = Keyword {
            id: Some(91),
            name: "woman director".into(),
        };
        let p = rating_profile(&[4.0; 8]).unwrap();
        for i in 0..4 {
            let s = interaction_signal(4.5, &p, Some(0.5), 1, false);
            obs.extend(observations_from_film(
                &format!("woman dir {i}"),
                4.5,
                Some(200 + i),
                &s,
                Some(0.5),
                &["Drama".into()],
                &[],
                &[woman.clone()],
                Some(2002),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut curves = related_keyword("Real Women Have Curves", 30309, "coming of age");
        curves.keywords.push(woman);
        curves.genres = vec!["Drama".into(), "Comedy".into()];
        let mut prestige = watchlist_drama("The Prestige", 1124);
        prestige.credits = vec![Credit {
            id: Some(525),
            name: "Christopher Nolan".into(),
            job: "Director".into(),
        }];
        prestige.sources.push(RetrievalSource {
            kind: RetrievalKind::Filmography,
            label: "Christopher Nolan".into(),
            seed_tmdb_id: Some(155),
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        });
        let curves_row = score_candidate(&profile, &curves);
        let prestige_row = score_candidate(&profile, &prestige);
        assert!(
            curves_row.contextual_only,
            "coming-of-age plus an attribute tag is not enough evidence, {:?}",
            curves_row.reasons
        );
        assert!(!prestige_row.contextual_only);
        assert!(
            evidence_grade(&prestige_row) > evidence_grade(&curves_row),
            "Nolan+watchlist grade {} should beat coming-of-age grade {}",
            evidence_grade(&prestige_row),
            evidence_grade(&curves_row)
        );
    }

    /// Real 627-film run: The Legend of Ochi surfaced as dysfunctional family
    /// even when a more meaningful visual/creature signal was also present.
    #[test]
    fn ochi_display_prefers_creature_over_family_trope() {
        use crate::taste::features::Keyword;
        let p = rating_profile(&[4.0; 8]).unwrap();
        let family = Keyword {
            id: Some(21),
            name: "dysfunctional family".into(),
        };
        let creature = Keyword {
            id: Some(22),
            name: "creature".into(),
        };
        let mut obs = observations_from_film(
            "Avatar: The Way of Water",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Science Fiction".into(), "Adventure".into()],
            &[],
            &[family.clone(), creature.clone()],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Evil Dead Burn",
            4.0,
            Some(2),
            &interaction_signal(4.0, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Horror".into()],
            &[],
            &[family.clone(), creature.clone()],
            Some(2026),
            None,
        ));
        let profile = build_profile(&obs);
        let cand = Candidate {
            tmdb_id: Some(90),
            title: "The Legend of Ochi".into(),
            year: Some(2025),
            poster: None,
            genres: vec!["Fantasy".into(), "Adventure".into()],
            credits: vec![],
            keywords: vec![family, creature],
            runtime: Some(100),
            vote_count: Some(200),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to Avatar: The Way of Water".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.8,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(!scored.contextual_only, "Ochi may survive, got {:?}", scored.reasons);
        assert!(
            scored.display_reasons[0].to_lowercase().contains("creature"),
            "display must lead with the meaningful signal, got {:?}",
            scored.display_reasons
        );
        assert!(
            !scored.display_reasons[0].to_lowercase().contains("dysfunctional"),
            "family trope must not headline, got {:?}",
            scored.display_reasons
        );
        assert!(
            scored.reasons.iter().any(|r| r.to_lowercase().contains("dysfunctional family")),
            "scoring can still keep the family keyword, got {:?}",
            scored.reasons
        );
    }

    /// Real 627-film run: The Karate Guard showed Giacchino 0.91 from two films.
    #[test]
    fn karate_guard_display_exposes_thin_giacchino_evidence() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let giacchino = Credit {
            id: Some(9),
            name: "Michael Giacchino".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Crime".into()],
            &[giacchino.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Spider-Man: No Way Home",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Action".into()],
            &[giacchino.clone()],
            &[],
            Some(2021),
            None,
        ));
        let profile = build_profile(&obs);
        let aff = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Michael Giacchino")
            .unwrap();
        assert_eq!(aff.appearances, 2);
        let cand = Candidate {
            tmdb_id: Some(55),
            title: "The Karate Guard".into(),
            year: Some(2005),
            poster: None,
            genres: vec!["Animation".into(), "Comedy".into()],
            credits: vec![giacchino],
            keywords: vec![],
            runtime: Some(8),
            vote_count: Some(50),
            watchlist: false,
            sources: vec![RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Michael Giacchino".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
        };
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.reasons.iter().any(|r| r.contains("Giacchino") && r.contains("0.")),
            "scoring reasons may keep the mean, got {:?}",
            scored.reasons
        );
        assert!(
            scored
                .display_reasons
                .iter()
                .any(|r| r.contains("Giacchino") && r.contains("2 films") && r.contains("limited evidence")),
            "display must expose sample size, got {:?}",
            scored.display_reasons
        );
        assert!(
            scored.matched_features.iter().any(|f| f.name.contains("Giacchino") && f.appearances == 2),
            "provenance must keep appearances, got {:?}",
            scored.matched_features
        );
        assert!(
            scored
                .eligibility
                .passed_because
                .iter()
                .any(|s| s == "short-runtime"),
            "recommendation policy must drop shorts, got {:?}",
            scored.eligibility
        );
        assert!(!scored.eligibility.passed);
    }

    #[test]
    fn filmography_close_to_requires_compatible_history_not_the_resume() {
        let (profile, powell) = powell_profile();
        let ice_age = filmography_candidate(
            "Ice Age: The Meltdown",
            201,
            &["Comedy", "Animation", "Family"],
            powell.clone(),
        );
        let ice = score_candidate(&profile, &ice_age);
        assert!(
            ice.evidence.iter().any(|e| e == "Kung Fu Panda"),
            "compatible animation history may be Close to, got {:?}",
            ice.evidence
        );
        let be_cool = filmography_candidate(
            "Be Cool",
            301,
            &["Comedy", "Crime"],
            powell,
        );
        let scored = score_candidate(&profile, &be_cool);
        assert!(
            scored
                .evidence
                .iter()
                .all(|e| e != "Minions & Monsters" && e != "Kung Fu Panda"),
            "a comedy tag must not inherit the animation résumé as Close to, got {:?}",
            scored.evidence
        );
        assert!(scored.eligibility.candidate_fit < 1.0);
        let ms = crate::taste::confidence::match_score(&scored);
        assert!(
            ms <= crate::taste::confidence::SINGLE_BRIDGE_CAP,
            "Be Cool must not display Strong possibility, got {ms}"
        );
    }

    #[test]
    fn close_to_allowlist_keeps_rated_history_only() {
        use crate::taste::retrieve::FilmRecord;
        let history = FilmRecord {
            key: "a".into(),
            title: "The Batman".into(),
            year: Some(2022),
            tmdb_id: Some(1),
            rating: Some(5.0),
            liked: true,
            watched: true,
            watchlist: false,
            viewings: 1,
            last_date: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            recommendations: vec![],
            similar: vec![],
            collection_name: None,
            collection: vec![],
            runtime: Some(176),
            poster: None,
            vote_count: None,
            review: None,
            signal: None,
            age_years: None,
        };
        let watchlist = FilmRecord {
            title: "Wonder Man".into(),
            year: Some(2026),
            tmdb_id: Some(2),
            rating: None,
            liked: false,
            watched: false,
            watchlist: true,
            viewings: 0,
            runtime: Some(120),
            ..history.clone()
        };
        let short = FilmRecord {
            title: "Minions & Monsters".into(),
            year: Some(2010),
            tmdb_id: Some(3),
            rating: Some(5.0),
            liked: true,
            watched: true,
            watchlist: false,
            viewings: 1,
            runtime: Some(12),
            ..history.clone()
        };
        let catalog = FilmRecord {
            title: "Spider-Man: Brand New Day".into(),
            year: Some(2026),
            tmdb_id: Some(4),
            rating: Some(4.0),
            liked: true,
            watched: false,
            watchlist: false,
            viewings: 0,
            runtime: Some(120),
            vote_count: Some(10),
            ..history.clone()
        };
        let current_year_watched = FilmRecord {
            title: "Spider-Man: Brand New Day".into(),
            year: Some(chrono::Utc::now().year()),
            tmdb_id: Some(4),
            rating: Some(4.0),
            liked: true,
            watched: true,
            watchlist: false,
            viewings: 1,
            runtime: Some(145),
            vote_count: Some(2129),
            ..history.clone()
        };
        let allow = close_to_allowlist(&[history, watchlist, short, catalog, current_year_watched]);
        assert!(allow.contains("the batman"));
        assert!(!allow.contains("wonder man"));
        assert!(!allow.contains("minions & monsters"));
        assert!(!allow.contains("spider-man: brand new day"));
        let filtered = filter_close_to_evidence(
            &[
                "The Batman".into(),
                "Wonder Man".into(),
                "Backrooms".into(),
                "Minions & Monsters".into(),
                "Spider-Man: Brand New Day".into(),
            ],
            &allow,
        );
        assert_eq!(filtered, vec!["The Batman".to_string()]);
    }

    #[test]
    fn dp_credit_does_not_make_every_action_comedy_a_specific_fit() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(101),
            name: "Mauro Fiore".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "Avatar",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &[
                "Action".into(),
                "Adventure".into(),
                "Fantasy".into(),
                "Science Fiction".into(),
            ],
            &[dp.clone()],
            &[],
            Some(2009),
            Some(162),
        );
        obs.extend(observations_from_film(
            "Avatar: The Way of Water",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &[
                "Action".into(),
                "Adventure".into(),
                "Fantasy".into(),
                "Science Fiction".into(),
            ],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(192),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("other{i}"),
                4.0,
                Some(30 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let trouble = filmography_candidate("Trouble Bound", 400, &["Action", "Comedy"], dp);
        assert!(
            !filmography_supported(&profile, &trouble),
            "a DP credit plus Action must not dump the rest of the résumé"
        );
        let scored = score_candidate(&profile, &trouble);
        assert!(
            scored.eligibility.candidate_fit < 0.999,
            "visual mode from having a cinematographer is not movie-specific evidence, fit={}",
            scored.eligibility.candidate_fit
        );
        assert!(scored.evidence.is_empty(), "got {:?}", scored.evidence);
    }

    #[test]
    fn broad_action_comedy_is_not_a_specific_powell_match() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let powell = composer("John Powell", 50);
        let mut obs = observations_from_film(
            "Kung Fu Panda",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &[
                "Comedy".into(),
                "Animation".into(),
                "Family".into(),
                "Action".into(),
            ],
            &[powell.clone()],
            &[],
            Some(2008),
            Some(90),
        );
        obs.extend(observations_from_film(
            "How to Train Your Dragon",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.8), 1, false),
            Some(0.8),
            &[
                "Comedy".into(),
                "Animation".into(),
                "Family".into(),
                "Action".into(),
            ],
            &[powell.clone()],
            &[],
            Some(2010),
            Some(98),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("log{i}"),
                4.5,
                Some(100 + i),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let chill = filmography_candidate(
            "Chill Factor",
            401,
            &["Action", "Comedy", "Thriller"],
            powell,
        );
        assert!(
            !filmography_supported(&profile, &chill),
            "Action+Comedy with a composer is not enough to investigate the résumé"
        );
        let scored = score_candidate(&profile, &chill);
        assert!(
            scored.eligibility.candidate_fit < 0.999,
            "two broad genres must not count as a specific movie match, fit={}",
            scored.eligibility.candidate_fit
        );
    }

    #[test]
    fn fraser_crime_overlap_stays_a_specific_movie_match() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Thriller".into(), "Action".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Dune: Part Two",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Science Fiction".into(), "Adventure".into()],
            &[dp.clone()],
            &[],
            Some(2024),
            Some(166),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("log{i}"),
                4.5,
                Some(50 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let kts = filmography_candidate(
            "Killing Them Softly",
            402,
            &["Crime", "Thriller"],
            dp,
        );
        assert!(filmography_supported(&profile, &kts));
        let scored = score_candidate(&profile, &kts);
        assert!(
            (scored.eligibility.candidate_fit - 1.0).abs() < f32::EPSILON,
            "Crime overlap with The Batman is movie-specific, fit={}",
            scored.eligibility.candidate_fit
        );
        assert!(
            scored.evidence.iter().any(|e| e == "The Batman"),
            "got {:?}",
            scored.evidence
        );
    }

    #[test]
    fn two_craft_people_without_movie_overlap_are_not_perfect_fit() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(1),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let composer = Credit {
            id: Some(2),
            name: "Hans Zimmer".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into(), "Thriller".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Dune",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Science Fiction".into()],
            &[dp.clone()],
            &[],
            Some(2021),
            Some(155),
        ));
        obs.extend(observations_from_film(
            "Interstellar",
            5.0,
            Some(3),
            &interaction_signal(5.0, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Science Fiction".into(), "Adventure".into()],
            &[composer.clone()],
            &[],
            Some(2014),
            Some(169),
        ));
        obs.extend(observations_from_film(
            "Dune: Part Two",
            5.0,
            Some(4),
            &interaction_signal(5.0, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[composer.clone()],
            &[],
            Some(2024),
            Some(166),
        ));
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.0,
                Some(80 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let mut cand = filmography_candidate(
            "Infinite",
            900,
            &["Romance", "Comedy"],
            dp,
        );
        cand.credits.push(composer);
        let scored = score_candidate(&profile, &cand);
        assert!(
            scored.eligibility.candidate_fit < 0.999,
            "DP+composer without specific movie overlap must not be fit=1.0, got {}",
            scored.eligibility.candidate_fit
        );
    }

    fn rec_src(seed: i64, rating: f32, title: &str) -> RetrievalSource {
        RetrievalSource::new(
            RetrievalKind::RelatedRecommendations,
            format!("recommended from {title}"),
            Some(seed),
        )
        .with_rating(Some(rating))
    }

    fn similar_src(seed: i64, rating: f32, title: &str) -> RetrievalSource {
        RetrievalSource::new(
            RetrievalKind::RelatedSimilar,
            format!("similar to {title}"),
            Some(seed),
        )
        .with_rating(Some(rating))
    }

    fn noir_profile() -> FeatureProfile {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let kw = Keyword {
            id: Some(99),
            name: "neo-noir".into(),
        };
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Crime".into()],
            &[dp.clone()],
            &[kw.clone()],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Dune",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp],
            &[kw],
            Some(2021),
            Some(155),
        ));
        for i in 0..6i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.0,
                Some(80 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        build_profile(&obs)
    }

    fn cand_with(
        title: &str,
        tmdb: i64,
        genres: &[&str],
        credits: Vec<Credit>,
        keywords: Vec<Keyword>,
        sources: Vec<RetrievalSource>,
    ) -> Candidate {
        Candidate {
            tmdb_id: Some(tmdb),
            title: title.into(),
            year: Some(2018),
            poster: None,
            genres: genres.iter().map(|g| (*g).to_string()).collect(),
            credits,
            keywords,
            runtime: Some(120),
            vote_count: Some(800),
            watchlist: false,
            sources,
            friend_affinity: 0.0,
            tmdb_related: 1.0,
            media_kind: MediaKind::Movie,
        }
    }

    fn covered_semantic_fit(fit: f32) -> SemanticScore {
        SemanticScore {
            positive_similarity: fit,
            negative_similarity: 0.1,
            fit,
            coverage: true,
            positive_matches: 3,
            negative_matches: 1,
        }
    }

    #[test]
    fn semantic_two_similar_without_candidate_metadata_stay_none() {
        let profile = noir_profile();
        let scored = score_candidate_with_semantic(
            &profile,
            &cand_with(
                "Happy Feet",
                12,
                &["Animation", "Family"],
                vec![],
                vec![],
                vec![
                    similar_src(10, 5.0, "Minions"),
                    similar_src(11, 4.5, "SpongeBob"),
                ],
            ),
            &covered_semantic_fit(0.95),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::None);
        // C1: strong Content semantic can admit even when legacy grade is None.
        assert!(scored.eligibility.passed);
        assert!(scored.eligibility.predicted_fit >= 0.58);
    }

    #[test]
    fn semantic_contextual_keyword_cannot_supply_candidate_metadata_fit() {
        let profile = noir_profile();
        let scored = score_candidate_with_semantic(
            &profile,
            &cand_with(
                "Prestige Crime",
                13,
                &["Crime"],
                vec![],
                vec![Keyword {
                    id: Some(13),
                    name: "woman director".into(),
                }],
                vec![
                    rec_src(10, 5.0, "The Batman"),
                    rec_src(11, 4.5, "Dune"),
                ],
            ),
            &covered_semantic_fit(0.95),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::None);
        // Legacy grade stays None; C1 may still admit on Content Fit_v1.
        assert!(scored.eligibility.passed);
    }

    #[test]
    fn neutral_semantic_fit_keeps_displayable_evidence_but_respects_ceiling() {
        let profile = noir_profile();
        let scored = score_candidate_with_semantic(
            &profile,
            &cand_with(
                "Deterministic Bridge",
                16,
                &["Science Fiction"],
                vec![],
                vec![Keyword {
                    id: Some(99),
                    name: "neo-noir".into(),
                }],
                vec![
                    rec_src(10, 5.0, "The Batman"),
                    rec_src(11, 4.5, "Dune"),
                ],
            ),
            &covered_semantic_fit(0.52),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::Medium);
        assert!(!scored.contextual_only);
        assert!(scored.eligibility.passed);
    }

    #[test]
    fn semantic_similar_actor_needs_a_matching_positive_cluster() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let actor = Credit {
            id: Some(5),
            name: "Cluster Star".into(),
            job: "Actor".into(),
        };
        let mut obs = Vec::new();
        for i in 0..3i64 {
            obs.extend(observations_from_film(
                &format!("Crime film {i}"),
                4.5,
                Some(100 + i),
                &interaction_signal(4.5, &p, Some(0.4), 1, false),
                Some(0.4),
                &["Crime".into()],
                &[actor.clone()],
                &[],
                Some(2010 + i as i32),
                Some(120),
            ));
        }
        let profile = build_profile(&obs);
        let no_fit = score_candidate_with_semantic(
            &profile,
            &cand_with(
                "Unrelated Actor Film",
                14,
                &["Drama"],
                vec![actor.clone()],
                vec![],
                vec![similar_src(100, 5.0, "Crime film 0")],
            ),
            &covered_semantic_fit(0.95),
        );
        assert_eq!(no_fit.eligibility.evidence_grade, EvidenceGrade::None);

        let fit = score_candidate_with_semantic(
            &profile,
            &cand_with(
                "Matching Actor Film",
                15,
                &["Crime"],
                vec![actor],
                vec![],
                vec![similar_src(100, 5.0, "Crime film 0")],
            ),
            &covered_semantic_fit(0.95),
        );
        assert_eq!(fit.eligibility.evidence_grade, EvidenceGrade::Medium);
        assert!(fit.eligibility.passed);
    }

    #[test]
    fn two_loved_recs_with_two_film_keyword_fit_are_capped_to_medium() {
        let profile = noir_profile();
        let kw = Keyword {
            id: Some(99),
            name: "neo-noir".into(),
        };
        let scored = score_candidate(
            &profile,
            &cand_with(
                "Star Trek",
                13475,
                &["Science Fiction", "Action"],
                vec![],
                vec![kw],
                vec![
                    rec_src(330_459, 5.0, "Rogue One"),
                    rec_src(19_995, 5.0, "Avatar"),
                ],
            ),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::Medium);
        assert!(!scored.contextual_only);
        assert!(scored.eligibility.passed);
    }

    #[test]
    fn lone_similar_to_is_not_displayable() {
        let profile = noir_profile();
        let scored = score_candidate(
            &profile,
            &cand_with(
                "The Boss Baby",
                295_693,
                &["Animation", "Family", "Comedy"],
                vec![],
                vec![],
                vec![similar_src(10, 5.0, "Minions")],
            ),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::None);
        assert!(scored.contextual_only);
        assert!(!scored.eligibility.passed);
    }

    #[test]
    fn two_similar_without_metadata_fit_are_not_displayable() {
        let profile = noir_profile();
        let scored = score_candidate(
            &profile,
            &cand_with(
                "Happy Feet",
                12,
                &["Animation", "Family"],
                vec![],
                vec![],
                vec![
                    similar_src(10, 5.0, "Minions"),
                    similar_src(11, 4.5, "SpongeBob"),
                ],
            ),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::None);
        assert!(scored.contextual_only);
    }

    #[test]
    fn two_similar_with_keyword_fit_are_medium() {
        let profile = noir_profile();
        let kw = Keyword {
            id: Some(99),
            name: "neo-noir".into(),
        };
        let scored = score_candidate(
            &profile,
            &cand_with(
                "Nightcrawler",
                242_582,
                &["Crime", "Thriller"],
                vec![],
                vec![kw],
                vec![
                    similar_src(1, 5.0, "The Batman"),
                    similar_src(2, 4.5, "Dune"),
                ],
            ),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::Medium);
        assert!(!scored.contextual_only);
    }

    #[test]
    fn similar_plus_cameo_actor_is_not_displayable() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let actor = Credit {
            id: Some(5),
            name: "Cameo Star".into(),
            job: "Actor".into(),
        };
        let mut obs = observations_from_film(
            "Pad A",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[actor.clone()],
            &[],
            Some(2010),
            Some(100),
        );
        for i in 0..8i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.0,
                Some(80 + i),
                &s,
                Some(0.4),
                &["Comedy".into()],
                &[],
                &[],
                Some(2018),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let scored = score_candidate(
            &profile,
            &cand_with(
                "Random Film",
                99,
                &["Drama"],
                vec![actor],
                vec![],
                vec![similar_src(1, 5.0, "Pad A")],
            ),
        );
        assert_eq!(scored.eligibility.evidence_grade, EvidenceGrade::None);
        assert!(scored.contextual_only);
    }

    #[test]
    fn thematic_keyword_does_not_transfer_across_incompatible_contexts() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let parent = Keyword {
            id: Some(1_001),
            name: "parent child relationship".into(),
        };
        let mut obs = Vec::new();
        for (i, title) in ["Prisoners", "Manchester by the Sea", "The Place Beyond the Pines"]
            .iter()
            .enumerate()
        {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                title,
                4.5,
                Some(10 + i as i64),
                &s,
                Some(0.4),
                &["Drama".into(), "Crime".into()],
                &[],
                &[parent.clone()],
                Some(2013),
                Some(120),
            ));
        }
        let profile = build_profile(&obs);
        let animation = cand_with(
            "Kids Movie",
            55_001,
            &["Animation", "Family", "Comedy"],
            vec![],
            vec![parent.clone()],
            vec![similar_src(10, 4.5, "Prisoners")],
        );
        let scored = score_candidate(&profile, &animation);
        assert!(
            !scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("parent child relationship")),
            "drama-loved parent/child must not cite on an animation/family comedy, matched={:?}",
            scored.matched_features
        );
    }

    #[test]
    fn thematic_keyword_transfers_when_liked_context_matches() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let parent = Keyword {
            id: Some(1_001),
            name: "parent child relationship".into(),
        };
        let mut obs = Vec::new();
        for (i, title) in ["The Incredibles", "Finding Nemo", "Inside Out"]
            .iter()
            .enumerate()
        {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                title,
                4.5,
                Some(20 + i as i64),
                &s,
                Some(0.4),
                &["Animation".into(), "Family".into(), "Comedy".into()],
                &[],
                &[parent.clone()],
                Some(2004),
                Some(100),
            ));
        }
        let profile = build_profile(&obs);
        let animation = cand_with(
            "Another Family Animation",
            55_002,
            &["Animation", "Family", "Comedy"],
            vec![],
            vec![parent],
            vec![similar_src(20, 4.5, "The Incredibles")],
        );
        let scored = score_candidate(&profile, &animation);
        assert!(
            scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("parent child relationship")),
            "animation-loved parent/child must transfer to similar animation, matched={:?}",
            scored.matched_features
        );
    }

    #[test]
    fn strong_keyword_still_transfers_without_genre_match() {
        let profile = noir_profile();
        let kw = Keyword {
            id: Some(99),
            name: "neo-noir".into(),
        };
        let scored = score_candidate(
            &profile,
            &cand_with(
                "Odd Neo Noir",
                88_001,
                &["Comedy"],
                vec![],
                vec![kw],
                vec![similar_src(1, 5.0, "The Batman")],
            ),
        );
        assert!(
            scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("neo-noir")),
            "strong craft keywords remain portable taste signatures"
        );
    }

    #[test]
    fn broad_adventure_genre_does_not_drive_content_score() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = Vec::new();
        for (i, title) in ["Mad Max: Fury Road", "Dune", "The Matrix"].iter().enumerate() {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                title,
                4.5,
                Some(10 + i as i64),
                &s,
                Some(0.4),
                &["Action".into(), "Adventure".into(), "Science Fiction".into()],
                &[],
                &[],
                Some(2015),
                Some(130),
            ));
        }
        let profile = build_profile(&obs);
        let kids = cand_with(
            "Kids Adventure",
            77_001,
            &["Animation", "Adventure", "Family"],
            vec![],
            vec![],
            vec![similar_src(10, 4.5, "Mad Max: Fury Road")],
        );
        let scored = score_candidate(&profile, &kids);
        assert!(
            !scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("Adventure")),
            "broad Adventure must not cite as preference evidence, matched={:?}",
            scored.matched_features
        );
        assert!(
            !scored
                .matched_features
                .iter()
                .any(|f| f.cited && f.name.eq_ignore_ascii_case("Action")),
            "broad Action must not cite as preference evidence, matched={:?}",
            scored.matched_features
        );
    }

    #[test]
    fn weak_semantic_demotes_thin_related_medium() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let director = Credit {
            id: Some(1),
            name: "Thin Bridge".into(),
            job: "Director".into(),
        };
        let mut obs = observations_from_film(
            "Loved Seed",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Crime".into(), "Thriller".into()],
            &[director.clone()],
            &[],
            Some(2010),
            Some(110),
        );
        for i in 0..6i64 {
            let s = interaction_signal(4.0, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("pad{i}"),
                4.0,
                Some(80 + i),
                &s,
                Some(0.4),
                &["Crime".into()],
                &[director.clone()],
                &[],
                Some(2012),
                Some(100),
            ));
        }
        let profile = build_profile(&obs);
        let cand = cand_with(
            "Off Taste Neighbor",
            99_001,
            &["Crime", "Comedy"],
            vec![director],
            vec![],
            vec![similar_src(1, 4.5, "Loved Seed")],
        );
        let scored = score_candidate(&profile, &cand);
        assert_eq!(
            scored.eligibility.evidence_grade,
            EvidenceGrade::Medium,
            "fixture needs thin related Medium before demotion, got {:?}",
            scored.eligibility.evidence_grade
        );
        let cited: Vec<&FeatureAffinity> = profile
            .affinities
            .iter()
            .filter(|a| {
                scored
                    .matched_features
                    .iter()
                    .any(|f| f.cited && f.name == a.key.name)
            })
            .collect();
        let demoted = compute_evidence_grade_with_semantic(
            &cand,
            &cited,
            scored.eligibility.candidate_fit,
            &crate::taste::semantic::SemanticScore {
                positive_similarity: 0.2,
                negative_similarity: 0.55,
                fit: 0.40,
                coverage: true,
                positive_matches: 1,
                negative_matches: 2,
            },
        );
        assert_eq!(
            demoted,
            EvidenceGrade::None,
            "dislike-aligned embeddings must demote thin Medium related"
        );
    }
}
