//! Milestone B1.1 Craft calibration: role ablations, actor variants, λ sweep,
//! rescue/damage vs Content-only. Content score stays frozen.

use crate::storage::db::Database;
use crate::taste::eval::{
    rating_bucket, stratified_holdout_inputs, MetricSummary, RatingBucket, ReplayInputs,
    BENCHMARK_HOLDOUT_FRAC, BENCHMARK_SEEDS,
};
use crate::taste::family_fit::{
    score_pool_family_fit_with_config, ActorMode, CraftConfig, CraftRoleMask, FamilyFitResult,
};
use crate::taste::retrieve::{
    build_retrieval_pool, identity_key, select_fair_pool, Candidate, FilmRecord,
};
use crate::taste::semantic::score_candidates_from_cache;
use crate::taste::workspace::ALGORITHM_VERSION;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct AblationSpec {
    name: String,
    config: CraftConfig,
    note: Option<String>,
}

fn ablation_matrix() -> Vec<AblationSpec> {
    let base_lambda = 0.40;
    let mut specs = vec![
        AblationSpec {
            name: "contentOnly".into(),
            config: CraftConfig::content_only(),
            note: None,
        },
        AblationSpec {
            name: "director".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_director(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "writer".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_writer(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "dp".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_dp(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "composer".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_composer(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "actor".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_actor(),
                actor_mode: ActorMode::Full,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "studio".into(),
            config: CraftConfig {
                roles: CraftRoleMask::only_studio(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: Some("studio FeatureFamily not yet hydrated — expect ≈ contentOnly".into()),
        },
        AblationSpec {
            name: "authorship".into(),
            config: CraftConfig {
                roles: CraftRoleMask::authorship(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "actorStudio".into(),
            config: CraftConfig {
                roles: CraftRoleMask::actor_studio(),
                actor_mode: ActorMode::Full,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: Some("studio unused; effectively actor-only".into()),
        },
        AblationSpec {
            name: "fullCraft".into(),
            config: CraftConfig {
                roles: CraftRoleMask::full(),
                actor_mode: ActorMode::Full,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "noActor".into(),
            config: CraftConfig {
                roles: CraftRoleMask::without_actor(),
                actor_mode: ActorMode::Removed,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "actorCapped".into(),
            config: CraftConfig {
                roles: CraftRoleMask::full(),
                actor_mode: ActorMode::PositiveCapped,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
        AblationSpec {
            name: "actorCorroboration".into(),
            config: CraftConfig {
                roles: CraftRoleMask::full(),
                actor_mode: ActorMode::ContentCorroboration,
                lambda: base_lambda,
                lineage_diminishing: true,
            },
            note: None,
        },
    ];
    for &lambda in &[0.0_f32, 0.15, 0.25, 0.40, 0.60, 1.00] {
        specs.push(AblationSpec {
            name: format!("lambda_{lambda:.2}"),
            config: CraftConfig {
                roles: CraftRoleMask::without_actor(),
                actor_mode: ActorMode::Removed,
                lambda,
                lineage_diminishing: true,
            },
            note: Some("authorship roles (no actor); λ sweep".into()),
        });
    }
    specs
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RescueDamage {
    pub rescued_positives: usize,
    pub damaged_positives: usize,
    pub rescued_dislikes: usize,
    pub damaged_dislikes: usize,
    /// Content rank > 250 but craft model ≤ 250 (held-out positives).
    pub rescued_positive_titles: Vec<String>,
    pub damaged_positive_titles: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AblationMetrics {
    pub name: String,
    pub note: Option<String>,
    pub loved_vs_disliked: f32,
    pub liked_vs_disliked: f32,
    pub mean_rank_loved: Option<f32>,
    pub mean_rank_disliked: Option<f32>,
    pub ndcg_at_100: f32,
    pub ndcg_at_250: f32,
    pub recall_at_250: f32,
    pub delta_vs_content: AblationDelta,
    pub rescue_damage: RescueDamage,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AblationDelta {
    pub loved_vs_disliked: f32,
    pub liked_vs_disliked: f32,
    pub mean_rank_loved: Option<f32>,
    pub mean_rank_disliked: Option<f32>,
    pub ndcg_at_100: f32,
    pub ndcg_at_250: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftCalibrationFold {
    pub seed: u64,
    pub pool_size: usize,
    pub held_out: usize,
    pub ablations: Vec<AblationMetrics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftCalibrationReport {
    pub protocol: String,
    pub algorithm_version: String,
    pub folds: usize,
    pub holdout_frac: f32,
    pub seeds: Vec<u64>,
    pub fold_reports: Vec<CraftCalibrationFold>,
    pub mean_ablations: Vec<MeanAblation>,
    pub gate: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeanAblation {
    pub name: String,
    pub note: Option<String>,
    pub loved_vs_disliked: MetricSummary,
    pub liked_vs_disliked: MetricSummary,
    pub mean_rank_loved: MetricSummary,
    pub mean_rank_disliked: MetricSummary,
    pub ndcg_at_100: MetricSummary,
    pub ndcg_at_250: MetricSummary,
    pub delta_loved_vs_disliked: MetricSummary,
    pub delta_mean_rank_loved: MetricSummary,
    pub delta_mean_rank_disliked: MetricSummary,
    pub delta_ndcg_at_250: MetricSummary,
    pub rescued_positives: MetricSummary,
    pub damaged_positives: MetricSummary,
}

fn id_of(c: &Candidate) -> String {
    identity_key(c.tmdb_id, &c.title, c.year)
}

fn held_out_rated<'a>(films: &'a [FilmRecord], held: &HashSet<String>) -> Vec<&'a FilmRecord> {
    films
        .iter()
        .filter(|f| {
            f.rating.is_some() && held.contains(&identity_key(f.tmdb_id, &f.title, f.year))
        })
        .collect()
}

fn relevance(bucket: RatingBucket) -> f32 {
    match bucket {
        RatingBucket::Loved => 3.0,
        RatingBucket::Liked => 2.0,
        RatingBucket::Meh => 1.0,
        RatingBucket::Disliked => 0.0,
    }
}

fn ndcg_at(ordered: &[String], rel: &HashMap<String, f32>, k: usize) -> f32 {
    let mut dcg = 0.0;
    for (i, id) in ordered.iter().take(k).enumerate() {
        let r = rel.get(id).copied().unwrap_or(0.0);
        if r > 0.0 {
            dcg += (2f32.powf(r) - 1.0) / ((i as f32 + 2.0).log2());
        }
    }
    let mut ideal: Vec<f32> = rel.values().copied().filter(|r| *r > 0.0).collect();
    ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let mut idcg = 0.0;
    for (i, r) in ideal.into_iter().take(k).enumerate() {
        idcg += (2f32.powf(r) - 1.0) / ((i as f32 + 2.0).log2());
    }
    if idcg <= 1e-8 {
        0.0
    } else {
        dcg / idcg
    }
}

fn pairwise(
    ranks: &HashMap<String, usize>,
    higher: &[String],
    lower: &[String],
) -> f32 {
    let mut ok = 0u32;
    let mut total = 0u32;
    for h in higher {
        let Some(&hr) = ranks.get(h) else { continue };
        for l in lower {
            let Some(&lr) = ranks.get(l) else { continue };
            total += 1;
            if hr < lr {
                ok += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        ok as f32 / total as f32
    }
}

fn mean_rank(ranks: &HashMap<String, usize>, ids: &[String]) -> Option<f32> {
    let vals: Vec<f32> = ids
        .iter()
        .filter_map(|id| ranks.get(id).map(|r| *r as f32))
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum::<f32>() / vals.len() as f32)
    }
}

fn order_ids(ranked: &[(Candidate, FamilyFitResult)]) -> Vec<String> {
    ranked.iter().map(|(c, _)| id_of(c)).collect()
}

fn rank_map(ids: &[String]) -> HashMap<String, usize> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i + 1))
        .collect()
}

fn metrics_core(
    ordered: &[String],
    held: &[&FilmRecord],
) -> (
    f32,
    f32,
    Option<f32>,
    Option<f32>,
    f32,
    f32,
    f32,
    HashMap<String, usize>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let mut rel = HashMap::new();
    let mut loved = Vec::new();
    let mut liked = Vec::new();
    let mut disliked = Vec::new();
    let mut positives = Vec::new();
    for film in held {
        let id = identity_key(film.tmdb_id, &film.title, film.year);
        let rating = film.rating.unwrap_or(3.0);
        let bucket = rating_bucket(rating);
        rel.insert(id.clone(), relevance(bucket));
        match bucket {
            RatingBucket::Loved => {
                loved.push(id.clone());
                positives.push(id.clone());
            }
            RatingBucket::Liked => {
                liked.push(id.clone());
                positives.push(id.clone());
            }
            RatingBucket::Disliked => disliked.push(id.clone()),
            RatingBucket::Meh => {}
        }
    }
    let ranks = rank_map(ordered);
    let recall_250 = {
        let hit = positives
            .iter()
            .filter(|id| ranks.get(*id).copied().unwrap_or(usize::MAX) <= 250)
            .count();
        if positives.is_empty() {
            0.0
        } else {
            hit as f32 / positives.len() as f32
        }
    };
    (
        pairwise(&ranks, &loved, &disliked),
        pairwise(&ranks, &liked, &disliked),
        mean_rank(&ranks, &loved),
        mean_rank(&ranks, &disliked),
        ndcg_at(ordered, &rel, 100),
        ndcg_at(ordered, &rel, 250),
        recall_250,
        ranks,
        loved,
        liked,
        disliked,
    )
}

fn rescue_damage(
    content_ranks: &HashMap<String, usize>,
    model_ranks: &HashMap<String, usize>,
    positives: &[String],
    disliked: &[String],
    title_of: &HashMap<String, String>,
) -> RescueDamage {
    let mut rescued_positives = 0;
    let mut damaged_positives = 0;
    let mut rescued_dislikes = 0;
    let mut damaged_dislikes = 0;
    let mut rescued_positive_titles = Vec::new();
    let mut damaged_positive_titles = Vec::new();
    for id in positives {
        let cr = content_ranks.get(id).copied().unwrap_or(usize::MAX);
        let mr = model_ranks.get(id).copied().unwrap_or(usize::MAX);
        if cr > 250 && mr <= 250 {
            rescued_positives += 1;
            if rescued_positive_titles.len() < 6 {
                rescued_positive_titles.push(title_of.get(id).cloned().unwrap_or_else(|| id.clone()));
            }
        }
        if cr <= 250 && mr > 250 {
            damaged_positives += 1;
            if damaged_positive_titles.len() < 6 {
                damaged_positive_titles.push(title_of.get(id).cloned().unwrap_or_else(|| id.clone()));
            }
        }
    }
    for id in disliked {
        let cr = content_ranks.get(id).copied().unwrap_or(usize::MAX);
        let mr = model_ranks.get(id).copied().unwrap_or(usize::MAX);
        // For dislikes: "rescued" = pushed out of top 250; "damaged" = pulled into top 250
        if cr <= 250 && mr > 250 {
            rescued_dislikes += 1;
        }
        if cr > 250 && mr <= 250 {
            damaged_dislikes += 1;
        }
    }
    RescueDamage {
        rescued_positives,
        damaged_positives,
        rescued_dislikes,
        damaged_dislikes,
        rescued_positive_titles,
        damaged_positive_titles,
    }
}

fn evaluate_fold(
    db: &Database,
    seed: u64,
    inputs: &ReplayInputs,
    all_films: &[FilmRecord],
) -> Result<CraftCalibrationFold, String> {
    let held = held_out_rated(all_films, &inputs.held_out);
    let pool = build_retrieval_pool(
        db,
        &inputs.training_films,
        &inputs.profile,
        &inputs.seen,
        false,
    )?;
    let candidates = select_fair_pool(pool.by_key, 1000);
    let semantic = score_candidates_from_cache(db, &inputs.training_films, &candidates);
    let title_of: HashMap<String, String> = held
        .iter()
        .map(|f| {
            (
                identity_key(f.tmdb_id, &f.title, f.year),
                f.title.clone(),
            )
        })
        .collect();

    let content_ranked = score_pool_family_fit_with_config(
        &inputs.profile,
        &candidates,
        &semantic,
        &CraftConfig::content_only(),
    );
    let content_ids = order_ids(&content_ranked);
    let (
        c_loved,
        c_liked,
        c_loved_rank,
        c_dis_rank,
        c_ndcg100,
        c_ndcg250,
        _,
        content_ranks,
        positives_loved,
        positives_liked,
        disliked,
    ) = metrics_core(&content_ids, &held);
    let positives: Vec<String> = positives_loved
        .iter()
        .chain(positives_liked.iter())
        .cloned()
        .collect();

    let mut ablations = Vec::new();
    for spec in ablation_matrix() {
        let ranked = if spec.name == "contentOnly" {
            content_ranked.clone()
        } else {
            score_pool_family_fit_with_config(
                &inputs.profile,
                &candidates,
                &semantic,
                &spec.config,
            )
        };
        let ids = order_ids(&ranked);
        let (
            loved,
            liked,
            loved_rank,
            dis_rank,
            ndcg100,
            ndcg250,
            recall250,
            model_ranks,
            _,
            _,
            _,
        ) = metrics_core(&ids, &held);
        let rd = rescue_damage(
            &content_ranks,
            &model_ranks,
            &positives,
            &disliked,
            &title_of,
        );
        ablations.push(AblationMetrics {
            name: spec.name,
            note: spec.note,
            loved_vs_disliked: loved,
            liked_vs_disliked: liked,
            mean_rank_loved: loved_rank,
            mean_rank_disliked: dis_rank,
            ndcg_at_100: ndcg100,
            ndcg_at_250: ndcg250,
            recall_at_250: recall250,
            delta_vs_content: AblationDelta {
                loved_vs_disliked: loved - c_loved,
                liked_vs_disliked: liked - c_liked,
                mean_rank_loved: match (loved_rank, c_loved_rank) {
                    (Some(a), Some(b)) => Some(a - b),
                    _ => None,
                },
                mean_rank_disliked: match (dis_rank, c_dis_rank) {
                    (Some(a), Some(b)) => Some(a - b),
                    _ => None,
                },
                ndcg_at_100: ndcg100 - c_ndcg100,
                ndcg_at_250: ndcg250 - c_ndcg250,
            },
            rescue_damage: rd,
        });
    }

    Ok(CraftCalibrationFold {
        seed,
        pool_size: candidates.len(),
        held_out: held.len(),
        ablations,
    })
}

fn summarize(xs: &[f32]) -> MetricSummary {
    if xs.is_empty() {
        return MetricSummary::default();
    }
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
    MetricSummary {
        mean,
        min: xs.iter().copied().fold(f32::INFINITY, f32::min),
        max: xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        stdev: var.sqrt(),
    }
}

fn mean_ablations(folds: &[CraftCalibrationFold]) -> Vec<MeanAblation> {
    if folds.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = folds[0].ablations.iter().map(|a| a.name.clone()).collect();
    names
        .into_iter()
        .map(|name| {
            let rows: Vec<&AblationMetrics> = folds
                .iter()
                .filter_map(|f| f.ablations.iter().find(|a| a.name == name))
                .collect();
            let note = rows.first().and_then(|r| r.note.clone());
            let col = |f: fn(&AblationMetrics) -> f32| summarize(&rows.iter().map(|r| f(r)).collect::<Vec<_>>());
            let col_opt = |f: fn(&AblationMetrics) -> Option<f32>| {
                summarize(&rows.iter().filter_map(|r| f(r)).collect::<Vec<_>>())
            };
            MeanAblation {
                name,
                note,
                loved_vs_disliked: col(|a| a.loved_vs_disliked),
                liked_vs_disliked: col(|a| a.liked_vs_disliked),
                mean_rank_loved: col_opt(|a| a.mean_rank_loved),
                mean_rank_disliked: col_opt(|a| a.mean_rank_disliked),
                ndcg_at_100: col(|a| a.ndcg_at_100),
                ndcg_at_250: col(|a| a.ndcg_at_250),
                delta_loved_vs_disliked: col(|a| a.delta_vs_content.loved_vs_disliked),
                delta_mean_rank_loved: col_opt(|a| a.delta_vs_content.mean_rank_loved),
                delta_mean_rank_disliked: col_opt(|a| a.delta_vs_content.mean_rank_disliked),
                delta_ndcg_at_250: col(|a| a.delta_vs_content.ndcg_at_250),
                rescued_positives: col(|a| a.rescue_damage.rescued_positives as f32),
                damaged_positives: col(|a| a.rescue_damage.damaged_positives as f32),
            }
        })
        .collect()
}

fn decide_gate(means: &[MeanAblation]) -> String {
    let content = means.iter().find(|m| m.name == "contentOnly");
    let best = means
        .iter()
        .filter(|m| m.name != "contentOnly" && !m.name.starts_with("lambda_0.00"))
        .max_by(|a, b| {
            let score = |m: &MeanAblation| {
                m.delta_loved_vs_disliked.mean * 2.0
                    + m.delta_ndcg_at_250.mean
                    - m.delta_mean_rank_loved.mean / 200.0
                    + m.delta_mean_rank_disliked.mean / 300.0
                    + (m.rescued_positives.mean - m.damaged_positives.mean) * 0.02
            };
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let Some(content) = content else {
        return "HOLD_B1_1: missing contentOnly baseline".into();
    };
    let Some(best) = best else {
        return "HOLD_B1_1: no craft variants".into();
    };

    let pairwise_ok = best.loved_vs_disliked.mean + 0.005 >= content.loved_vs_disliked.mean;
    // Material loved-rank regression fails even if pairwise ticks up.
    let loved_ok = best.mean_rank_loved.mean <= content.mean_rank_loved.mean + 5.0;
    let dislike_ok = best.mean_rank_disliked.mean + 5.0 >= content.mean_rank_disliked.mean;
    let ndcg_ok = best.ndcg_at_250.mean + 0.005 >= content.ndcg_at_250.mean;
    let rescue_ok = best.rescued_positives.mean + 0.1 >= best.damaged_positives.mean;
    let any_real_gain = best.delta_loved_vs_disliked.mean >= 0.01
        || best.delta_ndcg_at_250.mean >= 0.01
        || best.delta_mean_rank_loved.mean <= -10.0
        || best.rescued_positives.mean >= best.damaged_positives.mean + 1.0;

    if pairwise_ok && loved_ok && dislike_ok && ndcg_ok && rescue_ok && any_real_gain {
        format!(
            "PASS_B1_1: best craft variant `{name}` vs Content — loved>dis {cl:.3}→{bl:.3} (Δ{dl:+.3}), lovedRank {clr:.0}→{blr:.0}, dislikedRank {cdr:.0}→{bdr:.0}, ndcg@250 {cn:.3}→{bn:.3}, rescue {r:.1} vs damage {d:.1}",
            name = best.name,
            cl = content.loved_vs_disliked.mean,
            bl = best.loved_vs_disliked.mean,
            dl = best.delta_loved_vs_disliked.mean,
            clr = content.mean_rank_loved.mean,
            blr = best.mean_rank_loved.mean,
            cdr = content.mean_rank_disliked.mean,
            bdr = best.mean_rank_disliked.mean,
            cn = content.ndcg_at_250.mean,
            bn = best.ndcg_at_250.mean,
            r = best.rescued_positives.mean,
            d = best.damaged_positives.mean,
        )
    } else {
        format!(
            "HOLD_B1_1: no craft variant cleanly beats Content-only without loved-rank regression. Best candidate `{name}` (loved>dis Δ{dl:+.3}, lovedRank Δ{lr:+.0}, dislikedRank Δ{dr:+.0}, rescue {r:.1}/damage {d:.1}). Prefer Content-led Fit; only keep roles that help.",
            name = best.name,
            dl = best.delta_loved_vs_disliked.mean,
            lr = best.delta_mean_rank_loved.mean,
            dr = best.delta_mean_rank_disliked.mean,
            r = best.rescued_positives.mean,
            d = best.damaged_positives.mean,
        )
    }
}

pub fn run_craft_calibration(
    db: &Database,
    films: &[FilmRecord],
    seeds: &[u64],
    holdout_frac: f32,
) -> Result<CraftCalibrationReport, String> {
    let mut fold_reports = Vec::new();
    for &seed in seeds {
        let inputs = stratified_holdout_inputs(films, seed, holdout_frac);
        fold_reports.push(evaluate_fold(db, seed, &inputs, films)?);
    }
    let mean_ablations = mean_ablations(&fold_reports);
    let gate = decide_gate(&mean_ablations);
    Ok(CraftCalibrationReport {
        protocol: "milestone-b1.1-craft-calibration".into(),
        algorithm_version: ALGORITHM_VERSION.into(),
        folds: seeds.len(),
        holdout_frac,
        seeds: seeds.to_vec(),
        fold_reports,
        mean_ablations,
        gate,
    })
}

pub fn write_craft_calibration_artifact(
    taste_runs_dir: &std::path::Path,
    report: &CraftCalibrationReport,
) -> Result<String, String> {
    let dir = taste_runs_dir.join("benchmarks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("craft-calibration-{stamp}.json"));
    let latest = dir.join("craft-calibration-latest.json");
    let body = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&latest, &body);
    Ok(path.display().to_string())
}

/// Convenience for the live ignored test.
pub fn default_seeds() -> &'static [u64] {
    &BENCHMARK_SEEDS
}

pub fn default_holdout() -> f32 {
    BENCHMARK_HOLDOUT_FRAC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ablation_matrix_includes_content_and_actor_variants() {
        let names: Vec<_> = ablation_matrix().into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"contentOnly".into()));
        assert!(names.contains(&"actor".into()));
        assert!(names.contains(&"noActor".into()));
        assert!(names.contains(&"actorCorroboration".into()));
        assert!(names.iter().any(|n| n.starts_with("lambda_")));
    }
}
