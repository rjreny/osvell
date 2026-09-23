//! Production recommendation policy.
//!
//! `taste-v1-fit-first`:
//! - Semantic retrieval is era-balanced, not vote-count truncated at 2,000
//! - Personal fit (content + shrunk people + era) orders the board
//! - TMDB quality is a tie-break inside a 0.02 fit bucket only
//! - Quality does not enter the fit number
//! - Critique and narration still use the model; they do not choose the list

use crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP;
use crate::taste::family_fit::FamilyFitConfig;
use crate::taste::hybrid_exam::HYBRID_BROAD_INDEX_CAP;
use crate::taste::workspace::NEW_MAX;

/// Human-readable freeze id (also mirrored in ALGORITHM_VERSION).
pub const V1_POLICY_ID: &str = "taste-v1-fit-first";

/// Assert the production wiring matches the current contract.
pub fn assert_v1_production_policy() {
    assert_eq!(V1_ACTIVE_SEMANTIC_CAP, 8_000);
    assert_eq!(HYBRID_BROAD_INDEX_CAP, 10_000);
    assert_eq!(NEW_MAX, 50);

    let families = FamilyFitConfig::personal();
    assert!(
        families.craft.lambda > 0.05 && families.craft.lambda < 0.5,
        "people kernel is on and bounded"
    );
    assert!(families.form.era, "era kernel is on");
    assert!(!families.form.runtime && !families.form.language);
    assert!(families.form.lambda > 0.0);
    assert!(!families.quality.any_enabled(), "TMDB quality stays out of fit");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::diversify::DiversifyConfig;
    use crate::taste::workspace::ALGORITHM_VERSION;

    #[test]
    fn v1_production_policy_frozen() {
        assert_v1_production_policy();
        assert_eq!(ALGORITHM_VERSION, V1_POLICY_ID);
        // D1.1 / F2 remain available as experiments, not production defaults.
        assert!(DiversifyConfig::light().fit_equivalence);
        assert!(!DiversifyConfig::light().recommendation_value);
        assert!(DiversifyConfig::light_with_f2().recommendation_value);
    }
}
