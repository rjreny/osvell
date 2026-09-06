//! E1.1 examination allocation: semantic consolidation + family soft caps.
//!
//! Frozen: generator definitions, query vectors, Content Fit, C1, D1.1.
//! Only examination merge / capacity allocation changes.
//!
//! Live matrix (HOLD_E1_1): none of the 10k@1000 allocation modes beat the 2k
//! control @1000. Production therefore keeps the full embedding store but uses a
//! narrower *active* retrieval universe ([`V1_ACTIVE_SEMANTIC_CAP`]) until F1
//! hybrid discovery (core 2k + protected 10k lane) is calibrated.

use serde::Serialize;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::taste::retrieve::GeneratorFamily;

/// Vote-count-ranked active search size while 10k embeddings stay stored.
/// E1.1 matrix: 10k active @1000 still lags 2k control; prefer this over forcing breadth.
pub const V1_ACTIVE_SEMANTIC_CAP: usize = 2_000;

/// 0 = use [`V1_ACTIVE_SEMANTIC_CAP`]. Benches set an explicit override via
/// [`crate::taste::semantic::with_semantic_index_cap`].
static ACTIVE_SEMANTIC_CAP_OVERRIDE: AtomicUsize = AtomicUsize::new(0);

/// Allocation matrix modes for E1.1 calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum ExamMode {
    /// Legacy fair-pool + wide FilmLocal retrieval.
    Current = 0,
    /// Tighter seed diversity + FilmLocal consolidation scoring.
    SemanticDedupe = 1,
    /// Soft per-family caps on the 1k examination cut.
    FamilySoftCaps = 2,
    /// Both levers.
    DedupeAndSoftCaps = 3,
}

impl ExamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::SemanticDedupe => "semanticDedupe",
            Self::FamilySoftCaps => "familySoftCaps",
            Self::DedupeAndSoftCaps => "dedupeAndSoftCaps",
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::SemanticDedupe,
            2 => Self::FamilySoftCaps,
            3 => Self::DedupeAndSoftCaps,
            _ => Self::Current,
        }
    }

    pub fn all() -> &'static [ExamMode] {
        &[
            Self::Current,
            Self::SemanticDedupe,
            Self::FamilySoftCaps,
            Self::DedupeAndSoftCaps,
        ]
    }
}

/// Production default after HOLD_E1_1: keep the 2k-proven examination merge.
static EXAM_MODE: AtomicU8 = AtomicU8::new(ExamMode::Current as u8);

pub fn exam_mode() -> ExamMode {
    ExamMode::from_u8(EXAM_MODE.load(Ordering::SeqCst))
}

pub fn with_exam_mode<T>(mode: ExamMode, f: impl FnOnce() -> T) -> T {
    let prev = EXAM_MODE.swap(mode as u8, Ordering::SeqCst);
    let out = f();
    EXAM_MODE.store(prev, Ordering::SeqCst);
    out
}

/// Effective active semantic index size for retrieval (not storage).
pub fn active_semantic_index_cap() -> usize {
    let over = ACTIVE_SEMANTIC_CAP_OVERRIDE.load(Ordering::SeqCst);
    if over > 0 {
        over
    } else {
        V1_ACTIVE_SEMANTIC_CAP
    }
}

/// Override the active retrieval cap for the duration of `f`.
/// Pass `None` / `Some(0)` to restore production default ([`V1_ACTIVE_SEMANTIC_CAP`]).
/// Pass `Some(n)` to force a specific active size (large `n` ≈ full index).
pub fn with_active_semantic_cap<T>(cap: Option<usize>, f: impl FnOnce() -> T) -> T {
    let prev = ACTIVE_SEMANTIC_CAP_OVERRIDE.swap(cap.unwrap_or(0), Ordering::SeqCst);
    let out = f();
    ACTIVE_SEMANTIC_CAP_OVERRIDE.store(prev, Ordering::SeqCst);
    out
}

pub fn semantic_consolidate() -> bool {
    matches!(
        exam_mode(),
        ExamMode::SemanticDedupe | ExamMode::DedupeAndSoftCaps
    )
}

pub fn family_soft_caps_enabled() -> bool {
    matches!(
        exam_mode(),
        ExamMode::FamilySoftCaps | ExamMode::DedupeAndSoftCaps
    )
}

/// FilmLocal seed budget. Consolidation mode trades volume for distinct neighborhoods.
pub fn local_seed_cap() -> usize {
    if semantic_consolidate() {
        18
    } else {
        32
    }
}

pub fn local_neighbors() -> usize {
    if semantic_consolidate() {
        30
    } else {
        50
    }
}

/// Max cosine between FilmLocal query seeds. Lower ⇒ more retrieval diversity.
pub fn seed_diversity_max_sim() -> f32 {
    if semantic_consolidate() {
        0.85
    } else {
        0.92
    }
}

/// Soft ceiling so one high-volume family cannot consume nearly all of `cap`.
/// Returns `None` when soft caps are off or the family is uncapped.
pub fn family_soft_cap(family: GeneratorFamily, total_cap: usize) -> Option<usize> {
    if !family_soft_caps_enabled() || total_cap == 0 {
        return None;
    }
    let pct = match family {
        GeneratorFamily::SemanticFilmLocal => 22, // ~220 of 1000
        GeneratorFamily::SemanticProfile => 18,   // ~180
        GeneratorFamily::Collection => 12,        // ~120
        GeneratorFamily::Filmography => 20,       // ~200
        GeneratorFamily::Related => 28,           // ~280
        GeneratorFamily::Discovery | GeneratorFamily::Exploration => 8,
        GeneratorFamily::Friend | GeneratorFamily::Watchlist => 6,
    };
    Some(((total_cap * pct) / 100).max(16))
}
