//! Read-only taste diagnostics. These findings never feed candidate scoring.

use crate::taste::features::{observations_from_film, FeatureFamily, FeatureKey, FeatureProfile};
use crate::taste::retrieve::{attach_signals, FilmRecord};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const EXCEPTION_OBSERVED_FLOOR: f32 = 0.35;
const EXCEPTION_RESIDUAL_FLOOR: f32 = 0.45;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteException {
    pub title: String,
    pub tmdb_id: Option<i64>,
    pub rating: f32,
    pub observed_preference: f32,
    pub expected_preference: f32,
    pub residual: f32,
    #[serde(default)]
    pub matching_features: Vec<String>,
    #[serde(default)]
    pub evidence_domains: Vec<String>,
    #[serde(default)]
    pub supporting_films: Vec<String>,
    #[serde(default)]
    pub opposing_films: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCombination {
    pub first_feature: String,
    pub second_feature: String,
    pub first_family: String,
    pub second_family: String,
    #[serde(default)]
    pub supporting_films: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteDiagnostics {
    #[serde(default)]
    pub exceptions: Vec<TasteException>,
    #[serde(default)]
    pub evidence_combinations: Vec<EvidenceCombination>,
}

pub fn derive(films: &[FilmRecord], profile: &FeatureProfile) -> TasteDiagnostics {
    TasteDiagnostics {
        exceptions: detect_exceptions(films),
        evidence_combinations: positive_combinations(films, profile),
    }
}

fn detect_exceptions(films: &[FilmRecord]) -> Vec<TasteException> {
    let mut exceptions = Vec::new();
    for (index, film) in films.iter().enumerate() {
        let (Some(rating), Some(signal)) = (film.rating, film.signal.as_ref()) else {
            continue;
        };
        let observed = signal.preference.affinity_preference;
        if observed.abs() < EXCEPTION_OBSERVED_FLOOR {
            continue;
        }
        let mut others: Vec<_> = films
            .iter()
            .enumerate()
            .filter_map(|(other_index, other)| (other_index != index).then_some(other.clone()))
            .collect();
        attach_signals(&mut others);
        let leave_one_out = crate::taste::feature_profile_from_films(&others);
        let keys = film_feature_keys(film);
        let matches: Vec<_> = leave_one_out
            .affinities
            .iter()
            .filter(|affinity| {
                affinity.citeable()
                    && keys.iter().any(|key| key.storage_key() == affinity.key.storage_key())
            })
            .collect();
        let domains: HashSet<_> = matches
            .iter()
            .map(|affinity| evidence_domain(affinity.key.family).to_string())
            .collect();
        if matches.len() < 3 || domains.len() < 2 {
            continue;
        }
        let expected = matches
            .iter()
            .map(|affinity| affinity.recommendation_mean * affinity.confidence)
            .sum::<f32>()
            / matches.len() as f32;
        let residual = observed - expected;
        if residual.abs() < EXCEPTION_RESIDUAL_FLOOR {
            continue;
        }
        let mut supporting = Vec::new();
        let mut opposing = Vec::new();
        for affinity in &matches {
            append_unique(
                &mut supporting,
                affinity
                    .positive_evidence
                    .iter()
                    .map(|evidence| evidence.title.clone()),
            );
            append_unique(
                &mut opposing,
                affinity
                    .negative_evidence
                    .iter()
                    .map(|evidence| evidence.title.clone()),
            );
        }
        supporting.truncate(6);
        opposing.truncate(6);
        let mut evidence_domains: Vec<_> = domains.into_iter().collect();
        evidence_domains.sort();
        exceptions.push(TasteException {
            title: film.title.clone(),
            tmdb_id: film.tmdb_id,
            rating,
            observed_preference: observed,
            expected_preference: expected,
            residual,
            matching_features: matches
                .iter()
                .map(|affinity| affinity.key.name.clone())
                .collect(),
            evidence_domains,
            supporting_films: supporting,
            opposing_films: opposing,
        });
    }
    exceptions.sort_by(|left, right| {
        crate::taste::ord::cmp_f32_desc(left.residual.abs(), right.residual.abs())
    });
    exceptions.truncate(8);
    exceptions
}

fn positive_combinations(films: &[FilmRecord], profile: &FeatureProfile) -> Vec<EvidenceCombination> {
    let eligible: HashSet<_> = profile
        .affinities
        .iter()
        .filter(|affinity| affinity.citeable() && affinity.recommendation_mean > 0.0)
        .map(|affinity| affinity.key.storage_key())
        .collect();
    let mut pairs: HashMap<(String, String), (FeatureKey, FeatureKey, Vec<String>)> = HashMap::new();
    for film in films.iter().filter(|film| {
        film.rating.unwrap_or(0.0) >= 4.0
            && film
                .signal
                .as_ref()
                .map(|signal| signal.preference.affinity_preference > 0.0)
                .unwrap_or(false)
    }) {
        let mut keys: Vec<_> = film_feature_keys(film)
            .into_iter()
            .filter(|key| eligible.contains(&key.storage_key()))
            .collect();
        keys.sort_by_key(|key| key.storage_key());
        keys.dedup_by(|left, right| left.storage_key() == right.storage_key());
        keys.truncate(12);
        for left_index in 0..keys.len() {
            for right_index in left_index + 1..keys.len() {
                let left = keys[left_index].clone();
                let right = keys[right_index].clone();
                let pair_key = (left.storage_key(), right.storage_key());
                let entry = pairs
                    .entry(pair_key)
                    .or_insert_with(|| (left, right, Vec::new()));
                if !entry.2.iter().any(|title| title == &film.title) {
                    entry.2.push(film.title.clone());
                }
            }
        }
    }
    let mut combinations: Vec<_> = pairs
        .into_values()
        .filter(|(_, _, films)| films.len() >= 3)
        .map(|(first, second, supporting_films)| EvidenceCombination {
            first_feature: first.name,
            second_feature: second.name,
            first_family: evidence_domain(first.family).into(),
            second_family: evidence_domain(second.family).into(),
            supporting_films,
        })
        .collect();
    combinations.sort_by(|left, right| {
        right
            .supporting_films
            .len()
            .cmp(&left.supporting_films.len())
            .then_with(|| left.first_feature.cmp(&right.first_feature))
            .then_with(|| left.second_feature.cmp(&right.second_feature))
    });
    combinations.truncate(24);
    combinations
}

fn film_feature_keys(film: &FilmRecord) -> Vec<FeatureKey> {
    let Some(signal) = film.signal.as_ref() else {
        return Vec::new();
    };
    observations_from_film(
        &film.title,
        film.rating.unwrap_or_default(),
        film.tmdb_id,
        signal,
        film.age_years,
        &film.genres,
        &film.credits,
        &film.keywords,
        film.year,
        film.runtime,
    )
    .into_iter()
    .map(|observation| observation.key)
    .filter(|key| !key.family.is_contextual())
    .collect()
}

fn evidence_domain(family: FeatureFamily) -> &'static str {
    match family {
        FeatureFamily::Director | FeatureFamily::Writer => "authorial",
        FeatureFamily::Cinematographer => "visual",
        FeatureFamily::Composer => "sound",
        FeatureFamily::Actor => "performance",
        FeatureFamily::Genre | FeatureFamily::Keyword => "narrative",
        FeatureFamily::Decade | FeatureFamily::Runtime => "contextual",
    }
}

fn append_unique(target: &mut Vec<String>, source: impl IntoIterator<Item = String>) {
    for value in source {
        if !target.iter().any(|existing| existing == &value) {
            target.push(value);
        }
    }
}
