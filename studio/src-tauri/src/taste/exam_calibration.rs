//! E1.1 exam-pressure calibration: allocation matrix + oracle∩@1000-miss diagnostics.

use crate::storage::db::Database;
use crate::taste::eval::{
    recall_at, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs, rating_bucket,
};
use crate::taste::exam_policy::{with_exam_mode, ExamMode};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord, GeneratorFamily,
    RetrievalKind,
};
use crate::taste::retrieval_bench::ids_of;
use crate::taste::semantic::{self, with_semantic_index_cap};
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

const INDEX_CAPS: &[usize] = &[2_000, 10_000];
/// Optional diagnostic exam caps (10k index only).
const DIAG_EXAM_CAPS: &[usize] = &[1_250, 1_500];

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
        let file = dir.join("benchmarks").join("exam-pressure-progress.txt");
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapMissSample {
    pub key: String,
    pub title: String,
    pub generators: Vec<String>,
    pub film_local: bool,
    pub profile: bool,
    pub both_semantic: bool,
    pub native_best_rank: Option<u32>,
    pub merged_exam_rank: Option<usize>,
    pub same_family_ahead: usize,
    pub duplicate_neighborhood: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExamPoint {
    pub mode: String,
    pub index_cap: usize,
    pub index_effective: usize,
    pub exam_cap: usize,
    pub oracle_recall: MetricSummary,
    pub recall_at_100: MetricSummary,
    pub recall_at_250: MetricSummary,
    pub recall_at_1000: MetricSummary,
    pub pool_size: MetricSummary,
    pub candidate_cap_failures: MetricSummary,
    pub semantic_only_recovery: MetricSummary,
    pub film_local_share: MetricSummary,
    pub profile_share: MetricSummary,
    pub retrieval_ms: MetricSummary,
    pub hydration_count: MetricSummary,
    pub sample_cap_misses: Vec<CapMissSample>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExamCalibrationReport {
    pub algorithm_version: String,
    pub control_2k_at_1000: f32,
    pub control_2k_oracle: f32,
    pub active_retrieval_cap: usize,
    pub matrix: Vec<ExamPoint>,
    pub chosen_mode: String,
    pub chosen_index_cap: usize,
    pub gate: String,
}

fn positives_of(inputs: &ReplayInputs, all_films: &[FilmRecord]) -> HashSet<String> {
    all_films
        .iter()
        .filter(|f| {
            inputs.held_out.contains(&identity_key(f.tmdb_id, &f.title, f.year))
                && matches!(
                    f.rating.map(rating_bucket),
                    Some(RatingBucket::Loved | RatingBucket::Liked)
                )
        })
        .map(|f| identity_key(f.tmdb_id, &f.title, f.year))
        .collect()
}

fn family_label(kind: RetrievalKind) -> &'static str {
    match kind.generator_family() {
        GeneratorFamily::SemanticFilmLocal => "SemanticFilmLocal",
        GeneratorFamily::SemanticProfile => "SemanticProfile",
        GeneratorFamily::Related => "Related",
        GeneratorFamily::Filmography => "Filmography",
        GeneratorFamily::Collection => "Collection",
        GeneratorFamily::Friend => "Friend",
        GeneratorFamily::Watchlist => "Watchlist",
        GeneratorFamily::Discovery => "Discovery",
        GeneratorFamily::Exploration => "Exploration",
    }
}

fn diagnose_cap_misses(
    map: &HashMap<String, Candidate>,
    uncapped: &[Candidate],
    capped: &[Candidate],
    positives: &HashSet<String>,
    limit: usize,
) -> Vec<CapMissSample> {
    let uncapped_pos: HashSet<usize> = uncapped
        .iter()
        .enumerate()
        .filter(|(_, c)| positives.contains(&identity_key(c.tmdb_id, &c.title, c.year)))
        .map(|(i, _)| i)
        .collect();
    let capped_keys: HashSet<String> = capped
        .iter()
        .map(|c| identity_key(c.tmdb_id, &c.title, c.year))
        .collect();

    let mut samples = Vec::new();
    for (i, c) in uncapped.iter().enumerate() {
        if !uncapped_pos.contains(&i) {
            continue;
        }
        let key = identity_key(c.tmdb_id, &c.title, c.year);
        if capped_keys.contains(&key) {
            continue;
        }
        let row = map.get(&key).unwrap_or(c);
        let film_local = row.sources.iter().any(|s| {
            s.kind.generator_family() == GeneratorFamily::SemanticFilmLocal
        });
        let profile = row
            .sources
            .iter()
            .any(|s| s.kind.generator_family() == GeneratorFamily::SemanticProfile);
        let mut gens: Vec<String> = row
            .sources
            .iter()
            .map(|s| family_label(s.kind).to_string())
            .collect();
        gens.sort();
        gens.dedup();

        let native_best_rank = row
            .sources
            .iter()
            .filter(|s| {
                matches!(
                    s.kind.generator_family(),
                    GeneratorFamily::SemanticFilmLocal | GeneratorFamily::SemanticProfile
                )
            })
            .filter_map(|s| s.neighbor_rank)
            .min();

        let primary = if film_local {
            GeneratorFamily::SemanticFilmLocal
        } else if profile {
            GeneratorFamily::SemanticProfile
        } else {
            row.sources
                .first()
                .map(|s| s.kind.generator_family())
                .unwrap_or(GeneratorFamily::Related)
        };

        let same_family_ahead = capped
            .iter()
            .take_while(|x| identity_key(x.tmdb_id, &x.title, x.year) != key)
            .filter(|x| {
                x.sources
                    .iter()
                    .any(|s| s.kind.generator_family() == primary)
            })
            .count();

        // Heuristic: strong native rank but crowded out by many same-family rows
        // that share overlapping seed neighborhoods (multi-seed FilmLocal).
        let duplicate_neighborhood = film_local
            && native_best_rank.map(|r| r <= 20).unwrap_or(false)
            && same_family_ahead >= 80;

        samples.push(CapMissSample {
            key: key.clone(),
            title: c.title.clone(),
            generators: gens,
            film_local,
            profile,
            both_semantic: film_local && profile,
            native_best_rank,
            merged_exam_rank: Some(i + 1),
            same_family_ahead,
            duplicate_neighborhood,
        });
        if samples.len() >= limit {
            break;
        }
    }
    samples
}

fn fold_point(
    db: &Database,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
    mode: ExamMode,
    index_cap: usize,
    exam_cap: usize,
) -> Result<(f32, f32, f32, f32, f32, f32, f32, f32, f32, f32, usize, Vec<CapMissSample>), String>
{
    let positives = positives_of(inputs, all_films);
    let t0 = Instant::now();
    let (pool, index_effective) = with_exam_mode(mode, || {
        with_semantic_index_cap(Some(index_cap), || {
            let pool = build_retrieval_pool(
                db,
                &inputs.training_films,
                &inputs.profile,
                &inputs.seen,
                false,
            );
            let eff =
                semantic::semantic_universe_stats(db, &inputs.seen, None).semantic_index_movies;
            (pool, eff)
        })
    });
    let pool = pool?;
    let ms = t0.elapsed().as_secs_f32() * 1000.0;

    let (uncapped, capped, samples) = with_exam_mode(mode, || {
        let uncapped = select_fair_pool(pool.by_key.clone(), usize::MAX / 4);
        let capped = select_fair_pool(pool.by_key.clone(), exam_cap);
        let samples = diagnose_cap_misses(&pool.by_key, &uncapped, &capped, &positives, 12);
        (uncapped, capped, samples)
    });

    let ids_u = ids_of(&uncapped);
    let ids_c = ids_of(&capped);
    let oracle = recall_at(&ids_u, &positives, ids_u.len().max(1));
    let r100 = recall_at(&ids_c, &positives, 100.min(exam_cap));
    let r250 = recall_at(&ids_c, &positives, 250.min(exam_cap));
    let r_exam = recall_at(&ids_c, &positives, exam_cap);

    let uncapped_set: HashSet<_> = ids_u.iter().cloned().collect();
    let capped_set: HashSet<_> = ids_c.iter().cloned().collect();
    let cap_fail = positives
        .iter()
        .filter(|id| uncapped_set.contains(*id) && !capped_set.contains(*id))
        .count() as f32
        / positives.len().max(1) as f32;

    let sem_only_rec = positives
        .iter()
        .filter(|id| {
            if !capped_set.contains(*id) {
                return false;
            }
            let Some(c) = pool.by_key.get(*id) else {
                return false;
            };
            let sem = c.sources.iter().any(|s| {
                matches!(
                    s.kind.generator_family(),
                    GeneratorFamily::SemanticFilmLocal | GeneratorFamily::SemanticProfile
                )
            });
            let graph = c.sources.iter().any(|s| {
                matches!(
                    s.kind.generator_family(),
                    GeneratorFamily::Related
                        | GeneratorFamily::Filmography
                        | GeneratorFamily::Collection
                )
            });
            sem && !graph
        })
        .count() as f32
        / positives.len().max(1) as f32;

    let fl_share = capped
        .iter()
        .filter(|c| {
            c.sources
                .iter()
                .any(|s| s.kind.generator_family() == GeneratorFamily::SemanticFilmLocal)
        })
        .count() as f32
        / capped.len().max(1) as f32;
    let pr_share = capped
        .iter()
        .filter(|c| {
            c.sources
                .iter()
                .any(|s| s.kind.generator_family() == GeneratorFamily::SemanticProfile)
        })
        .count() as f32
        / capped.len().max(1) as f32;

    Ok((
        oracle,
        r100,
        r250,
        r_exam,
        capped.len() as f32,
        cap_fail,
        sem_only_rec,
        fl_share,
        pr_share,
        ms,
        index_effective,
        samples,
    ))
}

fn run_cell(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
    mode: ExamMode,
    index_cap: usize,
    exam_cap: usize,
) -> Result<ExamPoint, String> {
    let mut oracles = Vec::new();
    let mut r100s = Vec::new();
    let mut r250s = Vec::new();
    let mut r1ks = Vec::new();
    let mut pools = Vec::new();
    let mut fails = Vec::new();
    let mut sem_recs = Vec::new();
    let mut fls = Vec::new();
    let mut prs = Vec::new();
    let mut mss = Vec::new();
    let mut eff = 0usize;
    let mut samples = Vec::new();

    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        let (
            oracle,
            r100,
            r250,
            r1k,
            pool,
            fail,
            sem_rec,
            fl,
            pr,
            ms,
            index_effective,
            fold_samples,
        ) = fold_point(db, &inputs, films, mode, index_cap, exam_cap)?;
        progress_log(&format!(
            "  mode={} idx={} exam={} seed={} oracle={:.3} @exam={:.3} capFail={:.3} ms={:.0}",
            mode.as_str(),
            index_cap,
            exam_cap,
            seed,
            oracle,
            r1k,
            fail,
            ms
        ));
        oracles.push(oracle);
        r100s.push(r100);
        r250s.push(r250);
        r1ks.push(r1k);
        pools.push(pool);
        fails.push(fail);
        sem_recs.push(sem_rec);
        fls.push(fl);
        prs.push(pr);
        mss.push(ms);
        eff = index_effective;
        if samples.len() < 24 {
            samples.extend(fold_samples);
        }
    }

    Ok(ExamPoint {
        mode: mode.as_str().into(),
        index_cap,
        index_effective: eff,
        exam_cap,
        oracle_recall: summarize(&oracles),
        recall_at_100: summarize(&r100s),
        recall_at_250: summarize(&r250s),
        recall_at_1000: summarize(&r1ks),
        pool_size: summarize(&pools),
        candidate_cap_failures: summarize(&fails),
        semantic_only_recovery: summarize(&sem_recs),
        film_local_share: summarize(&fls),
        profile_share: summarize(&prs),
        retrieval_ms: summarize(&mss),
        hydration_count: summarize(&pools),
        sample_cap_misses: samples.into_iter().take(24).collect(),
    })
}

fn decide_gate(matrix: &[ExamPoint]) -> (String, usize, String) {
    let control = matrix
        .iter()
        .find(|p| p.mode == ExamMode::Current.as_str() && p.index_cap == 2_000 && p.exam_cap == 1_000)
        .or_else(|| matrix.first());
    let Some(ctrl) = control else {
        return (
            ExamMode::Current.as_str().into(),
            2_000,
            "HOLD_E1_1: empty matrix".into(),
        );
    };
    let ctrl_1k = ctrl.recall_at_1000.mean;
    let ctrl_oracle = ctrl.oracle_recall.mean;

    let mut best: Option<&ExamPoint> = None;
    for p in matrix {
        if p.exam_cap != 1_000 {
            continue; // diagnostic caps only
        }
        if p.index_cap < 8_000 {
            continue;
        }
        // Must not regress vs 2k control @1000; keep oracle benefit of widen.
        let ok_1k = p.recall_at_1000.mean + 0.005 >= ctrl_1k;
        let ok_oracle = p.oracle_recall.mean + 0.01 >= ctrl_oracle.max(0.70);
        if !(ok_1k && ok_oracle) {
            continue;
        }
        best = match best {
            None => Some(p),
            Some(b) => {
                if p.recall_at_1000.mean > b.recall_at_1000.mean + 0.005
                    || (p.recall_at_1000.mean + 0.005 >= b.recall_at_1000.mean
                        && p.oracle_recall.mean > b.oracle_recall.mean + 0.01)
                {
                    Some(p)
                } else {
                    Some(b)
                }
            }
        };
    }

    if let Some(p) = best {
        let strong = p.recall_at_1000.mean >= 0.61 && p.oracle_recall.mean >= 0.74;
        let gate = if strong {
            format!(
                "PASS_E1_1: mode={} 10k @1000={:.3} (2k ctrl={:.3}) oracle={:.3}",
                p.mode, p.recall_at_1000.mean, ctrl_1k, p.oracle_recall.mean
            )
        } else {
            format!(
                "PASS_E1_1_WEAK: mode={} 10k @1000={:.3} >= 2k ctrl={:.3}; oracle={:.3} (practical benefit thin)",
                p.mode, p.recall_at_1000.mean, ctrl_1k, p.oracle_recall.mean
            )
        };
        return (p.mode.clone(), p.index_cap, gate);
    }

    // Prefer narrower active retrieval universe if widen still hurts @1000.
    // Diagnostic note: current@10k with exam=1250 recovered @exam≈0.67 while
    // keeping oracle≈0.77 — cheap alternative if we later prefer breadth over
    // the v1 active-2k policy.
    (
        ExamMode::Current.as_str().into(),
        2_000,
        format!(
            "HOLD_E1_1: 10k still @1000-worse than 2k ctrl={ctrl_1k:.3}; keep 10k stored, active retrieval cap={}",
            crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP
        ),
    )
}

pub fn run_exam_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<ExamCalibrationReport, String> {
    let mut matrix = Vec::new();

    for &index_cap in INDEX_CAPS {
        for &mode in ExamMode::all() {
            progress_log(&format!(
                "E1.1: mode={} index_cap={} exam=1000 …",
                mode.as_str(),
                index_cap
            ));
            matrix.push(run_cell(
                db,
                films,
                seeds,
                holdout_frac,
                mode,
                index_cap,
                1_000,
            )?);
        }
    }

    // Optional diagnostic: raise exam cap under current merge at 10k.
    for &exam_cap in DIAG_EXAM_CAPS {
        progress_log(&format!(
            "E1.1 diagnostic: mode=current index=10000 exam={exam_cap} …"
        ));
        matrix.push(run_cell(
            db,
            films,
            seeds,
            holdout_frac,
            ExamMode::Current,
            10_000,
            exam_cap,
        )?);
    }

    let (chosen_mode, chosen_index_cap, gate) = decide_gate(&matrix);
    let control = matrix
        .iter()
        .find(|p| p.mode == "current" && p.index_cap == 2_000 && p.exam_cap == 1_000);
    Ok(ExamCalibrationReport {
        algorithm_version: ALGORITHM_VERSION.into(),
        control_2k_at_1000: control.map(|c| c.recall_at_1000.mean).unwrap_or(0.0),
        control_2k_oracle: control.map(|c| c.oracle_recall.mean).unwrap_or(0.0),
        active_retrieval_cap: crate::taste::exam_policy::V1_ACTIVE_SEMANTIC_CAP,
        matrix,
        chosen_mode,
        chosen_index_cap,
        gate,
    })
}

pub fn write_exam_calibration_artifact(
    runs_dir: &Path,
    report: &ExamCalibrationReport,
) -> Result<String, String> {
    let dir = runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("exam-pressure-{stamp}.json"));
    let latest = dir.join("exam-pressure-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    std::fs::write(&latest, &body).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::exam_policy::{family_soft_cap, with_exam_mode, ExamMode};
    use crate::taste::retrieve::{
        select_fair_pool, Candidate, MediaKind, RetrievalKind, RetrievalSource,
    };

    #[test]
    fn soft_caps_limit_semantic_local_volume() {
        with_exam_mode(ExamMode::FamilySoftCaps, || {
            let mut map = HashMap::new();
            for i in 0..600i64 {
                map.insert(
                    format!("sem-{i}"),
                    Candidate {
                        tmdb_id: Some(90_000 + i),
                        title: format!("Sem {i}"),
                        year: Some(2015),
                        poster: None,
                        genres: vec![],
                        credits: vec![],
                        keywords: vec![],
                        runtime: Some(100),
                        vote_count: Some(10),
                        watchlist: false,
                        sources: vec![RetrievalSource::new(
                            RetrievalKind::SemanticFilmLocal,
                            format!("near {i}"),
                            Some(i % 40),
                        )
                        .with_similarity(0.95 - (i as f32) * 0.0001, (i % 30) as u32 + 1)],
                        friend_affinity: 0.0,
                        tmdb_related: 0.0,
                        media_kind: MediaKind::Movie,
                    },
                );
            }
            for i in 0..400i64 {
                map.insert(
                    format!("rel-{i}"),
                    Candidate {
                        tmdb_id: Some(50_000 + i),
                        title: format!("Rel {i}"),
                        year: Some(2010),
                        poster: None,
                        genres: vec![],
                        credits: vec![],
                        keywords: vec![],
                        runtime: Some(100),
                        vote_count: Some(10),
                        watchlist: false,
                        sources: vec![RetrievalSource::new(
                            RetrievalKind::RelatedRecommendations,
                            format!("rec {i}"),
                            Some(i % 50),
                        )],
                        friend_affinity: 0.0,
                        tmdb_related: 1.0,
                        media_kind: MediaKind::Movie,
                    },
                );
            }
            let selected = select_fair_pool(map, 1000);
            let local = selected
                .iter()
                .filter(|c| {
                    c.sources
                        .iter()
                        .any(|s| s.kind == RetrievalKind::SemanticFilmLocal)
                })
                .count();
            let limit = family_soft_cap(GeneratorFamily::SemanticFilmLocal, 1000).unwrap_or(1000);
            let hard = limit + limit / 2;
            assert!(
                local <= hard + 40,
                "FilmLocal share {local} should respect hard cap ~{hard} (soft {limit})"
            );
            assert!(local < 400, "FilmLocal must not dominate examination");
        });
    }

    #[test]
    fn gate_prefers_10k_when_at1000_recovers() {
        let ctrl = ExamPoint {
            mode: "current".into(),
            index_cap: 2_000,
            index_effective: 2_000,
            exam_cap: 1_000,
            oracle_recall: MetricSummary {
                mean: 0.684,
                min: 0.6,
                max: 0.7,
                stdev: 0.0,
            },
            recall_at_100: MetricSummary {
                mean: 0.13,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            recall_at_250: MetricSummary {
                mean: 0.26,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            recall_at_1000: MetricSummary {
                mean: 0.605,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            pool_size: MetricSummary {
                mean: 1000.0,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            candidate_cap_failures: MetricSummary {
                mean: 0.1,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            semantic_only_recovery: MetricSummary {
                mean: 0.0,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            film_local_share: MetricSummary {
                mean: 0.2,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            profile_share: MetricSummary {
                mean: 0.1,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            retrieval_ms: MetricSummary {
                mean: 500.0,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            hydration_count: MetricSummary {
                mean: 1000.0,
                min: 0.0,
                max: 0.0,
                stdev: 0.0,
            },
            sample_cap_misses: vec![],
        };
        let mut good = ctrl.clone();
        good.mode = "dedupeAndSoftCaps".into();
        good.index_cap = 10_000;
        good.index_effective = 9_929;
        good.oracle_recall.mean = 0.77;
        good.recall_at_1000.mean = 0.63;
        let (mode, cap, gate) = decide_gate(&[ctrl, good]);
        assert_eq!(mode, "dedupeAndSoftCaps");
        assert_eq!(cap, 10_000);
        assert!(gate.starts_with("PASS_E1_1"), "{gate}");
    }
}
