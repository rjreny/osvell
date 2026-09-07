//! Controlled A/B live board: active-2k control vs hybrid150.
//! Experiment only — does not change the production default exam policy.

use crate::storage::db::Database;
use crate::taste::confidence;
use crate::taste::diversify::{fit_of, semantic_cluster_key};
use crate::taste::hybrid_exam::{select_hybrid_exam, HybridExamConfig};
use crate::taste::match_calibration::fit_to_match_percent;
use crate::taste::retrieve::{identity_key, Candidate, FilmRecord};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::{self, score_candidates_from_cache};
use crate::taste::workspace::{self, ALGORITHM_VERSION};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const BOARD_N: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LaneProvenance {
    CoreOnly,
    BroadOnly,
    Both,
    Unknown,
}

impl LaneProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoreOnly => "core-only",
            Self::BroadOnly => "broad-only",
            Self::Both => "both",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbBoardRow {
    pub board_rank: usize,
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub fit: f32,
    pub match_percent: u8,
    pub lane: String,
    pub semantic_margin: f32,
    pub semantic_positive: f32,
    pub semantic_negative: f32,
    pub raw_content_rank_new: usize,
    pub final_d1_rank: usize,
    pub diversify_delta: i32,
    pub prototype_cluster: Option<String>,
    pub seed_labels: Vec<String>,
    pub profile_labels: Vec<String>,
    pub best_neighbor_rank: Option<u32>,
    pub best_similarity: Option<f32>,
    /// Control board title this displaced (hybrid only), if any.
    pub displaced_control_title: Option<String>,
    pub fit_delta_vs_displaced: Option<f32>,
    pub accuracy_label: String,
    pub value_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbBoardReport {
    pub label: String,
    pub policy: String,
    pub exam_cap: usize,
    pub core_slots: usize,
    pub broad_slots: usize,
    pub examined: usize,
    pub broad_selected: usize,
    pub broad_only_pool_size: usize,
    pub retrieval_ms: f32,
    pub score_ms: f32,
    pub board: Vec<AbBoardRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbComparisonReport {
    pub algorithm_version: String,
    pub notes: Vec<String>,
    pub control: AbBoardReport,
    pub hybrid: AbBoardReport,
    pub control_only_titles: Vec<String>,
    pub hybrid_only_titles: Vec<String>,
    pub shared_titles: Vec<String>,
}

fn provenance(
    key: &str,
    core_keys: &HashSet<String>,
    broad_only_keys: &HashSet<String>,
) -> LaneProvenance {
    let in_core = core_keys.contains(key);
    let in_broad = broad_only_keys.contains(key);
    match (in_core, in_broad) {
        (true, false) => LaneProvenance::CoreOnly,
        (false, true) => LaneProvenance::BroadOnly,
        (true, true) => LaneProvenance::Both,
        (false, false) => LaneProvenance::Unknown,
    }
}

fn seed_and_profile(c: &Candidate) -> (Vec<String>, Vec<String>, Option<u32>, Option<f32>) {
    use crate::taste::retrieve::GeneratorFamily;
    let mut seeds = Vec::new();
    let mut profiles = Vec::new();
    let mut best_rank = None;
    let mut best_sim = None;
    for s in &c.sources {
        match s.kind.generator_family() {
            GeneratorFamily::SemanticFilmLocal => {
                if !s.label.is_empty() {
                    seeds.push(s.label.clone());
                }
                if let Some(r) = s.neighbor_rank {
                    if best_rank.map(|b| r < b).unwrap_or(true) {
                        best_rank = Some(r);
                        best_sim = s.similarity;
                    }
                }
            }
            GeneratorFamily::SemanticProfile => {
                if !s.label.is_empty() {
                    profiles.push(s.label.clone());
                }
            }
            _ => {}
        }
    }
    seeds.sort();
    seeds.dedup();
    profiles.sort();
    profiles.dedup();
    (seeds, profiles, best_rank, best_sim)
}

fn build_board_from_exam(
    db: &Database,
    films: &[FilmRecord],
    label: &str,
    cfg: HybridExamConfig,
) -> Result<(AbBoardReport, HashSet<String>, HashMap<String, f32>), String> {
    let profile = crate::taste::feature_profile_from_films(films);
    let seen = crate::taste::retrieve::seen_keys(films);

    let hybrid = select_hybrid_exam(db, films, &profile, &seen, cfg)?;
    let examined = hybrid.examined;
    let broad_only = hybrid.broad_only_keys.clone();

    // Core pool keys ≈ everything examined that is not broad-only.
    let mut core_keys: HashSet<String> = HashSet::new();
    for c in &examined {
        let k = identity_key(c.tmdb_id, &c.title, c.year);
        if !broad_only.contains(&k) {
            core_keys.insert(k);
        }
    }

    let t1 = std::time::Instant::now();
    let semantic_map = score_candidates_from_cache(db, films, &examined);
    let mut scored_pool = score_pool_with_semantic(&profile, &examined, &semantic_map, None);
    semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.ranked);
    semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.dropped_contextual);
    let score_ms = t1.elapsed().as_secs_f32() * 1000.0;

    let mut cand_by_key: HashMap<String, Candidate> = HashMap::new();
    for c in &examined {
        cand_by_key.insert(identity_key(c.tmdb_id, &c.title, c.year), c.clone());
    }

    let mut new_content: Vec<ScoredCandidate> = scored_pool
        .ranked
        .iter()
        .filter(|c| confidence::occupies_new(c))
        .cloned()
        .collect();
    new_content.sort_by(|a, b| {
        fit_of(b)
            .partial_cmp(&fit_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.candidate.title.cmp(&b.candidate.title))
    });
    let raw_new_rank: HashMap<String, usize> = new_content
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                identity_key(r.candidate.tmdb_id, &r.candidate.title, r.candidate.year),
                i + 1,
            )
        })
        .collect();

    // Pre-D1 New shortlist (Recommended then Exploratory).
    let mut rec: Vec<_> = new_content
        .iter()
        .filter(|c| c.eligibility.state == "recommended")
        .cloned()
        .collect();
    let exp: Vec<_> = new_content
        .iter()
        .filter(|c| c.eligibility.state == "exploratory")
        .cloned()
        .collect();
    rec.extend(exp);
    let pre_div_rank: HashMap<String, usize> = rec
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                identity_key(r.candidate.tmdb_id, &r.candidate.title, r.candidate.year),
                i + 1,
            )
        })
        .collect();

    let ws = workspace::assemble(&scored_pool.ranked);
    let board: Vec<ScoredCandidate> = ws.new_picks.iter().take(BOARD_N).cloned().collect();

    let mut board_fits: HashMap<String, f32> = HashMap::new();
    let mut rows = Vec::new();
    for (i, row) in board.iter().enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        board_fits.insert(key.clone(), fit_of(row));
        let cand = cand_by_key.get(&key);
        let (seeds, profiles, nrank, sim) = cand
            .map(seed_and_profile)
            .unwrap_or_else(|| (vec![], vec![], None, None));
        let fit_detail = cand
            .map(|c| {
                let sem = row
                    .candidate
                    .tmdb_id
                    .and_then(|id| semantic_map.get(&id))
                    .cloned()
                    .unwrap_or_default();
                crate::taste::family_fit::score_family_fit_with_config(
                    &profile,
                    c,
                    &sem,
                    &crate::taste::family_fit::CraftConfig::fit_v1(),
                )
            });
        let pre = *pre_div_rank.get(&key).unwrap_or(&(i + 1));
        let raw = *raw_new_rank.get(&key).unwrap_or(&9999);
        let lane = provenance(&key, &core_keys, &broad_only);
        rows.push(AbBoardRow {
            board_rank: i + 1,
            title: row.candidate.title.clone(),
            year: row.candidate.year,
            tmdb_id: row.candidate.tmdb_id,
            fit: fit_of(row),
            match_percent: fit_to_match_percent(row.eligibility.predicted_fit),
            lane: lane.as_str().into(),
            semantic_margin: fit_detail
                .as_ref()
                .map(|f| f.content_detail.semantic_margin)
                .unwrap_or(0.0),
            semantic_positive: fit_detail
                .as_ref()
                .map(|f| f.content_detail.semantic_positive)
                .unwrap_or(0.0),
            semantic_negative: fit_detail
                .as_ref()
                .map(|f| f.content_detail.semantic_negative)
                .unwrap_or(0.0),
            raw_content_rank_new: raw,
            final_d1_rank: i + 1,
            diversify_delta: (i + 1) as i32 - pre as i32,
            prototype_cluster: semantic_cluster_key(row),
            seed_labels: seeds.into_iter().take(4).collect(),
            profile_labels: profiles.into_iter().take(3).collect(),
            best_neighbor_rank: nrank,
            best_similarity: sim,
            displaced_control_title: None,
            fit_delta_vs_displaced: None,
            accuracy_label: String::new(),
            value_label: String::new(),
        });
    }

    let report = AbBoardReport {
        label: label.into(),
        policy: cfg.name(),
        exam_cap: cfg.exam_cap,
        core_slots: cfg.core_slots(),
        broad_slots: cfg.broad_slots,
        examined: examined.len(),
        broad_selected: hybrid.broad_selected,
        broad_only_pool_size: hybrid.broad_only_pool_size,
        retrieval_ms: hybrid.ms,
        score_ms,
        board: rows,
    };
    Ok((report, board_fits.keys().cloned().collect(), board_fits))
}

fn annotate_displacements(hybrid: &mut AbBoardReport, control: &AbBoardReport) {
    let control_keys: HashSet<String> = control
        .board
        .iter()
        .filter_map(|r| r.tmdb_id.map(|id| format!("tmdb:{id}")))
        .collect();
    let hybrid_keys: HashSet<String> = hybrid
        .board
        .iter()
        .filter_map(|r| r.tmdb_id.map(|id| format!("tmdb:{id}")))
        .collect();

    // Control titles missing from hybrid — potential displacees.
    let missing: Vec<&AbBoardRow> = control
        .board
        .iter()
        .filter(|r| {
            r.tmdb_id
                .map(|id| !hybrid_keys.contains(&format!("tmdb:{id}")))
                .unwrap_or(true)
        })
        .collect();

    for row in &mut hybrid.board {
        let key = row
            .tmdb_id
            .map(|id| format!("tmdb:{id}"))
            .unwrap_or_default();
        if control_keys.contains(&key) {
            continue;
        }
        // New to hybrid: nearest-fit missing control title.
        if let Some(victim) = missing.iter().min_by(|a, b| {
            (a.fit - row.fit)
                .abs()
                .partial_cmp(&(b.fit - row.fit).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            row.displaced_control_title =
                Some(format!("#{} {}", victim.board_rank, victim.title));
            row.fit_delta_vs_displaced = Some(row.fit - victim.fit);
        }
    }
}

pub fn run_ab_hybrid150_comparison(
    db: &Database,
    films: &[FilmRecord],
) -> Result<AbComparisonReport, String> {
    let (control, control_keys, _) =
        build_board_from_exam(db, films, "A_control", HybridExamConfig::control())?;
    let (mut hybrid, hybrid_keys, _) =
        build_board_from_exam(db, films, "B_hybrid150", HybridExamConfig::hybrid(150))?;
    annotate_displacements(&mut hybrid, &control);

    let control_titles: HashSet<_> = control.board.iter().map(|r| r.title.clone()).collect();
    let hybrid_titles: HashSet<_> = hybrid.board.iter().map(|r| r.title.clone()).collect();
    let mut shared: Vec<_> = control_titles.intersection(&hybrid_titles).cloned().collect();
    shared.sort();
    let mut control_only: Vec<_> = control_titles.difference(&hybrid_titles).cloned().collect();
    control_only.sort();
    let mut hybrid_only: Vec<_> = hybrid_titles.difference(&control_titles).cloned().collect();
    hybrid_only.sort();
    let _ = (control_keys, hybrid_keys);

    Ok(AbComparisonReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        notes: vec![
            "Controlled experiment: production default remains active-2k + D1.1. hybrid150 is not the v1 default.".into(),
            "Frozen: Content Fit_v1, margin fix, C1, D1.1 ε=0.0075, prototype clusters, lane routing.".into(),
            "hybrid150 exam: 850 core/active-2k + 150 protected broad/full-10k. No Fit bonus for broad.".into(),
            "Label Accuracy: Good|Plausible|Bad. Value: Perfect|Rediscovery|Discovery|Fine|Bad.".into(),
            "Blind tip: judge Value by your reaction first; lane column reveals provenance after.".into(),
        ],
        control,
        hybrid,
        control_only_titles: control_only,
        hybrid_only_titles: hybrid_only,
        shared_titles: shared,
    })
}

pub fn write_ab_artifact(runs_dir: &Path, report: &AbComparisonReport) -> Result<String, String> {
    let dir = runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("ab-hybrid150-{stamp}.json"));
    let latest = dir.join("ab-hybrid150-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    std::fs::write(&latest, &body).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn print_ab_for_judgment(report: &AbComparisonReport) {
    eprintln!("=== A/B live boards (control 2k vs hybrid150) ===");
    eprintln!("algo={}", report.algorithm_version);
    for n in &report.notes {
        eprintln!("note: {n}");
    }

    // Pass 1 — blind: title + fit/Match only. No lane / seeds / displace.
    eprintln!("\n----- BLIND JUDGMENT (label Accuracy + Value before reading REVEAL) -----");
    for board in [&report.control, &report.hybrid] {
        eprintln!(
            "\n========== {}  (policy hidden for labeling) ==========",
            board.label
        );
        eprintln!("{:<3} {:<40} {:>5} {:>4}%", "#", "title", "fit", "M");
        for r in &board.board {
            let year = r.year.map(|y| format!(" ({y})")).unwrap_or_default();
            let mut title = format!("{}{year}", r.title);
            if title.len() > 40 {
                title = format!("{}…", &title[..39]);
            }
            eprintln!(
                "{:<3} {:<40} {:>5.3} {:>3}%",
                r.board_rank, title, r.fit, r.match_percent
            );
            eprintln!(
                "     Accuracy: ________  Value: ________  (Perfect/Rediscovery/Discovery/Fine/Bad)"
            );
        }
    }

    // Pass 2 — diagnostics after labeling.
    eprintln!("\n----- REVEAL (after labeling) -----");
    eprintln!(
        "shared={:?}\ncontrol_only={:?}\nhybrid_only={:?}",
        report.shared_titles, report.control_only_titles, report.hybrid_only_titles
    );
    for board in [&report.control, &report.hybrid] {
        eprintln!(
            "\n========== {} ({}) exam={} core={} broad={} ==========",
            board.label, board.policy, board.examined, board.core_slots, board.broad_slots
        );
        eprintln!(
            "broad_selected={} broad_only_pool={} ms_ret={:.0} ms_score={:.0}",
            board.broad_selected, board.broad_only_pool_size, board.retrieval_ms, board.score_ms
        );
        eprintln!(
            "{:<3} {:<34} {:>5} {:>4}% {:<10} {:>6} {:>4}/{:<4} {:<18} {}",
            "#", "title", "fit", "M", "lane", "margin", "raw", "d1", "cluster", "seeds / displace"
        );
        for r in &board.board {
            let year = r.year.map(|y| format!(" ({y})")).unwrap_or_default();
            let mut title = format!("{}{year}", r.title);
            if title.len() > 34 {
                title = format!("{}…", &title[..33]);
            }
            let cluster = r
                .prototype_cluster
                .clone()
                .unwrap_or_else(|| "-".into());
            let cluster = if cluster.len() > 18 {
                format!("{}…", &cluster[..17])
            } else {
                cluster
            };
            let mut extra = r
                .seed_labels
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            if let Some(d) = &r.displaced_control_title {
                if !extra.is_empty() {
                    extra.push_str(" | ");
                }
                extra.push_str(&format!(
                    "disp {} Δ={:+.4}",
                    d,
                    r.fit_delta_vs_displaced.unwrap_or(0.0)
                ));
            }
            eprintln!(
                "{:<3} {:<34} {:>5.3} {:>3}% {:<10} {:>6.3} {:>4}/{:<4} {:<18} {}",
                r.board_rank,
                title,
                r.fit,
                r.match_percent,
                r.lane,
                r.semantic_margin,
                r.raw_content_rank_new,
                r.final_d1_rank,
                cluster,
                extra
            );
        }
    }
    eprintln!(
        "\nWin check: hybrid bad ≤ control bad; more Perfect/Rediscovery/Discovery; fewer Fine/redundant."
    );
}
