//! E1 semantic-universe recall curve: 2k / 5k / 10k / 25k / max.

use crate::storage::db::Database;
use crate::taste::eval::{
    stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs, rating_bucket,
};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, FilmRecord, GeneratorFamily,
};
use crate::taste::retrieval_bench::ids_of;
use crate::taste::semantic::{self, with_semantic_index_cap};
use crate::taste::semantic_universe::{semantic_novelty_for_retrieval, DEFAULT_UNIVERSE_TARGET};
use crate::taste::workspace::ALGORITHM_VERSION;
use crate::taste::eval::recall_at;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

const CURVE_CAPS: &[usize] = &[2_000, 5_000, 10_000, 25_000];
/// E1 curve uses fewer folds than A2 — each fold rebuilds a full retrieval pool.
const E1_SEEDS: &[u64] = &[17, 42];

fn progress_log(msg: &str) {
    eprintln!("{msg}");
    let path = std::env::var_os("STUDIO_TASTE_RUNS_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("STUDIO_DB").map(|db| {
                std::path::Path::new(&db)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("taste-runs")
            })
        });
    if let Some(dir) = path {
        let _ = std::fs::create_dir_all(dir.join("benchmarks"));
        let file = dir.join("benchmarks").join("universe-curve-progress.txt");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
        {
            use std::io::Write;
            let _ = writeln!(f, "{msg}");
            let _ = f.sync_all();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniverseSizePoint {
    pub index_cap: usize,
    pub index_effective: usize,
    pub oracle_recall: MetricSummary,
    pub recall_at_100: MetricSummary,
    pub recall_at_250: MetricSummary,
    pub recall_at_1000: MetricSummary,
    pub pool_size: MetricSummary,
    pub semantic_local_delta: MetricSummary,
    pub semantic_profile_delta: MetricSummary,
    pub no_generator_capable: MetricSummary,
    pub semantic_only_share: MetricSummary,
    pub retrieval_ms: MetricSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniverseCalibrationReport {
    pub algorithm_version: String,
    pub control_index_movies: usize,
    pub target_v1: usize,
    pub curve: Vec<UniverseSizePoint>,
    pub chosen_index_cap: usize,
    pub gate: String,
}

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

fn family_delta(
    map: &HashMap<String, crate::taste::retrieve::Candidate>,
    positives: &HashSet<String>,
    family: GeneratorFamily,
) -> f32 {
    let full = select_fair_pool(map.clone(), 1000);
    let full_r = recall_at(&ids_of(&full), positives, 1000);
    let mut without = HashMap::with_capacity(map.len());
    for (k, c) in map {
        let keep = c
            .sources
            .iter()
            .any(|s| s.kind.generator_family() != family);
        if keep {
            without.insert(k.clone(), c.clone());
        }
    }
    let without_pool = select_fair_pool(without, 1000);
    let without_r = recall_at(&ids_of(&without_pool), positives, 1000);
    full_r - without_r
}

fn fold_at_cap(
    db: &Database,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
    cap: usize,
) -> Result<(f32, f32, f32, f32, f32, f32, f32, f32, f32, usize, f32), String> {
    let positives: HashSet<String> = all_films
        .iter()
        .filter(|f| {
            inputs.held_out.contains(&identity_key(f.tmdb_id, &f.title, f.year))
                && matches!(
                    f.rating.map(rating_bucket),
                    Some(RatingBucket::Loved | RatingBucket::Liked)
                )
        })
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect();

    let t0 = Instant::now();
    let pool = with_semantic_index_cap(Some(cap), || {
        build_retrieval_pool(
            db,
            &inputs.training_films,
            &inputs.profile,
            &inputs.seen,
            false,
        )
    })?;
    let ms = t0.elapsed().as_secs_f32() * 1000.0;

    let index_effective = with_semantic_index_cap(Some(cap), || {
        semantic::semantic_universe_stats(db, &inputs.seen, None).semantic_index_movies
    });

    let uncapped = select_fair_pool(pool.by_key.clone(), usize::MAX / 4);
    let c1000 = select_fair_pool(pool.by_key.clone(), 1000);
    let ids_u = ids_of(&uncapped);
    let ids_1k = ids_of(&c1000);
    let oracle = recall_at(&ids_u, &positives, ids_u.len().max(1));
    let r100 = recall_at(&ids_1k, &positives, 100);
    let r250 = recall_at(&ids_1k, &positives, 250);
    let r1000 = recall_at(&ids_1k, &positives, 1000);

    let local_d = 0.0;
    let profile_d = 0.0;
    // family_delta skipped on the size curve (too expensive); use A3 for marginals.

    let uncapped_set: HashSet<_> = ids_u.iter().cloned().collect();
    let miss = positives
        .iter()
        .filter(|id| !uncapped_set.contains(*id))
        .count() as f32;
    let no_gen = if positives.is_empty() {
        0.0
    } else {
        miss / positives.len() as f32
    };

    let novelty = crate::taste::semantic_universe::semantic_novelty_for_retrieval(
        &c1000.iter().take(50).cloned().collect::<Vec<_>>(),
    );
    let sem_only = if novelty.new_count == 0 {
        0.0
    } else {
        novelty.semantic_only as f32 / novelty.new_count as f32
    };

    Ok((
        oracle,
        r100,
        r250,
        r1000,
        c1000.len() as f32,
        local_d,
        profile_d,
        no_gen,
        sem_only,
        index_effective,
        ms,
    ))
}

fn decide_gate(control: &UniverseSizePoint, curve: &[UniverseSizePoint]) -> (usize, String) {
    let ctrl_oracle = control.oracle_recall.mean;
    let ctrl_1k = control.recall_at_1000.mean;
    let mut best = control.index_cap;
    let mut best_oracle = ctrl_oracle;
    let mut best_r1k_ok = true;
    for p in curve {
        if p.index_cap <= control.index_cap {
            continue;
        }
        let oracle_gain = p.oracle_recall.mean - ctrl_oracle;
        let r1k_ok = p.recall_at_1000.mean + 0.02 >= ctrl_1k;
        let no_gen_ok = p.no_generator_capable.mean <= control.no_generator_capable.mean + 0.02;
        if oracle_gain >= 0.02
            && r1k_ok
            && no_gen_ok
            && p.oracle_recall.mean > best_oracle + 0.005
        {
            best = p.index_cap;
            best_oracle = p.oracle_recall.mean;
            best_r1k_ok = r1k_ok;
        }
    }

    // Saturation: if 10k and 25k within 0.015 oracle, prefer 10k when it also keeps @1000.
    let p10 = curve.iter().find(|p| p.index_cap == 10_000);
    let p25 = curve.iter().find(|p| p.index_cap == 25_000);
    if let (Some(a), Some(b)) = (p10, p25) {
        if (b.oracle_recall.mean - a.oracle_recall.mean).abs() < 0.015
            && a.recall_at_1000.mean + 0.02 >= ctrl_1k
            && a.oracle_recall.mean >= ctrl_oracle + 0.02
        {
            best = 10_000;
            best_r1k_ok = true;
        }
    }

    let chosen = curve
        .iter()
        .find(|p| p.index_cap == best)
        .unwrap_or(control);
    let gain = chosen.oracle_recall.mean - ctrl_oracle;
    let r1k_delta = chosen.recall_at_1000.mean - ctrl_1k;

    // Largest measured point for diagnostics even when we keep control.
    let largest = curve
        .iter()
        .max_by(|a, b| {
            a.oracle_recall
                .mean
                .partial_cmp(&b.oracle_recall.mean)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(control);
    let large_gain = largest.oracle_recall.mean - ctrl_oracle;
    let large_r1k = largest.recall_at_1000.mean - ctrl_1k;

    if best != control.index_cap && gain >= 0.02 && r1k_delta >= -0.02 && chosen.no_generator_capable.mean < 0.05
    {
        (
            best,
            format!(
                "PASS_E1: cap={best} oracle={:.3}->{:.3} @1000={:.3}->{:.3}",
                ctrl_oracle, chosen.oracle_recall.mean, ctrl_1k, chosen.recall_at_1000.mean
            ),
        )
    } else if large_gain >= 0.02 && large_r1k < -0.02 {
        // Classic widen pathology: more discoverable, less survives the exam cap.
        (
            control.index_cap,
            format!(
                "HOLD_E1: oracle↑@{lg:.0}k ({ctrl_oracle:.3}->{lo:.3}) but @1000↓ ({ctrl_1k:.3}->{l1:.3}); fix exam/merge before enabling full index",
                lg = largest.index_cap as f32 / 1000.0,
                lo = largest.oracle_recall.mean,
                l1 = largest.recall_at_1000.mean,
            ),
        )
    } else if large_gain < 0.015 && control.index_effective >= 1800 {
        (
            control.index_cap,
            format!("PASS_E1: saturated_at_control oracle={ctrl_oracle:.3}"),
        )
    } else {
        let _ = best_r1k_ok;
        (
            control.index_cap,
            format!(
                "HOLD_E1: best={best} oracle_gain={gain:.3} @1000_delta={r1k_delta:.3} no_gen={:.3}",
                chosen.no_generator_capable.mean
            ),
        )
    }
}

pub fn run_universe_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<UniverseCalibrationReport, String> {
    let control_n =
        semantic::semantic_universe_stats(db, &HashSet::new(), None).semantic_index_movies;
    let max_n = control_n.max(CURVE_CAPS.iter().copied().max().unwrap_or(0));
    let mut caps: Vec<usize> = CURVE_CAPS
        .iter()
        .copied()
        .filter(|&c| c <= max_n || c == CURVE_CAPS[0])
        .collect();
    // Always include control size and largest available.
    if !caps.contains(&control_n.min(2_000).max(1)) {
        caps.insert(0, control_n.min(2_000).max(1));
    }
    // Normalize first point to actual control when near 2k.
    if let Some(first) = caps.first_mut() {
        if control_n > 0 && control_n <= 2_500 {
            *first = control_n;
        }
    }
    caps.push(max_n);
    caps.sort_unstable();
    caps.dedup();

    let mut curve = Vec::new();
    for &cap in &caps {
        progress_log(&format!("E1 curve: measuring cap={cap} …"));
        let mut oracle = Vec::new();
        let mut r100 = Vec::new();
        let mut r250 = Vec::new();
        let mut r1000 = Vec::new();
        let mut pool = Vec::new();
        let mut local = Vec::new();
        let mut profile = Vec::new();
        let mut nogen = Vec::new();
        let mut semonly = Vec::new();
        let mut ms = Vec::new();
        let mut eff = 0usize;
        let seeds = if seeds.len() > E1_SEEDS.len() {
            E1_SEEDS
        } else {
            seeds
        };
        for &seed in seeds {
            let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
            let (
                o,
                a,
                b,
                c,
                p,
                ld,
                pd,
                ng,
                so,
                ie,
                t,
            ) = fold_at_cap(db, &inputs, films, cap)?;
            progress_log(&format!(
                "  seed={seed} oracle={o:.3} @1000={c:.3} pool={p:.0} ms={t:.0} idx={ie}"
            ));
            oracle.push(o);
            r100.push(a);
            r250.push(b);
            r1000.push(c);
            pool.push(p);
            local.push(ld);
            profile.push(pd);
            nogen.push(ng);
            semonly.push(so);
            ms.push(t);
            eff = ie;
        }
        curve.push(UniverseSizePoint {
            index_cap: cap,
            index_effective: eff,
            oracle_recall: summarize(&oracle),
            recall_at_100: summarize(&r100),
            recall_at_250: summarize(&r250),
            recall_at_1000: summarize(&r1000),
            pool_size: summarize(&pool),
            semantic_local_delta: summarize(&local),
            semantic_profile_delta: summarize(&profile),
            no_generator_capable: summarize(&nogen),
            semantic_only_share: summarize(&semonly),
            retrieval_ms: summarize(&ms),
        });
    }

    let control = curve
        .iter()
        .min_by_key(|p| p.index_cap)
        .cloned()
        .ok_or_else(|| "empty curve".to_string())?;
    let (chosen, gate) = decide_gate(&control, &curve);

    Ok(UniverseCalibrationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        control_index_movies: control_n,
        target_v1: DEFAULT_UNIVERSE_TARGET,
        curve,
        chosen_index_cap: chosen,
        gate,
    })
}

pub fn write_universe_calibration_artifact(
    out_dir: &Path,
    report: &UniverseCalibrationReport,
) -> Result<String, String> {
    let bench = out_dir.join("benchmarks");
    std::fs::create_dir_all(&bench).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let stamped = bench.join(format!("universe-calibration-{stamp}.json"));
    let latest = bench.join("universe-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&stamped, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(stamped.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_caps_cover_requested_ladder() {
        assert!(CURVE_CAPS.contains(&2_000));
        assert!(CURVE_CAPS.contains(&10_000));
        assert!(CURVE_CAPS.contains(&25_000));
    }
}
