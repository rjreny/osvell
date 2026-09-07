//! F1 hybrid discovery calibration: 2k core + protected 10k examination lane.

use crate::storage::db::Database;
use crate::taste::diversify::{
    assign_semantic_clusters, semantic_cluster_key, SEMANTIC_CLUSTER_SIM,
};
use crate::taste::eval::{
    recall_at, stratified_holdout_inputs, MetricSummary, ReplayInputs,
};
use crate::taste::hybrid_exam::{
    select_hybrid_exam, HybridExamConfig, HYBRID_BROAD_INDEX_CAP, HYBRID_EXAM_CAP,
};
use crate::taste::retrieve::{identity_key, FilmRecord};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::{self, load_embedding_by_tmdb, score_candidates_from_cache};
use crate::taste::workspace::{self, ALGORITHM_VERSION};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn summarize(xs: &[f32]) -> MetricSummary {
    if xs.is_empty() {
        return MetricSummary {
            mean: 0.0,
            min: 0.0,
            max: 0.0,
            stdev: 0.0,
        };
    }
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    let min = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
    MetricSummary {
        mean,
        min,
        max,
        stdev: var.sqrt(),
    }
}

fn fit_of(c: &ScoredCandidate) -> f32 {
    if c.eligibility.predicted_fit > 0.0 {
        c.eligibility.predicted_fit
    } else {
        ((c.score.content + 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

fn state_ok(c: &ScoredCandidate) -> bool {
    matches!(
        c.eligibility.state.as_str(),
        "recommended" | "exploratory"
    ) || (c.eligibility.passed && c.eligibility.predicted_fit >= 0.50)
}

fn positives_of(inputs: &ReplayInputs, all: &[FilmRecord]) -> HashSet<String> {
    let held: HashSet<_> = inputs.held_out.iter().cloned().collect();
    all.iter()
        .filter(|f| {
            let id = identity_key(f.tmdb_id, &f.title, f.year);
            held.contains(&id)
                && f.rating
                    .map(|r| r >= 7.0)
                    .unwrap_or(false)
        })
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridPolicyFold {
    pub name: String,
    pub broad_slots: usize,
    pub core_slots: usize,
    pub examined: usize,
    pub core_pool_size: usize,
    pub broad_only_pool_size: usize,
    pub broad_selected: usize,
    pub oracle_recall: f32,
    pub recall_at_100: f32,
    pub recall_at_250: f32,
    pub recall_at_1000: f32,
    pub broad_only_eligible: usize,
    pub broad_only_top25: usize,
    pub broad_only_top12: usize,
    pub board_cluster_unique: usize,
    pub board_broad_cluster_share: f32,
    pub ms: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridFold {
    pub seed: u64,
    pub policies: Vec<HybridPolicyFold>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeanHybridPolicy {
    pub name: String,
    pub broad_slots: usize,
    pub oracle_recall: MetricSummary,
    pub recall_at_1000: MetricSummary,
    pub broad_only_eligible: MetricSummary,
    pub broad_only_top25: MetricSummary,
    pub broad_only_top12: MetricSummary,
    pub board_broad_cluster_share: MetricSummary,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridCalibrationReport {
    pub algorithm_version: String,
    pub seeds: Vec<u64>,
    pub folds: Vec<HybridFold>,
    pub mean_policies: Vec<MeanHybridPolicy>,
    pub gate: String,
    pub notes: Vec<String>,
}

const POLICIES: &[usize] = &[0, 100, 150, 200, 250];

fn evaluate_policy(
    db: &Database,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
    broad_slots: usize,
) -> Result<HybridPolicyFold, String> {
    let cfg = if broad_slots == 0 {
        HybridExamConfig::control()
    } else {
        HybridExamConfig::hybrid(broad_slots)
    };
    let positives = positives_of(inputs, all_films);
    let hybrid = select_hybrid_exam(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        cfg,
    )?;

    let exam_ids: Vec<String> = hybrid
        .examined
        .iter()
        .map(|c| identity_key(c.tmdb_id, &c.title, c.year))
        .collect();
    let r100 = recall_at(&exam_ids, &positives, 100.min(exam_ids.len()));
    let r250 = recall_at(&exam_ids, &positives, 250.min(exam_ids.len()));
    let r1000 = recall_at(&exam_ids, &positives, exam_ids.len().max(1));
    let oracle = r1000;

    let semantic = score_candidates_from_cache(db, &inputs.training_films, &hybrid.examined);
    let mut scored = score_pool_with_semantic(&inputs.profile, &hybrid.examined, &semantic, None).ranked;
    // Attach clusters for board novelty diagnostic.
    let mut vectors = HashMap::new();
    for row in &scored {
        if let Some(id) = row.candidate.tmdb_id {
            if let Some(v) = load_embedding_by_tmdb(db, id) {
                vectors.insert(id, v);
            }
        }
    }
    assign_semantic_clusters(&mut scored, &vectors, SEMANTIC_CLUSTER_SIM);

    let mut eligible: Vec<ScoredCandidate> = scored.into_iter().filter(|c| state_ok(c)).collect();
    eligible.sort_by(|a, b| {
        fit_of(b)
            .partial_cmp(&fit_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // New-capable Content order (matches what D1/assemble actually shortlists).
    let new_eligible: Vec<&ScoredCandidate> = eligible
        .iter()
        .filter(|c| crate::taste::confidence::occupies_new(c))
        .collect();

    let broad = &hybrid.broad_only_keys;
    let is_broad = |c: &ScoredCandidate| {
        broad.contains(&identity_key(
            c.candidate.tmdb_id,
            &c.candidate.title,
            c.candidate.year,
        ))
    };
    let broad_eligible = new_eligible.iter().filter(|c| is_broad(c)).count();
    let broad_top25 = new_eligible.iter().take(25).filter(|c| is_broad(c)).count();

    let ws = workspace::assemble(&eligible);
    let final12: Vec<ScoredCandidate> = ws.new_picks.iter().take(12).cloned().collect();

    let broad_top12 = final12.iter().filter(|c| is_broad(c)).count();

    let mut clusters = HashSet::new();
    let mut broad_cluster = 0usize;
    for c in &final12 {
        if let Some(k) = semantic_cluster_key(c) {
            clusters.insert(k.clone());
            let key = identity_key(c.candidate.tmdb_id, &c.candidate.title, c.candidate.year);
            if broad.contains(&key) {
                broad_cluster += 1;
            }
        } else {
            clusters.insert(format!(
                "solo:{}",
                c.candidate.tmdb_id.unwrap_or_default()
            ));
        }
    }

    Ok(HybridPolicyFold {
        name: cfg.name(),
        broad_slots: cfg.broad_slots,
        core_slots: cfg.core_slots(),
        examined: hybrid.examined.len(),
        core_pool_size: hybrid.core_pool_size,
        broad_only_pool_size: hybrid.broad_only_pool_size,
        broad_selected: hybrid.broad_selected,
        oracle_recall: oracle,
        recall_at_100: r100,
        recall_at_250: r250,
        recall_at_1000: r1000,
        broad_only_eligible: broad_eligible,
        broad_only_top25: broad_top25,
        broad_only_top12: broad_top12,
        board_cluster_unique: clusters.len(),
        board_broad_cluster_share: broad_cluster as f32 / final12.len().max(1) as f32,
        ms: hybrid.ms,
    })
}

fn gate_vs_control(control: &MeanHybridPolicy, cand: &MeanHybridPolicy) -> String {
    if cand.broad_slots == 0 {
        return "CONTROL".into();
    }
    let mut fail = Vec::new();
    // Do not sacrifice @1000 recall vs control beyond noise.
    if cand.recall_at_1000.mean + 0.015 < control.recall_at_1000.mean {
        fail.push(format!(
            "r1000_drop:{:.3}->{:.3}",
            control.recall_at_1000.mean, cand.recall_at_1000.mean
        ));
    }
    // Discovery must actually reach the scarce board sometimes.
    if cand.broad_only_top12.mean < 0.5 && cand.broad_only_top25.mean < 1.0 {
        fail.push(format!(
            "no_board_reach:top12={:.2} top25={:.2}",
            cand.broad_only_top12.mean, cand.broad_only_top25.mean
        ));
    }
    if fail.is_empty() {
        "PASS_INCLUDE".into()
    } else {
        format!("HOLD: {}", fail.join(", "))
    }
}

pub fn run_hybrid_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<HybridCalibrationReport, String> {
    let stored = semantic::semantic_universe_stats(db, &HashSet::new(), None).semantic_index_movies;
    let mut folds = Vec::new();
    for &seed in seeds {
        eprintln!("hybrid fold seed={seed}");
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        let mut policies = Vec::new();
        for &broad in POLICIES {
            eprintln!("  policy broad_slots={broad}");
            policies.push(evaluate_policy(db, &inputs, films, broad)?);
        }
        folds.push(HybridFold { seed, policies });
    }

    let mut mean_policies = Vec::new();
    for &broad in POLICIES {
        let name = if broad == 0 {
            "control".to_string()
        } else {
            format!("hybrid{broad}")
        };
        let mut oracle = Vec::new();
        let mut r1000 = Vec::new();
        let mut belig = Vec::new();
        let mut b25 = Vec::new();
        let mut b12 = Vec::new();
        let mut bshare = Vec::new();
        for fold in &folds {
            if let Some(p) = fold.policies.iter().find(|p| p.broad_slots == broad) {
                oracle.push(p.oracle_recall);
                r1000.push(p.recall_at_1000);
                belig.push(p.broad_only_eligible as f32);
                b25.push(p.broad_only_top25 as f32);
                b12.push(p.broad_only_top12 as f32);
                bshare.push(p.board_broad_cluster_share);
            }
        }
        mean_policies.push(MeanHybridPolicy {
            name,
            broad_slots: broad,
            oracle_recall: summarize(&oracle),
            recall_at_1000: summarize(&r1000),
            broad_only_eligible: summarize(&belig),
            broad_only_top25: summarize(&b25),
            broad_only_top12: summarize(&b12),
            board_broad_cluster_share: summarize(&bshare),
            gate: String::new(),
        });
    }

    let control = mean_policies
        .iter()
        .find(|p| p.broad_slots == 0)
        .cloned()
        .expect("control");
    for p in &mut mean_policies {
        p.gate = gate_vs_control(&control, p);
    }

    // Prefer smallest broad lane that PASSes and places broad-only onto the board.
    let pick = [100, 150, 200, 250]
        .into_iter()
        .find(|&b| {
            mean_policies
                .iter()
                .find(|p| p.broad_slots == b)
                .map(|p| {
                    p.gate.starts_with("PASS")
                        && p.broad_only_top12.mean >= 1.0
                        && p.recall_at_1000.mean + 0.01 >= control.recall_at_1000.mean
                })
                .unwrap_or(false)
        });

    let gate = match pick {
        Some(b) => format!("PASS_F1: hybrid{b}"),
        None => {
            let any_pass = mean_policies
                .iter()
                .filter(|p| p.broad_slots > 0 && p.gate.starts_with("PASS"))
                .map(|p| p.name.clone())
                .collect::<Vec<_>>();
            if any_pass.is_empty() {
                "HOLD_F1: no hybrid policy cleared recall+reach gates".into()
            } else {
                format!("HOLD_F1: pass_but_weak_reach={}", any_pass.join(","))
            }
        }
    };

    let stored: usize = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM taste_embeddings WHERE model = ?1",
            rusqlite::params![crate::taste::semantic::EMBEDDING_MODEL],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize;

    Ok(HybridCalibrationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        seeds: seeds.to_vec(),
        folds,
        mean_policies,
        gate,
        notes: vec![
            "F1 diagnostics only — production remains active-2k + D1.1 until a hybrid policy is chosen.".into(),
            format!(
                "storedEmbeddings={stored} coreIndex={} broadIndex={} examCap={}",
                crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP,
                HYBRID_BROAD_INDEX_CAP,
                HYBRID_EXAM_CAP
            ),
            "broad-only = present under broad index pool, absent from core-2k pool.".into(),
            "No Fit bonus for broad lane; Content + D1.1 unchanged.".into(),
        ],
    })
}

pub fn write_hybrid_calibration_artifact(
    out_dir: &Path,
    report: &HybridCalibrationReport,
) -> Result<String, String> {
    let bench = out_dir.join("benchmarks");
    std::fs::create_dir_all(&bench).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let stamped = bench.join(format!("hybrid-calibration-{stamp}.json"));
    let latest = bench.join("hybrid-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&stamped, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(stamped.to_string_lossy().into_owned())
}

pub fn print_hybrid_report(report: &HybridCalibrationReport) {
    eprintln!("=== F1 hybrid discovery calibration ===");
    eprintln!("gate: {}", report.gate);
    for n in &report.notes {
        eprintln!("note: {n}");
    }
    for p in &report.mean_policies {
        eprintln!(
            "{:<12} r@1000={:.3}±{:.3} broadElig={:.1} top25={:.2} top12={:.2} boardBroadShare={:.2} gate={}",
            p.name,
            p.recall_at_1000.mean,
            p.recall_at_1000.stdev,
            p.broad_only_eligible.mean,
            p.broad_only_top25.mean,
            p.broad_only_top12.mean,
            p.board_broad_cluster_share.mean,
            p.gate
        );
    }
}
