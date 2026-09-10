pub mod board_validation;
pub mod cache;
pub mod confidence;
pub mod continuity;
pub mod continuity_calibration;
pub mod craft_calibration;
pub mod dimensions;
pub mod diagnostics;
pub mod discover;
pub mod diversify;
pub mod diversify_calibration;
pub mod eligibility;
pub mod eligibility_calibration;
pub mod eval;
pub mod exam_calibration;
pub mod exam_policy;
pub mod explain;
pub mod f2_board_ab;
pub mod family_fit;
pub mod features;
pub mod feedback;
pub mod form;
pub mod form_calibration;
pub mod freeze;
pub mod hybrid_board_ab;
pub mod hybrid_calibration;
pub mod hybrid_exam;
pub mod match_calibration;
pub mod ord;
pub mod preference;
pub mod provenance;
pub mod quality;
pub mod quality_calibration;
pub mod ranking_bench;
pub mod reason;
pub mod retrieve;
pub mod retrieval_bench;
pub mod runlog;
pub mod score;
pub mod semantic;
pub mod semantic_universe;
pub mod shortlist;
pub mod universe_calibration;
pub mod v1_policy;
pub mod validate;
pub mod workspace;

use crate::catalog::tmdb;
use crate::models::JobProgress;
use crate::storage::db::Database;
use crate::taste::features::{
    build_profile, execution_keywords_for_review, execution_signal_polarity,
    observations_from_film, repeated_execution_signals, FeatureFamily, FeatureProfile, Keyword,
    PORTABLE_CONTEXTUAL,
};
use crate::taste::dimensions::ModeFilm;
use crate::taste::preference::MIN_RATINGS;
use crate::taste::reason::{
    call1_payload, call2_payload, empty_critic, parse_critic, parse_reasoner, CALL1_SYSTEM,
    CALL2_SYSTEM, CriticReport, ReasonerPick,
};
use crate::taste::retrieve::{
    attach_signals, load_films, retrieve_with_coverage, seen_keys, Candidate, FilmRecord,
    MediaKind,
};
use crate::taste::score::ScoredCandidate;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

const KEYRING_SERVICE: &str = "studio";
const KEYRING_USER: &str = "openrouter_api_key";
const META_REPORT: &str = "taste_report";
const META_MODEL: &str = "taste_model";
const META_WEB: &str = "taste_web";
const OPENROUTER_CHAT: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_KEY: &str = "https://openrouter.ai/api/v1/key";
const MODEL_DEEPSEEK: &str = "deepseek/deepseek-v4-pro-0813";
const MODEL_QWEN_MAX: &str = "qwen/qwen3.8-2.4t-a95b";
const MODEL_QWEN_BALANCED: &str = "qwen/qwen3.8-27b";
const MODEL_GEMINI: &str = "google/gemini-3.7-flash";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteModelInfo {
    pub id: String,
    pub label: String,
    pub blurb: String,
    pub context: String,
    pub cost: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteKeyStatus {
    pub stored: bool,
    pub valid: Option<bool>,
    pub last_error: Option<String>,
    pub model: String,
    #[serde(default)]
    pub web: bool,
    #[serde(default)]
    pub models: Vec<TasteModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteAffinity {
    pub label: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteDimension {
    pub name: String,
    pub take: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteEvidence {
    pub title: String,
    #[serde(default)]
    pub film_id: Option<String>,
    #[serde(default)]
    pub tmdb_id: Option<i64>,
    #[serde(default)]
    pub poster: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TastePick {
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    pub why: String,
    #[serde(default)]
    pub rhymes_with: Vec<String>,
    pub film_id: Option<String>,
    pub tmdb_id: Option<i64>,
    pub source: String,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub scoring_reasons: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub evidence_items: Vec<TasteEvidence>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub origin_label: Option<String>,
    #[serde(default)]
    pub origin_display: Option<String>,
    #[serde(default)]
    pub matched_features: Vec<crate::taste::explain::MatchedFeatureView>,
    #[serde(default)]
    pub hidden_features: Vec<crate::taste::explain::MatchedFeatureView>,
    #[serde(default)]
    pub eligibility: crate::taste::explain::EligibilityTrace,
    #[serde(default)]
    pub match_score: u8,
    #[serde(default)]
    pub thin_evidence: bool,
    #[serde(default = "default_semantic_fit")]
    pub semantic_fit: f32,
    #[serde(default)]
    pub semantic_coverage: bool,
    #[serde(default)]
    pub attribution: Option<crate::taste::feedback::TasteAttribution>,
    /// Library state only — does not affect fit, eligibility, or rank.
    #[serde(default)]
    pub watchlist: bool,
}

fn default_semantic_fit() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteStat {
    pub label: String,
    pub count: u32,
    pub avg: f64,
    #[serde(default)]
    pub affinity: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteSnapshot {
    pub rated_count: u32,
    pub loved_count: u32,
    pub hated_count: u32,
    pub avg_rating: Option<f64>,
    pub genres: Vec<TasteStat>,
    pub decades: Vec<TasteStat>,
    pub directors: Vec<TasteStat>,
    pub actors: Vec<TasteStat>,
    #[serde(default)]
    pub cinematographers: Vec<TasteStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteReport {
    #[serde(default)]
    pub algorithm_version: String,
    pub title: String,
    pub summary: String,
    pub affinities: Vec<TasteAffinity>,
    pub aversions: Vec<TasteAffinity>,
    pub dimensions: Vec<TasteDimension>,
    #[serde(default)]
    pub new_picks: Vec<TastePick>,
    #[serde(default)]
    pub explore_picks: Vec<TastePick>,
    #[serde(default)]
    pub watchlist_picks: Vec<TastePick>,
    pub picks: Vec<TastePick>,
    pub model: String,
    pub generated_at: String,
    pub rated_count: u32,
    #[serde(default)]
    pub web_used: bool,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub run_log_path: Option<String>,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub diagnostics: crate::taste::diagnostics::TasteDiagnostics,
    #[serde(default)]
    pub board_eval: Option<eval::BoardEval>,
}

impl TasteReport {
    pub fn normalize(mut self) -> Self {
        if self.new_picks.is_empty() && self.watchlist_picks.is_empty() && !self.picks.is_empty() {
            self.new_picks = self.picks.clone();
        }
        self.picks = self
            .new_picks
            .iter()
            .cloned()
            .chain(self.watchlist_picks.iter().cloned())
            .collect();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteState {
    pub key: TasteKeyStatus,
    pub snapshot: TasteSnapshot,
    pub report: Option<TasteReport>,
    /// True when a saved board exists but was built under a different algorithm version.
    #[serde(default)]
    pub report_stale: bool,
    #[serde(default)]
    pub feedback: Vec<crate::taste::feedback::TasteFeedback>,
    #[serde(default)]
    pub observation: crate::taste::feedback::TasteObservationSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmTasteFit {
    pub available: bool,
    pub score: Option<u8>,
    pub band: String,
    pub evidence_grade: String,
    pub semantic_fit: f32,
    pub semantic_coverage: bool,
    pub supporting_signals: Vec<String>,
    pub counter_signals: Vec<String>,
    pub evidence_titles: Vec<String>,
    pub watched: bool,
    pub unavailable_reason: Option<String>,
}

impl FilmTasteFit {
    fn unavailable(reason: impl Into<String>, watched: bool) -> Self {
        Self {
            available: false,
            score: None,
            band: "notEnoughEvidence".into(),
            evidence_grade: "none".into(),
            semantic_fit: 0.5,
            semantic_coverage: false,
            supporting_signals: Vec::new(),
            counter_signals: Vec::new(),
            evidence_titles: Vec::new(),
            watched,
            unavailable_reason: Some(reason.into()),
        }
    }
}

pub fn default_model() -> String {
    MODEL_DEEPSEEK.to_string()
}

pub fn normalize_model(raw: &str) -> String {
    let compact = raw.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    if compact.contains("deepseek") {
        MODEL_DEEPSEEK.into()
    } else if compact.contains("gemini-3.7") || compact == "gemini" {
        MODEL_GEMINI.into()
    } else if compact.contains("qwen3.8-2.4t") {
        MODEL_QWEN_MAX.into()
    } else if compact.contains("qwen3.8-27b") {
        MODEL_QWEN_BALANCED.into()
    } else {
        // Legacy picker values are no longer offered. Use the recommended
        // model instead of sending an obsolete provider ID.
        default_model()
    }
}

fn openrouter_model_id(model: &str) -> &'static str {
    match model {
        MODEL_DEEPSEEK => MODEL_DEEPSEEK,
        MODEL_QWEN_MAX => MODEL_QWEN_MAX,
        MODEL_QWEN_BALANCED => MODEL_QWEN_BALANCED,
        MODEL_GEMINI => MODEL_GEMINI,
        _ => MODEL_DEEPSEEK,
    }
}

fn model_label(model: &str) -> &'static str {
    match model {
        MODEL_DEEPSEEK => "DeepSeek V4 Pro 0813",
        MODEL_QWEN_MAX => "Qwen3.8 2.4T A95B",
        MODEL_QWEN_BALANCED => "Qwen3.8 27B",
        MODEL_GEMINI => "Gemini 3.7 Flash",
        _ => "DeepSeek V4 Pro 0813",
    }
}

pub fn model_catalog() -> Vec<TasteModelInfo> {
    let mut models = vec![
        TasteModelInfo {
            id: MODEL_DEEPSEEK.into(),
            label: "DeepSeek V4 Pro 0813".into(),
            blurb: "Best fit for nuanced taste synthesis and long scored shortlists.".into(),
            context: "1M".into(),
            cost: "premium".into(),
        },
        TasteModelInfo {
            id: MODEL_QWEN_MAX.into(),
            label: "Qwen3.8 2.4T A95B".into(),
            blurb: "Maximum-depth alternative for a large, careful editorial pass.".into(),
            context: "32–164k".into(),
            cost: "highest".into(),
        },
        TasteModelInfo {
            id: MODEL_GEMINI.into(),
            label: "Gemini 3.7 Flash".into(),
            blurb: "Fast, lower-cost fallback with plenty of context for the full pass.".into(),
            context: "1M".into(),
            cost: "low".into(),
        },
        TasteModelInfo {
            id: MODEL_QWEN_BALANCED.into(),
            label: "Qwen3.8 27B".into(),
            blurb: "Balanced open-weight option when you want speed without a tiny model.".into(),
            context: "1M".into(),
            cost: "mid".into(),
        },
    ];
    models[1].context = "1M".into();
    models
}

fn request_timeout(_model: &str) -> Duration {
    Duration::from_secs(150)
}

fn fallback_model(model: &str) -> &'static str {
    if model == MODEL_DEEPSEEK {
        MODEL_GEMINI
    } else {
        MODEL_DEEPSEEK
    }
}

fn empty_status() -> TasteKeyStatus {
    TasteKeyStatus {
        stored: false,
        valid: None,
        last_error: None,
        model: default_model(),
        web: true,
        models: Vec::new(),
    }
}

fn with_prefs(db: &Database, mut status: TasteKeyStatus) -> Result<TasteKeyStatus, String> {
    status.model = stored_model(db)?;
    status.web = stored_web(db)?;
    status.models = model_catalog();
    Ok(status)
}

pub fn stored_model(db: &Database) -> Result<String, String> {
    Ok(db
        .get_meta(META_MODEL)?
        .map(|m| normalize_model(&m))
        .unwrap_or_else(default_model))
}

pub fn set_model(db: &Database, model: &str) -> Result<String, String> {
    let id = normalize_model(model);
    db.set_meta(META_MODEL, &id)?;
    crate::taste::cache::invalidate_snapshot(db)?;
    Ok(id)
}

pub fn stored_web(db: &Database) -> Result<bool, String> {
    Ok(match db.get_meta(META_WEB)?.as_deref() {
        Some("0") | Some("false") => false,
        _ => true,
    })
}

pub fn set_web(db: &Database, enabled: bool) -> Result<bool, String> {
    db.set_meta(META_WEB, if enabled { "1" } else { "0" })?;
    crate::taste::cache::invalidate_snapshot(db)?;
    Ok(enabled)
}

pub fn get_api_key() -> Result<Option<String>, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| e.to_string())?
        .get_password()
        .map(|k| {
            let trimmed = k.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .or_else(|e| {
            if e.to_string().contains("No matching entry") {
                Ok(None)
            } else {
                Err(e.to_string())
            }
        })
}

fn set_api_key(key: &str) -> Result<(), String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| e.to_string())?
        .set_password(key)
        .map_err(|e| e.to_string())
}

pub fn clear_api_key() -> Result<(), String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| e.to_string())?
        .set_password("")
        .map_err(|e| e.to_string())
}

fn probe_key(key: &str) -> Result<TasteKeyStatus, String> {
    let req = ureq::get(OPENROUTER_KEY)
        .set("Authorization", &format!("Bearer {key}"))
        .set("User-Agent", "Osvell/0.7 (local film app)")
        .timeout(Duration::from_secs(12));
    match req.call() {
        Ok(_) => Ok(TasteKeyStatus {
            stored: false,
            valid: Some(true),
            last_error: None,
            model: default_model(),
            web: true,
            models: Vec::new(),
        }),
        Err(err) => {
            let text = err.to_string();
            let invalid = text.contains("401") || text.contains("403");
            Ok(TasteKeyStatus {
                stored: false,
                valid: if invalid { Some(false) } else { None },
                last_error: Some(if invalid {
                    "OpenRouter rejected this key. Create a pay-as-you-go key at openrouter.ai/keys."
                        .into()
                } else {
                    format!("Could not reach OpenRouter: {text}")
                }),
                model: default_model(),
                web: true,
                models: Vec::new(),
            })
        }
    }
}

pub fn stored_status(db: &Database) -> Result<TasteKeyStatus, String> {
    let Some(key) = get_api_key()? else {
        return with_prefs(db, empty_status());
    };
    let mut status = empty_status();
    status.stored = true;
    status.valid = Some(true);
    let _ = key;
    with_prefs(db, status)
}

pub fn key_status(db: &Database) -> Result<TasteKeyStatus, String> {
    match get_api_key()? {
        None => with_prefs(db, empty_status()),
        Some(key) => {
            let mut status = probe_key(&key)?;
            status.stored = true;
            with_prefs(db, status)
        }
    }
}

pub fn store_api_key(db: &Database, key: &str) -> Result<TasteKeyStatus, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("Paste an OpenRouter key first.".into());
    }
    let mut status = probe_key(trimmed)?;
    if status.valid == Some(false) {
        return Ok(status);
    }
    set_api_key(trimmed)?;
    status.stored = true;
    with_prefs(db, status)
}

fn snapshot_of(films: &[FilmRecord], profile: Option<&FeatureProfile>) -> TasteSnapshot {
    let rated: Vec<&FilmRecord> = films.iter().filter(|f| f.rating.is_some()).collect();
    let loved = rated
        .iter()
        .filter(|f| f.rating.unwrap() >= 4.5)
        .count() as u32;
    let hated = rated
        .iter()
        .filter(|f| f.rating.unwrap() <= 2.5)
        .count() as u32;
    let avg = if rated.is_empty() {
        None
    } else {
        Some(
            (rated.iter().map(|f| f.rating.unwrap() as f64).sum::<f64>() / rated.len() as f64
                * 100.0)
                .round()
                / 100.0,
        )
    };
    let stats = |family: FeatureFamily| -> Vec<TasteStat> {
        profile
            .map(|p| {
                p.affinities
                    .iter()
                    .filter(|a| {
                        a.key.family == family
                            && (!family.is_contextual() || a.portability >= PORTABLE_CONTEXTUAL)
                    })
                    .take(16)
                    .map(|a| TasteStat {
                        label: a.key.name.clone(),
                        count: a.appearances,
                        avg: (a.preference_mean as f64 * 100.0).round() / 100.0,
                        affinity: Some((a.scoring_affinity() as f64 * 100.0).round() / 100.0),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    TasteSnapshot {
        rated_count: rated.len() as u32,
        loved_count: loved,
        hated_count: hated,
        avg_rating: avg,
        genres: stats(FeatureFamily::Genre),
        decades: stats(FeatureFamily::Decade),
        directors: stats(FeatureFamily::Director),
        actors: stats(FeatureFamily::Actor),
        cinematographers: stats(FeatureFamily::Cinematographer),
    }
}

pub(crate) fn feature_profile_from_films(films: &[FilmRecord]) -> FeatureProfile {
    let review_texts: Vec<&str> = films
        .iter()
        .filter(|film| film.rating.is_some() && film.signal.is_some())
        .filter_map(|film| film.review.as_deref())
        .collect();
    let repeated_execution = repeated_execution_signals(&review_texts);
    let enriched_keywords: Vec<Vec<Keyword>> = films
        .iter()
        .map(|film| {
            let mut keywords = film.keywords.clone();
            if let Some(review) = film.review.as_deref() {
                keywords.extend(execution_keywords_for_review(
                    review,
                    &repeated_execution,
                ));
            }
            keywords
        })
        .collect();
    let mut obs = Vec::new();
    for (index, film) in films.iter().enumerate() {
        let (Some(rating), Some(signal)) = (film.rating, film.signal.as_ref()) else {
            continue;
        };
        let review_keywords = film
            .review
            .as_deref()
            .map(|review| execution_keywords_for_review(review, &repeated_execution))
            .unwrap_or_default();
        let mut film_observations = observations_from_film(
            &film.title,
            rating,
            film.tmdb_id,
            signal,
            film.age_years,
            &film.genres,
            &film.credits,
            &enriched_keywords[index],
            film.year,
            film.runtime,
        );
        for observation in &mut film_observations {
            if !review_keywords
                .iter()
                .any(|keyword| keyword.name.eq_ignore_ascii_case(&observation.key.name))
            {
                continue;
            }
            let Some(polarity) = execution_signal_polarity(&observation.key.name) else {
                continue;
            };
            let strength = observation.affinity_preference.abs().clamp(0.35, 1.0);
            let supplemental = strength * 0.20;
            observation.affinity_preference =
                (observation.affinity_preference + polarity * supplemental).clamp(-1.0, 1.0);
            if polarity > 0.0 {
                observation.positive += supplemental;
            } else {
                observation.negative += supplemental;
            }
        }
        obs.extend(film_observations);
    }
    let mut profile = build_profile(&obs);
    let inputs: Vec<ModeFilm<'_>> = films
        .iter()
        .zip(enriched_keywords.iter())
        .map(|(f, keywords)| ModeFilm {
            title: &f.title,
            rating: f.rating,
            tmdb_id: f.tmdb_id,
            genres: &f.genres,
            credits: &f.credits,
            keywords,
            signal: f.signal.as_ref(),
            age_years: f.age_years,
        })
        .collect();
    let (dimensions, modes, mode_shifts) = crate::taste::dimensions::derive(&inputs);
    profile.dimensions = dimensions;
    profile.modes = modes;
    profile.mode_shifts = mode_shifts;
    profile
}

/// Scores the detail page with the same deterministic taste profile used by
/// recommendations. It never invokes the reasoner or mutates feedback state.
pub fn film_taste_detail(db: &Database, id: &str) -> Result<FilmTasteFit, String> {
    let detail = crate::queries::get_film(db, id)?;
    let watched = !detail.your_history.is_empty();
    let mut films = load_films(db)?;
    attach_signals(&mut films);
    let rated = films.iter().filter(|film| film.rating.is_some()).count();
    if rated < MIN_RATINGS {
        return Ok(FilmTasteFit::unavailable(
            format!("Rate at least {MIN_RATINGS} films to see a Taste fit."),
            watched,
        ));
    }
    if detail.genres.is_empty() && detail.crew.is_empty() && detail.cast.is_empty() {
        return Ok(FilmTasteFit::unavailable(
            "This film needs TMDB details before Osvell can compare it to your taste.",
            watched,
        ));
    }

    let mut profile = feature_profile_from_films(&films);
    let adjustments = crate::taste::feedback::active_feedback_adjustments(db)?;
    crate::taste::feedback::apply_feedback_adjustments(&mut profile, &adjustments);
    if profile.affinities.is_empty() {
        return Ok(FilmTasteFit::unavailable(
            "Osvell does not have enough rated metadata to explain this fit yet.",
            watched,
        ));
    }

    let candidate = Candidate {
        tmdb_id: detail.tmdb_id,
        title: detail.title,
        year: detail.year,
        poster: detail.poster,
        genres: detail.genres,
        credits: detail
            .crew
            .into_iter()
            .map(|member| crate::taste::features::Credit {
                id: member.tmdb_id,
                name: member.name,
                job: member.job,
            })
            .chain(detail.cast.into_iter().take(16).map(|member| crate::taste::features::Credit {
                id: member.tmdb_id,
                name: member.name,
                job: "Actor".into(),
            }))
            .collect(),
        keywords: detail
            .keywords
            .into_iter()
            .map(|name| crate::taste::features::Keyword { id: None, name })
            .collect(),
        runtime: detail.runtime,
        vote_count: detail.tmdb_vote_count.map(i64::from),
        watchlist: false,
        sources: Vec::new(),
        friend_affinity: 0.0,
        tmdb_related: 0.0,
        media_kind: MediaKind::Movie,
    };
    let scored = crate::taste::score::score_candidate(&profile, &candidate);
    let evidence_grade = format!("{:?}", scored.eligibility.evidence_grade).to_ascii_lowercase();
    let match_score = crate::taste::confidence::match_score(&scored);
    let available = scored.eligibility.evidence_grade.displayable();
    let band = if !available {
        "notEnoughEvidence"
    } else if match_score >= 70 {
        "strong"
    } else if match_score >= 55 {
        "mixed"
    } else {
        "weak"
    };
    Ok(FilmTasteFit {
        available,
        score: available.then_some(match_score),
        band: band.into(),
        evidence_grade,
        semantic_fit: scored.score.semantic_fit,
        semantic_coverage: scored.score.semantic_coverage,
        supporting_signals: scored.display_reasons.into_iter().take(4).collect(),
        counter_signals: scored.negative_features.into_iter().take(3).collect(),
        evidence_titles: scored.evidence.into_iter().take(6).collect(),
        watched,
        unavailable_reason: (!available).then_some(
            "Osvell found too little direct evidence to make this a useful Taste fit.".into(),
        ),
    })
}

pub fn load_state(db: &Database) -> Result<TasteState, String> {
    let (report, report_stale) = load_saved_report(db)?;
    let snapshot = load_display_snapshot(db, report.is_some())?;
    let feedback = crate::taste::feedback::list_feedback(db).unwrap_or_default();
    let hide = crate::taste::feedback::hide_ids(&feedback);
    let report = report
        .map(|r| filter_report_with_mood(r.normalize(), &hide, db))
        .transpose()?;
    Ok(TasteState {
        key: stored_status(db)?,
        snapshot,
        report,
        report_stale,
        feedback,
        observation: crate::taste::feedback::observation_summary(db).unwrap_or_default(),
    })
}

fn load_display_snapshot(db: &Database, has_saved_report: bool) -> Result<TasteSnapshot, String> {
    let mut films = load_films(db)?;
    // Saved reports already contain the displayed affinities and dimensions.
    // Only the first-run screen needs derived snapshot suggestions; reopening
    // a saved board needs current counts, not another full profile calculation.
    if has_saved_report {
        return Ok(snapshot_of(&films, None));
    }
    attach_signals(&mut films);
    let mut profile = feature_profile_from_films(&films);
    let adjustments = crate::taste::feedback::active_feedback_adjustments(db)?;
    crate::taste::feedback::apply_feedback_adjustments(&mut profile, &adjustments);
    Ok(snapshot_of(&films, Some(&profile)))
}

/// Load the last Taste board even when the algorithm version has moved on.
/// Stale boards still display so reopen does not wipe recommendations.
fn load_saved_report(db: &Database) -> Result<(Option<TasteReport>, bool), String> {
    let Some(raw) = db.get_meta(META_REPORT)? else {
        return Ok((None, false));
    };
    let Ok(report) = serde_json::from_str::<TasteReport>(&raw) else {
        return Ok((None, false));
    };
    let stale = report.algorithm_version != workspace::ALGORITHM_VERSION;
    Ok((Some(report), stale))
}

#[cfg(test)]
fn load_current_report(db: &Database) -> Result<Option<TasteReport>, String> {
    let (report, stale) = load_saved_report(db)?;
    Ok(report.filter(|_| !stale))
}

fn filter_report(mut report: TasteReport, hide: &std::collections::HashSet<i64>) -> TasteReport {
    let keep = |p: &TastePick| p.tmdb_id.map(|id| !hide.contains(&id)).unwrap_or(true);
    report.new_picks.retain(keep);
    report.explore_picks.retain(keep);
    report.watchlist_picks.retain(keep);
    report.picks = report
        .new_picks
        .iter()
        .cloned()
        .chain(report.watchlist_picks.iter().cloned())
        .collect();
    report
}

fn filter_report_with_mood(
    mut report: TasteReport,
    hide: &std::collections::HashSet<i64>,
    db: &Database,
) -> Result<TasteReport, String> {
    report = filter_report(report, hide);
    let keep = |pick: &TastePick| {
        pick
            .attribution
            .as_ref()
            .map(|attribution| {
                !crate::taste::feedback::mood_signature_is_suppressed(
                    db,
                    &attribution.mood_signature,
                )
                .unwrap_or(false)
            })
            .unwrap_or(true)
    };
    report.new_picks.retain(keep);
    report.explore_picks.retain(keep);
    report.watchlist_picks.retain(keep);
    report.picks = report
        .new_picks
        .iter()
        .cloned()
        .chain(report.watchlist_picks.iter().cloned())
        .collect();
    Ok(report)
}

pub fn analyze(
    db: &Database,
    progress: &mut dyn FnMut(JobProgress),
) -> Result<TasteReport, String> {
    analyze_with_run_log(db, progress, None, false)
}

pub fn analyze_with_run_log(
    db: &Database,
    progress: &mut dyn FnMut(JobProgress),
    run_log_dir: Option<&Path>,
    force_refresh: bool,
) -> Result<TasteReport, String> {
    let model = stored_model(db)?;
    let web = stored_web(db)?;
    let mut films = load_films(db)?;
    let rated = films.iter().filter(|f| f.rating.is_some()).count();
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Reading your log · {rated} ratings"),
        current: 1,
        total: 6,
        detail: Some("Building taste signals from ratings, likes, and watchlist".into()),
        ..Default::default()
    });
    if rated < MIN_RATINGS {
        return Err("Rate at least 8 films first so the agent has edges to work with.".into());
    }
    attach_signals(&mut films);
    let friend_keys = retrieve::friend_identity_keys(db);
    if !force_refresh {
        if let Ok(Some(snap)) = crate::taste::cache::load_snapshot(db) {
            if crate::taste::cache::snapshot_usable(db, &snap, &films, &friend_keys)? {
                progress(JobProgress {
                    job: "taste".into(),
                    label: "Using cached recommendations…".into(),
                    current: 6,
                    total: 6,
                    detail: Some(format!(
                        "{} scored titles ready · skipping model calls",
                        snap.scored_pool.len()
                    )),
                    ..Default::default()
                });
                return finish_from_snapshot(db, &snap, &model, web, rated as u32, None);
            }
        }
    }
    let key = get_api_key()?.ok_or_else(|| {
        "Add an OpenRouter key in Settings. DeepSeek V4 Pro 0813 is the recommended default."
            .to_string()
    })?;
    let seeds_refreshed =
        retrieve::enrich_eligible_seeds(db, &mut films, retrieve::SEED_HYDRATE_CAP, force_refresh);
    let mut profile = feature_profile_from_films(&films);
    let feedback_adjustments = crate::taste::feedback::active_feedback_adjustments(db)?;
    crate::taste::feedback::apply_feedback_adjustments(&mut profile, &feedback_adjustments);
    let seen = seen_keys(&films);
    progress(JobProgress {
        job: "taste".into(),
        label: "Gathering candidates…".into(),
        current: 2,
        total: 6,
        detail: Some(format!(
            "Refreshing {seeds_refreshed} seeds · related films, filmographies, watchlist"
        )),
        ..Default::default()
    });
    let retrieved = retrieve_with_coverage(db, &films, &profile, &seen, force_refresh)?;
    let mut coverage = retrieved.coverage;
    coverage.seeds_refreshed = seeds_refreshed;
    let mut candidates = retrieved.candidates;
    // The watchlist is an explicit user request and currently contains the
    // sparse metadata most likely to be dropped by the evidence gate. Give
    // the one-time backfill enough room to cover it before related results.
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Enriching {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some(format!(
            "{} seeds with related · {} already in catalog",
            coverage.seeds_with_usable_related, coverage.candidates_with_catalog
        )),
        ..Default::default()
    });
    retrieve::enrich_missing(db, &mut candidates, 320, force_refresh);
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Scoring {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some("Comparing against liked and disliked films".into()),
        ..Default::default()
    });
    let (semantic_scores, semantic_stats) =
        semantic::score_candidates(db, &key, &films, &candidates);
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Ranking {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some(format!(
            "Embeddings {}/{} · {} cache hits",
            semantic_stats.candidate_coverage,
            semantic_stats.candidate_items.max(1),
            semantic_stats.cache_hits
        )),
        ..Default::default()
    });
    let quality_catalog = crate::taste::quality::load_quality_catalog(db).ok();
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Ranking {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some(format!(
            "{} taste signals · scoring content fit",
            profile.affinities.len()
        )),
        ..Default::default()
    });
    let mut pool = crate::taste::score::score_pool_with_semantic(
        &profile,
        &candidates,
        &semantic_scores,
        quality_catalog.as_ref(),
    );
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Ranking {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some(format!(
            "{} cleared eligibility · clustering",
            pool.ranked.len()
        )),
        ..Default::default()
    });
    crate::taste::semantic::attach_semantic_clusters_from_db(db, &mut pool.ranked);
    crate::taste::semantic::attach_semantic_clusters_from_db(db, &mut pool.dropped_contextual);
    crate::taste::feedback::filter_mood_suppressed_candidates(db, &mut pool.ranked)?;
    progress(JobProgress {
        job: "taste".into(),
        label: format!("Ranking {} candidates…", candidates.len()),
        current: 2,
        total: 6,
        detail: Some(format!("{} on the shortlist path", pool.ranked.len())),
        ..Default::default()
    });
    let replay = if cfg!(debug_assertions) && std::env::var_os("STUDIO_TASTE_REPLAY").is_some() {
        Some(match eval::run_replay(db, &key, &films, 20) {
            Ok(report) => report,
            Err(err) => eval::ReplayReport {
                error: Some(err),
                ..Default::default()
            },
        })
    } else {
        None
    };
    let _benchmark_path = if std::env::var_os("STUDIO_TASTE_BENCHMARK").is_some() {
        match eval::run_retrieval_benchmark(
            db,
            &films,
            &eval::BENCHMARK_SEEDS,
            eval::BENCHMARK_HOLDOUT_FRAC,
        ) {
            Ok(report) => {
                let dir = std::env::var_os("STUDIO_TASTE_RUNS_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::env::temp_dir().join("studio-taste-runs")
                    });
                match eval::write_benchmark_artifact(&dir, &report) {
                    Ok(path) => {
                        eprintln!(
                            "[studio] retrieval benchmark recall@250 mean={:.3}±{:.3} pool≈{:.0} → {path}",
                            report.recall_at_250.mean,
                            report.recall_at_250.stdev,
                            report.candidate_pool_size.mean
                        );
                        Some(path)
                    }
                    Err(err) => {
                        eprintln!("[studio] retrieval benchmark write failed: {err}");
                        None
                    }
                }
            }
            Err(err) => {
                eprintln!("[studio] retrieval benchmark failed: {err}");
                None
            }
        }
    } else {
        None
    };
    let llm_ranked = shortlist::llm_pool(&pool.ranked);
    let short = shortlist::shortlist(&llm_ranked);
    if short.is_empty() {
        return Err("Could not build a candidate shortlist. Finish matching posters, then try again.".into());
    }

    let mut used_model = model.clone();
    let mut used_web = false;
    let mut note = semantic_stats
        .error
        .as_ref()
        .map(|err| format!("Semantic fit was partially unavailable: {err}"));
    let call1_body = call1_payload(&films, &profile, &short);
    progress(JobProgress {
        job: "taste".into(),
        label: format!(
            "Asking {} to critique the shortlist…",
            model_label(&model)
        ),
        current: 3,
        total: 6,
        detail: Some(format!(
            "{} shortlist titles · {} KB prompt",
            short.len(),
            (call1_body.to_string().len() / 1024).max(1)
        )),
        ..Default::default()
    });
    let critic = match run_json(&key, &model, CALL1_SYSTEM, &call1_body, false)
    {
        Ok((raw, _)) => critic_from_model_text(&raw),
        Err(err) if should_fallback(&err) => {
            let next = fallback_model(&model);
            used_model = next.to_string();
            note = Some(format!(
                "Fell back to {} after {} stalled on the critic pass.",
                model_label(next),
                model_label(&model)
            ));
            run_json(&key, next, CALL1_SYSTEM, &call1_body, false)
                .ok()
                .map(|(raw, _)| critic_from_model_text(&raw))
                .unwrap_or_else(empty_critic)
        }
        Err(_) => empty_critic(),
    };
    reason::apply_critic_rerank(&mut pool.ranked, &critic);
    let mut short = short;
    reason::apply_critic_rerank(&mut short, &critic);

    progress(JobProgress {
        job: "taste".into(),
        label: if web {
            "Running targeted discovery…".into()
        } else {
            "Skipping web discovery…".into()
        },
        current: 4,
        total: 6,
        detail: Some(if web {
            format!(
                "Up to {} research queries from the critic",
                critic.discovery_queries.len().min(3)
            )
        } else {
            "Web search is off in Settings".into()
        }),
        ..Default::default()
    });
    let mut discoveries = Vec::new();
    if web {
        for q in critic.discovery_queries.iter().take(3) {
            progress(JobProgress {
                job: "taste".into(),
                label: "Running targeted discovery…".into(),
                current: 4,
                total: 6,
                detail: Some(format!("Researching “{}”", q.query)),
                ..Default::default()
            });
            match run_json(
                &key,
                &used_model,
                "Return JSON {\"titles\":[{\"title\":\"...\",\"year\":1999}]} for this research query. At most 5 real films. JSON only.",
                &json!({ "query": q.query, "targetFacet": q.target_facet, "why": q.why }),
                true,
            ) {
                Ok((raw, actually_web)) => {
                    used_web = used_web || actually_web;
                    if let Ok(v) = extract_json(&raw) {
                        let titles = discover::parse_search_titles(&v);
                        discoveries.extend(discover::materialize(
                            db,
                            &titles,
                            &q.query,
                            &seen,
                            &profile,
                        ));
                    }
                }
                Err(_) => {}
            }
        }
        discoveries.truncate(discover::MAX_DISCOVERIES);
    }

    let mut ranked_for_workspace = pool.ranked.clone();
    ranked_for_workspace.extend(discoveries.iter().cloned());
    let validated = validate::hard_validate(
        &[],
        &ranked_for_workspace,
        &[],
        &seen,
        profile.modes.len(),
    );
    let call2_body = call2_payload(&films, &profile, &short, &critic, &discoveries);
    progress(JobProgress {
        job: "taste".into(),
        label: format!(
            "Asking {} for your taste profile…",
            model_label(&used_model)
        ),
        current: 5,
        total: 6,
        detail: Some(format!(
            "{} discoveries folded in · {} KB prompt",
            discoveries.len(),
            (call2_body.to_string().len() / 1024).max(1)
        )),
        ..Default::default()
    });
    let mut reasoner = run_reasoner(&key, &used_model, &call2_body)
        .map_err(|e| format!("taste profile · {}: {e}", model_label(&used_model)))?;
    reasoner = reason::ground_reasoner_with(
        reasoner,
        &profile,
        &films
            .iter()
            .filter(|f| f.watchlist)
            .map(|f| f.title.clone())
            .collect::<Vec<_>>(),
    );

    progress(JobProgress {
        job: "taste".into(),
        label: "Matching posters…".into(),
        current: 6,
        total: 6,
        detail: Some(format!(
            "Assembling board from {} ranked titles",
            validated.workspace.pre_feedback_pool.len()
        )),
        ..Default::default()
    });
    let report_run_id = Uuid::new_v4().to_string();
    let display_ws = crate::taste::cache::workspace_from_pool(
        db,
        &validated.workspace.pre_feedback_pool,
    )?;
    let probe_titles = crate::taste::feedback::list_feedback(db)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.action == crate::taste::feedback::ACTION_REJECTED)
        .filter_map(|row| {
            films
                .iter()
                .find(|film| film.tmdb_id == Some(row.tmdb_id))
                .map(|film| film.title.clone())
        })
        .collect::<Vec<_>>();
    let probe_refs: Vec<&str> = probe_titles.iter().map(|title| title.as_str()).collect();
    let board_eval =
        eval::evaluate_displayed_board(&display_ws.new_picks, &films, &probe_refs);
    let novelty =
        crate::taste::semantic_universe::semantic_novelty_for_candidates(&display_ws.new_picks);
    let mut semantic_stats = semantic_stats;
    semantic_stats.semantic_only_new = novelty.semantic_only;
    semantic_stats.also_graph_reachable_new = novelty.also_graph_reachable;
    let new_picks = to_taste_picks(
        db,
        &report_run_id,
        &reasoner.picks,
        &display_ws.new_picks,
        &critic,
        &discoveries,
    )?;
    let watch_picks = to_taste_picks(
        db,
        &report_run_id,
        &reasoner.picks,
        &display_ws.watchlist_picks,
        &critic,
        &discoveries,
    )?;
    let explore_picks = to_taste_picks(
        db,
        &report_run_id,
        &reasoner.picks,
        &display_ws.explore_picks,
        &critic,
        &discoveries,
    )?;
    let picks_out = (
        [
            new_picks.0.clone(),
            watch_picks.0.clone(),
        ]
        .concat(),
        [new_picks.1.clone(), watch_picks.1.clone(), explore_picks.1.clone()].concat(),
    );
    let run = runlog::assemble(
        model_label(&used_model),
        used_web,
        rated as u32,
        candidates.len(),
        &profile,
        &pool,
        &short,
        &discoveries,
        &critic,
        &reasoner,
        &validated,
        &picks_out.1,
        call1_body,
        call2_body,
        &coverage,
        &semantic_stats,
        replay.as_ref(),
    );
    let run_log_path = match run_log_dir {
        Some(dir) => match runlog::persist(dir, &run) {
            Ok(path) => Some(path.to_string_lossy().into_owned()),
            Err(err) => {
                note = Some(match note {
                    Some(prev) => format!("{prev} Run log failed: {err}"),
                    None => format!("Run log failed: {err}"),
                });
                None
            }
        },
        None => None,
    };
    let narrative = crate::taste::cache::TasteNarrative {
        title: if reasoner.title.trim().is_empty() {
            "Taste".into()
        } else {
            reasoner.title.clone()
        },
        summary: reasoner.summary.clone(),
        affinities: reasoner.affinities.clone(),
        aversions: reasoner.aversions.clone(),
        dimensions: reasoner.dimensions.clone(),
    };
    let mut fps = crate::taste::cache::fingerprints(
        &films,
        &candidates,
        &friend_keys,
        &used_model,
        used_web,
    );
    let pool_ids: Vec<i64> = validated
        .workspace
        .pre_feedback_pool
        .iter()
        .filter_map(|c| c.candidate.tmdb_id)
        .collect();
    fps.candidate_input_fingerprint = crate::taste::cache::bind_catalog_fingerprint(
        db,
        &fps.candidate_input_fingerprint,
        &pool_ids,
    );
    let _ = crate::taste::cache::save_snapshot(
        db,
        &crate::taste::cache::RunSnapshot {
            fingerprints: fps,
            catalog_valid_until: crate::taste::cache::catalog_valid_until_from_now(),
            scored_pool: validated.workspace.pre_feedback_pool.clone(),
            narrative: narrative.clone(),
        },
    );
    let diagnostics = crate::taste::diagnostics::derive(&films, &profile);
    let report = TasteReport {
        algorithm_version: workspace::ALGORITHM_VERSION.into(),
        title: narrative.title,
        summary: narrative.summary,
        affinities: narrative.affinities,
        aversions: narrative.aversions,
        dimensions: narrative.dimensions,
        new_picks: new_picks.0,
        explore_picks: explore_picks.0,
        watchlist_picks: watch_picks.0,
        picks: picks_out.0,
        model: model_label(&used_model).into(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        rated_count: rated as u32,
        web_used: used_web,
        note,
        run_log_path,
        run_id: report_run_id,
        diagnostics,
        board_eval: Some(board_eval),
    };
    db.set_meta(META_REPORT, &serde_json::to_string(&report).unwrap_or_default())?;
    Ok(report)
}

fn finish_from_snapshot(
    db: &Database,
    snap: &crate::taste::cache::RunSnapshot,
    model: &str,
    web: bool,
    rated: u32,
    note: Option<String>,
) -> Result<TasteReport, String> {
    let ws = crate::taste::cache::workspace_from_pool(db, &snap.scored_pool)?;
    let report_run_id = Uuid::new_v4().to_string();
    let mut films = load_films(db)?;
    attach_signals(&mut films);
    let diagnostics = crate::taste::diagnostics::derive(&films, &feature_profile_from_films(&films));
    let probe_titles = crate::taste::feedback::list_feedback(db)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.action == crate::taste::feedback::ACTION_REJECTED)
        .filter_map(|row| {
            films
                .iter()
                .find(|film| film.tmdb_id == Some(row.tmdb_id))
                .map(|film| film.title.clone())
        })
        .collect::<Vec<_>>();
    let probe_refs: Vec<&str> = probe_titles.iter().map(|title| title.as_str()).collect();
    let board_eval = eval::evaluate_displayed_board(&ws.new_picks, &films, &probe_refs);
    let empty_critic = empty_critic();
    let new_picks = to_taste_picks(db, &report_run_id, &[], &ws.new_picks, &empty_critic, &[])?;
    let explore_picks = to_taste_picks(db, &report_run_id, &[], &ws.explore_picks, &empty_critic, &[])?;
    let watch_picks = to_taste_picks(db, &report_run_id, &[], &ws.watchlist_picks, &empty_critic, &[])?;
    let new_list = new_picks.0;
    let explore_list = explore_picks.0;
    let watch_list = watch_picks.0;
    let report = TasteReport {
        algorithm_version: workspace::ALGORITHM_VERSION.into(),
        title: snap.narrative.title.clone(),
        summary: snap.narrative.summary.clone(),
        affinities: snap.narrative.affinities.clone(),
        aversions: snap.narrative.aversions.clone(),
        dimensions: snap.narrative.dimensions.clone(),
        picks: [new_list.clone(), watch_list.clone()].concat(),
        new_picks: new_list,
        explore_picks: explore_list,
        watchlist_picks: watch_list,
        model: model_label(model).into(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        rated_count: rated,
        web_used: web,
        note,
        run_log_path: None,
        run_id: report_run_id,
        diagnostics,
        board_eval: Some(board_eval),
    };
    db.set_meta(META_REPORT, &serde_json::to_string(&report).unwrap_or_default())?;
    Ok(report)
}

/// Critic pass is best-effort: prose / empty / broken JSON must not abort Taste.
fn critic_from_model_text(raw: &str) -> CriticReport {
    extract_json(raw)
        .ok()
        .and_then(|v| parse_critic(&v).ok())
        .unwrap_or_else(empty_critic)
}

fn run_reasoner(key: &str, model: &str, payload: &Value) -> Result<reason::ReasonerReport, String> {
    let (raw, _) = match run_json(key, model, CALL2_SYSTEM, payload, false) {
        Ok(v) => v,
        Err(err) if should_fallback(&err) => {
            let next = fallback_model(model);
            run_json(key, next, CALL2_SYSTEM, payload, false).map_err(|e| friendly_err(&e, next))?
        }
        Err(err) => return Err(friendly_err(&err, model)),
    };
    let parsed = extract_json(&raw).and_then(|v| parse_reasoner(&v));
    match parsed {
        Ok(r) if !r.title.is_empty() || !r.summary.is_empty() => Ok(r),
        _ => {
            let next = fallback_model(model);
            let (raw2, _) = run_json(key, next, CALL2_SYSTEM, payload, false)
                .map_err(|e| friendly_err(&e, next))?;
            parse_reasoner(&extract_json(&raw2)?).map_err(|e| {
                format!(
                    "{} wrote invalid JSON ({e}). Try Gemini 3.7 Flash with web search off.",
                    model_label(next)
                )
            })
        }
    }
}

fn to_taste_picks(
    db: &Database,
    run_id: &str,
    reasoner_picks: &[ReasonerPick],
    validated: &[ScoredCandidate],
    critic: &CriticReport,
    discoveries: &[ScoredCandidate],
) -> Result<(Vec<TastePick>, Vec<crate::taste::provenance::RecommendationProvenance>), String> {
    let films = crate::taste::retrieve::load_films(db).unwrap_or_default();
    let allow = crate::taste::score::close_to_allowlist(&films);
    let mut out = Vec::new();
    let mut traces = Vec::new();
    for scored in validated {
        let rp = reasoner_picks.iter().find(|p| {
            (!p.id.is_empty()
                && scored.candidate.tmdb_id.map(|id| format!("tmdb:{id}")).as_deref() == Some(p.id.as_str()))
                || p.title.eq_ignore_ascii_case(&scored.candidate.title)
        });
        let mut poster = scored.candidate.poster.clone();
        let mut film_id = scored.candidate.tmdb_id.map(|id| format!("tmdb:{id}"));
        if poster.is_none() {
            if let Some(id) = scored.candidate.tmdb_id {
                if let Ok(Some(hit)) = tmdb::lookup_movie(&scored.candidate.title, scored.candidate.year)
                {
                    poster = hit.poster;
                    let _ = id;
                }
            }
        }
        if let Some(tid) = scored.candidate.tmdb_id {
            if let Ok(Some(local)) = library_id_for_tmdb(db, tid) {
                film_id = Some(local);
            }
        }
        let origin = if discoveries.iter().any(|d| {
            d.candidate.tmdb_id.is_some() && d.candidate.tmdb_id == scored.candidate.tmdb_id
        }) {
            "discovery"
        } else {
            "shortlist"
        };
        let assess = critic.candidate_assessments.iter().find(|a| {
            scored
                .candidate
                .tmdb_id
                .map(|id| a.id == format!("tmdb:{id}"))
                .unwrap_or(false)
        });
        let (retrieval_kind, origin_label) =
            crate::taste::explain::primary_retrieval(&scored.candidate.sources);
        let provenance = crate::taste::provenance::RecommendationProvenance {
            tmdb_id: scored.candidate.tmdb_id,
            title: scored.candidate.title.clone(),
            origin: crate::taste::provenance::origin_from_sources(&scored.candidate.sources),
            retrieval_kind: retrieval_kind.clone(),
            retrieval_sources: scored.candidate.sources.clone(),
            deterministic_score: scored.score.clone(),
            llm_mode: crate::taste::provenance::RecommendationMode::parse(
                rp.map(|p| p.mode.as_str()).unwrap_or(origin),
            ),
            seed_films: scored.evidence.clone(),
            scoring_reasons: scored.scoring_reasons.clone(),
            display_reasons: scored.display_reasons.clone(),
            matched_features: scored.matched_features.clone(),
            hidden_features: scored.hidden_features.clone(),
            eligibility: scored.eligibility.clone(),
            positive_features: scored.positive_features.clone(),
            negative_features_considered: scored.negative_features.clone(),
            call1_fit: assess.map(|a| a.fit.clone()),
            call1_concerns: assess.map(|a| a.concerns.clone()).unwrap_or_default(),
        };
        traces.push(provenance);
        let display = crate::taste::explain::canonicalize_reason_lines(if scored.display_reasons.is_empty() {
            scored.reasons.clone()
        } else {
            scored.display_reasons.clone()
        });
        let provenance = crate::taste::explain::format_provenance(&scored.candidate.sources);
        let why = rp
            .map(|p| p.why.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_default();
        let why = crate::taste::explain::ground_why(&why, &display, &scored.hidden_features);
        let evidence = if films.is_empty() {
            scored.evidence.clone()
        } else {
            crate::taste::score::filter_close_to_evidence(&scored.evidence, &allow)
        };
        let evidence_items = evidence_items_for(&scored.candidate.sources, &evidence, &films);
        let attribution = scored.candidate.tmdb_id.map(|tmdb_id| {
            let cited_positive = scored
                .matched_features
                .iter()
                .filter(|feature| feature.cited && feature.recommendation_mean > 0.0)
                .cloned()
                .collect();
            let cited_negative = scored
                .hidden_features
                .iter()
                .filter(|feature| {
                    scored
                        .negative_features
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&feature.name))
                })
                .cloned()
                .collect();
            crate::taste::feedback::record_exposure(
                db,
                crate::taste::feedback::TasteAttribution {
                    exposure_id: Uuid::new_v4().to_string(),
                    run_id: run_id.into(),
                    tmdb_id,
                    title: scored.candidate.title.clone(),
                    evidence_grade: format!("{:?}", scored.eligibility.evidence_grade).to_ascii_lowercase(),
                    cited_positive,
                    cited_negative,
                    seed_films: scored.evidence.clone(),
                    semantic_fit: scored.score.semantic_fit,
                    diversity_adjustment: 0.0,
                    retrieval_source: retrieval_kind.clone(),
                    ranking_rationale: scored.display_reasons.clone(),
                    mood_signature: crate::taste::feedback::mood_signature_for_candidate(scored),
                    prior_candidate_exposures: 0,
                    prior_feature_exposures: Vec::new(),
                },
            )
        }).transpose()?;
        out.push(TastePick {
            title: scored.candidate.title.clone(),
            year: scored.candidate.year,
            poster,
            why,
            rhymes_with: evidence.clone(),
            film_id,
            tmdb_id: scored.candidate.tmdb_id,
            source: origin.into(),
            reasons: display,
            scoring_reasons: scored.scoring_reasons.clone(),
            evidence,
            evidence_items,
            mode: rp.map(|p| p.mode.clone()).filter(|s| !s.is_empty()),
            origin: Some(retrieval_kind),
            origin_label: if origin_label.is_empty() {
                None
            } else {
                Some(origin_label)
            },
            origin_display: if provenance.is_empty() {
                None
            } else {
                Some(provenance)
            },
            matched_features: scored.matched_features.clone(),
            hidden_features: scored.hidden_features.clone(),
            eligibility: scored.eligibility.clone(),
            match_score: crate::taste::confidence::match_score(scored),
            thin_evidence: crate::taste::confidence::thin_evidence(scored),
            semantic_fit: scored.score.semantic_fit,
            semantic_coverage: scored.score.semantic_coverage,
            attribution,
            watchlist: scored.candidate.watchlist,
        });
    }
    Ok((out, traces))
}

/// Compact, navigable evidence for a Taste pick. Prefer explicit related-film
/// retrieval seeds, then accept title evidence only when the local match is
/// unambiguous. The scorer's existing evidence list intentionally contains
/// strings, so guessing between same-title remakes here would open the wrong
/// detail page.
fn evidence_items_for(
    sources: &[crate::taste::retrieve::RetrievalSource],
    evidence: &[String],
    films: &[FilmRecord],
) -> Vec<TasteEvidence> {
    const MAX_ITEMS: usize = 2;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for source in sources {
        if !source.kind.is_related() {
            continue;
        }
        let Some(tmdb_id) = source.seed_tmdb_id else {
            continue;
        };
        if let Some(film) = films.iter().find(|film| film.tmdb_id == Some(tmdb_id)) {
            push_evidence_item(&mut items, &mut seen, film, MAX_ITEMS);
        }
    }

    for title in evidence {
        if items.len() >= MAX_ITEMS {
            break;
        }
        let title_key = title.trim().to_ascii_lowercase();
        let mut matches = films
            .iter()
            .filter(|film| film.title.trim().to_ascii_lowercase() == title_key);
        let Some(film) = matches.next() else {
            continue;
        };
        if matches.next().is_none() {
            push_evidence_item(&mut items, &mut seen, film, MAX_ITEMS);
        }
    }

    items
}

fn push_evidence_item(
    items: &mut Vec<TasteEvidence>,
    seen: &mut std::collections::HashSet<String>,
    film: &FilmRecord,
    max_items: usize,
) {
    if items.len() >= max_items || !seen.insert(film.key.clone()) {
        return;
    }
    items.push(TasteEvidence {
        title: film.title.clone(),
        film_id: Some(film.key.clone()),
        tmdb_id: film.tmdb_id,
        poster: film.poster.clone(),
    });
}

fn library_id_for_tmdb(db: &Database, tmdb_id: i64) -> Result<Option<String>, String> {
    db.conn()
        .query_row(
            r#"
            SELECT COALESCE(ml.movie_id, smr.normalized_title || ':' || IFNULL(CAST(smr.release_year AS TEXT), ''))
            FROM movies m
            JOIN movie_links ml ON ml.movie_id = m.id
            JOIN source_movie_records smr ON smr.id = ml.source_movie_record_id
            WHERE m.tmdb_id = ?1
            LIMIT 1
            "#,
            params![tmdb_id],
            |row| row.get(0),
        )
        .optional_row()
}

fn run_json(
    key: &str,
    model: &str,
    system: &str,
    payload: &Value,
    web: bool,
) -> Result<(String, bool), String> {
    match send_chat(key, model, system, payload, web) {
        Ok(raw) => Ok((raw, web)),
        Err(err) if web && (tools_unsupported(&err) || is_guardrail(&err)) => {
            send_chat(key, model, system, payload, false).map(|raw| (raw, false))
        }
        Err(err) => Err(err),
    }
}

fn send_chat(
    key: &str,
    model: &str,
    system: &str,
    payload: &Value,
    web: bool,
) -> Result<String, String> {
    let timeout = request_timeout(model);
    let body = chat_body(model, system, payload, web);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(timeout)
        .timeout_write(Duration::from_secs(30))
        .timeout(timeout + Duration::from_secs(20))
        .build();
    let response = match agent
        .post(OPENROUTER_CHAT)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .set("HTTP-Referer", "https://github.com/rjreny/osvell")
        .set("X-Title", "Osvell Taste")
        .set("User-Agent", "Osvell/0.7 (local film app)")
        .send_string(&body.to_string())
    {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format_openrouter_err(Some(code), &text));
        }
        Err(err) => return Err(err.to_string()),
    };
    let v: Value = serde_json::from_str(&response)
        .map_err(|e| format!("OpenRouter returned non-JSON: {e}"))?;
    if v.get("error").is_some() {
        return Err(format_openrouter_err(None, &response));
    }
    let content = v["choices"]
        .as_array()
        .and_then(|c| c.first())
        .and_then(choice_text)
        .ok_or_else(|| "OpenRouter response had no text".to_string())?;
    Ok(content)
}

fn chat_body(model: &str, system: &str, payload: &Value, web: bool) -> Value {
    let mut body = json!({
        "model": openrouter_model_id(model),
        "temperature": 0.35,
        "max_tokens": 6000,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": payload.to_string() }
        ]
    });
    if model == MODEL_DEEPSEEK {
        body["reasoning"] = json!({ "effort": "low", "exclude": true });
    }
    if web {
        body["tools"] = json!([{
            "type": "openrouter:web_search",
            "parameters": {
                "max_results": 4,
                "max_total_results": 8,
                "max_uses": 3,
                "search_context_size": "low"
            }
        }]);
    } else {
        body["response_format"] = json!({ "type": "json_object" });
    }
    body
}

fn choice_text(choice: &Value) -> Option<String> {
    let msg = &choice["message"];
    if let Some(s) = msg["content"].as_str().filter(|s| !s.trim().is_empty()) {
        return Some(s.to_string());
    }
    if let Some(parts) = msg["content"].as_array() {
        let joined: String = parts
            .iter()
            .filter_map(|part| part["text"].as_str().or_else(|| part.as_str()))
            .collect::<Vec<_>>()
            .join("");
        if !joined.trim().is_empty() {
            return Some(joined);
        }
    }
    msg["reasoning"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .or_else(|| choice["text"].as_str().map(|s| s.to_string()))
}

fn is_timeout_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("10060") || lower.contains("timed out") || lower.contains("timeout") || lower.contains("time out")
}

fn is_guardrail(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no endpoints") || lower.contains("guardrail") || lower.contains("data policy")
}

fn tools_unsupported(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("does not support")
        || lower.contains("tool use")
        || lower.contains("tools are not")
        || lower.contains("unknown tool")
}

fn should_fallback(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    is_timeout_error(err)
        || is_guardrail(err)
        || lower.contains("context length")
        || lower.contains("too many tokens")
        || lower.contains("input is too long")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("524")
        || lower.contains("connection reset")
        || lower.contains("failed to connect")
        || lower.contains("provider returned error")
}

fn clip_log(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn format_openrouter_err(http_status: Option<u16>, body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let err = parsed.as_ref().map(|v| &v["error"]);
    let msg = err
        .and_then(|e| e["message"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let code = err.and_then(|e| {
        e["code"]
            .as_u64()
            .map(|c| c.to_string())
            .or_else(|| e["code"].as_i64().map(|c| c.to_string()))
            .or_else(|| e["code"].as_str().map(|s| s.to_string()))
    });
    let provider = err.and_then(|e| {
        e["metadata"]["provider_name"]
            .as_str()
            .or_else(|| e["metadata"]["provider"].as_str())
    });
    let raw = err.and_then(|e| e["metadata"]["raw"].as_str());
    let mut parts = Vec::new();
    if let Some(status) = http_status {
        parts.push(format!("HTTP {status}"));
    }
    if let Some(code) = code {
        parts.push(format!("code {code}"));
    }
    if let Some(provider) = provider {
        parts.push(provider.to_string());
    }
    parts.push(msg.unwrap_or("OpenRouter returned an error").to_string());
    if let Some(raw) = raw {
        let clipped = clip_log(raw, 400);
        if !clipped.is_empty() {
            parts.push(format!("raw: {clipped}"));
        }
    } else if parsed.is_none() {
        let clipped = clip_log(body, 400);
        if !clipped.is_empty() {
            parts.push(clipped);
        }
    }
    if parts.len() == 1 && http_status.is_none() && body.trim().is_empty() {
        return "OpenRouter returned an error status".into();
    }
    parts.join(" · ")
}

fn friendly_err(err: &str, model: &str) -> String {
    if is_guardrail(err) {
        return format!(
            "{} has no OpenRouter endpoint that matches your privacy settings. Taste retries without web search, then with another model from your list. To allow more models, open openrouter.ai/settings/privacy",
            model_label(model)
        );
    }
    if is_timeout_error(err) {
        return format!(
            "{} did not finish in time. Taste will retry with another model from your list.",
            model_label(model)
        );
    }
    err.to_string()
}

pub fn extract_json(raw: &str) -> Result<Value, String> {
    let cleaned = strip_think(&strip_fences(raw));
    let object = first_object(&cleaned).ok_or_else(|| "Model did not return JSON".to_string())?;
    for candidate in [
        object.clone(),
        strip_trailing_commas(&object),
        close_truncated(&strip_trailing_commas(&object)),
    ] {
        if let Ok(v) = serde_json::from_str::<Value>(&candidate) {
            if v.is_object() {
                return Ok(v);
            }
        }
    }
    Err("Model did not return JSON".into())
}

fn strip_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest
    } else if let Some(start) = trimmed.find("```json") {
        &trimmed[start + 7..]
    } else {
        trimmed
    };
    inner.trim().trim_end_matches('`').trim().into()
}

fn strip_think(raw: &str) -> String {
    let mut s = raw.replace(['\u{201c}', '\u{201d}'], "\"");
    if let Some(end) = s.find("</think>") {
        s = s[end + 8..].to_string();
    }
    s
}

fn first_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut out = String::new();
    for c in s[start..].chars() {
        out.push(c);
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(out);
                }
            }
            _ => {}
        }
    }
    Some(out)
}

fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn close_truncated(s: &str) -> String {
    let mut out = s.trim_end().to_string();
    if out.ends_with(',') {
        out.pop();
    }
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    for c in out.chars() {
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    if in_string {
        out.push('"');
    }
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

trait OptionalRow<T> {
    fn optional_row(self) -> Result<Option<T>, String>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional_row(self) -> Result<Option<T>, String> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence_film(
        key: &str,
        title: &str,
        tmdb_id: Option<i64>,
        poster: Option<&str>,
    ) -> FilmRecord {
        FilmRecord {
            key: key.into(),
            title: title.into(),
            year: Some(2000),
            tmdb_id,
            rating: Some(4.5),
            liked: true,
            watched: true,
            watchlist: false,
            viewings: 1,
            last_date: None,
            genres: vec![],
            credits: vec![],
            keywords: vec![],
            recommendations: vec![],
            similar: vec![],
            collection_name: None,
            collection: vec![],
            runtime: Some(100),
            poster: poster.map(str::to_string),
            vote_count: Some(100),
            review: None,
            signal: None,
            age_years: None,
        }
    }

    #[test]
    fn extract_json_unwraps_fences() {
        let raw = "```json\n{\"title\":\"Night owl\",\"summary\":\"x\",\"picks\":[]}\n```";
        let v = extract_json(raw).unwrap();
        assert_eq!(v["title"], "Night owl");
    }

    #[test]
    fn critic_pass_survives_non_json_model_text() {
        let critic = critic_from_model_text("sorry, I can't critique that right now");
        assert!(critic.candidate_assessments.is_empty());
        assert!(critic.discovery_queries.is_empty());
    }

    #[test]
    fn model_ids_map() {
        assert_eq!(default_model(), MODEL_DEEPSEEK);
        assert_eq!(normalize_model("deepseek"), MODEL_DEEPSEEK);
        assert_eq!(normalize_model("Qwen2.5 72B"), MODEL_DEEPSEEK);
        assert_eq!(normalize_model("Qwen3.8 2.4T A95B"), MODEL_QWEN_MAX);
        assert_eq!(openrouter_model_id(MODEL_DEEPSEEK), "deepseek/deepseek-v4-pro-0813");
        assert_eq!(model_catalog().len(), 4);
    }

    #[test]
    fn deepseek_request_reserves_output_for_json() {
        let body = chat_body(MODEL_DEEPSEEK, "system", &json!({ "films": [] }), false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["reasoning"]["exclude"], true);
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn context_length_errors_are_retryable() {
        assert!(should_fallback(
            "HTTP 400: maximum context length is 32768 tokens"
        ));
    }

    #[test]
    fn openrouter_error_keeps_provider_and_raw() {
        let body = r#"{
            "error": {
                "message": "Provider returned error",
                "code": 502,
                "metadata": {
                    "provider_name": "DeepInfra",
                    "raw": "upstream overloaded\ntry again"
                }
            }
        }"#;
        let out = format_openrouter_err(Some(502), body);
        assert!(out.contains("HTTP 502"), "{out}");
        assert!(out.contains("DeepInfra"), "{out}");
        assert!(out.contains("upstream overloaded"), "{out}");
        assert!(should_fallback(&out));
    }

    #[test]
    fn saved_display_does_not_require_profile_rebuilding() {
        let db = Database::in_memory().unwrap();
        // Profile rebuilding reads adjustment events. A saved display must
        // remain independent of that work, including for older saved reports.
        db.conn().execute_batch("DROP TABLE taste_feedback_events").unwrap();
        let snapshot = load_display_snapshot(&db, true).unwrap();
        assert_eq!(snapshot.rated_count, 0);
        assert!(snapshot.genres.is_empty());
        assert!(load_display_snapshot(&db, false).is_err());
    }

    #[test]
    fn old_report_json_loads_as_new_section() {
        let raw = r#"{
            "title":"Night owl",
            "summary":"x",
            "affinities":[],
            "aversions":[],
            "dimensions":[],
            "picks":[{
                "title":"Heat",
                "year":1995,
                "poster":null,
                "why":"because",
                "rhymesWith":[],
                "filmId":null,
                "tmdbId":949,
                "source":"related"
            }],
            "model":"llama",
            "generatedAt":"2020-01-01T00:00:00Z",
            "ratedCount":8
        }"#;
        let report = serde_json::from_str::<TasteReport>(raw).unwrap().normalize();
        assert_eq!(report.new_picks.len(), 1);
        assert_eq!(report.new_picks[0].title, "Heat");
        assert!(report.new_picks[0].evidence_items.is_empty());
        assert!(report.watchlist_picks.is_empty());
        assert!(report.explore_picks.is_empty());
        assert_eq!(report.picks.len(), 1);
        let db = Database::in_memory().unwrap();
        db.set_meta(META_REPORT, raw).unwrap();
        // Empty algorithm version is treated as stale but still returned for display.
        let (saved, stale) = load_saved_report(&db).unwrap();
        assert!(stale);
        assert_eq!(saved.unwrap().normalize().new_picks[0].title, "Heat");
        assert!(load_current_report(&db).unwrap().is_none());
        let mut current = report;
        current.algorithm_version = "taste-workspace-23-effective-viewings".into();
        db.set_meta(META_REPORT, &serde_json::to_string(&current).unwrap()).unwrap();
        let (saved, stale) = load_saved_report(&db).unwrap();
        assert!(stale);
        assert_eq!(saved.unwrap().new_picks[0].title, "Heat");
        assert!(load_current_report(&db).unwrap().is_none());
        current.algorithm_version = workspace::ALGORITHM_VERSION.into();
        db.set_meta(META_REPORT, &serde_json::to_string(&current).unwrap()).unwrap();
        let (saved, stale) = load_saved_report(&db).unwrap();
        assert!(!stale);
        assert_eq!(saved.unwrap().new_picks[0].title, "Heat");
        assert_eq!(load_current_report(&db).unwrap().unwrap().new_picks[0].title, "Heat");
    }

    #[test]
    fn evidence_items_prefer_related_seeds_and_skip_ambiguous_titles() {
        use crate::taste::retrieve::{RetrievalKind, RetrievalSource};

        let films = vec![
            evidence_film("seed-id", "Seed", Some(1), Some("seed-poster")),
            evidence_film("duplicate-a", "Duplicate", Some(2), Some("duplicate-a-poster")),
            evidence_film("duplicate-b", "Duplicate", Some(3), Some("duplicate-b-poster")),
            evidence_film("unique-id", "Unique", None, Some("unique-poster")),
        ];
        let sources = vec![RetrievalSource::new(
            RetrievalKind::Related,
            "similar to Seed",
            Some(1),
        )];
        let evidence = vec!["Duplicate".into(), "Unique".into()];

        let items = evidence_items_for(&sources, &evidence, &films);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Seed");
        assert_eq!(items[0].film_id.as_deref(), Some("seed-id"));
        assert_eq!(items[0].tmdb_id, Some(1));
        assert_eq!(items[0].poster.as_deref(), Some("seed-poster"));
        assert_eq!(items[1].title, "Unique");
        assert_eq!(items[1].film_id.as_deref(), Some("unique-id"));
        assert_eq!(items[1].tmdb_id, None);
    }

    #[test]
    fn saved_report_drops_rejected_ids_on_load() {
        let raw = r#"{
            "title":"T",
            "summary":"s",
            "affinities":[],
            "aversions":[],
            "dimensions":[],
            "newPicks":[{
                "title":"Keep","year":2000,"poster":null,"why":"y","rhymesWith":[],
                "filmId":null,"tmdbId":2,"source":"related"
            },{
                "title":"Hide","year":2000,"poster":null,"why":"y","rhymesWith":[],
                "filmId":null,"tmdbId":1,"source":"related"
            }],
            "watchlistPicks":[],
            "explorePicks":[{
                "title":"HideExplore","year":1992,"poster":null,"why":"","rhymesWith":[],
                "filmId":null,"tmdbId":500,"source":"related"
            }],
            "picks":[],
            "model":"llama",
            "generatedAt":"2020-01-01T00:00:00Z",
            "ratedCount":8
        }"#;
        let report = serde_json::from_str::<TasteReport>(raw).unwrap().normalize();
        let mut hide = std::collections::HashSet::new();
        hide.insert(1);
        let filtered = filter_report(report, &hide);
        assert!(filtered.new_picks.iter().all(|p| p.tmdb_id != Some(1)));
        assert!(filtered.new_picks.iter().any(|p| p.tmdb_id == Some(2)));
        hide.insert(500);
        let filtered = filter_report(
            serde_json::from_str::<TasteReport>(raw).unwrap().normalize(),
            &hide,
        );
        assert!(filtered.explore_picks.iter().all(|p| p.tmdb_id != Some(500)));
    }
}
