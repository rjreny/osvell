use crate::taste::retrieve::SeedCoverage;
use crate::taste::explain::primary_retrieval;
use crate::taste::features::FeatureProfile;
use crate::taste::provenance::RecommendationProvenance;
use crate::taste::reason::{CriticReport, ReasonerReport};
use crate::taste::score::{ScorePool, ScoredCandidate};
use crate::taste::semantic::SemanticStats;
use crate::taste::validate::ValidationResult;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const KEEP_RUNS: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteRunLog {
    pub generated_at: String,
    pub model: String,
    pub web_used: bool,
    pub rated_count: u32,
    pub profile: Value,
    pub retrieval_counts: RetrievalCounts,
    pub semantic: SemanticStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<crate::taste::eval::ReplayReport>,
    pub ranked: Vec<Value>,
    pub shortlist: Vec<Value>,
    pub dropped_contextual: Vec<Value>,
    pub dropped_filmography: Vec<String>,
    pub discoveries: Vec<Value>,
    pub critic: Value,
    pub reasoner: Value,
    pub validation: Value,
    pub picks: Vec<Value>,
    pub call1_payload: Value,
    pub call2_payload: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalCounts {
    pub candidates_scored: usize,
    pub ranked: usize,
    pub shortlist: usize,
    pub dropped_contextual: usize,
    pub dropped_filmography: usize,
    pub discoveries: usize,
    pub final_picks: usize,
    pub eligible_seeds: usize,
    pub seeds_with_usable_related: usize,
    pub seeds_refreshed: usize,
    pub seeds_with_catalog: usize,
    pub candidates_with_catalog: usize,
    pub seed_catalog_coverage: f32,
    pub candidate_catalog_coverage: f32,
}

pub fn candidate_trace(c: &ScoredCandidate) -> Value {
    candidate_trace_in(c, None)
}

pub fn candidate_trace_in(
    c: &ScoredCandidate,
    ws: Option<&crate::taste::workspace::Workspace>,
) -> Value {
    let (kind, label) = primary_retrieval(&c.candidate.sources);
    let section = ws
        .map(|w| crate::taste::workspace::displayed_section(c, w))
        .unwrap_or_else(|| crate::taste::confidence::placement(c));
    let omit_reason = ws.and_then(|w| crate::taste::workspace::omit_reason(c, w));
    serde_json::json!({
        "title": c.candidate.title,
        "year": c.candidate.year,
        "tmdbId": c.candidate.tmdb_id,
        "origin": kind,
        "originLabel": label,
        "watchlist": c.candidate.watchlist,
        "section": section,
        "omitReason": omit_reason,
        "matchScore": crate::taste::confidence::match_score(c),
        "legacyMatchScore": crate::taste::confidence::legacy_match_score(c),
        "predictedFit": c.eligibility.predicted_fit,
        "fitConfidence": c.eligibility.confidence,
        "eligibilityState": c.eligibility.state,
        "eligibilityReason": c.eligibility.primary_reason,
        "semanticFit": c.score.semantic_fit,
        "semanticCoverage": c.score.semantic_coverage,
        "filterReason": crate::taste::confidence::filter_reason(c),
        "filmographyOnly": c.candidate.sources.iter().all(|s| s.kind == crate::taste::retrieve::RetrievalKind::Filmography)
            && !c.candidate.sources.is_empty()
            && !c.candidate.watchlist,
        "relatedOnly": crate::taste::confidence::related_only(c),
        "independentBridge": crate::taste::confidence::has_independent_new_bridge(c),
        "limitedEvidence": crate::taste::confidence::thin_evidence(c),
        "occupiesNew": crate::taste::confidence::occupies_new(c),
        "occupiesExplore": crate::taste::confidence::occupies_explore(c),
        "evidenceGrade": format!("{:?}", c.eligibility.evidence_grade).to_lowercase(),
        "sources": c.candidate.sources,
        "genres": c.candidate.genres,
        "directors": c.candidate.directors,
        "modes": c.candidate.modes,
        "contextualOnly": c.contextual_only,
        "score": c.score,
        "scoringReasons": c.scoring_reasons,
        "displayReasons": c.display_reasons,
        "reasons": c.reasons,
        "evidence": c.evidence,
        "positiveFeatures": c.positive_features,
        "negativeFeatures": c.negative_features,
        "matchedFeatures": c.matched_features,
        "hiddenFeatures": c.hidden_features,
        "eligibility": c.eligibility,
        "personKeys": c.person_keys,
    })
}

pub fn profile_trace(profile: &FeatureProfile) -> Value {
    let affinities: Vec<Value> = profile
        .affinities
        .iter()
        .take(40)
        .map(|a| {
            serde_json::json!({
                "feature": a.key.name,
                "family": a.key.family,
                "appearances": a.appearances,
                "recommendationMean": round2(a.recommendation_mean),
                "scoringAffinity": round2(a.scoring_affinity()),
                "confidence": round2(a.confidence),
                "portability": round2(a.portability),
                "citeable": a.citeable(),
                "positiveEvidence": a.positive_evidence.iter().map(|e| &e.title).take(4).collect::<Vec<_>>(),
                "negativeEvidence": a.negative_evidence.iter().map(|e| &e.title).take(4).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "affinities": affinities,
        "polarizing": profile.polarizing,
        "shifts": profile.shifts,
        "modes": profile.modes,
        "modeShifts": profile.mode_shifts,
    })
}

pub fn assemble(
    model: &str,
    web_used: bool,
    rated_count: u32,
    candidate_count: usize,
    profile: &FeatureProfile,
    pool: &ScorePool,
    shortlist: &[ScoredCandidate],
    discoveries: &[ScoredCandidate],
    critic: &CriticReport,
    reasoner: &ReasonerReport,
    validated: &ValidationResult,
    pick_provenance: &[RecommendationProvenance],
    call1_payload: Value,
    call2_payload: Value,
    coverage: &SeedCoverage,
    semantic: &SemanticStats,
    replay: Option<&crate::taste::eval::ReplayReport>,
) -> TasteRunLog {
    let candidates_scored = candidate_count;
    TasteRunLog {
        generated_at: chrono::Utc::now().to_rfc3339(),
        model: model.to_string(),
        web_used,
        rated_count,
        profile: profile_trace(profile),
        semantic: semantic.clone(),
        replay: replay.cloned(),
        retrieval_counts: RetrievalCounts {
            candidates_scored,
            ranked: pool.ranked.len(),
            shortlist: shortlist.len(),
            dropped_contextual: pool.dropped_contextual_total,
            dropped_filmography: pool.dropped_filmography_total,
            discoveries: discoveries.len(),
            final_picks: validated.picks.len(),
            eligible_seeds: coverage.eligible_seeds,
            seeds_with_usable_related: coverage.seeds_with_usable_related,
            seeds_refreshed: coverage.seeds_refreshed,
            seeds_with_catalog: coverage.seeds_with_catalog,
            candidates_with_catalog: coverage.candidates_with_catalog,
            seed_catalog_coverage: coverage_ratio(
                coverage.seeds_with_catalog,
                coverage.eligible_seeds,
            ),
            candidate_catalog_coverage: coverage_ratio(
                coverage.candidates_with_catalog,
                candidates_scored,
            ),
        },
        ranked: pool
            .ranked
            .iter()
            .map(|c| candidate_trace_in(c, Some(&validated.workspace)))
            .collect(),
        shortlist: shortlist
            .iter()
            .map(|c| candidate_trace_in(c, Some(&validated.workspace)))
            .collect(),
        dropped_contextual: pool.dropped_contextual.iter().map(candidate_trace).collect(),
        dropped_filmography: pool.dropped_filmography.clone(),
        discoveries: discoveries.iter().map(candidate_trace).collect(),
        critic: serde_json::to_value(critic).unwrap_or(Value::Null),
        reasoner: serde_json::json!({
            "title": reasoner.title,
            "summary": reasoner.summary,
            "affinities": reasoner.affinities,
            "aversions": reasoner.aversions,
            "picks": reasoner.picks.iter().map(|p| serde_json::json!({
                "id": p.id,
                "title": p.title,
                "year": p.year,
                "why": p.why,
                "mode": p.mode,
                "rhymesWith": p.rhymes_with,
            })).collect::<Vec<_>>(),
        }),
        validation: serde_json::json!({
            "dropped": validated.dropped,
            "warnings": validated.warnings.iter().map(|w| &w.message).collect::<Vec<_>>(),
            "narrowProfile": validated.narrow_profile,
        }),
        picks: pick_provenance
            .iter()
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .collect(),
        call1_payload,
        call2_payload,
    }
}

pub fn persist(dir: &Path, log: &TasteRunLog) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("taste-runs dir: {e}"))?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("{stamp}.json"));
    let body = serde_json::to_vec_pretty(log).map_err(|e| format!("taste run json: {e}"))?;
    fs::write(&path, body).map_err(|e| format!("write taste run: {e}"))?;
    let latest = dir.join("latest.json");
    let _ = fs::copy(&path, &latest);
    prune(dir);
    Ok(path)
}

fn prune(dir: &Path) {
    let mut files: Vec<_> = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".json") && n != "latest.json")
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| e.file_name());
    let extra = files.len().saturating_sub(KEEP_RUNS);
    for e in files.into_iter().take(extra) {
        let _ = fs::remove_file(e.path());
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn persist_writes_latest_and_prunes() {
        let dir = std::env::temp_dir().join(format!(
            "studio-taste-runs-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(1)
        ));
        let _ = fs::remove_dir_all(&dir);
        let log = TasteRunLog {
            generated_at: "now".into(),
            model: "llama".into(),
            web_used: false,
            rated_count: 8,
            profile: serde_json::json!({}),
            retrieval_counts: RetrievalCounts {
                candidates_scored: 0,
                ranked: 0,
                shortlist: 0,
                dropped_contextual: 0,
                dropped_filmography: 0,
                discoveries: 0,
                final_picks: 0,
                eligible_seeds: 0,
                seeds_with_usable_related: 0,
                seeds_refreshed: 0,
                seeds_with_catalog: 0,
                candidates_with_catalog: 0,
                seed_catalog_coverage: 0.0,
                candidate_catalog_coverage: 0.0,
            },
            semantic: SemanticStats::default(),
            replay: None,
            ranked: vec![],
            shortlist: vec![],
            dropped_contextual: vec![],
            dropped_filmography: vec![],
            discoveries: vec![],
            critic: serde_json::json!({}),
            reasoner: serde_json::json!({}),
            validation: serde_json::json!({}),
            picks: vec![],
            call1_payload: serde_json::json!({}),
            call2_payload: serde_json::json!({}),
        };
        let path = persist(&dir, &log).expect("write");
        assert!(path.exists());
        assert!(dir.join("latest.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

fn coverage_ratio(covered: usize, total: usize) -> f32 {
    if total == 0 {
        0.0
    } else {
        covered as f32 / total as f32
    }
}
