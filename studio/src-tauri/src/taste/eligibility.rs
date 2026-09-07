//! C1 decision layer: Content Fit_v1 eligibility bands.
//!
//! Fit dominates. Confidence modulates trust (Recommended vs Exploratory),
//! it does not auto-kill high-fit discoveries. Retrieval provenance,
//! EvidenceGrade, and neighbor floors are intentionally unused.
//!
//! Absolute floors are a safety net. Live pools use **relative** bands derived
//! from the scored pool's Content Fit distribution (Content scores compress).

use serde::{Deserialize, Serialize};

/// Internal New-board admission state. UI may collapse Exploratory into New.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EligibilityState {
    Recommended,
    Exploratory,
    #[default]
    Held,
}

impl EligibilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::Exploratory => "exploratory",
            Self::Held => "held",
        }
    }

    pub fn board_eligible(self) -> bool {
        matches!(self, Self::Recommended | Self::Exploratory)
    }
}

/// Inputs for the decision layer. No retrieval / EvidenceGrade fields.
#[derive(Debug, Clone, Copy)]
pub struct EligibilityInput {
    /// Content Fit_v1 mapped to 0..=1 via `(score + 1) * 0.5`.
    pub predicted_fit: f32,
    pub confidence: f32,
    pub hydration_completeness: f32,
    pub semantic_coverage: bool,
    pub semantic_negative: f32,
    pub semantic_margin: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityDecision {
    pub state: EligibilityState,
    pub primary_reason: String,
    pub fit: f32,
    pub confidence: f32,
    pub hydration_completeness: f32,
}

/// Fit cut points. Prefer `bands_from_fits` over the absolute fallback.
#[derive(Debug, Clone, Copy)]
pub struct EligibilityBands {
    pub excellent: f32,
    pub good: f32,
    pub soft: f32,
    pub poor: f32,
}

impl EligibilityBands {
    /// Absolute fallback when the pool is too small to estimate percentiles.
    pub fn absolute_fallback() -> Self {
        Self {
            excellent: 0.67,
            good: 0.63,
            soft: 0.58,
            poor: 0.54,
        }
    }
}

const CONF_LOW: f32 = 0.35;
const HYDRATION_MIN_WITHOUT_SEMANTIC: f32 = 0.22;
const CONTRADICTION_MARGIN: f32 = -0.12;
const CONTRADICTION_NEG: f32 = 0.45;
/// Never admit below this even if the pool is uniformly weak.
const ABSOLUTE_FLOOR: f32 = 0.48;

pub fn content_score_to_predicted_fit(content_score: f32) -> f32 {
    ((content_score + 1.0) * 0.5).clamp(0.0, 1.0)
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.5;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Derive fit-dominant bands from the current scored pool / holdout fits.
/// p40→poor, p55→soft, p70→good, p85→excellent (clamped to stay ordered).
pub fn bands_from_fits(fits: &[f32]) -> EligibilityBands {
    if fits.len() < 12 {
        return EligibilityBands::absolute_fallback();
    }
    let mut sorted = fits.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut poor = percentile(&sorted, 0.40).max(ABSOLUTE_FLOOR);
    let mut soft = percentile(&sorted, 0.55).max(poor + 0.01);
    let mut good = percentile(&sorted, 0.70).max(soft + 0.01);
    let mut excellent = percentile(&sorted, 0.85).max(good + 0.01);
    // Keep a usable exploratory band even when scores are tightly clustered.
    if excellent - poor < 0.03 {
        let mid = 0.5 * (poor + excellent);
        poor = (mid - 0.02).max(ABSOLUTE_FLOOR);
        soft = mid;
        good = mid + 0.015;
        excellent = mid + 0.03;
    }
    EligibilityBands {
        excellent,
        good,
        soft,
        poor,
    }
}

pub fn classify_eligibility(input: &EligibilityInput) -> EligibilityDecision {
    classify_with_bands(input, &EligibilityBands::absolute_fallback())
}

pub fn classify_with_bands(
    input: &EligibilityInput,
    bands: &EligibilityBands,
) -> EligibilityDecision {
    let fit = input.predicted_fit.clamp(0.0, 1.0);
    let confidence = input.confidence.clamp(0.0, 1.0);
    let hydration = input.hydration_completeness.clamp(0.0, 1.0);

    let decide = |state: EligibilityState, reason: &'static str| EligibilityDecision {
        state,
        primary_reason: reason.into(),
        fit,
        confidence,
        hydration_completeness: hydration,
    };

    if !input.semantic_coverage
        && hydration < HYDRATION_MIN_WITHOUT_SEMANTIC
        && fit < bands.soft
    {
        return decide(EligibilityState::Held, "insufficient_hydration");
    }
    if input.semantic_coverage
        && input.semantic_margin < CONTRADICTION_MARGIN
        && input.semantic_negative > CONTRADICTION_NEG
    {
        return decide(EligibilityState::Held, "semantic_contradiction");
    }
    if fit < bands.poor.max(ABSOLUTE_FLOOR) {
        return decide(EligibilityState::Held, "low_fit");
    }

    let conf_for_admit = if input.semantic_coverage {
        CONF_LOW
    } else {
        0.18
    };

    if fit >= bands.excellent {
        if confidence >= conf_for_admit {
            return decide(EligibilityState::Recommended, "recommended");
        }
        return decide(EligibilityState::Exploratory, "high_fit_low_confidence");
    }

    if fit >= bands.good {
        if confidence >= conf_for_admit {
            return decide(EligibilityState::Recommended, "recommended");
        }
        return decide(EligibilityState::Exploratory, "high_fit_low_confidence");
    }

    if fit >= bands.soft {
        if confidence >= conf_for_admit {
            return decide(EligibilityState::Exploratory, "exploratory_fill");
        }
        return decide(EligibilityState::Held, "outside_primary_band");
    }

    decide(EligibilityState::Held, "outside_primary_band")
}

pub fn is_board_eligible(state: EligibilityState) -> bool {
    state.board_eligible()
}

/// Re-apply relative bands to the New lane only (mutates eligibility).
///
/// Callers must pass **New-capable** rows (not watchlist). Include provisional
/// absolute Holds so percentiles reflect the full New distribution — watchlist
/// must never compete for these scarce admission slots.
pub fn apply_pool_bands(rows: &mut [crate::taste::score::ScoredCandidate]) {
    let fits: Vec<f32> = rows
        .iter()
        .filter(|c| !c.candidate.watchlist)
        .filter(|c| {
            // Hard exclusions stay out of band estimation.
            !matches!(
                c.eligibility.primary_reason.as_str(),
                "short-runtime" | "semantic_contradiction" | "insufficient_hydration"
            )
        })
        .map(|c| c.eligibility.predicted_fit)
        .collect();
    let bands = bands_from_fits(&fits);
    for row in rows.iter_mut() {
        if row.candidate.watchlist {
            continue;
        }
        if matches!(
            row.eligibility.primary_reason.as_str(),
            "short-runtime" | "semantic_contradiction" | "insufficient_hydration"
        ) {
            continue;
        }
        let decision = classify_with_bands(
            &EligibilityInput {
                predicted_fit: row.eligibility.predicted_fit,
                confidence: row.eligibility.confidence,
                hydration_completeness: row.eligibility.hydration_completeness,
                semantic_coverage: row.score.semantic_coverage,
                semantic_negative: 0.0, // already applied on first pass
                semantic_margin: 0.0,
            },
            &bands,
        );
        row.eligibility.state = decision.state.as_str().into();
        row.eligibility.primary_reason = decision.primary_reason.clone();
        row.eligibility.passed = decision.state.board_eligible();
        row.eligibility.passed_because = vec![format!(
            "c1:{}:{}",
            decision.state.as_str(),
            decision.primary_reason
        )];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(fit: f32, conf: f32) -> EligibilityInput {
        EligibilityInput {
            predicted_fit: fit,
            confidence: conf,
            hydration_completeness: 0.7,
            semantic_coverage: true,
            semantic_negative: 0.1,
            semantic_margin: 0.2,
        }
    }

    #[test]
    fn excellent_medium_confidence_is_recommended() {
        let d = classify_eligibility(&input(0.72, 0.45));
        assert_eq!(d.state, EligibilityState::Recommended);
    }

    #[test]
    fn good_high_confidence_is_recommended() {
        let d = classify_eligibility(&input(0.64, 0.72));
        assert_eq!(d.state, EligibilityState::Recommended);
    }

    #[test]
    fn excellent_low_confidence_is_exploratory_not_held() {
        let d = classify_eligibility(&input(0.72, 0.28));
        assert_eq!(d.state, EligibilityState::Exploratory);
        assert_eq!(d.primary_reason, "high_fit_low_confidence");
    }

    #[test]
    fn mediocre_high_confidence_is_held() {
        let d = classify_eligibility(&input(0.55, 0.90));
        assert_eq!(d.state, EligibilityState::Held);
    }

    #[test]
    fn poor_fit_is_held() {
        let d = classify_eligibility(&input(0.40, 0.81));
        assert_eq!(d.state, EligibilityState::Held);
        assert_eq!(d.primary_reason, "low_fit");
    }

    #[test]
    fn soft_fit_can_be_exploratory() {
        let d = classify_eligibility(&input(0.60, 0.40));
        assert_eq!(d.state, EligibilityState::Exploratory);
    }

    #[test]
    fn neutral_content_is_held() {
        let d = classify_eligibility(&input(0.50, 0.60));
        assert_eq!(d.state, EligibilityState::Held);
    }

    #[test]
    fn no_hard_and_trap_kills_high_fit_medium_conf() {
        let d = classify_eligibility(&input(0.68, 0.40));
        assert!(d.state.board_eligible());
    }

    #[test]
    fn relative_bands_separate_compressed_fits() {
        let fits: Vec<f32> = (0..100).map(|i| 0.60 + (i as f32) * 0.0005).collect();
        let bands = bands_from_fits(&fits);
        assert!(bands.poor < bands.soft);
        assert!(bands.soft < bands.good);
        assert!(bands.good < bands.excellent);
        let low = classify_with_bands(&input(bands.poor - 0.001, 0.6), &bands);
        let mid = classify_with_bands(&input(0.5 * (bands.soft + bands.good), 0.6), &bands);
        let high = classify_with_bands(&input(bands.excellent + 0.001, 0.6), &bands);
        assert_eq!(low.state, EligibilityState::Held);
        assert!(mid.state.board_eligible());
        assert_eq!(high.state, EligibilityState::Recommended);
    }

    #[test]
    fn content_score_maps_to_unit_interval() {
        assert!((content_score_to_predicted_fit(-1.0) - 0.0).abs() < 1e-5);
        assert!((content_score_to_predicted_fit(0.0) - 0.5).abs() < 1e-5);
        assert!((content_score_to_predicted_fit(1.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn semantic_contradiction_holds() {
        let mut i = input(0.75, 0.60);
        i.semantic_margin = -0.2;
        i.semantic_negative = 0.55;
        let d = classify_eligibility(&i);
        assert_eq!(d.state, EligibilityState::Held);
        assert_eq!(d.primary_reason, "semantic_contradiction");
    }
}
