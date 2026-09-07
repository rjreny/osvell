//! V1 production freeze — locked recommendation policy.
//!
//! `taste-v1-quality-first-final`:
//! - Broad retrieval + hard filters + C1 Content Fit admission
//! - Known shrunk quality prior G orders the board (missing G below known G)
//! - Content Fit may break ties only when `|ΔG| ≤ 0.04`
//! - Light diversity only inside those fixed G groups (no transitive widening)
//! - Match % remains Content Fit; `score.total` remains Content Fit
//! - B4 Quality Fit λ contribution remains OFF
//! - Watchlist is state only; no v2 / CF / MF / AI in production

use crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP;
use crate::taste::family_fit::{CraftConfig, FamilyFitConfig};
use crate::taste::hybrid_exam::HYBRID_BROAD_INDEX_CAP;
use crate::taste::quality::QUALITY_TIE_EPSILON;
use crate::taste::workspace::NEW_MAX;

/// Human-readable freeze id (also mirrored in ALGORITHM_VERSION).
pub const V1_POLICY_ID: &str = "taste-v1-quality-first-final";

/// Assert the production wiring matches the frozen v1 contract.
pub fn assert_v1_production_policy() {
    assert_eq!(V1_ACTIVE_SEMANTIC_CAP, 2_000);
    assert_eq!(HYBRID_BROAD_INDEX_CAP, 10_000);
    assert_eq!(NEW_MAX, 50);
    assert!((QUALITY_TIE_EPSILON - 0.04).abs() < 1e-9);

    let craft = CraftConfig::fit_v1();
    assert!(
        (craft.lambda - 0.0).abs() < 1e-9,
        "Fit_v1 Content path keeps Craft λ=0"
    );
    let families = FamilyFitConfig::fit_v1();
    assert!((families.craft.lambda - 0.0).abs() < 1e-9);
    assert!((families.form.lambda - 0.0).abs() < 1e-9);
    assert!(!families.quality.any_enabled(), "B4 Quality Fit contribution stays OFF");
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
