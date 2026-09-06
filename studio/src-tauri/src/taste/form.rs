//! Milestone B2 conditional Form.
//!
//! Soft distributions over runtime / era / language with hierarchical shrinkage.
//! Form is weak evidence: sparse support must yield neutral, not a guessed preference.
//! Live Fit_v1 keeps Form contribution at 0 until a component earns inclusion.

use crate::taste::dimensions::predicted_modes;
use crate::taste::features::{Credit, Keyword};
use crate::taste::retrieve::FilmRecord;
use serde::Serialize;

/// Aggressive shrink: Form needs substantial n_eff to leave zero.
const SHRINK_K: f32 = 16.0;
/// Minimum effective support before a context is trusted.
const CONTEXT_MIN_N_EFF: f32 = 8.0;
const BROAD_MIN_N_EFF: f32 = 12.0;
/// Runtime kernel half-width (minutes). 109 vs 111 are nearly identical.
const RUNTIME_KERNEL_HALF: f32 = 18.0;
const RUNTIME_BOUND: f32 = 0.06;
const ERA_BOUND: f32 = 0.05;
const LANGUAGE_BOUND: f32 = 0.04;

const MEANINGFUL_GENRES: &[&str] = &[
    "Animation",
    "Horror",
    "Documentary",
    "Science Fiction",
    "Western",
    "War",
    "Music",
    "Family",
    "Thriller",
    "Action",
    "Comedy",
    "Romance",
    "Crime",
    "Mystery",
    "Fantasy",
    "Adventure",
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormConfig {
    pub runtime: bool,
    pub era: bool,
    pub language: bool,
    /// Multiplier on already-bounded component contributions. Keep near 1.0.
    pub lambda: f32,
}

impl Default for FormConfig {
    fn default() -> Self {
        Self::off()
    }
}

impl FormConfig {
    pub fn off() -> Self {
        Self {
            runtime: false,
            era: false,
            language: false,
            lambda: 0.0,
        }
    }

    pub fn runtime_only() -> Self {
        Self {
            runtime: true,
            era: false,
            language: false,
            lambda: 1.0,
        }
    }

    pub fn era_only() -> Self {
        Self {
            runtime: false,
            era: true,
            language: false,
            lambda: 1.0,
        }
    }

    pub fn language_only() -> Self {
        Self {
            runtime: false,
            era: false,
            language: true,
            lambda: 1.0,
        }
    }

    pub fn runtime_and_era() -> Self {
        Self {
            runtime: true,
            era: true,
            language: false,
            lambda: 1.0,
        }
    }

    pub fn full() -> Self {
        Self {
            runtime: true,
            era: true,
            language: true,
            lambda: 1.0,
        }
    }

    pub fn any_enabled(&self) -> bool {
        (self.runtime || self.era || self.language) && self.lambda.abs() > 1e-8
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFilm {
    pub runtime: Option<i32>,
    pub year: Option<i32>,
    pub language: Option<String>,
    pub modes: Vec<String>,
    pub genres: Vec<String>,
    /// true = positive history, false = negative history.
    pub positive: bool,
    pub weight: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormPrior {
    pub films: Vec<FormFilm>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormComponentDiag {
    pub minutes: Option<i32>,
    pub year: Option<i32>,
    pub language: Option<String>,
    pub context_used: String,
    pub positive_n_eff: f32,
    pub negative_n_eff: f32,
    pub shrinkage: f32,
    pub raw_affinity: f32,
    pub final_affinity: f32,
    pub confidence: f32,
    pub fit_contribution: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormResult {
    pub score: f32,
    pub confidence: f32,
    pub support: f32,
    pub runtime: Option<FormComponentDiag>,
    pub era: Option<FormComponentDiag>,
    pub language: Option<FormComponentDiag>,
}

pub fn form_film_from_record(film: &FilmRecord) -> Option<FormFilm> {
    let rating = film.rating?;
    let positive = rating >= 4.0;
    let negative = rating <= 2.5;
    if !positive && !negative {
        return None;
    }
    let weight = film
        .signal
        .as_ref()
        .map(|s| s.preference_weight.max(0.15))
        .unwrap_or(1.0);
    Some(FormFilm {
        runtime: film.runtime,
        year: film.year,
        language: None, // not in local catalog yet — language stays neutral until hydrated
        modes: predicted_modes(&film.genres, &film.credits, &film.keywords),
        genres: film.genres.clone(),
        positive,
        weight,
    })
}

pub fn build_form_prior(films: &[FilmRecord]) -> FormPrior {
    FormPrior {
        films: films.iter().filter_map(form_film_from_record).collect(),
    }
}

pub fn build_form_prior_from_parts(
    films: &[(
        Option<i32>,
        Option<i32>,
        Option<String>,
        Vec<String>,
        Vec<Credit>,
        Vec<Keyword>,
        f32,
    )],
) -> FormPrior {
    let mut out = Vec::new();
    for (runtime, year, language, genres, credits, keywords, rating) in films {
        let positive = *rating >= 4.0;
        let negative = *rating <= 2.5;
        if !positive && !negative {
            continue;
        }
        out.push(FormFilm {
            runtime: *runtime,
            year: *year,
            language: language.clone(),
            modes: predicted_modes(genres, credits, keywords),
            genres: genres.clone(),
            positive,
            weight: 1.0,
        });
    }
    FormPrior { films: out }
}

fn primary_context(modes: &[String], genres: &[String]) -> String {
    if let Some(m) = modes.first() {
        return format!("mode:{m}");
    }
    for g in genres {
        if MEANINGFUL_GENRES
            .iter()
            .any(|m| g.eq_ignore_ascii_case(m))
        {
            return format!("genre:{g}");
        }
    }
    if let Some(g) = genres.first() {
        return format!("genre:{g}");
    }
    "broad".into()
}

fn context_matches(film: &FormFilm, context: &str) -> bool {
    if context == "broad" {
        return true;
    }
    if let Some(rest) = context.strip_prefix("mode:") {
        return film.modes.iter().any(|m| m.eq_ignore_ascii_case(rest));
    }
    if let Some(rest) = context.strip_prefix("genre:") {
        return film.genres.iter().any(|g| g.eq_ignore_ascii_case(rest));
    }
    false
}

fn context_ladder(modes: &[String], genres: &[String]) -> Vec<String> {
    let mut ladder = Vec::new();
    let primary = primary_context(modes, genres);
    ladder.push(primary.clone());
    if primary.starts_with("mode:") {
        // Fall back to meaningful genre before broad.
        for g in genres {
            if MEANINGFUL_GENRES
                .iter()
                .any(|m| g.eq_ignore_ascii_case(m))
            {
                let key = format!("genre:{g}");
                if !ladder.contains(&key) {
                    ladder.push(key);
                }
                break;
            }
        }
    }
    if !ladder.iter().any(|c| c == "broad") {
        ladder.push("broad".into());
    }
    ladder
}

fn runtime_kernel(candidate_rt: i32, film_rt: i32) -> f32 {
    let d = (candidate_rt - film_rt).abs() as f32;
    (1.0 - d / RUNTIME_KERNEL_HALF).max(0.0)
}

fn era_band(year: i32) -> &'static str {
    if year < 1970 {
        "pre-1970"
    } else if year < 1990 {
        "1970-1989"
    } else if year < 2005 {
        "1990-2004"
    } else if year < 2015 {
        "2005-2014"
    } else {
        "2015-present"
    }
}

fn era_soft_weight(candidate_year: i32, film_year: i32) -> f32 {
    // Soft within ~12 years; band equality gets a floor boost.
    let same_band = era_band(candidate_year) == era_band(film_year);
    let d = (candidate_year - film_year).abs() as f32;
    let soft = (-0.5 * (d / 12.0).powi(2)).exp();
    if same_band {
        soft.max(0.35)
    } else {
        soft
    }
}

fn shrink(raw: f32, n_eff: f32) -> (f32, f32) {
    let s = n_eff / (n_eff + SHRINK_K);
    (raw * s, s)
}

fn confidence_from_support(pos: f32, neg: f32) -> f32 {
    let n = pos + neg;
    (n / (n + SHRINK_K)).clamp(0.05, 0.95)
}

fn score_runtime_in_context(
    prior: &FormPrior,
    candidate_rt: i32,
    context: &str,
) -> (f32, f32, f32, f32) {
    let mut pos = 0.0;
    let mut neg = 0.0;
    for film in &prior.films {
        let Some(rt) = film.runtime else { continue };
        if !context_matches(film, context) {
            continue;
        }
        let w = film.weight * runtime_kernel(candidate_rt, rt);
        if w <= 1e-6 {
            continue;
        }
        if film.positive {
            pos += w;
        } else {
            neg += w;
        }
    }
    let denom = pos + neg;
    let raw = if denom < 1e-4 {
        0.0
    } else {
        ((pos - neg) / denom).clamp(-1.0, 1.0)
    };
    (raw, pos, neg, denom)
}

fn score_era_in_context(
    prior: &FormPrior,
    candidate_year: i32,
    context: &str,
) -> (f32, f32, f32, f32) {
    let mut pos = 0.0;
    let mut neg = 0.0;
    for film in &prior.films {
        let Some(y) = film.year else { continue };
        if !context_matches(film, context) {
            continue;
        }
        let w = film.weight * era_soft_weight(candidate_year, y);
        if w <= 1e-6 {
            continue;
        }
        if film.positive {
            pos += w;
        } else {
            neg += w;
        }
    }
    let denom = pos + neg;
    let raw = if denom < 1e-4 {
        0.0
    } else {
        ((pos - neg) / denom).clamp(-1.0, 1.0)
    };
    (raw, pos, neg, denom)
}

fn score_language_in_context(
    prior: &FormPrior,
    candidate_lang: &str,
    context: &str,
) -> (f32, f32, f32, f32) {
    let mut pos = 0.0;
    let mut neg = 0.0;
    for film in &prior.films {
        let Some(lang) = film.language.as_deref() else {
            continue;
        };
        if !lang.eq_ignore_ascii_case(candidate_lang) {
            continue;
        }
        if !context_matches(film, context) {
            continue;
        }
        let w = film.weight;
        if film.positive {
            pos += w;
        } else {
            neg += w;
        }
    }
    let denom = pos + neg;
    // Conservative negatives: small negative samples stay neutral.
    let raw = if denom < 1e-4 {
        0.0
    } else if neg > pos && neg < 8.0 {
        0.0
    } else if neg > pos {
        // Large consistent negative only → mild penalty.
        ((pos - neg) / denom).clamp(-0.35, 0.0)
    } else {
        ((pos - neg) / denom).clamp(0.0, 1.0)
    };
    (raw, pos, neg, denom)
}

fn pick_context_estimate<F>(
    ladder: &[String],
    mut score_ctx: F,
) -> (String, f32, f32, f32, f32, f32)
where
    F: FnMut(&str) -> (f32, f32, f32, f32),
{
    for (i, ctx) in ladder.iter().enumerate() {
        let (raw, pos, neg, n) = score_ctx(ctx);
        let min_n = if ctx == "broad" {
            BROAD_MIN_N_EFF
        } else {
            CONTEXT_MIN_N_EFF
        };
        if n >= min_n {
            let (final_aff, shrink) = shrink(raw, n);
            return (ctx.clone(), pos, neg, shrink, raw, final_aff);
        }
        // Last rung: if still thin, shrink hard toward 0 (do not invent preference).
        if i + 1 == ladder.len() {
            let (final_aff, shrink) = shrink(raw, n);
            return (ctx.clone(), pos, neg, shrink, raw, final_aff);
        }
    }
    ("broad".into(), 0.0, 0.0, 0.0, 0.0, 0.0)
}

fn runtime_component(
    prior: &FormPrior,
    modes: &[String],
    genres: &[String],
    minutes: Option<i32>,
) -> FormComponentDiag {
    let Some(rt) = minutes.filter(|&m| m >= 40) else {
        return FormComponentDiag {
            context_used: "missing".into(),
            ..Default::default()
        };
    };
    let ladder = context_ladder(modes, genres);
    let (context_used, pos, neg, shrinkage, raw, final_aff) =
        pick_context_estimate(&ladder, |ctx| score_runtime_in_context(prior, rt, ctx));
    let confidence = confidence_from_support(pos, neg);
    let fit_contribution = (final_aff * RUNTIME_BOUND).clamp(-RUNTIME_BOUND, RUNTIME_BOUND);
    FormComponentDiag {
        minutes: Some(rt),
        year: None,
        language: None,
        context_used,
        positive_n_eff: pos,
        negative_n_eff: neg,
        shrinkage,
        raw_affinity: raw,
        final_affinity: final_aff,
        confidence,
        fit_contribution,
    }
}

fn era_component(
    prior: &FormPrior,
    modes: &[String],
    genres: &[String],
    year: Option<i32>,
) -> FormComponentDiag {
    let Some(y) = year.filter(|&y| (1888..=2035).contains(&y)) else {
        return FormComponentDiag {
            context_used: "missing".into(),
            ..Default::default()
        };
    };
    let ladder = context_ladder(modes, genres);
    let (context_used, pos, neg, shrinkage, raw, final_aff) =
        pick_context_estimate(&ladder, |ctx| score_era_in_context(prior, y, ctx));
    let confidence = confidence_from_support(pos, neg);
    let fit_contribution = (final_aff * ERA_BOUND).clamp(-ERA_BOUND, ERA_BOUND);
    FormComponentDiag {
        minutes: None,
        year: Some(y),
        language: None,
        context_used,
        positive_n_eff: pos,
        negative_n_eff: neg,
        shrinkage,
        raw_affinity: raw,
        final_affinity: final_aff,
        confidence,
        fit_contribution,
    }
}

fn language_component(
    prior: &FormPrior,
    modes: &[String],
    genres: &[String],
    language: Option<&str>,
) -> FormComponentDiag {
    let Some(lang) = language.filter(|s| !s.trim().is_empty()) else {
        return FormComponentDiag {
            context_used: "missing".into(),
            ..Default::default()
        };
    };
    let ladder = context_ladder(modes, genres);
    let (context_used, pos, neg, shrinkage, raw, final_aff) =
        pick_context_estimate(&ladder, |ctx| score_language_in_context(prior, lang, ctx));
    let confidence = confidence_from_support(pos, neg);
    let fit_contribution = (final_aff * LANGUAGE_BOUND).clamp(-LANGUAGE_BOUND, LANGUAGE_BOUND);
    FormComponentDiag {
        minutes: None,
        year: None,
        language: Some(lang.to_string()),
        context_used,
        positive_n_eff: pos,
        negative_n_eff: neg,
        shrinkage,
        raw_affinity: raw,
        final_affinity: final_aff,
        confidence,
        fit_contribution,
    }
}

pub fn score_form(
    prior: &FormPrior,
    modes: &[String],
    genres: &[String],
    runtime: Option<i32>,
    year: Option<i32>,
    language: Option<&str>,
    config: &FormConfig,
) -> FormResult {
    if !config.any_enabled() {
        return FormResult::default();
    }
    let mut score = 0.0;
    let mut conf_acc = 0.0;
    let mut conf_w = 0.0;
    let mut support = 0.0;
    let mut runtime_diag = None;
    let mut era_diag = None;
    let mut language_diag = None;

    if config.runtime {
        let d = runtime_component(prior, modes, genres, runtime);
        score += d.fit_contribution;
        conf_acc += d.confidence * (d.positive_n_eff + d.negative_n_eff);
        conf_w += d.positive_n_eff + d.negative_n_eff;
        support += d.positive_n_eff + d.negative_n_eff;
        runtime_diag = Some(d);
    }
    if config.era {
        let d = era_component(prior, modes, genres, year);
        score += d.fit_contribution;
        conf_acc += d.confidence * (d.positive_n_eff + d.negative_n_eff);
        conf_w += d.positive_n_eff + d.negative_n_eff;
        support += d.positive_n_eff + d.negative_n_eff;
        era_diag = Some(d);
    }
    if config.language {
        let d = language_component(prior, modes, genres, language);
        score += d.fit_contribution;
        conf_acc += d.confidence * (d.positive_n_eff + d.negative_n_eff);
        conf_w += d.positive_n_eff + d.negative_n_eff;
        support += d.positive_n_eff + d.negative_n_eff;
        language_diag = Some(d);
    }

    let confidence = if conf_w > 1e-4 {
        (conf_acc / conf_w).clamp(0.05, 0.95)
    } else {
        0.1
    };
    FormResult {
        score: (score * config.lambda).clamp(-0.2, 0.2),
        confidence,
        support,
        runtime: runtime_diag,
        era: era_diag,
        language: language_diag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn film(
        rt: i32,
        year: i32,
        genres: &[&str],
        positive: bool,
        language: Option<&str>,
    ) -> FormFilm {
        FormFilm {
            runtime: Some(rt),
            year: Some(year),
            language: language.map(|s| s.into()),
            modes: if genres.iter().any(|g| g.eq_ignore_ascii_case("Animation")) {
                vec!["spectacle".into()]
            } else {
                vec!["story".into()]
            },
            genres: genres.iter().map(|s| (*s).into()).collect(),
            positive,
            weight: 1.0,
        }
    }

    #[test]
    fn sparse_runtime_stays_near_neutral() {
        let prior = FormPrior {
            films: vec![
                film(100, 2010, &["Drama"], true, None),
                film(105, 2011, &["Drama"], true, None),
            ],
        };
        let cfg = FormConfig::runtime_only();
        let result = score_form(
            &prior,
            &["story".into()],
            &["Drama".into()],
            Some(102),
            Some(2012),
            None,
            &cfg,
        );
        let rt = result.runtime.expect("runtime diag");
        assert!(
            rt.fit_contribution.abs() < 0.015,
            "sparse Form must not invent preference, got {}",
            rt.fit_contribution
        );
    }

    #[test]
    fn runtime_kernel_treats_nearby_minutes_alike() {
        let mut films = Vec::new();
        for _ in 0..12 {
            films.push(film(100, 2018, &["Animation"], true, None));
        }
        for _ in 0..4 {
            films.push(film(140, 2018, &["Animation"], false, None));
        }
        let prior = FormPrior { films };
        let cfg = FormConfig::runtime_only();
        let a = score_form(
            &prior,
            &["spectacle".into()],
            &["Animation".into()],
            Some(109),
            Some(2019),
            None,
            &cfg,
        );
        let b = score_form(
            &prior,
            &["spectacle".into()],
            &["Animation".into()],
            Some(111),
            Some(2019),
            None,
            &cfg,
        );
        let da = a.runtime.unwrap().fit_contribution;
        let db = b.runtime.unwrap().fit_contribution;
        assert!(
            (da - db).abs() < 0.005,
            "109 vs 111 should be nearly identical ({da} vs {db})"
        );
        assert!(da > 0.0, "strong mid-length positive mass should lift modestly");
        assert!(da <= RUNTIME_BOUND + 1e-5);
    }

    #[test]
    fn hierarchical_fallback_uses_broader_context_when_mode_thin() {
        let mut films = Vec::new();
        // Thin mode-specific history.
        films.push(film(95, 2010, &["Horror"], true, None));
        // Strong broad positive around 100 minutes across other genres.
        for _ in 0..20 {
            films.push(film(100, 2015, &["Comedy"], true, None));
        }
        let prior = FormPrior { films };
        let cfg = FormConfig::runtime_only();
        let result = score_form(
            &prior,
            &["intensity".into()],
            &["Horror".into()],
            Some(100),
            Some(2016),
            None,
            &cfg,
        );
        let rt = result.runtime.unwrap();
        assert!(
            rt.context_used == "broad" || rt.context_used.starts_with("genre:"),
            "expected fallback context, got {}",
            rt.context_used
        );
    }

    #[test]
    fn language_small_negative_sample_stays_neutral() {
        let prior = FormPrior {
            films: vec![
                film(100, 2010, &["Drama"], false, Some("ja")),
                film(110, 2011, &["Drama"], false, Some("ja")),
                film(105, 2012, &["Drama"], false, Some("ja")),
            ],
        };
        let cfg = FormConfig::language_only();
        let result = score_form(
            &prior,
            &["story".into()],
            &["Drama".into()],
            Some(100),
            Some(2015),
            Some("ja"),
            &cfg,
        );
        let lang = result.language.unwrap();
        assert_eq!(
            lang.fit_contribution, 0.0,
            "few Japanese negatives must not penalize"
        );
    }

    #[test]
    fn form_off_contributes_zero() {
        let prior = FormPrior {
            films: (0..30)
                .map(|_| film(100, 2018, &["Animation"], true, None))
                .collect(),
        };
        let result = score_form(
            &prior,
            &["spectacle".into()],
            &["Animation".into()],
            Some(100),
            Some(2019),
            None,
            &FormConfig::off(),
        );
        assert_eq!(result.score, 0.0);
        assert!(result.runtime.is_none());
    }

    #[test]
    fn era_component_is_bounded() {
        let mut films = Vec::new();
        for y in 2016..2024 {
            for _ in 0..4 {
                films.push(film(110, y, &["Action"], true, None));
            }
        }
        for y in 1972..1980 {
            for _ in 0..3 {
                films.push(film(110, y, &["Action"], false, None));
            }
        }
        let prior = FormPrior { films };
        let result = score_form(
            &prior,
            &["spectacle".into()],
            &["Action".into()],
            Some(110),
            Some(2020),
            None,
            &FormConfig::era_only(),
        );
        let era = result.era.unwrap();
        assert!(era.fit_contribution.abs() <= ERA_BOUND + 1e-5);
        assert!(era.fit_contribution > 0.0);
    }
}
