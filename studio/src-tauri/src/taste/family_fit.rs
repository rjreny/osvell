//! Milestone B family fit.
//!
//! **B1 locked — Fit_v1 = Content only.** Craft is still computed for
//! diagnostics/experimentation (`n_eff`, polarity, attribution, lineage,
//! per-role counterfactuals) but `craft_contribution_to_fit = 0` (λ = 0).
//! Actor corroboration machinery stays available; it does not affect Fit_v1.
//!
//! **B2 — conditional Form** is computed when enabled, with tight per-component
//! bounds and aggressive shrinkage. Live Fit_v1 keeps Form λ = 0 until a
//! component earns inclusion on the holdout.
//!
//! Retrieval stays frozen at A3. Fit and confidence stay orthogonal.
//! Missing fields are unknown — never treated as negative evidence.

use crate::taste::continuity::{
    score_continuity, ContinuityConfig, ContinuityPrior, ContinuityResult,
};
use crate::taste::dimensions::predicted_modes;
use crate::taste::features::{
    family_for_job, FeatureAffinity, FeatureFamily, FeatureKey, FeatureProfile, Keyword,
};
use crate::taste::form::{score_form, FormConfig, FormPrior, FormResult};
use crate::taste::quality::{score_quality, QualityCatalog, QualityConfig, QualityResult};
use crate::taste::retrieve::Candidate;
use crate::taste::semantic::SemanticScore;
use serde::Serialize;
use std::collections::HashMap;

const CONTENT_WEIGHT: f32 = 0.58;
const CRAFT_WEIGHT: f32 = 0.42;
/// Small ranking nudge from confidence — never `fit * confidence`.
const CONFIDENCE_RANK_NUDGE: f32 = 0.08;
const CRAFT_SHRINK_K: f32 = 4.0;
const LINEAGE_DIMINISH_K: f32 = 3.0;
const ACTOR_POSITIVE_CAP: f32 = 0.18;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorMode {
    Full,
    Removed,
    /// Positive actor pull capped; negatives still flow.
    PositiveCapped,
    /// Actor only amplifies when Content already leans positive.
    ContentCorroboration,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftRoleMask {
    pub director: bool,
    pub writer: bool,
    pub dp: bool,
    pub composer: bool,
    pub actor: bool,
    pub studio: bool,
}

impl CraftRoleMask {
    pub fn none() -> Self {
        Self {
            director: false,
            writer: false,
            dp: false,
            composer: false,
            actor: false,
            studio: false,
        }
    }

    pub fn full() -> Self {
        Self {
            director: true,
            writer: true,
            dp: true,
            composer: true,
            actor: true,
            studio: true,
        }
    }

    pub fn only_director() -> Self {
        Self {
            director: true,
            ..Self::none()
        }
    }

    pub fn only_writer() -> Self {
        Self {
            writer: true,
            ..Self::none()
        }
    }

    pub fn only_dp() -> Self {
        Self {
            dp: true,
            ..Self::none()
        }
    }

    pub fn only_composer() -> Self {
        Self {
            composer: true,
            ..Self::none()
        }
    }

    pub fn only_actor() -> Self {
        Self {
            actor: true,
            ..Self::none()
        }
    }

    pub fn only_studio() -> Self {
        Self {
            studio: true,
            ..Self::none()
        }
    }

    pub fn authorship() -> Self {
        Self {
            director: true,
            writer: true,
            dp: true,
            ..Self::none()
        }
    }

    pub fn actor_studio() -> Self {
        Self {
            actor: true,
            studio: true,
            ..Self::none()
        }
    }

    pub fn without_actor() -> Self {
        Self {
            actor: false,
            ..Self::full()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftConfig {
    pub roles: CraftRoleMask,
    pub actor_mode: ActorMode,
    /// Additive Craft weight: Fit ≈ Content + λ * Craft (not fit*confidence).
    pub lambda: f32,
    pub lineage_diminishing: bool,
}

impl Default for CraftConfig {
    fn default() -> Self {
        // Production baseline after B1 lock.
        Self::fit_v1()
    }
}

impl CraftConfig {
    /// Locked Fit_v1: Content drives fit; Craft hydrates for diagnostics only.
    pub fn fit_v1() -> Self {
        Self {
            roles: CraftRoleMask::full(),
            actor_mode: ActorMode::ContentCorroboration,
            lambda: 0.0,
            lineage_diminishing: true,
        }
    }

    /// Ablation: skip Craft hydration entirely (roles off).
    pub fn content_only() -> Self {
        Self {
            roles: CraftRoleMask::none(),
            actor_mode: ActorMode::Removed,
            lambda: 0.0,
            lineage_diminishing: true,
        }
    }

    /// Experimental Content+Craft combine used by calibration / FitMode benches.
    /// Not the live Fit_v1 baseline — Craft did not win on the holdout.
    pub fn experimental_craft() -> Self {
        Self {
            roles: CraftRoleMask::full(),
            actor_mode: ActorMode::Full,
            lambda: CRAFT_WEIGHT / CONTENT_WEIGHT,
            lineage_diminishing: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteCandidateFeatures {
    pub embedding_present: bool,
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub modes: Vec<String>,
    pub director: Vec<PersonCreditFeat>,
    pub writers: Vec<PersonCreditFeat>,
    pub dp: Vec<PersonCreditFeat>,
    pub composer: Vec<PersonCreditFeat>,
    pub cast: Vec<PersonCreditFeat>,
    pub production_companies: Vec<String>,
    pub runtime: Option<i32>,
    pub year: Option<i32>,
    pub language: Option<String>,
    pub collection: Option<String>,
    pub vote_average: Option<f32>,
    pub vote_count: Option<i64>,
    /// Fraction of expected content/craft fields that are present (0..=1).
    pub hydration_completeness: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCreditFeat {
    pub id: Option<i64>,
    pub name: String,
    pub job: String,
    /// Cast billing strength 0..=1; 1.0 for non-cast craft roles.
    pub attribution: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyResult {
    pub score: f32,
    pub confidence: f32,
    pub support: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentResult {
    pub score: f32,
    pub confidence: f32,
    pub semantic_positive: f32,
    pub semantic_negative: f32,
    pub semantic_margin: f32,
    pub motif_affinity: f32,
    pub mode_affinity: f32,
    pub genre_affinity: f32,
    pub support: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftContribution {
    pub name: String,
    pub role: String,
    pub rated_support: u32,
    pub n_eff: f32,
    pub weighted_rating_affinity: f32,
    pub positive_evidence_weight: f32,
    pub negative_evidence_weight: f32,
    pub shrunk_affinity: f32,
    pub candidate_role_weight: f32,
    pub contribution: f32,
    pub support_film_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftLineageDiag {
    pub raw_craft_sum: f32,
    pub unique_prior_films: usize,
    pub unique_people: usize,
    pub effective_support_movies: f32,
    pub effective_craft_score: f32,
    pub without_actor: f32,
    pub without_director: f32,
    pub without_writer: f32,
    pub without_dp: f32,
    pub without_composer: f32,
    pub without_studio: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftResult {
    pub score: f32,
    pub confidence: f32,
    pub support: f32,
    pub contributions: Vec<CraftContribution>,
    pub lineage: CraftLineageDiag,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyScores {
    pub content: FamilyResult,
    pub craft: FamilyResult,
    #[serde(default)]
    pub form: FamilyResult,
    #[serde(default)]
    pub continuity: FamilyResult,
    #[serde(default)]
    pub quality: FamilyResult,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterfactualSlice {
    pub fit: f32,
    pub rank_delta: i32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counterfactuals {
    pub without_content: CounterfactualSlice,
    pub without_craft: CounterfactualSlice,
    #[serde(default)]
    pub without_form: CounterfactualSlice,
    #[serde(default)]
    pub without_continuity: CounterfactualSlice,
    #[serde(default)]
    pub without_quality: CounterfactualSlice,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyFitResult {
    pub fit: f32,
    pub confidence: f32,
    /// Ranking key: fit + small confidence-aware uncertainty adjustment.
    pub ranking_score: f32,
    pub families: FamilyScores,
    pub content_detail: ContentResult,
    pub craft_detail: CraftResult,
    #[serde(default)]
    pub form_detail: FormResult,
    #[serde(default)]
    pub continuity_detail: ContinuityResult,
    #[serde(default)]
    pub quality_detail: QualityResult,
    pub features: TasteCandidateFeatures,
    pub counterfactual: Counterfactuals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitMode {
    /// Legacy CandidateScore.total path (A3 retrieval + old ranking).
    Legacy,
    ContentOnly,
    ContentAndCraft,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyFitConfig {
    pub craft: CraftConfig,
    pub form: FormConfig,
    pub continuity: ContinuityConfig,
    pub quality: QualityConfig,
}

impl Default for FamilyFitConfig {
    fn default() -> Self {
        Self::fit_v1()
    }
}

impl FamilyFitConfig {
    /// Production baseline: Content only; other families diagnostic (λ = 0).
    pub fn fit_v1() -> Self {
        Self {
            craft: CraftConfig::fit_v1(),
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        }
    }

    pub fn content_only() -> Self {
        Self {
            craft: CraftConfig::content_only(),
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        }
    }

    pub fn with_form(form: FormConfig) -> Self {
        Self {
            craft: CraftConfig::fit_v1(),
            form,
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        }
    }

    pub fn with_continuity(continuity: ContinuityConfig) -> Self {
        Self {
            craft: CraftConfig::fit_v1(),
            form: FormConfig::off(),
            continuity,
            quality: QualityConfig::off(),
        }
    }

    pub fn with_quality(quality: QualityConfig) -> Self {
        Self {
            craft: CraftConfig::fit_v1(),
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FitPriors<'a> {
    pub form: Option<&'a FormPrior>,
    pub continuity: Option<&'a ContinuityPrior>,
    pub quality: Option<&'a QualityCatalog>,
}

pub fn hydrate_candidate(
    profile: &FeatureProfile,
    candidate: &Candidate,
    embedding_present: bool,
) -> TasteCandidateFeatures {
    let _ = profile; // reserved for future company/collection enrichment from profile context
    let modes = predicted_modes(&candidate.genres, &candidate.credits, &candidate.keywords);
    let mut director = Vec::new();
    let mut writers = Vec::new();
    let mut dp = Vec::new();
    let mut composer = Vec::new();
    let mut cast = Vec::new();
    let mut actor_idx = 0usize;
    for credit in &candidate.credits {
        let Some(family) = family_for_job(&credit.job) else {
            continue;
        };
        let attribution = match family {
            FeatureFamily::Actor => {
                let a = cast_billing_weight(actor_idx);
                actor_idx += 1;
                a
            }
            FeatureFamily::Writer => writer_job_weight(&credit.job),
            _ => 1.0,
        };
        let feat = PersonCreditFeat {
            id: credit.id,
            name: credit.name.clone(),
            job: credit.job.clone(),
            attribution,
        };
        match family {
            FeatureFamily::Director => director.push(feat),
            FeatureFamily::Writer => writers.push(feat),
            FeatureFamily::Cinematographer => dp.push(feat),
            FeatureFamily::Composer => composer.push(feat),
            FeatureFamily::Actor => cast.push(feat),
            _ => {}
        }
    }
    let keywords: Vec<String> = candidate.keywords.iter().map(|k| k.name.clone()).collect();
    let mut present = 0.0;
    let mut expected = 0.0;
    let check = |ok: bool, expected: &mut f32, present: &mut f32| {
        *expected += 1.0;
        if ok {
            *present += 1.0;
        }
    };
    check(embedding_present, &mut expected, &mut present);
    check(!candidate.genres.is_empty(), &mut expected, &mut present);
    check(!keywords.is_empty(), &mut expected, &mut present);
    check(!modes.is_empty(), &mut expected, &mut present);
    check(!director.is_empty(), &mut expected, &mut present);
    check(!writers.is_empty(), &mut expected, &mut present);
    check(!cast.is_empty(), &mut expected, &mut present);
    check(candidate.runtime.is_some(), &mut expected, &mut present);
    check(candidate.year.is_some(), &mut expected, &mut present);
    check(candidate.vote_count.is_some(), &mut expected, &mut present);
    // language / companies / collection / vote_average intentionally unknown for now
    expected += 4.0;

    TasteCandidateFeatures {
        embedding_present,
        genres: candidate.genres.clone(),
        keywords,
        modes,
        director,
        writers,
        dp,
        composer,
        cast,
        production_companies: Vec::new(),
        runtime: candidate.runtime,
        year: candidate.year,
        language: None,
        collection: None,
        vote_average: None,
        vote_count: candidate.vote_count,
        hydration_completeness: if expected > 0.0 {
            present / expected
        } else {
            0.0
        },
    }
}

fn cast_billing_weight(index: usize) -> f32 {
    match index {
        0 => 1.0,
        1 => 0.92,
        2 => 0.84,
        3..=5 => 0.72,
        6..=9 => 0.58,
        10..=14 => 0.45,
        _ => 0.32,
    }
}

fn writer_job_weight(job: &str) -> f32 {
    match job {
        "Screenplay" | "Original Screenplay" => 1.0,
        "Writer" => 0.85,
        "Story" => 0.55,
        "Characters" => 0.35,
        _ => 0.70,
    }
}

fn lookup_affinity<'a>(
    profile: &'a FeatureProfile,
    family: FeatureFamily,
    id: Option<i64>,
    name: &str,
) -> Option<&'a FeatureAffinity> {
    let key = FeatureKey::new(family, id, name);
    profile
        .affinities
        .iter()
        .find(|a| a.key.storage_key() == key.storage_key())
        .or_else(|| {
            // Fall back to name-only match when id is missing on one side.
            profile.affinities.iter().find(|a| {
                a.key.family == family && a.key.name.eq_ignore_ascii_case(name)
            })
        })
}

/// Consolidate correlated theme signals: take the strongest signal per theme
/// family, never sum sports + boxing + competition as independent boosts.
fn consolidate_theme_affinities(
    profile: &FeatureProfile,
    family: FeatureFamily,
    names: &[String],
    keywords: &[Keyword],
    modes: &[String],
    genres: &[String],
) -> (f32, f32, usize) {
    let mut by_theme: HashMap<String, f32> = HashMap::new();
    let mut support = 0.0f32;
    let mut hits = 0usize;
    for aff in &profile.affinities {
        if aff.key.family != family {
            continue;
        }
        let name_hit = names
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&aff.key.name));
        if !name_hit {
            continue;
        }
        if family == FeatureFamily::Keyword
            && !aff.evidence_cluster.overlaps(genres, keywords, modes)
            && !aff.evidence_cluster.is_empty()
        {
            continue;
        }
        // Broad genres: only allow negative pull; positives are too diffuse.
        if family == FeatureFamily::Genre && is_broad_genre(&aff.key.name) {
            if aff.recommendation_mean >= 0.0 {
                continue;
            }
        }
        let theme = theme_bucket(&aff.key.name);
        let signal = aff.recommendation_mean * aff.confidence * aff.portability.max(0.35);
        let entry = by_theme.entry(theme).or_insert(0.0);
        // Keep the strongest magnitude with sign (not a sum of correlated labels).
        if signal.abs() > entry.abs() {
            *entry = signal;
        }
        support += aff.appearances as f32 * aff.confidence;
        hits += 1;
    }
    if by_theme.is_empty() {
        return (0.0, 0.0, 0);
    }
    let mean = by_theme.values().sum::<f32>() / by_theme.len() as f32;
    let conf = (hits as f32 / (hits as f32 + 3.0)).clamp(0.0, 1.0);
    (mean.clamp(-1.0, 1.0), conf, hits)
}

fn is_broad_genre(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "drama"
            | "comedy"
            | "thriller"
            | "action"
            | "adventure"
            | "romance"
            | "crime"
            | "mystery"
            | "fantasy"
            | "science fiction"
            | "horror"
            | "family"
            | "animation"
    )
}

fn theme_bucket(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    if matches!(
        n.as_str(),
        "boxing" | "sport" | "sports" | "competition" | "wrestling" | "martial arts"
    ) {
        return "combat_sport".into();
    }
    if matches!(
        n.as_str(),
        "vampire" | "werewolf" | "zombie" | "ghost" | "haunting" | "supernatural"
    ) {
        return "supernatural".into();
    }
    if matches!(
        n.as_str(),
        "space" | "spaceship" | "alien" | "astronaut" | "sci-fi" | "science fiction"
    ) {
        return "space_scifi".into();
    }
    if matches!(
        n.as_str(),
        "war" | "world war" | "soldier" | "military" | "battlefield"
    ) {
        return "war".into();
    }
    n
}

pub fn score_content(
    profile: &FeatureProfile,
    features: &TasteCandidateFeatures,
    semantic: &SemanticScore,
) -> ContentResult {
    let pos = if semantic.coverage {
        semantic.positive_similarity
    } else {
        0.0
    };
    let neg = if semantic.coverage {
        semantic.negative_similarity
    } else {
        0.0
    };
    let margin = pos - neg;

    // Keep absolute levels: high-pos/high-neg ≠ low-pos/low-neg even with equal margin.
    // Negative neighborhood can drive a genuine negative Content score.
    // Polarized = resembles likes AND dislikes strongly with thin margin — uncertainty,
    // not a strong positive. Theme must not rescue these into the top of New.
    let polarized = semantic.coverage && pos >= 0.50 && neg >= 0.50 && margin < 0.08;
    let semantic_component = if semantic.coverage {
        let level = pos - 1.25 * neg;
        let absolute = pos * (1.0 - neg) - 0.18;
        let mut combined = 0.72 * level + 0.28 * absolute;
        if polarized {
            // Extra ambiguity penalty so .62/.60 cannot outrank .62/.49.
            let ambiguity = ((0.08 - margin) / 0.08).clamp(0.0, 1.0) * neg.min(pos);
            combined -= 0.22 * ambiguity;
        }
        combined.clamp(-1.0, 1.0)
    } else {
        0.0 // unknown, not negative
    };

    let kw: Vec<Keyword> = features
        .keywords
        .iter()
        .map(|n| Keyword {
            id: None,
            name: n.clone(),
        })
        .collect();
    let (genre_affinity, genre_conf, genre_hits) = consolidate_theme_affinities(
        profile,
        FeatureFamily::Genre,
        &features.genres,
        &kw,
        &features.modes,
        &features.genres,
    );
    let (motif_affinity, motif_conf, motif_hits) = consolidate_theme_affinities(
        profile,
        FeatureFamily::Keyword,
        &features.keywords,
        &kw,
        &features.modes,
        &features.genres,
    );
    let mode_affinity = {
        let mut sum = 0.0;
        let mut n = 0.0;
        for mode in &profile.modes {
            if features
                .modes
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&mode.dimension))
            {
                sum += mode.strength.clamp(-1.0, 1.0);
                n += 1.0;
            }
        }
        if n > 0.0 {
            sum / n
        } else {
            0.0
        }
    };
    let mode_conf = if features.modes.is_empty() {
        0.0
    } else {
        0.45
    };
    let mode_hits = if mode_affinity.abs() > 1e-4 { 1 } else { 0 };

    // Theme block: max-ish consolidate across motif/genre/mode — do not sum all three.
    let theme_signals = [
        (motif_affinity, motif_conf + 0.15),
        (genre_affinity, genre_conf),
        (mode_affinity, mode_conf),
    ];
    let (theme_score, theme_w) = {
        let mut best = 0.0f32;
        let mut best_w = 0.0f32;
        for (s, w) in theme_signals {
            if w <= 0.0 {
                continue;
            }
            if s.abs() * w >= best.abs() * best_w {
                best = s;
                best_w = w;
            }
        }
        (best, best_w.clamp(0.0, 1.0))
    };

    let sem_w = if semantic.coverage { 0.72 } else { 0.0 };
    // Theme may corroborate a clear semantic winner; it must not override a
    // polarized (high-pos / high-neg) neighborhood into a top Content score.
    let theme_blend_w = if theme_w > 0.0 {
        if polarized {
            0.08
        } else if semantic.coverage && margin < 0.10 {
            let t = ((margin - 0.04) / 0.06).clamp(0.0, 1.0);
            0.08 + 0.20 * t
        } else {
            0.28
        }
    } else {
        0.0
    };
    let denom = sem_w + theme_blend_w;
    let score = if denom > 0.0 {
        (sem_w * semantic_component + theme_blend_w * theme_score) / denom
    } else {
        0.0
    };

    let support = (semantic.positive_matches + semantic.negative_matches) as f32
        + genre_hits as f32
        + motif_hits as f32
        + mode_hits as f32;
    let confidence = {
        let sem_c = if semantic.coverage {
            // Polarized neighborhoods (high pos AND high neg) lower confidence.
            let polar = (pos.min(neg) * 2.0).clamp(0.0, 1.0);
            (0.55 + 0.35 * pos - 0.25 * polar).clamp(0.15, 0.95)
        } else {
            0.2
        };
        let theme_c = (theme_w * 0.5 + features.hydration_completeness * 0.5).clamp(0.1, 0.9);
        if semantic.coverage {
            (0.7 * sem_c + 0.3 * theme_c).clamp(0.1, 0.98)
        } else {
            (0.35 * sem_c + 0.65 * theme_c).clamp(0.1, 0.85)
        }
    };

    ContentResult {
        score: score.clamp(-1.0, 1.0),
        confidence,
        semantic_positive: pos,
        semantic_negative: neg,
        semantic_margin: margin,
        motif_affinity,
        mode_affinity,
        genre_affinity,
        support,
    }
}

pub fn score_craft(profile: &FeatureProfile, features: &TasteCandidateFeatures) -> CraftResult {
    // Full role mask for diagnostic Craft; Fit_v1 still uses λ=0 at combine time.
    score_craft_with_config(profile, features, &CraftConfig::experimental_craft(), None)
}

pub fn score_craft_with_config(
    profile: &FeatureProfile,
    features: &TasteCandidateFeatures,
    config: &CraftConfig,
    content: Option<&ContentResult>,
) -> CraftResult {
    let mut contributions = Vec::new();

    let push = |family: FeatureFamily,
                people: &[PersonCreditFeat],
                contributions: &mut Vec<CraftContribution>| {
        for person in people {
            let Some(aff) = lookup_affinity(profile, family, person.id, &person.name) else {
                continue;
            };
            if aff.appearances == 0 {
                continue;
            }
            // Effective support: rated mass × candidate attribution (billing / job).
            let n_eff = (aff.positive_weight + aff.negative_weight).max(0.05)
                * person.attribution.max(0.15);
            let shrink = n_eff / (n_eff + CRAFT_SHRINK_K);
            // Separate positive vs negative craft evidence — polarized résumés shrink.
            let pos = aff.positive_weight.max(0.0);
            let neg = aff.negative_weight.max(0.0);
            let polar = if pos + neg > 1e-4 {
                (pos.min(neg) / (pos + neg)).clamp(0.0, 0.5)
            } else {
                0.0
            };
            let net = aff.recommendation_mean * (1.0 - polar);
            let role_w = family.weight() * person.attribution;
            let mut shrunk = net * shrink;
            let mut contribution = shrunk * role_w * aff.confidence.max(0.15);
            if !contribution.is_finite() {
                contribution = 0.0;
            }

            if family == FeatureFamily::Actor {
                match config.actor_mode {
                    ActorMode::Removed => continue,
                    ActorMode::PositiveCapped => {
                        if contribution > ACTOR_POSITIVE_CAP {
                            contribution = ACTOR_POSITIVE_CAP;
                            shrunk = if role_w * aff.confidence.max(0.15) > 1e-4 {
                                contribution / (role_w * aff.confidence.max(0.15))
                            } else {
                                shrunk
                            };
                        }
                    }
                    ActorMode::ContentCorroboration => {
                        let content_ok = content.map(|c| c.score > 0.08).unwrap_or(false);
                        if !content_ok {
                            // Actor may still apply mild negative pull.
                            if contribution > 0.0 {
                                contribution *= 0.15;
                            }
                        } else if contribution > 0.0 {
                            // Amplify agreement rather than create plausibility alone.
                            contribution = (contribution * 0.55).min(0.35);
                        }
                    }
                    ActorMode::Full => {}
                }
            }

            let support_film_ids: Vec<i64> = aff
                .positive_evidence
                .iter()
                .chain(aff.negative_evidence.iter())
                .filter_map(|e| e.tmdb_id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            contributions.push(CraftContribution {
                name: person.name.clone(),
                role: format!("{family:?}"),
                rated_support: aff.appearances,
                n_eff,
                weighted_rating_affinity: aff.recommendation_mean,
                positive_evidence_weight: pos,
                negative_evidence_weight: neg,
                shrunk_affinity: shrunk,
                candidate_role_weight: role_w,
                contribution,
                support_film_ids,
            });
        }
    };

    if config.roles.director {
        push(FeatureFamily::Director, &features.director, &mut contributions);
    }
    if config.roles.writer {
        push(FeatureFamily::Writer, &features.writers, &mut contributions);
    }
    if config.roles.dp {
        push(
            FeatureFamily::Cinematographer,
            &features.dp,
            &mut contributions,
        );
    }
    if config.roles.composer {
        push(FeatureFamily::Composer, &features.composer, &mut contributions);
    }
    if config.roles.actor && config.actor_mode != ActorMode::Removed {
        push(FeatureFamily::Actor, &features.cast, &mut contributions);
    }
    // Studio affinities are not yet a FeatureFamily — slot reserved; no silent inventing.
    let _ = config.roles.studio;

    let raw_craft_sum: f32 = contributions.iter().map(|c| c.contribution).sum();
    let unique_people = contributions.len();
    let mut all_films = std::collections::HashSet::new();
    for c in &contributions {
        for id in &c.support_film_ids {
            all_films.insert(*id);
        }
    }
    let unique_prior_films = all_films.len();

    // Lineage diminishing returns: credits that share the same prior films
    // collapse toward the strongest contribution in that lineage.
    let score = if config.lineage_diminishing && !contributions.is_empty() {
        lineage_effective_score(&contributions)
    } else if !contributions.is_empty() {
        let wsum: f32 = contributions
            .iter()
            .map(|c| c.candidate_role_weight.max(0.05))
            .sum();
        if wsum > 1e-4 {
            contributions
                .iter()
                .map(|c| c.contribution)
                .sum::<f32>()
                / wsum
        } else {
            0.0
        }
    } else {
        0.0
    };

    let film_scale = if unique_prior_films == 0 {
        0.0
    } else {
        unique_prior_films as f32 / (unique_prior_films as f32 + LINEAGE_DIMINISH_K)
    };
    let effective_craft_score = (score * (0.55 + 0.45 * film_scale)).clamp(-1.0, 1.0);
    let support: f32 = contributions.iter().map(|c| c.n_eff).sum();
    let effective_support_movies = if unique_prior_films == 0 {
        0.0
    } else {
        // Soft count: overlapping evidence films across people count once.
        unique_prior_films as f32 * film_scale
    };

    let without = |role: &str, contribs: &[CraftContribution]| -> f32 {
        let filtered: Vec<_> = contribs
            .iter()
            .filter(|c| !c.role.eq_ignore_ascii_case(role))
            .cloned()
            .collect();
        if filtered.is_empty() {
            0.0
        } else if config.lineage_diminishing {
            lineage_effective_score(&filtered)
        } else {
            let w: f32 = filtered.iter().map(|c| c.candidate_role_weight.max(0.05)).sum();
            if w > 1e-4 {
                filtered.iter().map(|c| c.contribution).sum::<f32>() / w
            } else {
                0.0
            }
        }
    };

    let lineage = CraftLineageDiag {
        raw_craft_sum,
        unique_prior_films,
        unique_people,
        effective_support_movies,
        effective_craft_score,
        without_actor: without("Actor", &contributions),
        without_director: without("Director", &contributions),
        without_writer: without("Writer", &contributions),
        without_dp: without("Cinematographer", &contributions),
        without_composer: without("Composer", &contributions),
        without_studio: effective_craft_score,
    };

    contributions.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.contribution.abs(), b.contribution.abs())
    });
    contributions.truncate(12);

    let confidence = if support <= 0.0 {
        0.12
    } else {
        let base = (1.0 - (-support / 8.0).exp()).clamp(0.12, 0.95);
        base * features.hydration_completeness.max(0.4) * (0.6 + 0.4 * film_scale)
    };

    CraftResult {
        score: effective_craft_score,
        confidence: confidence.clamp(0.1, 0.98),
        support,
        contributions,
        lineage,
    }
}

/// Collapse correlated credits that share prior-film lineage into one cluster signal.
fn lineage_effective_score(contributions: &[CraftContribution]) -> f32 {
    if contributions.is_empty() {
        return 0.0;
    }
    let mut used = vec![false; contributions.len()];
    let mut cluster_scores = Vec::new();
    for i in 0..contributions.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut best = contributions[i].contribution;
        let set_i: std::collections::HashSet<i64> =
            contributions[i].support_film_ids.iter().copied().collect();
        for j in (i + 1)..contributions.len() {
            if used[j] {
                continue;
            }
            let set_j: std::collections::HashSet<i64> =
                contributions[j].support_film_ids.iter().copied().collect();
            if set_i.is_empty() || set_j.is_empty() {
                continue;
            }
            let inter = set_i.intersection(&set_j).count();
            let union = set_i.union(&set_j).count().max(1);
            if inter as f32 / union as f32 >= 0.4 {
                used[j] = true;
                if contributions[j].contribution.abs() > best.abs() {
                    best = contributions[j].contribution;
                }
            }
        }
        cluster_scores.push(best);
    }
    if cluster_scores.is_empty() {
        return 0.0;
    }
    // Soft diminishing across independent lineages.
    cluster_scores.sort_by(|a, b| crate::taste::ord::cmp_f32_desc(a.abs(), b.abs()));
    let mut acc = 0.0;
    let mut w = 0.0;
    for (idx, s) in cluster_scores.iter().enumerate() {
        let decay = 1.0 / (1.0 + idx as f32 * 0.55);
        acc += s * decay;
        w += decay;
    }
    if w > 1e-4 {
        (acc / w).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Fit = Content + λ * Craft (Content frozen). Confidence stays orthogonal.
pub fn combine_content_craft(
    content: &ContentResult,
    craft: &CraftResult,
    lambda: f32,
) -> (f32, f32) {
    if lambda.abs() < 1e-6 {
        return (content.score, content.confidence);
    }
    let craft_presence = (craft.support / (craft.support + 6.0)).clamp(0.0, 1.0);
    let fit = (content.score + lambda * craft.score * craft_presence).clamp(-1.0, 1.0);
    let confidence = (0.72 * content.confidence
        + 0.28 * craft.confidence * craft_presence.max(0.25))
    .clamp(0.1, 0.98);
    (fit, confidence)
}

fn combine_fit(content: &ContentResult, craft: &CraftResult, include_craft: bool) -> (f32, f32) {
    if !include_craft {
        return (content.score, content.confidence);
    }
    combine_content_craft(content, craft, CraftConfig::experimental_craft().lambda)
}

fn ranking_score(fit: f32, confidence: f32) -> f32 {
    // Conservative: prefer higher fit; slight penalty when confidence is low.
    let fit = if fit.is_finite() { fit } else { 0.0 };
    let confidence = if confidence.is_finite() {
        confidence
    } else {
        0.5
    };
    fit + CONFIDENCE_RANK_NUDGE * (confidence - 0.5)
}

pub fn score_family_fit(
    profile: &FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
    mode: FitMode,
) -> FamilyFitResult {
    let config = match mode {
        FitMode::ContentOnly => FamilyFitConfig::fit_v1(),
        FitMode::ContentAndCraft | FitMode::Legacy => FamilyFitConfig {
            craft: CraftConfig::experimental_craft(),
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        },
    };
    score_family_fit_full(profile, candidate, semantic, &config, FitPriors::default())
}

pub fn score_family_fit_with_config(
    profile: &FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
    config: &CraftConfig,
) -> FamilyFitResult {
    score_family_fit_full(
        profile,
        candidate,
        semantic,
        &FamilyFitConfig {
            craft: *config,
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        },
        FitPriors::default(),
    )
}

pub fn score_family_fit_full(
    profile: &FeatureProfile,
    candidate: &Candidate,
    semantic: &SemanticScore,
    config: &FamilyFitConfig,
    priors: FitPriors<'_>,
) -> FamilyFitResult {
    let features = hydrate_candidate(profile, candidate, semantic.coverage);
    let content = score_content(profile, &features, semantic);
    let craft = score_craft_with_config(profile, &features, &config.craft, Some(&content));
    let form = match priors.form {
        Some(prior) if config.form.any_enabled() => score_form(
            prior,
            &features.modes,
            &features.genres,
            features.runtime,
            features.year,
            features.language.as_deref(),
            &config.form,
        ),
        _ => FormResult::default(),
    };
    let continuity = match priors.continuity {
        Some(prior) if config.continuity.any_enabled() => {
            score_continuity(prior, candidate, &config.continuity)
        }
        _ => ContinuityResult::default(),
    };
    let quality = match priors.quality {
        Some(catalog) if config.quality.any_enabled() => {
            score_quality(catalog, candidate, &config.quality)
        }
        _ => QualityResult::default(),
    };
    let (mut fit, mut confidence) =
        combine_content_craft(&content, &craft, config.craft.lambda);
    fit = (fit + form.score + continuity.score + quality.score).clamp(-1.0, 1.0);
    if form.support > 1e-4 {
        confidence = (0.85 * confidence + 0.15 * form.confidence).clamp(0.1, 0.98);
    }
    if continuity.support > 1e-4 {
        confidence = (0.88 * confidence + 0.12 * continuity.confidence).clamp(0.1, 0.98);
    }
    if quality.support > 1e-4 {
        confidence = (0.92 * confidence + 0.08 * quality.confidence).clamp(0.1, 0.98);
    }
    let craft_term = if config.craft.lambda.abs() < 1e-6 {
        0.0
    } else {
        let craft_presence = (craft.support / (craft.support + 6.0)).clamp(0.0, 1.0);
        config.craft.lambda * craft.score * craft_presence
    };
    let without_content_fit = craft_term + form.score + continuity.score + quality.score;
    let without_craft_fit = content.score + form.score + continuity.score + quality.score;
    let without_form_fit = content.score + craft_term + continuity.score + quality.score;
    let without_continuity_fit = content.score + craft_term + form.score + quality.score;
    let without_quality_fit = content.score + craft_term + form.score + continuity.score;

    FamilyFitResult {
        ranking_score: ranking_score(fit, confidence),
        fit,
        confidence,
        families: FamilyScores {
            content: FamilyResult {
                score: content.score,
                confidence: content.confidence,
                support: content.support,
            },
            craft: FamilyResult {
                score: craft.score,
                confidence: craft.confidence,
                support: craft.support,
            },
            form: FamilyResult {
                score: form.score,
                confidence: form.confidence,
                support: form.support,
            },
            continuity: FamilyResult {
                score: continuity.score,
                confidence: continuity.confidence,
                support: continuity.support,
            },
            quality: FamilyResult {
                score: quality.score,
                confidence: quality.confidence,
                support: quality.support,
            },
        },
        content_detail: content,
        craft_detail: craft,
        form_detail: form,
        continuity_detail: continuity,
        quality_detail: quality,
        features,
        counterfactual: Counterfactuals {
            without_content: CounterfactualSlice {
                fit: without_content_fit.clamp(-1.0, 1.0),
                rank_delta: 0,
            },
            without_craft: CounterfactualSlice {
                fit: without_craft_fit.clamp(-1.0, 1.0),
                rank_delta: 0,
            },
            without_form: CounterfactualSlice {
                fit: without_form_fit.clamp(-1.0, 1.0),
                rank_delta: 0,
            },
            without_continuity: CounterfactualSlice {
                fit: without_continuity_fit.clamp(-1.0, 1.0),
                rank_delta: 0,
            },
            without_quality: CounterfactualSlice {
                fit: without_quality_fit.clamp(-1.0, 1.0),
                rank_delta: 0,
            },
        },
    }
}

pub fn score_pool_family_fit_with_config(
    profile: &FeatureProfile,
    candidates: &[Candidate],
    semantic_scores: &HashMap<i64, SemanticScore>,
    config: &CraftConfig,
) -> Vec<(Candidate, FamilyFitResult)> {
    score_pool_family_fit_full(
        profile,
        candidates,
        semantic_scores,
        &FamilyFitConfig {
            craft: *config,
            form: FormConfig::off(),
            continuity: ContinuityConfig::off(),
            quality: QualityConfig::off(),
        },
        FitPriors::default(),
    )
}

pub fn score_pool_family_fit_full(
    profile: &FeatureProfile,
    candidates: &[Candidate],
    semantic_scores: &HashMap<i64, SemanticScore>,
    config: &FamilyFitConfig,
    priors: FitPriors<'_>,
) -> Vec<(Candidate, FamilyFitResult)> {
    let mut out: Vec<(Candidate, FamilyFitResult)> = candidates
        .iter()
        .map(|c| {
            let semantic = c
                .tmdb_id
                .and_then(|id| semantic_scores.get(&id))
                .cloned()
                .unwrap_or_default();
            let fit = score_family_fit_full(profile, c, &semantic, config, priors);
            (c.clone(), fit)
        })
        .collect();
    out.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.1.ranking_score, b.1.ranking_score).then_with(|| {
            a.0.tmdb_id
                .unwrap_or(i64::MAX)
                .cmp(&b.0.tmdb_id.unwrap_or(i64::MAX))
        })
    });
    out
}

pub fn fill_counterfactual_ranks(
    row: &mut FamilyFitResult,
    full_rank: usize,
    without_content_rank: usize,
    without_craft_rank: usize,
) {
    row.counterfactual.without_content.rank_delta =
        full_rank as i32 - without_content_rank as i32;
    row.counterfactual.without_craft.rank_delta = full_rank as i32 - without_craft_rank as i32;
}

/// Map family fit onto the legacy total scale roughly so workspace buffers still sort.
pub fn family_fit_as_legacy_total(fit: &FamilyFitResult) -> f32 {
    // Legacy totals are roughly in -1.5..=1.5; fit is -1..=1.
    fit.ranking_score.clamp(-1.5, 1.5)
}

pub fn score_pool_family_fit(
    profile: &FeatureProfile,
    candidates: &[Candidate],
    semantic_scores: &HashMap<i64, SemanticScore>,
    mode: FitMode,
) -> Vec<(Candidate, FamilyFitResult)> {
    let mut out: Vec<(Candidate, FamilyFitResult)> = candidates
        .iter()
        .map(|c| {
            let semantic = c
                .tmdb_id
                .and_then(|id| semantic_scores.get(&id))
                .cloned()
                .unwrap_or_default();
            let fit = score_family_fit(profile, c, &semantic, mode);
            (c.clone(), fit)
        })
        .collect();
    out.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.1.ranking_score, b.1.ranking_score).then_with(|| {
            a.0.tmdb_id
                .unwrap_or(i64::MAX)
                .cmp(&b.0.tmdb_id.unwrap_or(i64::MAX))
        })
    });
    // Counterfactual rank deltas
    let content_only_order: HashMap<String, usize> = {
        let mut tmp = candidates
            .iter()
            .map(|c| {
                let semantic = c
                    .tmdb_id
                    .and_then(|id| semantic_scores.get(&id))
                    .cloned()
                    .unwrap_or_default();
                let fit = score_family_fit(profile, c, &semantic, FitMode::ContentOnly);
                (identity_of(c), fit.ranking_score)
            })
            .collect::<Vec<_>>();
        tmp.sort_by(|a, b| {
            crate::taste::ord::cmp_f32_desc(a.1, b.1).then_with(|| a.0.cmp(&b.0))
        });
        tmp.into_iter()
            .enumerate()
            .map(|(i, (k, _))| (k, i + 1))
            .collect()
    };
    let craft_only_order: HashMap<String, usize> = {
        let mut tmp = candidates
            .iter()
            .map(|c| {
                let semantic = c
                    .tmdb_id
                    .and_then(|id| semantic_scores.get(&id))
                    .cloned()
                    .unwrap_or_default();
                let features = hydrate_candidate(profile, c, semantic.coverage);
                let craft = score_craft(profile, &features);
                let rank_key = ranking_score(craft.score, craft.confidence);
                (identity_of(c), rank_key)
            })
            .collect::<Vec<_>>();
        tmp.sort_by(|a, b| {
            crate::taste::ord::cmp_f32_desc(a.1, b.1).then_with(|| a.0.cmp(&b.0))
        });
        tmp.into_iter()
            .enumerate()
            .map(|(i, (k, _))| (k, i + 1))
            .collect()
    };
    for (i, (c, fit)) in out.iter_mut().enumerate() {
        let key = identity_of(c);
        let full_rank = i + 1;
        let wc = content_only_order.get(&key).copied().unwrap_or(full_rank);
        let wk = craft_only_order.get(&key).copied().unwrap_or(full_rank);
        // without_content ≈ craft-only ordering
        fill_counterfactual_ranks(fit, full_rank, wk, wc);
    }
    out
}

fn identity_of(c: &Candidate) -> String {
    crate::taste::retrieve::identity_key(c.tmdb_id, &c.title, c.year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::features::EvidenceFilm;
    use crate::taste::retrieve::MediaKind;

    fn aff(family: FeatureFamily, name: &str, mean: f32, appearances: u32) -> FeatureAffinity {
        FeatureAffinity {
            key: FeatureKey::new(family, None, name),
            appearances,
            weighted_mean: mean,
            preference_mean: mean,
            recommendation_mean: mean,
            weighted_variance: 0.1,
            positive_weight: if mean > 0.0 { mean.abs() } else { 0.0 },
            negative_weight: if mean < 0.0 { mean.abs() } else { 0.0 },
            recent_weight: 1.0,
            long_term_weight: 1.0,
            confidence: (appearances as f32 / (appearances as f32 + 2.0)).clamp(0.3, 0.95),
            feature_strength: 1.0,
            portability: 1.0,
            feedback_adjustment: 0.0,
            positive_evidence: vec![EvidenceFilm {
                title: "E".into(),
                rating: 4.5,
                tmdb_id: Some(1),
                people: vec![],
                keywords: vec![],
                genres: vec![],
                year: Some(2000),
                runtime: Some(100),
            }],
            negative_evidence: vec![],
            evidence_cluster: Default::default(),
        }
    }

    fn profile_with(rows: Vec<FeatureAffinity>) -> FeatureProfile {
        FeatureProfile {
            affinities: rows,
            polarizing: vec![],
            shifts: vec![],
            dimensions: vec![],
            modes: vec![],
            mode_shifts: vec![],
        }
    }

    fn candidate_with(genres: &[&str], keywords: &[&str], director: &str) -> Candidate {
        Candidate {
            tmdb_id: Some(42),
            title: "Test".into(),
            year: Some(2010),
            poster: None,
            genres: genres.iter().map(|s| (*s).into()).collect(),
            credits: vec![crate::taste::features::Credit {
                id: Some(7),
                name: director.into(),
                job: "Director".into(),
            }],
            keywords: keywords
                .iter()
                .map(|s| Keyword {
                    id: None,
                    name: (*s).into(),
                })
                .collect(),
            runtime: Some(110),
            vote_count: Some(1000),
            watchlist: false,
            sources: vec![],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        }
    }

    #[test]
    fn missing_embedding_is_unknown_not_negative() {
        let profile = profile_with(vec![]);
        let c = candidate_with(&["Drama"], &[], "Nobody");
        let features = hydrate_candidate(&profile, &c, false);
        let content = score_content(&profile, &features, &SemanticScore::default());
        assert!(content.score.abs() < 1e-5);
        assert!(content.semantic_positive.abs() < 1e-5);
        assert!(content.semantic_negative.abs() < 1e-5);
    }

    #[test]
    fn high_negative_semantic_can_drive_negative_content() {
        let profile = profile_with(vec![]);
        let c = candidate_with(&["Drama"], &[], "Nobody");
        let features = hydrate_candidate(&profile, &c, true);
        let a = score_content(
            &profile,
            &features,
            &SemanticScore {
                positive_similarity: 0.80,
                negative_similarity: 0.48,
                fit: 0.5,
                coverage: true,
                positive_matches: 3,
                negative_matches: 2,
            },
        );
        let b = score_content(
            &profile,
            &features,
            &SemanticScore {
                positive_similarity: 0.48,
                negative_similarity: 0.16,
                fit: 0.5,
                coverage: true,
                positive_matches: 3,
                negative_matches: 2,
            },
        );
        assert!((a.semantic_margin - b.semantic_margin).abs() < 1e-4);
        assert!(
            (a.score - b.score).abs() > 0.02,
            "equal-margin pairs must score differently: a={} b={}",
            a.score,
            b.score
        );
        let toxic = score_content(
            &profile,
            &features,
            &SemanticScore {
                positive_similarity: 0.35,
                negative_similarity: 0.72,
                fit: 0.2,
                coverage: true,
                positive_matches: 2,
                negative_matches: 3,
            },
        );
        assert!(
            toxic.score < -0.1,
            "dislike neighborhood must yield negative content, got {}",
            toxic.score
        );
    }

    #[test]
    fn polarized_semantic_not_rescued_by_mode_theme() {
        // Live failure: Jack-like .62/.60 with mode=1.0 outranked Rush-like .62/.49.
        let mut profile = profile_with(vec![]);
        profile.modes.push(crate::taste::dimensions::TasteMode {
            dimension: "spectacle".into(),
            strength: 1.0,
            members: vec![],
            recent_share: 0.4,
            long_term_share: 0.6,
        });
        let polarized = candidate_with(&["Adventure", "Fantasy"], &[], "Polarized");
        let mut features_p = hydrate_candidate(&profile, &polarized, true);
        features_p.modes = vec!["spectacle".into()];
        let polar = score_content(
            &profile,
            &features_p,
            &SemanticScore {
                positive_similarity: 0.623,
                negative_similarity: 0.597,
                fit: 0.5,
                coverage: true,
                positive_matches: 4,
                negative_matches: 3,
            },
        );
        let clear = candidate_with(&["Drama"], &[], "Clear");
        let features_c = hydrate_candidate(&profile, &clear, true);
        let healthy = score_content(
            &profile,
            &features_c,
            &SemanticScore {
                positive_similarity: 0.622,
                negative_similarity: 0.486,
                fit: 0.5,
                coverage: true,
                positive_matches: 4,
                negative_matches: 2,
            },
        );
        assert!(
            polar.semantic_margin < 0.05,
            "fixture margin {}",
            polar.semantic_margin
        );
        assert!(
            healthy.semantic_margin > 0.10,
            "healthy margin {}",
            healthy.semantic_margin
        );
        assert!(
            polar.mode_affinity > 0.5,
            "fixture should carry mode affinity, got {}",
            polar.mode_affinity
        );
        assert!(
            healthy.score > polar.score + 0.03,
            "healthy-margin Content must beat polarized+mode rescue: polar={} healthy={}",
            polar.score,
            healthy.score
        );
    }

    #[test]
    fn correlated_theme_labels_do_not_triple_count() {
        let profile = profile_with(vec![
            aff(FeatureFamily::Keyword, "boxing", 0.8, 3),
            aff(FeatureFamily::Keyword, "sport", 0.75, 3),
            aff(FeatureFamily::Keyword, "competition", 0.7, 3),
        ]);
        let c = candidate_with(
            &["Drama"],
            &["boxing", "sport", "competition"],
            "Nobody",
        );
        let features = hydrate_candidate(&profile, &c, false);
        let content = score_content(&profile, &features, &SemanticScore::default());
        assert!(
            content.motif_affinity <= 0.85,
            "correlated labels must consolidate, got {}",
            content.motif_affinity
        );
    }

    #[test]
    fn craft_shrinks_singleton_affinity() {
        let profile = profile_with(vec![aff(FeatureFamily::Director, "Nolan", 0.9, 1)]);
        let mut c = candidate_with(&["Drama"], &[], "Nolan");
        c.credits[0].id = None;
        let features = hydrate_candidate(&profile, &c, false);
        let craft = score_craft(&profile, &features);
        assert!(!craft.contributions.is_empty());
        assert!(
            craft.contributions[0].shrunk_affinity < 0.9 * 0.95,
            "singleton should shrink, got {}",
            craft.contributions[0].shrunk_affinity
        );
    }

    #[test]
    fn fit_and_confidence_stay_orthogonal() {
        let profile = profile_with(vec![aff(FeatureFamily::Director, "Villeneuve", 0.85, 5)]);
        let c = candidate_with(&["Drama"], &["tension"], "Villeneuve");
        let semantic = SemanticScore {
            positive_similarity: 0.78,
            negative_similarity: 0.12,
            fit: 0.8,
            coverage: true,
            positive_matches: 4,
            negative_matches: 1,
        };
        let fit = score_family_fit(&profile, &c, &semantic, FitMode::ContentAndCraft);
        assert!((fit.ranking_score - fit.fit).abs() < 0.1);
        let product = fit.fit * fit.confidence;
        assert!(
            (fit.ranking_score - product).abs() > 0.02,
            "ranking must not silently become fit*confidence"
        );
    }

    #[test]
    fn fit_v1_ignores_craft_contribution() {
        let profile = profile_with(vec![aff(FeatureFamily::Director, "Villeneuve", 0.95, 8)]);
        let c = candidate_with(&["Drama"], &["tension"], "Villeneuve");
        let semantic = SemanticScore {
            positive_similarity: 0.55,
            negative_similarity: 0.20,
            fit: 0.6,
            coverage: true,
            positive_matches: 3,
            negative_matches: 1,
        };
        let cfg = CraftConfig::fit_v1();
        assert_eq!(cfg.lambda, 0.0);
        assert_eq!(cfg.actor_mode, ActorMode::ContentCorroboration);
        let fit = score_family_fit_with_config(&profile, &c, &semantic, &cfg);
        assert!(
            (fit.fit - fit.families.content.score).abs() < 1e-5,
            "Fit_v1 must equal Content when λ=0"
        );
        // Craft still hydrates for diagnostics.
        assert!(
            fit.craft_detail.support > 0.0 || !fit.craft_detail.contributions.is_empty(),
            "Craft infrastructure should still compute"
        );
    }

    #[test]
    fn writer_screenplay_outranks_characters_job() {
        assert!(writer_job_weight("Screenplay") > writer_job_weight("Story"));
        assert!(writer_job_weight("Story") > writer_job_weight("Characters"));
    }

    #[test]
    fn craft_contribution_sort_survives_nan() {
        let mut contributions = vec![
            CraftContribution {
                name: "A".into(),
                role: "Director".into(),
                rated_support: 1,
                n_eff: 1.0,
                weighted_rating_affinity: 0.5,
                positive_evidence_weight: 1.0,
                negative_evidence_weight: 0.0,
                shrunk_affinity: 0.5,
                candidate_role_weight: 1.0,
                contribution: 0.4,
                support_film_ids: vec![],
            },
            CraftContribution {
                name: "B".into(),
                role: "Director".into(),
                rated_support: 1,
                n_eff: 1.0,
                weighted_rating_affinity: 0.5,
                positive_evidence_weight: 1.0,
                negative_evidence_weight: 0.0,
                shrunk_affinity: 0.5,
                candidate_role_weight: 1.0,
                contribution: f32::NAN,
                support_film_ids: vec![],
            },
        ];
        contributions.sort_by(|a, b| {
            crate::taste::ord::cmp_f32_desc(a.contribution.abs(), b.contribution.abs())
        });
        assert_eq!(contributions.len(), 2);
    }
}
