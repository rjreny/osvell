//! Frozen preference/scoring constants. This file was captured before the
//! recommendation-workspace epic. Workspace code must not change these values.

/// Formula id mixed into scoring fingerprints. Bump only when W_* change.
pub const FROZEN_FORMULA_ID: &str = "w-content-0.45-tmdb-0.20-friend-0.15-recent-0.10-watchlist-0.05-novelty-0.00-negative-0.35-semantic-blend-0.35";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::preference::{
        ABSOLUTE_NEUTRAL, HEART_WEIGHT, RECENCY_LAMBDA, RECENCY_MAX, RECENCY_MIN, RELATIVE_CLAMP,
        STD_FLOOR,
    };
    use crate::taste::score::{
        W_CONTENT, W_FRIEND, W_NEGATIVE, W_NOVELTY, W_RECENT, W_TMDB, W_WATCHLIST,
    };

    #[test]
    fn scoring_weights_match_baseline_snapshot() {
        assert_eq!(W_CONTENT, 0.45);
        assert_eq!(W_TMDB, 0.20);
        assert_eq!(W_FRIEND, 0.15);
        assert_eq!(W_RECENT, 0.10);
        assert_eq!(W_WATCHLIST, 0.05);
        assert_eq!(W_NOVELTY, 0.0);
        assert_eq!(W_NEGATIVE, 0.35);
        assert_eq!(crate::taste::score::W_SEMANTIC, 0.35);
        assert_eq!(
            FROZEN_FORMULA_ID,
            "w-content-0.45-tmdb-0.20-friend-0.15-recent-0.10-watchlist-0.05-novelty-0.00-negative-0.35-semantic-blend-0.35"
        );
    }

    #[test]
    fn preference_constants_match_baseline_snapshot() {
        assert_eq!(RECENCY_MAX, 1.35);
        assert_eq!(RECENCY_MIN, 0.70);
        assert_eq!(RECENCY_LAMBDA, 0.51);
        assert_eq!(STD_FLOOR, 0.35);
        assert_eq!(ABSOLUTE_NEUTRAL, 3.0);
        assert_eq!(HEART_WEIGHT, 1.25);
        assert_eq!(RELATIVE_CLAMP, 2.5);
    }
}
