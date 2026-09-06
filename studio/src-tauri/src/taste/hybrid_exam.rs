//! F1 — Hybrid examination: proven 2k core + protected full-index discovery lane.
//!
//! Long-tail / broad-index candidates get a fixed slice of exam capacity without
//! crowding the validated 2k core pool. Same Content scoring once admitted —
//! no discovery bonus in Fit.

use crate::storage::db::Database;
use crate::taste::exam_policy::{with_active_semantic_cap, V1_ACTIVE_SEMANTIC_CAP};
use crate::taste::features::FeatureProfile;
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord, RetrievalPool,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Default total examination cut (unchanged from v1).
pub const HYBRID_EXAM_CAP: usize = 1_000;

/// Full stored semantic universe size used for the protected discovery lane.
pub const HYBRID_BROAD_INDEX_CAP: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridExamConfig {
    pub exam_cap: usize,
    /// Slots reserved for candidates that appear only under the broad index.
    pub broad_slots: usize,
    pub core_index_cap: usize,
    pub broad_index_cap: usize,
}

impl HybridExamConfig {
    pub fn control() -> Self {
        Self {
            exam_cap: HYBRID_EXAM_CAP,
            broad_slots: 0,
            core_index_cap: V1_ACTIVE_SEMANTIC_CAP,
            broad_index_cap: HYBRID_BROAD_INDEX_CAP,
        }
    }

    pub fn hybrid(broad_slots: usize) -> Self {
        Self {
            exam_cap: HYBRID_EXAM_CAP,
            broad_slots,
            core_index_cap: V1_ACTIVE_SEMANTIC_CAP,
            broad_index_cap: HYBRID_BROAD_INDEX_CAP,
        }
    }

    pub fn core_slots(self) -> usize {
        self.exam_cap.saturating_sub(self.broad_slots)
    }

    pub fn name(self) -> String {
        if self.broad_slots == 0 {
            "control".into()
        } else {
            format!("hybrid{}", self.broad_slots)
        }
    }
}

#[derive(Debug, Clone)]
pub struct HybridExamResult {
    pub config: HybridExamConfig,
    pub examined: Vec<Candidate>,
    pub core_selected: usize,
    pub broad_selected: usize,
    pub core_pool_size: usize,
    pub broad_pool_size: usize,
    pub broad_only_pool_size: usize,
    /// Keys admitted via the protected broad lane (not present in core pool).
    pub broad_only_keys: HashSet<String>,
    pub ms: f32,
}

/// Merge core fair-pool + protected broad-only fair-pool into one exam list.
pub fn merge_hybrid_exam(
    core_pool: &HashMap<String, Candidate>,
    broad_pool: &HashMap<String, Candidate>,
    cfg: HybridExamConfig,
) -> (Vec<Candidate>, HashSet<String>) {
    let core_keys: HashSet<String> = core_pool.keys().cloned().collect();
    let broad_only: HashMap<String, Candidate> = broad_pool
        .iter()
        .filter(|(k, _)| !core_keys.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let core_n = cfg.core_slots();
    let mut core_exam = select_fair_pool(core_pool.clone(), core_n);
    let broad_exam = select_fair_pool(broad_only.clone(), cfg.broad_slots);

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(cfg.exam_cap);
    let mut broad_only_keys = HashSet::new();

    for c in core_exam.drain(..) {
        let key = identity_key(c.tmdb_id, &c.title, c.year);
        if seen.insert(key) {
            out.push(c);
        }
    }
    for c in broad_exam {
        let key = identity_key(c.tmdb_id, &c.title, c.year);
        if seen.insert(key.clone()) {
            broad_only_keys.insert(key);
            out.push(c);
        }
    }

    // Backfill from remaining core pool if broad lane under-filled.
    if out.len() < cfg.exam_cap {
        let refill = select_fair_pool(core_pool.clone(), cfg.exam_cap);
        for c in refill {
            if out.len() >= cfg.exam_cap {
                break;
            }
            let key = identity_key(c.tmdb_id, &c.title, c.year);
            if seen.insert(key) {
                out.push(c);
            }
        }
    }

    out.truncate(cfg.exam_cap);
    (out, broad_only_keys)
}

pub fn select_hybrid_exam(
    db: &Database,
    films: &[FilmRecord],
    profile: &FeatureProfile,
    seen: &HashSet<String>,
    cfg: HybridExamConfig,
) -> Result<HybridExamResult, String> {
    let t0 = std::time::Instant::now();

    let pool_core: RetrievalPool =
        with_active_semantic_cap(Some(cfg.core_index_cap), || {
            build_retrieval_pool(db, films, profile, seen, false)
        })?;

    let (examined, broad_only_keys, broad_pool_size, broad_only_pool_size) =
        if cfg.broad_slots == 0 {
            let examined = select_fair_pool(pool_core.by_key.clone(), cfg.exam_cap);
            (examined, HashSet::new(), 0, 0)
        } else {
            let pool_broad: RetrievalPool =
                with_active_semantic_cap(Some(cfg.broad_index_cap), || {
                    build_retrieval_pool(db, films, profile, seen, false)
                })?;
            let broad_only_n = pool_broad
                .by_key
                .keys()
                .filter(|k| !pool_core.by_key.contains_key(*k))
                .count();
            let broad_size = pool_broad.by_key.len();
            let (examined, broad_keys) =
                merge_hybrid_exam(&pool_core.by_key, &pool_broad.by_key, cfg);
            (examined, broad_keys, broad_size, broad_only_n)
        };

    let broad_selected = broad_only_keys.len();
    let core_selected = examined.len().saturating_sub(broad_selected);

    Ok(HybridExamResult {
        config: cfg,
        examined,
        core_selected,
        broad_selected,
        core_pool_size: pool_core.by_key.len(),
        broad_pool_size,
        broad_only_pool_size,
        broad_only_keys,
        ms: t0.elapsed().as_secs_f32() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::retrieve::{MediaKind, RetrievalKind, RetrievalSource};

    fn cand(id: i64, title: &str, watchlist: bool) -> Candidate {
        Candidate {
            tmdb_id: Some(id),
            title: title.into(),
            year: Some(2000),
            poster: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            runtime: Some(100),
            vote_count: Some(100),
            watchlist,
            sources: vec![RetrievalSource {
                kind: if watchlist {
                    RetrievalKind::Watchlist
                } else {
                    RetrievalKind::Discovery
                },
                label: "t".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: Some(0.5),
                neighbor_rank: Some(1),
            }],
            friend_affinity: 0.0,
            tmdb_related: 0.0,
            media_kind: MediaKind::Movie,
        }
    }

    #[test]
    fn control_uses_only_core() {
        let mut core = HashMap::new();
        for i in 1..=50 {
            core.insert(format!("tmdb:{i}"), cand(i, &format!("C{i}"), false));
        }
        let broad = core.clone();
        let (out, broad_keys) = merge_hybrid_exam(&core, &broad, HybridExamConfig::control());
        assert_eq!(out.len(), 50.min(HYBRID_EXAM_CAP));
        assert!(broad_keys.is_empty());
    }

    #[test]
    fn protected_lane_admits_broad_only() {
        let mut core = HashMap::new();
        for i in 1..=20 {
            core.insert(format!("tmdb:{i}"), cand(i, &format!("C{i}"), false));
        }
        let mut broad = core.clone();
        for i in 100..120 {
            broad.insert(format!("tmdb:{i}"), cand(i, &format!("B{i}"), false));
        }
        let cfg = HybridExamConfig {
            exam_cap: 25,
            broad_slots: 5,
            core_index_cap: 2_000,
            broad_index_cap: 10_000,
        };
        let (out, broad_keys) = merge_hybrid_exam(&core, &broad, cfg);
        assert_eq!(out.len(), 25);
        assert_eq!(broad_keys.len(), 5);
        for k in &broad_keys {
            let id: i64 = k.trim_start_matches("tmdb:").parse().unwrap();
            assert!(id >= 100, "expected broad-only id, got {k}");
            assert!(!core.contains_key(k));
        }
    }

    #[test]
    fn broad_underfill_backfills_from_core() {
        let mut core = HashMap::new();
        for i in 1..=30 {
            core.insert(format!("tmdb:{i}"), cand(i, &format!("C{i}"), false));
        }
        // Broad pool identical to core → no broad-only candidates.
        let broad = core.clone();
        let cfg = HybridExamConfig {
            exam_cap: 25,
            broad_slots: 10,
            core_index_cap: 2_000,
            broad_index_cap: 10_000,
        };
        let (out, broad_keys) = merge_hybrid_exam(&core, &broad, cfg);
        assert_eq!(out.len(), 25);
        assert!(broad_keys.is_empty());
    }
}
