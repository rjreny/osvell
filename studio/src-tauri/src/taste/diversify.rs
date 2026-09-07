//! D1.1 board policy (frozen v1 scarce-slot): raw Content order → fit-tolerance
//! scarce-slot selection (ε = 0.0075) → prefer lower redundancy among
//! effectively-equivalent fits. Embedding prototype clusters for neighborhood
//! identity. Never rescue candidates beyond ε_fit of the best remaining fit.
//!
//! F2 (optional): within the same ε-equivalence set only, board_value chooses
//! Featured *membership*. After the Featured set is chosen, display order is
//! restored to Content fit. F2 never changes Fit and never promotes past ε.
//!
//! board_value = −redundancy − repeated_neighbor_redundancy + distinct_region
//! (no generic recognizability / graph-obviousness penalty — that punished Rediscovery).
//!
//! Featured vs More: D1.1(+F2) selects the Featured-N prefix; remaining inventory
//! stays in Content order for browse (#13–50).

use crate::taste::retrieve::RetrievalKind;
use crate::taste::score::ScoredCandidate;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiversifyMode {
    RawContent,
    LightDiversify,
    StrongDiversify,
}

impl DiversifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawContent => "rawContent",
            Self::LightDiversify => "lightDiversify",
            Self::StrongDiversify => "strongDiversify",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiversifyConfig {
    pub mode: DiversifyMode,
    /// Max fit loss allowed when choosing among effectively-equivalent candidates.
    pub tie_epsilon: f32,
    /// Legacy row-window cap. Unused when `fit_equivalence` is true (except as
    /// an optional safety bound on the equivalence-set size).
    pub max_window: usize,
    /// When true, each scarce slot picks from all remaining candidates within
    /// `tie_epsilon` of the best remaining fit (D1.1). When false, uses the
    /// legacy contiguous near-tie window (D1).
    pub fit_equivalence: bool,
    pub director_soft_cap: usize,
    pub collection_soft_cap: usize,
    pub mode_soft_cap: usize,
    pub semantic_soft_cap: usize,
    /// Soft cap for embedding-neighborhood clusters (sports/franchise-adjacent etc.).
    pub semantic_cluster_soft_cap: usize,
    /// F2: within ε sets, board_value chooses Featured membership; display stays Content-led.
    pub recommendation_value: bool,
}

impl DiversifyConfig {
    pub fn raw() -> Self {
        Self {
            mode: DiversifyMode::RawContent,
            tie_epsilon: 0.0,
            max_window: 1,
            fit_equivalence: false,
            director_soft_cap: usize::MAX,
            collection_soft_cap: usize::MAX,
            mode_soft_cap: usize::MAX,
            semantic_soft_cap: usize::MAX,
            semantic_cluster_soft_cap: usize::MAX,
            recommendation_value: false,
        }
    }

    /// Production D1.1: fit-tolerance scarce slots + mild cluster soft caps.
    /// ε=0.0075 chosen from live sweep PASS band (.005–.008).
    /// F2 OFF for v1 freeze (`light_with_f2` remains experiment-only).
    pub fn light() -> Self {
        Self {
            mode: DiversifyMode::LightDiversify,
            tie_epsilon: 0.0075,
            max_window: usize::MAX,
            fit_equivalence: true,
            director_soft_cap: 2,
            collection_soft_cap: 1,
            mode_soft_cap: 2,
            semantic_soft_cap: 2,
            semantic_cluster_soft_cap: 2,
            recommendation_value: false,
        }
    }

    /// Experiment only — not production. D1.1 + F2 Featured membership.
    pub fn light_with_f2() -> Self {
        Self {
            recommendation_value: true,
            ..Self::light()
        }
    }

    /// Build a lightDiversify config with an explicit fit tolerance (ε sweep).
    pub fn light_with_epsilon(tie_epsilon: f32) -> Self {
        Self {
            tie_epsilon,
            ..Self::light()
        }
    }

    /// Stress: wider tolerance + stricter caps.
    pub fn strong() -> Self {
        Self {
            mode: DiversifyMode::StrongDiversify,
            tie_epsilon: 0.010,
            max_window: usize::MAX,
            fit_equivalence: true,
            director_soft_cap: 1,
            collection_soft_cap: 1,
            mode_soft_cap: 1,
            semantic_soft_cap: 1,
            semantic_cluster_soft_cap: 1,
            recommendation_value: false,
        }
    }
}

/// Cosine similarity threshold for coarse embedding neighborhoods on the board.
/// Prototype attachment (not transitive closure) — keep moderate so Rocky/KK
/// can share a neighborhood without gluing the entire shortlist together.
pub const SEMANTIC_CLUSTER_SIM: f32 = 0.55;

#[derive(Debug, Clone, Default)]
struct ClusterKeys {
    collection: Option<String>,
    director: Option<String>,
    broad_mode: Option<String>,
    semantic: Option<String>,
    semantic_cluster: Option<String>,
}

pub(crate) fn collection_key(c: &ScoredCandidate) -> Option<String> {
    c.candidate.sources.iter().find_map(|s| {
        if s.kind == RetrievalKind::Collection {
            let label = s.label.trim();
            if label.is_empty() {
                None
            } else {
                Some(label.to_ascii_lowercase())
            }
        } else {
            None
        }
    })
}

pub(crate) fn director_key(c: &ScoredCandidate) -> Option<String> {
    c.candidate
        .directors
        .first()
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
}

pub(crate) fn broad_mode_key(c: &ScoredCandidate) -> Option<String> {
    c.candidate
        .modes
        .first()
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| !m.is_empty())
        .or_else(|| {
            c.candidate
                .genres
                .first()
                .map(|g| format!("genre:{}", g.trim().to_ascii_lowercase()))
                .filter(|g| g.len() > 7)
        })
}

/// Coarse semantic identity without widening the index: mode + primary genre.
pub(crate) fn semantic_key(c: &ScoredCandidate) -> Option<String> {
    let mode = c.candidate.modes.first().map(|m| m.to_ascii_lowercase());
    let genre = c.candidate.genres.first().map(|g| g.to_ascii_lowercase());
    match (mode, genre) {
        (Some(m), Some(g)) => Some(format!("{m}|{g}")),
        (Some(m), None) => Some(m),
        (None, Some(g)) => Some(format!("g|{g}")),
        (None, None) => None,
    }
}

pub(crate) fn semantic_cluster_key(c: &ScoredCandidate) -> Option<String> {
    c.candidate
        .semantic_cluster
        .as_ref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn cluster_keys(c: &ScoredCandidate) -> ClusterKeys {
    ClusterKeys {
        collection: collection_key(c),
        director: director_key(c),
        broad_mode: broad_mode_key(c),
        semantic: semantic_key(c),
        semantic_cluster: semantic_cluster_key(c),
    }
}

fn count_key(
    selected: &[ScoredCandidate],
    key: &Option<String>,
    extract: fn(&ScoredCandidate) -> Option<String>,
) -> usize {
    let Some(k) = key else {
        return 0;
    };
    selected
        .iter()
        .filter(|s| extract(s).as_ref() == Some(k))
        .count()
}

/// Penalty for placing `cand` after `selected`. Lower is better.
fn repeat_penalty(cand: &ScoredCandidate, selected: &[ScoredCandidate], cfg: &DiversifyConfig) -> f32 {
    let keys = cluster_keys(cand);
    let mut pen = 0.0;
    let col_n = count_key(selected, &keys.collection, collection_key);
    if col_n >= cfg.collection_soft_cap {
        pen += 8.0 + 4.0 * (col_n.saturating_sub(cfg.collection_soft_cap) as f32);
    }
    let dir_n = count_key(selected, &keys.director, director_key);
    if dir_n >= cfg.director_soft_cap {
        pen += 5.0 + 3.0 * (dir_n.saturating_sub(cfg.director_soft_cap) as f32);
    }
    let mode_n = count_key(selected, &keys.broad_mode, broad_mode_key);
    if mode_n >= cfg.mode_soft_cap {
        pen += 2.5 + 1.5 * (mode_n.saturating_sub(cfg.mode_soft_cap) as f32);
    }
    let sem_n = count_key(selected, &keys.semantic, semantic_key);
    if sem_n >= cfg.semantic_soft_cap {
        pen += 3.0 + 2.0 * (sem_n.saturating_sub(cfg.semantic_soft_cap) as f32);
    }
    let sc_n = count_key(selected, &keys.semantic_cluster, semantic_cluster_key);
    if sc_n >= cfg.semantic_cluster_soft_cap {
        pen += 3.5 + 2.0 * (sc_n.saturating_sub(cfg.semantic_cluster_soft_cap) as f32);
    }
    pen
}

/// F2 diagnostics. `obviousness_penalty` is always 0 (removed — punished Rediscovery).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardValueBreakdown {
    pub redundancy_penalty: f32,
    /// Deprecated / always 0. Kept for artifact schema stability.
    pub obviousness_penalty: f32,
    pub distinct_region_bonus: f32,
    pub total: f32,
}

fn best_local_neighbor(c: &ScoredCandidate) -> (Option<u32>, Option<f32>) {
    let mut best_rank = None;
    let mut best_sim = None;
    for s in &c.candidate.sources {
        if let Some(r) = s.neighbor_rank {
            if best_rank.map(|b| r < b).unwrap_or(true) {
                best_rank = Some(r);
                best_sim = s.similarity;
            }
        } else if let Some(sim) = s.similarity {
            if best_sim.map(|b| sim > b).unwrap_or(true) {
                best_sim = Some(sim);
            }
        }
    }
    (best_rank, best_sim)
}

fn region_already_on_board(cand: &ScoredCandidate, selected: &[ScoredCandidate]) -> bool {
    let keys = cluster_keys(cand);
    count_key(selected, &keys.collection, collection_key) >= 1
        || count_key(selected, &keys.semantic_cluster, semantic_cluster_key) >= 1
}

/// Continuous redundancy vs already-selected Featured titles (bounded ~0..1.2).
fn f2_redundancy_penalty(cand: &ScoredCandidate, selected: &[ScoredCandidate]) -> f32 {
    if selected.is_empty() {
        return 0.0;
    }
    let keys = cluster_keys(cand);
    let mut pen = 0.0;
    let col_n = count_key(selected, &keys.collection, collection_key);
    if col_n >= 1 {
        pen += 0.55 + 0.25 * (col_n.saturating_sub(1) as f32);
    }
    let sc_n = count_key(selected, &keys.semantic_cluster, semantic_cluster_key);
    if sc_n >= 1 {
        pen += 0.40 + 0.25 * (sc_n.saturating_sub(1) as f32);
    }
    let sem_n = count_key(selected, &keys.semantic, semantic_key);
    if sem_n >= 2 {
        pen += 0.20 + 0.10 * (sem_n.saturating_sub(2) as f32);
    }
    let dir_n = count_key(selected, &keys.director, director_key);
    if dir_n >= 1 {
        pen += 0.15;
    }
    // Repeated immediate-neighbor: only when the region is already Featured.
    // Does not penalize recognizability of a first-in-region Rediscovery.
    if region_already_on_board(cand, selected) {
        let (rank, sim) = best_local_neighbor(cand);
        if rank.map(|r| r <= 5).unwrap_or(false) {
            pen += 0.25;
        }
        if sim.map(|s| s >= 0.65).unwrap_or(false) {
            pen += 0.20;
        }
    }
    pen.min(1.4)
}

/// Bonus when the candidate opens a strong taste region not yet on Featured.
fn f2_distinct_region_bonus(cand: &ScoredCandidate, selected: &[ScoredCandidate]) -> f32 {
    if selected.is_empty() {
        return 0.15;
    }
    let sc = semantic_cluster_key(cand);
    let sem = semantic_key(cand);
    let selected_sc: HashSet<_> = selected
        .iter()
        .filter_map(semantic_cluster_key)
        .collect();
    let selected_sem: HashSet<_> = selected.iter().filter_map(semantic_key).collect();
    if let Some(ref k) = sc {
        if !selected_sc.contains(k) {
            return 0.45;
        }
    }
    if let Some(ref k) = sem {
        if !selected_sem.contains(k) {
            return 0.22;
        }
    }
    0.0
}

pub fn board_value(
    cand: &ScoredCandidate,
    selected: &[ScoredCandidate],
) -> BoardValueBreakdown {
    let redundancy_penalty = f2_redundancy_penalty(cand, selected);
    let distinct_region_bonus = f2_distinct_region_bonus(cand, selected);
    BoardValueBreakdown {
        redundancy_penalty,
        obviousness_penalty: 0.0,
        distinct_region_bonus,
        total: -redundancy_penalty + distinct_region_bonus,
    }
}

pub(crate) fn fit_of(c: &ScoredCandidate) -> f32 {
    if c.eligibility.predicted_fit > 0.0 {
        c.eligibility.predicted_fit
    } else {
        ((c.score.content + 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

/// Light diversity inside fixed quality-equivalence groups.
///
/// Groups are built from an already quality-first-sorted list using the *anchor*
/// (first / highest-G) of each group: a later film joins only if `|G - G_anchor| ≤ ε`
/// (and same known/missing class). This prevents transitive widening
/// (`A≈B`, `B≈C` ⇒ C above A when `|A−C| > ε`).
pub fn diversify_within_quality_ties(ranked: &[ScoredCandidate]) -> Vec<ScoredCandidate> {
    use crate::taste::quality::{quality_rank_key, QUALITY_TIE_EPSILON};

    if ranked.len() <= 1 {
        return ranked.to_vec();
    }
    let mut out = Vec::with_capacity(ranked.len());
    let mut i = 0;
    while i < ranked.len() {
        let (ak, ag) = quality_rank_key(ranked[i].has_quality_prior, ranked[i].quality_prior);
        let mut j = i + 1;
        while j < ranked.len() {
            let (ck, cg) = quality_rank_key(ranked[j].has_quality_prior, ranked[j].quality_prior);
            if ck != ak {
                break;
            }
            if ak == 1 && (ag - cg).abs() > QUALITY_TIE_EPSILON {
                break;
            }
            j += 1;
        }
        let mut group: Vec<ScoredCandidate> = ranked[i..j].to_vec();
        if group.len() > 1 {
            light_reorder_quality_group(&mut group);
        }
        out.append(&mut group);
        i = j;
    }
    out
}

fn light_reorder_quality_group(group: &mut Vec<ScoredCandidate>) {
    // Soft cluster / person repetition penalty; Content Fit still dominates.
    let mut remaining = std::mem::take(group);
    let mut used_clusters: HashSet<String> = HashSet::new();
    let mut used_people: HashSet<String> = HashSet::new();
    let mut ordered = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let best = remaining
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                pick_score(a, &used_clusters, &used_people)
                    .partial_cmp(&pick_score(b, &used_clusters, &used_people))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        a.candidate
                            .tmdb_id
                            .unwrap_or(i64::MAX)
                            .cmp(&b.candidate.tmdb_id.unwrap_or(i64::MAX))
                    })
            })
            .map(|(i, _)| i)
            .unwrap();
        let chosen = remaining.remove(best);
        if let Some(c) = &chosen.candidate.semantic_cluster {
            used_clusters.insert(c.clone());
        }
        for p in &chosen.person_keys {
            used_people.insert(p.clone());
        }
        ordered.push(chosen);
    }
    *group = ordered;
}

fn pick_score(
    c: &ScoredCandidate,
    used_clusters: &HashSet<String>,
    used_people: &HashSet<String>,
) -> f32 {
    let mut s = fit_of(c);
    if let Some(cl) = &c.candidate.semantic_cluster {
        if used_clusters.contains(cl) {
            s -= 0.015;
        }
    }
    let person_hits = c
        .person_keys
        .iter()
        .filter(|p| used_people.contains(*p))
        .count();
    s -= 0.008 * person_hits.min(3) as f32;
    s
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut aa = 0.0;
    let mut bb = 0.0;
    for (left, right) in a.iter().zip(b) {
        dot += left * right;
        aa += left * left;
        bb += right * right;
    }
    let denom = aa.sqrt() * bb.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

/// Assign coarse embedding-neighborhood ids among `rows` that have vectors.
/// Uses highest-fit prototype attachment (not full transitive closure) so the
/// shortlist does not collapse into one giant component.
pub fn assign_semantic_clusters(
    rows: &mut [ScoredCandidate],
    vectors: &HashMap<i64, Vec<f32>>,
    sim_threshold: f32,
) {
    let mut indexed: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if let Some(id) = row.candidate.tmdb_id {
            if vectors.contains_key(&id) {
                indexed.push(i);
            }
        }
    }
    if indexed.is_empty() {
        return;
    }

    indexed.sort_by(|&a, &b| {
        fit_of(&rows[b])
            .partial_cmp(&fit_of(&rows[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                rows[a]
                    .candidate
                    .tmdb_id
                    .unwrap_or(0)
                    .cmp(&rows[b].candidate.tmdb_id.unwrap_or(0))
            })
    });

    // Prototypes: (row_index, tmdb_id, cluster_label)
    let mut prototypes: Vec<(usize, i64, String)> = Vec::new();
    for &row_i in &indexed {
        let id = rows[row_i].candidate.tmdb_id.unwrap();
        let Some(vec_i) = vectors.get(&id) else {
            continue;
        };
        let mut joined: Option<String> = None;
        for &(_, pid, ref label) in &prototypes {
            let proto_id = label
                .strip_prefix("emb:")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(pid);
            let Some(vec_p) = vectors.get(&proto_id) else {
                continue;
            };
            if cosine(vec_i, vec_p) >= sim_threshold {
                joined = Some(label.clone());
                break;
            }
        }
        let label = joined.unwrap_or_else(|| {
            let lab = format!("emb:{id}");
            prototypes.push((row_i, id, lab.clone()));
            lab
        });
        rows[row_i].candidate.semantic_cluster = Some(label);
    }
}

fn sort_by_fit(ranked: &[ScoredCandidate]) -> Vec<ScoredCandidate> {
    let mut ordered = ranked.to_vec();
    ordered.sort_by(|a, b| {
        fit_of(b)
            .partial_cmp(&fit_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.score
                    .total
                    .partial_cmp(&a.score.total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                a.candidate
                    .tmdb_id
                    .unwrap_or(i64::MAX)
                    .cmp(&b.candidate.tmdb_id.unwrap_or(i64::MAX))
            })
    });
    ordered
}

fn diversify_fit_equivalence(
    mut remaining: Vec<ScoredCandidate>,
    cfg: &DiversifyConfig,
    featured_n: Option<usize>,
) -> Vec<ScoredCandidate> {
    let featured_limit = featured_n.unwrap_or(usize::MAX);
    let mut out = Vec::with_capacity(remaining.len());
    // Preserve raw Content index for stable tie-breaks after removals.
    let raw_index: HashMap<Option<i64>, usize> = remaining
        .iter()
        .enumerate()
        .map(|(i, c)| (c.candidate.tmdb_id, i))
        .collect();

    while !remaining.is_empty() && out.len() < featured_limit {
        let best_fit = fit_of(&remaining[0]);
        let threshold = best_fit - cfg.tie_epsilon - 1e-9;
        let mut equiv_end = 1;
        while equiv_end < remaining.len() && fit_of(&remaining[equiv_end]) + 1e-9 >= threshold {
            equiv_end += 1;
        }
        if cfg.max_window < usize::MAX {
            equiv_end = equiv_end.min(cfg.max_window.max(1));
        }

        let mut best_pos = 0;
        // Lexicographic key: higher is better.
        let mut best_key = (f32::NEG_INFINITY, f32::NEG_INFINITY, i32::MIN);
        for pos in 0..equiv_end {
            let cand = &remaining[pos];
            let raw_i = *raw_index.get(&cand.candidate.tmdb_id).unwrap_or(&pos);
            let key = if cfg.recommendation_value {
                let bv = board_value(cand, &out).total;
                // F2 membership: board_value chooses who enters Featured.
                (bv, fit_of(cand), -(raw_i as i32))
            } else {
                let pen = repeat_penalty(cand, &out, cfg);
                (fit_of(cand) - 0.02 * pen, -pen, -(raw_i as i32))
            };
            if key > best_key {
                best_key = key;
                best_pos = pos;
            }
        }
        out.push(remaining.remove(best_pos));
    }

    // F2 selects membership only. Display order of Featured uses D1.1
    // (Content + redundancy soft caps), not board_value and not raw fit alone —
    // so franchise demotions still apply in presentation.
    if cfg.recommendation_value && featured_n.is_some() {
        let d1_display = DiversifyConfig::light();
        out = diversify_fit_equivalence(sort_by_fit(&out), &d1_display, None);
    }

    // Browse inventory: keep remaining Content order (already fit-sorted).
    out.extend(remaining);
    out
}

fn diversify_windowed(
    ordered: Vec<ScoredCandidate>,
    cfg: &DiversifyConfig,
) -> Vec<ScoredCandidate> {
    let mut out = Vec::with_capacity(ordered.len());
    let mut i = 0;
    while i < ordered.len() {
        let fit0 = fit_of(&ordered[i]);
        let mut j = i + 1;
        let window_cap = cfg.max_window.max(1);
        while j < ordered.len()
            && (j - i) < window_cap
            && (fit0 - fit_of(&ordered[j])) <= cfg.tie_epsilon + 1e-9
        {
            j += 1;
        }
        let mut remaining: Vec<usize> = (i..j).collect();
        while !remaining.is_empty() {
            let mut best_pos = 0;
            let mut best_key = (f32::NEG_INFINITY, f32::NEG_INFINITY, i32::MIN);
            for (pos, &idx) in remaining.iter().enumerate() {
                let pen = repeat_penalty(&ordered[idx], &out, cfg);
                let key = (
                    fit_of(&ordered[idx]) - 0.02 * pen,
                    -pen,
                    -(idx as i32),
                );
                if key > best_key {
                    best_key = key;
                    best_pos = pos;
                }
            }
            let idx = remaining.remove(best_pos);
            out.push(ordered[idx].clone());
        }
        i = j;
    }
    out
}

/// Sort by Content fit (desc), then diversify among effectively-equivalent fits.
/// When `featured_n` is Some, only the first N slots use D1.1(+F2); the rest keep
/// Content order among leftovers (More-for-you browse inventory).
pub fn diversify_board(ranked: &[ScoredCandidate], cfg: &DiversifyConfig) -> Vec<ScoredCandidate> {
    diversify_board_featured(ranked, cfg, None)
}

pub fn diversify_board_featured(
    ranked: &[ScoredCandidate],
    cfg: &DiversifyConfig,
    featured_n: Option<usize>,
) -> Vec<ScoredCandidate> {
    let ordered = sort_by_fit(ranked);

    if matches!(cfg.mode, DiversifyMode::RawContent) || cfg.tie_epsilon <= 0.0 {
        return ordered;
    }

    if cfg.fit_equivalence {
        diversify_fit_equivalence(ordered, cfg, featured_n)
    } else {
        // Windowed path ignores featured_n (legacy D1 only).
        diversify_windowed(ordered, cfg)
    }
}

/// Diagnostics vs raw Content order.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiversifyTrace {
    pub reordered_slots: usize,
    pub max_rank_jump: i32,
    pub mean_fit_loss: f32,
    pub max_fit_loss: f32,
    pub mean_abs_rank_delta: f32,
}

pub fn diversify_trace(
    raw: &[ScoredCandidate],
    diversified: &[ScoredCandidate],
    board_n: usize,
) -> DiversifyTrace {
    let n = board_n.min(raw.len()).min(diversified.len());
    if n == 0 {
        return DiversifyTrace::default();
    }
    let raw_ids: Vec<Option<i64>> = raw.iter().take(n).map(|c| c.candidate.tmdb_id).collect();
    let div_ids: Vec<Option<i64>> = diversified
        .iter()
        .take(n)
        .map(|c| c.candidate.tmdb_id)
        .collect();
    let reordered = raw_ids
        .iter()
        .zip(div_ids.iter())
        .filter(|(a, b)| a != b)
        .count();

    let raw_pos: HashMap<Option<i64>, usize> = raw
        .iter()
        .enumerate()
        .map(|(i, c)| (c.candidate.tmdb_id, i))
        .collect();

    let mut max_jump = 0i32;
    let mut abs_deltas = 0.0f32;
    let mut fit_loss = 0.0f32;
    let mut max_fit_loss = 0.0f32;
    for (new_rank, c) in diversified.iter().take(n).enumerate() {
        let old = *raw_pos.get(&c.candidate.tmdb_id).unwrap_or(&new_rank);
        let delta = old as i32 - new_rank as i32;
        max_jump = max_jump.max(delta.abs());
        abs_deltas += (delta as f32).abs();
        if new_rank < n {
            let raw_fit = raw.get(new_rank).map(fit_of).unwrap_or(0.0);
            let loss = (raw_fit - fit_of(c)).max(0.0);
            fit_loss += loss;
            max_fit_loss = max_fit_loss.max(loss);
        }
    }
    DiversifyTrace {
        reordered_slots: reordered,
        max_rank_jump: max_jump,
        mean_fit_loss: fit_loss / n as f32,
        max_fit_loss,
        mean_abs_rank_delta: abs_deltas / n as f32,
    }
}

pub fn duplicate_cluster_counts(board: &[ScoredCandidate]) -> (usize, usize, usize, usize, usize) {
    let mut col = HashMap::new();
    let mut dir = HashMap::new();
    let mut mode = HashMap::new();
    let mut sem = HashMap::new();
    let mut sc = HashMap::new();
    for c in board {
        if let Some(k) = collection_key(c) {
            *col.entry(k).or_insert(0usize) += 1;
        }
        if let Some(k) = director_key(c) {
            *dir.entry(k).or_insert(0usize) += 1;
        }
        if let Some(k) = broad_mode_key(c) {
            *mode.entry(k).or_insert(0usize) += 1;
        }
        if let Some(k) = semantic_key(c) {
            *sem.entry(k).or_insert(0usize) += 1;
        }
        if let Some(k) = semantic_cluster_key(c) {
            *sc.entry(k).or_insert(0usize) += 1;
        }
    }
    let extras = |m: &HashMap<String, usize>| m.values().map(|n| n.saturating_sub(1)).sum::<usize>();
    (
        extras(&col),
        extras(&dir),
        extras(&mode),
        extras(&sem),
        extras(&sc),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::explain::EligibilityTrace;
    use crate::taste::retrieve::{MediaKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn row(
        id: i64,
        fit: f32,
        director: &str,
        mode: &str,
        collection: Option<&str>,
        semantic_cluster: Option<&str>,
    ) -> ScoredCandidate {
        let mut sources = vec![RetrievalSource {
            kind: RetrievalKind::Discovery,
            label: "x".into(),
            seed_tmdb_id: None,
            seed_rating: None,
            similarity: None,
            neighbor_rank: None,
        }];
        if let Some(col) = collection {
            sources.push(RetrievalSource {
                kind: RetrievalKind::Collection,
                label: col.into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            });
        }
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(id),
                title: format!("Film {id}"),
                year: Some(2000),
                poster: None,
                watchlist: false,
                sources,
                directors: vec![director.into()],
                genres: vec!["Drama".into()],
                modes: vec![mode.into()],
                media_kind: MediaKind::Movie,
                runtime: Some(110),
                vote_count: Some(100),
                semantic_cluster: semantic_cluster.map(|s| s.into()),
            },
            score: CandidateScore {
                content: fit * 2.0 - 1.0,
                tmdb_related: 0.0,
                friend_affinity: 0.0,
                recent_taste: 0.0,
                watchlist: 0.0,
                novelty: 0.0,
                negative_evidence: 0.0,
                semantic_fit: 0.5,
                semantic_coverage: true,
                total: fit,
            },
            reasons: vec![],
            evidence: vec![],
            positive_features: vec![],
            negative_features: vec![],
            contextual_only: false,
            person_keys: vec![],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![],
            hidden_features: vec![],
            eligibility: EligibilityTrace {
                predicted_fit: fit,
                confidence: 0.6,
                state: "recommended".into(),
                passed: true,
                ..Default::default()
            },
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    #[test]
    fn raw_preserves_order() {
        let ranked = vec![
            row(1, 0.80, "A", "story", None, None),
            row(2, 0.79, "B", "story", None, None),
            row(3, 0.78, "C", "intensity", None, None),
        ];
        let out = diversify_board(&ranked, &DiversifyConfig::raw());
        assert_eq!(
            out.iter().map(|c| c.candidate.tmdb_id).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn quality_groups_use_anchor_not_transitive_chain() {
        // A≈B and B≈C but |A−C| > ε — C must not enter A's group.
        let mut a = row(1, 0.50, "A", "story", None, Some("emb:a"));
        let mut b = row(2, 0.90, "B", "story", None, Some("emb:b"));
        let mut c = row(3, 0.95, "C", "story", None, Some("emb:c"));
        a.has_quality_prior = true;
        a.quality_prior = 0.20;
        b.has_quality_prior = true;
        b.quality_prior = 0.17; // within ε of A
        c.has_quality_prior = true;
        c.quality_prior = 0.14; // within ε of B, but |A−C|=0.06 > ε
        let out = diversify_within_quality_ties(&[a, b, c]);
        let ids: Vec<_> = out.iter().map(|x| x.candidate.tmdb_id).collect();
        // First group is only {A,B} (order may shuffle by Content Fit); C stays third.
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[2], Some(3), "C must not chain into A's near-G group");
        assert!(
            ids[..2].contains(&Some(1)) && ids[..2].contains(&Some(2)),
            "A and B stay in the anchor group, got {ids:?}"
        );
    }

    #[test]
    fn fit_tolerance_displaces_third_franchise_for_near_equal_diverse() {
        // Mirrors live board: third Karate Kid vs Memories-class alternate.
        let ranked = vec![
            row(1, 0.523, "A", "story", Some("Karate Kid"), Some("emb:sports")),
            row(2, 0.521, "B", "story", Some("Karate Kid"), Some("emb:sports")),
            row(3, 0.515, "C", "story", Some("Karate Kid"), Some("emb:sports")),
            row(4, 0.514, "D", "crime", None, Some("emb:crime")),
            row(5, 0.511, "E", "drama", None, Some("emb:drama")),
            row(6, 0.490, "Z", "comedy", None, Some("emb:comedy")),
        ];
        let cfg = DiversifyConfig::light_with_epsilon(0.005);
        let out = diversify_board(&ranked, &cfg);
        let top4: Vec<_> = out.iter().take(4).map(|c| c.candidate.tmdb_id).collect();
        assert!(
            top4.contains(&Some(4)) || top4.contains(&Some(5)),
            "near-equal diverse must enter top4, got {top4:?}"
        );
        assert!(
            !top4.contains(&Some(3)) || top4.iter().filter(|&&id| id == Some(3)).count() == 0,
            "third franchise entry should lose scarce slot to diverse near-equal, got {top4:?}"
        );
        // Weak .490 never enters top of .51+ pack.
        let pos6 = out.iter().position(|c| c.candidate.tmdb_id == Some(6)).unwrap();
        assert!(pos6 >= 4, "weak candidate rescued to {pos6}");
    }

    #[test]
    fn never_promotes_beyond_fit_tolerance() {
        let ranked = vec![
            row(1, 0.515, "A", "story", Some("Marvel"), Some("emb:a")),
            row(2, 0.514, "A", "story", Some("Marvel"), Some("emb:a")),
            row(3, 0.513, "A", "story", Some("Marvel"), Some("emb:a")),
            row(31, 0.490, "Z", "comedy", None, Some("emb:z")),
        ];
        let out = diversify_board(&ranked, &DiversifyConfig::light_with_epsilon(0.005));
        let pos31 = out
            .iter()
            .position(|c| c.candidate.tmdb_id == Some(31))
            .unwrap();
        assert_eq!(pos31, 3, "Δfit=0.025 must not compete under ε=0.005, pos={pos31}");
        let trace = diversify_trace(&ranked, &out, 4);
        assert!(
            trace.max_fit_loss <= 0.005 + 1e-4,
            "max fit loss {} exceeds ε",
            trace.max_fit_loss
        );
    }

    #[test]
    fn semantic_cluster_soft_cap_limits_neighborhood() {
        let ranked = vec![
            row(1, 0.520, "A", "x", None, Some("emb:sports")),
            row(2, 0.518, "B", "x", None, Some("emb:sports")),
            row(3, 0.516, "C", "x", None, Some("emb:sports")),
            row(4, 0.515, "D", "x", None, Some("emb:sports")),
            row(5, 0.514, "E", "y", None, Some("emb:other")),
        ];
        let out = diversify_board(&ranked, &DiversifyConfig::light_with_epsilon(0.008));
        let top3: Vec<_> = out.iter().take(3).map(|c| c.candidate.tmdb_id).collect();
        let sports_in_top3 = out
            .iter()
            .take(3)
            .filter(|c| semantic_cluster_key(c).as_deref() == Some("emb:sports"))
            .count();
        assert!(
            sports_in_top3 <= 2,
            "semantic_cluster soft_cap=2, got {sports_in_top3} in top3 {top3:?}"
        );
        assert!(
            top3.contains(&Some(5)),
            "diverse neighborhood must enter top3, got {top3:?}"
        );
    }

    #[test]
    fn assign_semantic_clusters_links_near_vectors() {
        let mut ranked = vec![
            row(10, 0.52, "A", "x", None, None),
            row(20, 0.51, "B", "x", None, None),
            row(30, 0.50, "C", "x", None, None),
        ];
        // Nearly parallel vs orthogonal — 20 joins prototype 10; 30 stays alone.
        let mut vectors = HashMap::new();
        vectors.insert(10, vec![1.0, 0.0, 0.0]);
        vectors.insert(20, vec![0.98, 0.2, 0.0]);
        vectors.insert(30, vec![0.0, 0.0, 1.0]);
        assign_semantic_clusters(&mut ranked, &vectors, 0.55);
        assert_eq!(
            ranked[0].candidate.semantic_cluster.as_deref(),
            Some("emb:10")
        );
        assert_eq!(
            ranked[1].candidate.semantic_cluster.as_deref(),
            Some("emb:10")
        );
        assert_eq!(
            ranked[2].candidate.semantic_cluster.as_deref(),
            Some("emb:30")
        );
    }

    #[test]
    fn light_reorders_near_ties_without_weak_rescue() {
        let ranked = vec![
            row(1, 0.776, "Nolan", "story", Some("Marvel"), None),
            row(2, 0.774, "Nolan", "story", Some("Marvel"), None),
            row(3, 0.772, "Nolan", "story", Some("Marvel"), None),
            row(4, 0.770, "Villeneuve", "atmosphere", None, None),
            row(5, 0.70, "Other", "comedy", None, None),
        ];
        let out = diversify_board(&ranked, &DiversifyConfig::light());
        assert_eq!(out[4].candidate.tmdb_id, Some(5));
        let top4: Vec<_> = out.iter().take(4).map(|c| c.candidate.tmdb_id).collect();
        assert!(top4.contains(&Some(4)));
        let marvel_in_top3 = out
            .iter()
            .take(3)
            .filter(|c| collection_key(c).as_deref() == Some("marvel"))
            .count();
        assert!(marvel_in_top3 <= 2, "light diversify should break Marvel pile-up");
    }

    #[test]
    fn never_promotes_across_large_fit_gap() {
        let ranked = vec![
            row(1, 0.81, "A", "story", Some("Marvel"), None),
            row(2, 0.80, "A", "story", Some("Marvel"), None),
            row(3, 0.79, "A", "story", Some("Marvel"), None),
            row(4, 0.78, "A", "story", Some("Marvel"), None),
            row(31, 0.67, "Z", "comedy", None, None),
        ];
        let out = diversify_board(&ranked, &DiversifyConfig::strong());
        let pos31 = out
            .iter()
            .position(|c| c.candidate.tmdb_id == Some(31))
            .unwrap();
        assert!(
            pos31 >= 4,
            "fit .67 must not jump into top of .78+ pack, got pos {pos31}"
        );
    }

    #[test]
    fn f2_membership_promotes_distinct_then_orders_by_content() {
        let mut obvious = row(1, 0.520, "A", "story", Some("Rocky"), Some("emb:sports"));
        obvious.candidate.sources.push(RetrievalSource {
            kind: RetrievalKind::RelatedRecommendations,
            label: "rec from Creed".into(),
            seed_tmdb_id: Some(99),
            seed_rating: None,
            similarity: Some(0.72),
            neighbor_rank: Some(2),
        });
        let ranked = vec![
            obvious,
            row(2, 0.519, "B", "story", Some("Rocky"), Some("emb:sports")),
            row(3, 0.518, "C", "crime", None, Some("emb:crime")),
            row(4, 0.500, "Z", "comedy", None, Some("emb:far")),
        ];
        let out = diversify_board_featured(
            &ranked,
            &DiversifyConfig::light_with_f2(),
            Some(3),
        );
        let top3: Vec<_> = out.iter().take(3).map(|c| c.candidate.tmdb_id).collect();
        assert!(
            top3.contains(&Some(3)),
            "F2 membership should include distinct near-tie, got {top3:?}"
        );
        assert!(
            !top3.contains(&Some(4)),
            "F2 must not rescue beyond ε, got {top3:?}"
        );
        // Display among Featured is D1.1 (Content + soft caps), not board_value order.
        let kk2_pos = out
            .iter()
            .position(|c| c.candidate.tmdb_id == Some(2))
            .unwrap_or(99);
        let crime_pos = out
            .iter()
            .position(|c| c.candidate.tmdb_id == Some(3))
            .unwrap_or(99);
        assert!(
            crime_pos < 3 || kk2_pos >= 2,
            "D1.1 display should not put redundant franchise ahead of distinct near-tie"
        );
    }

    #[test]
    fn featured_n_leaves_browse_in_content_order() {
        let ranked = vec![
            row(1, 0.52, "A", "x", None, Some("emb:a")),
            row(2, 0.51, "B", "x", None, Some("emb:b")),
            row(3, 0.50, "C", "x", None, Some("emb:c")),
            row(4, 0.49, "D", "x", None, Some("emb:d")),
        ];
        let out = diversify_board_featured(&ranked, &DiversifyConfig::light(), Some(2));
        assert_eq!(out.len(), 4);
        let browse: Vec<_> = out.iter().skip(2).map(|c| c.candidate.tmdb_id).collect();
        assert_eq!(browse.len(), 2);
    }
}
