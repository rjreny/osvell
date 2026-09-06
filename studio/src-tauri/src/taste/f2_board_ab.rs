//! Controlled A/B: active-2k + D1.1 vs active-2k + D1.1 + F2.
//! Does not enable hybrid150. Production remains F2-off until this A/B passes.

use crate::storage::db::Database;
use crate::taste::confidence;
use crate::taste::diversify::{
    board_value, diversify_trace, fit_of, semantic_cluster_key, BoardValueBreakdown, DiversifyConfig,
};
use crate::taste::match_calibration::fit_to_match_percent;
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord,
};
use crate::taste::score::{score_pool_with_semantic, ScoredCandidate};
use crate::taste::semantic::{self, score_candidates_from_cache};
use crate::taste::workspace::{self, ALGORITHM_VERSION, FEATURED_MAX, NEW_MAX};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct F2BoardRow {
    pub board_rank: usize,
    pub band: String,
    pub title: String,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub fit: f32,
    pub match_percent: u8,
    pub raw_content_rank_new: usize,
    pub diversify_delta: i32,
    pub prototype_cluster: Option<String>,
    pub seed_labels: Vec<String>,
    pub board_value: Option<BoardValueBreakdown>,
    pub accuracy_label: String,
    pub value_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct F2BoardReport {
    pub label: String,
    pub policy: String,
    pub f2_enabled: bool,
    pub examined: usize,
    pub inventory_n: usize,
    pub featured_n: usize,
    pub mean_fit_featured: f32,
    pub mean_fit_inventory: f32,
    pub diversify: crate::taste::diversify::DiversifyTrace,
    pub featured: Vec<F2BoardRow>,
    pub more_sample: Vec<F2BoardRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct F2AbReport {
    pub algorithm_version: String,
    pub notes: Vec<String>,
    pub control: F2BoardReport,
    pub f2: F2BoardReport,
    pub featured_shared: Vec<String>,
    pub control_only_featured: Vec<String>,
    pub f2_only_featured: Vec<String>,
    pub mean_fit_delta_featured: f32,
}

fn seed_labels(c: &Candidate) -> Vec<String> {
    use crate::taste::retrieve::GeneratorFamily;
    let mut seeds = Vec::new();
    for s in &c.sources {
        if matches!(
            s.kind.generator_family(),
            GeneratorFamily::SemanticFilmLocal | GeneratorFamily::Related
        ) && !s.label.is_empty()
        {
            seeds.push(s.label.clone());
        }
    }
    seeds.sort();
    seeds.dedup();
    seeds.into_iter().take(3).collect()
}

fn build_one(
    db: &Database,
    films: &[FilmRecord],
    label: &str,
    cfg: DiversifyConfig,
) -> Result<F2BoardReport, String> {
    let profile = crate::taste::feature_profile_from_films(films);
    let seen = crate::taste::retrieve::seen_keys(films);

    let pool = build_retrieval_pool(db, films, &profile, &seen, false)?;
    let examined = select_fair_pool(pool.by_key, 1_000);

    let semantic_map = score_candidates_from_cache(db, films, &examined);
    let mut scored_pool = score_pool_with_semantic(&profile, &examined, &semantic_map);
    semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.ranked);
    semantic::attach_semantic_clusters_from_db(db, &mut scored_pool.dropped_contextual);

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

    let ws = workspace::assemble_with_diversify(&scored_pool.ranked, &cfg);
    let inventory = &ws.new_picks;
    let featured: Vec<_> = inventory.iter().take(FEATURED_MAX).cloned().collect();
    let more: Vec<_> = inventory.iter().skip(FEATURED_MAX).cloned().collect();

    // Trace vs Content order of inventory before featured policy.
    let content_order: Vec<_> = {
        let mut v = inventory.to_vec();
        v.sort_by(|a, b| {
            fit_of(b)
                .partial_cmp(&fit_of(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    };
    let trace = diversify_trace(&content_order, inventory, FEATURED_MAX);

    let mut selected_for_bv: Vec<ScoredCandidate> = Vec::new();
    let mut featured_rows = Vec::new();
    for (i, row) in featured.iter().enumerate() {
        let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
        let raw = *raw_new_rank.get(&key).unwrap_or(&9999);
        let bv = if cfg.recommendation_value {
            Some(board_value(row, &selected_for_bv))
        } else {
            None
        };
        let seeds = cand_by_key.get(&key).map(seed_labels).unwrap_or_default();
        featured_rows.push(F2BoardRow {
            board_rank: i + 1,
            band: "featured".into(),
            title: row.candidate.title.clone(),
            year: row.candidate.year,
            tmdb_id: row.candidate.tmdb_id,
            fit: fit_of(row),
            match_percent: fit_to_match_percent(row.eligibility.predicted_fit),
            raw_content_rank_new: raw,
            diversify_delta: (i + 1) as i32 - raw as i32,
            prototype_cluster: semantic_cluster_key(row),
            seed_labels: seeds,
            board_value: bv,
            accuracy_label: String::new(),
            value_label: String::new(),
        });
        selected_for_bv.push(row.clone());
    }

    let more_sample: Vec<_> = more
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, row)| {
            let key = identity_key(row.candidate.tmdb_id, &row.candidate.title, row.candidate.year);
            let raw = *raw_new_rank.get(&key).unwrap_or(&9999);
            let rank = FEATURED_MAX + i + 1;
            F2BoardRow {
                board_rank: rank,
                band: "more".into(),
                title: row.candidate.title.clone(),
                year: row.candidate.year,
                tmdb_id: row.candidate.tmdb_id,
                fit: fit_of(row),
                match_percent: fit_to_match_percent(row.eligibility.predicted_fit),
                raw_content_rank_new: raw,
                diversify_delta: rank as i32 - raw as i32,
                prototype_cluster: semantic_cluster_key(row),
                seed_labels: cand_by_key.get(&key).map(seed_labels).unwrap_or_default(),
                board_value: None,
                accuracy_label: String::new(),
                value_label: String::new(),
            }
        })
        .collect();

    let mean_fit = |rows: &[ScoredCandidate]| {
        if rows.is_empty() {
            0.0
        } else {
            rows.iter().map(fit_of).sum::<f32>() / rows.len() as f32
        }
    };

    Ok(F2BoardReport {
        label: label.into(),
        policy: if cfg.recommendation_value {
            "d1.1+f2".into()
        } else {
            "d1.1".into()
        },
        f2_enabled: cfg.recommendation_value,
        examined: examined.len(),
        inventory_n: inventory.len().min(NEW_MAX),
        featured_n: featured.len(),
        mean_fit_featured: mean_fit(&featured),
        mean_fit_inventory: mean_fit(inventory),
        diversify: trace,
        featured: featured_rows,
        more_sample,
    })
}

pub fn run_f2_ab_comparison(db: &Database, films: &[FilmRecord]) -> Result<F2AbReport, String> {
    let control = build_one(db, films, "A_d1_1", DiversifyConfig::light())?;
    let f2 = build_one(db, films, "B_d1_1_f2", DiversifyConfig::light_with_f2())?;

    let cset: std::collections::HashSet<_> =
        control.featured.iter().map(|r| r.title.clone()).collect();
    let fset: std::collections::HashSet<_> = f2.featured.iter().map(|r| r.title.clone()).collect();
    let mut shared: Vec<_> = cset.intersection(&fset).cloned().collect();
    shared.sort();
    let mut control_only: Vec<_> = cset.difference(&fset).cloned().collect();
    control_only.sort();
    let mut f2_only: Vec<_> = fset.difference(&cset).cloned().collect();
    f2_only.sort();

    Ok(F2AbReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        notes: vec![
            "A/B on active-2k only. hybrid150 stays off. Content/C1/D1.1 ε=.0075 frozen.".into(),
            "Inventory ≈50; Featured 12; More (#13–50) Content-ordered leftovers.".into(),
            "F2 v2: membership-only; Featured display = D1.1 (not board_value order). board_value = −redundancy + distinct_region.".into(),
            "No generic obviousness penalty — Rediscovery must not lose to Discovery for being familiar.".into(),
            "Win: Bad not up; Perfect/Rediscovery/Discovery up; tiny fit sacrifice; no out-of-ε promotions.".into(),
            "Your live Value labels are ground truth. If B cannot clearly beat A, drop F2 for v1.".into(),
        ],
        mean_fit_delta_featured: f2.mean_fit_featured - control.mean_fit_featured,
        control,
        f2,
        featured_shared: shared,
        control_only_featured: control_only,
        f2_only_featured: f2_only,
    })
}

pub fn write_f2_ab_artifact(runs_dir: &Path, report: &F2AbReport) -> Result<String, String> {
    let dir = runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("ab-f2-{stamp}.json"));
    let latest = dir.join("ab-f2-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    std::fs::write(&latest, &body).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn print_f2_ab_for_judgment(report: &F2AbReport) {
    eprintln!("=== A/B Featured-12: D1.1 vs D1.1+F2 (active-2k) ===");
    eprintln!("algo={}", report.algorithm_version);
    for n in &report.notes {
        eprintln!("note: {n}");
    }
    eprintln!(
        "featured Δmean_fit={:+.5}  shared={:?}\ncontrol_only={:?}\nf2_only={:?}",
        report.mean_fit_delta_featured,
        report.featured_shared,
        report.control_only_featured,
        report.f2_only_featured
    );

    for board in [&report.control, &report.f2] {
        eprintln!(
            "\n========== {} ({}) inventory={} featured={} mean_fit_feat={:.4} ==========",
            board.label,
            board.policy,
            board.inventory_n,
            board.featured_n,
            board.mean_fit_featured
        );
        eprintln!(
            "trace: reordered={} max_fit_loss={:.5} mean_fit_loss={:.5}",
            board.diversify.reordered_slots,
            board.diversify.max_fit_loss,
            board.diversify.mean_fit_loss
        );
        eprintln!(
            "{:<3} {:<36} {:>5} {:>4}% {:>4}/{:<3} {:<16} {}",
            "#", "title", "fit", "M", "raw", "Δ", "cluster", "bv / seeds"
        );
        for r in &board.featured {
            let year = r.year.map(|y| format!(" ({y})")).unwrap_or_default();
            let mut title = format!("{}{year}", r.title);
            if title.len() > 36 {
                title = format!("{}…", &title[..35]);
            }
            let cluster = r
                .prototype_cluster
                .clone()
                .unwrap_or_else(|| "-".into());
            let cluster = if cluster.len() > 16 {
                format!("{}…", &cluster[..15])
            } else {
                cluster
            };
            let bv = r
                .board_value
                .map(|b| {
                    format!(
                        "bv={:+.2}(r{:.2}/o{:.2}/d{:.2})",
                        b.total, b.redundancy_penalty, b.obviousness_penalty, b.distinct_region_bonus
                    )
                })
                .unwrap_or_default();
            let seeds = r.seed_labels.iter().take(2).cloned().collect::<Vec<_>>().join("; ");
            let extra = [bv, seeds]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" | ");
            eprintln!(
                "{:<3} {:<36} {:>5.3} {:>3}% {:>4}/{:<+3} {:<16} {}",
                r.board_rank,
                title,
                r.fit,
                r.match_percent,
                r.raw_content_rank_new,
                r.diversify_delta,
                cluster,
                extra
            );
            eprintln!(
                "     Accuracy: ________  Value: ________  (Perfect/Rediscovery/Discovery/Fine/Bad)"
            );
        }
        if !board.more_sample.is_empty() {
            eprintln!("  --- More for you (sample) ---");
            for r in &board.more_sample {
                let year = r.year.map(|y| format!(" ({y})")).unwrap_or_default();
                eprintln!(
                    "  #{:<2} {}{year}  fit={:.3} raw={}",
                    r.board_rank, r.title, r.fit, r.raw_content_rank_new
                );
            }
        }
    }
    eprintln!(
        "\nWin check: F2 Bad ≤ control Bad; more Perfect/Rediscovery/Discovery; tiny fit sacrifice."
    );
}
