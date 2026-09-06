//! V1 production freeze — locked recommendation policy.
//!
//! Do not enable F2 or hybrid150 in production without a new experiment id and
//! a live A/B that clearly beats this baseline. Infrastructure may remain in-tree.

use crate::taste::diversify::DiversifyConfig;
use crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP;
use crate::taste::family_fit::{CraftConfig, FamilyFitConfig};
use crate::taste::hybrid_exam::HYBRID_BROAD_INDEX_CAP;
use crate::taste::workspace::{FEATURED_MAX, NEW_MAX};

/// Human-readable freeze id (also mirrored in ALGORITHM_VERSION).
pub const V1_POLICY_ID: &str = "taste-v1-active2k-d1";

/// Assert the production wiring matches the frozen v1 contract.
pub fn assert_v1_production_policy() {
    assert_eq!(V1_ACTIVE_SEMANTIC_CAP, 2_000);
    assert_eq!(HYBRID_BROAD_INDEX_CAP, 10_000);
    assert_eq!(NEW_MAX, 50);
    assert_eq!(FEATURED_MAX, 12);

    let cfg = DiversifyConfig::light();
    assert!(cfg.fit_equivalence, "D1.1 fit-equivalence required");
    assert!(
        (cfg.tie_epsilon - 0.0075).abs() < 1e-6,
        "D1.1 ε must be 0.0075, got {}",
        cfg.tie_epsilon
    );
    assert!(
        !cfg.recommendation_value,
        "F2 must be OFF in production DiversifyConfig::light()"
    );

    let craft = CraftConfig::fit_v1();
    assert!(
        (craft.lambda - 0.0).abs() < 1e-9,
        "Fit_v1 = Content only (Craft λ=0)"
    );
    let families = FamilyFitConfig::fit_v1();
    assert!((families.craft.lambda - 0.0).abs() < 1e-9);
    assert!((families.form.lambda - 0.0).abs() < 1e-9);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::workspace::ALGORITHM_VERSION;

    #[test]
    fn v1_production_policy_frozen() {
        assert_v1_production_policy();
        assert_eq!(ALGORITHM_VERSION, V1_POLICY_ID);
        // Experimental constructors exist but are not production defaults.
        assert!(DiversifyConfig::light_with_f2().recommendation_value);
        assert!(!DiversifyConfig::light().recommendation_value);
    }
}
