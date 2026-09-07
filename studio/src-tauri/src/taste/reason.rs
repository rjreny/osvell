use crate::taste::explain::MatchedFeatureView;
use crate::taste::features::{keyword_strength_label, EvidenceFilm, FeatureProfile};
use crate::taste::retrieve::FilmRecord;
use crate::taste::score::{evidence_grade, ScoredCandidate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CALL1_SYSTEM: &str = r#"You are a film-taste critic inside Osvell.
The deterministic system already scored a shortlist from the user's complete rating history.
Your assessments WILL adjust the ranking of assessed shortlist candidates within bounded limits.
You do NOT select recommendations, invent titles, or return a full ordered list. You critique the shortlist and optionally request targeted research.

Return JSON only:
{
  "candidateAssessments": [
    {"id":"tmdb:123","fit":"strong|mixed|superficial","reason":"...","concerns":["..."]}
  ],
  "tasteGaps": [
    {"facet":"...","missingFromShortlist":true}
  ],
  "discoveryQueries": [
    {"query":"...","targetFacet":"...","why":"..."}
  ]
}

Rules:
- Assess suspicious, superficial, polarizing, weak, or mismatched candidates among the shortlist. You may also mark clear strong fits.
- fit must be one of: strong, mixed, superficial.
- Mark contextualOnly candidates as fit=superficial unless another primary feature clearly saves them.
- Decade, runtime, and catalog exposure are not recommendation targets.
- At most 3 discoveryQueries. Each must name a specific facet the shortlist is missing, preferably a taste mode (visual, comedy, intensity, spectacle, atmosphere, comfort).
- Do not emit picks, rankings, a full ordered list, or titles absent from the provided shortlist.
- Do not ignore negative evidence or polarizing features.
- Raw JSON object only.
"#;

pub const CALL2_SYSTEM: &str = r#"You are the film-taste narrator inside Osvell.
The deterministic system already chose the recommendation lists. You do NOT select, rank, add, or remove films.

Return JSON only:
{
  "title": "short taste type, 2-5 words",
  "summary": "2-3 sentences on WHY they like and dislike. Cite specific films.",
  "affinities": [{"label":"...","evidence":"..."}],
  "aversions": [{"label":"...","evidence":"..."}],
  "dimensions": [
    {"name":"visual","take":"..."},
    {"name":"story","take":"..."},
    {"name":"intensity","take":"..."},
    {"name":"comedy","take":"..."},
    {"name":"spectacle","take":"..."},
    {"name":"atmosphere","take":"..."},
    {"name":"comfort","take":"..."}
  ]
}

Narrative contract:
- Do not emit a picks array. If you do, it will be ignored for membership and order.
- Use displayReasons and matchedFeatures when describing why the already-chosen films fit.
- Affinity evidence must be films that actually carry that person or feature.
- Do not hunt for a decade or treat catalog exposure as taste.
- If the visual dimension is strong, cinematography is a real factor.
- Do not invent titles.
- Address Call 1 concerns in the summary when relevant.
- Aversions require negative evidence from strongAversions. Do not infer a dislike of horror, conspiracy, comedy, action, or any other facet from one unrelated disliked film.
- Do not use Batman & Robin as evidence for horror or conspiracy aversion.
- Do not use a watchlist title as evidence of dislike. A saved film is not an aversion.
- Never write "may not always enjoy", "mixed reaction", or similar hedging to invent an aversion. If strongAversions is empty, write only what they like.
- Thin evidence (appearances <= 2, limitedEvidence true, or displayReasons containing "limited evidence") must not be described as a dominant taste.
- Prefer craft, story, and specific keywords over family tropes and reaction adjectives.
- Title must be grounded in tasteModes (at least two of visual, story, intensity, comedy, spectacle, atmosphere, comfort) or recurring citeable people.
- Summary must name the strongest citeable directors, writers, or cinematographers from primaryAffinities when they exist. A generic action/adventure/humor paragraph is not acceptable.
- Write in second person ("You"). Never say "this user" or describe the viewer in the third person.
- Do not open with a generic mix-of-genres sentence or boilerplate about "strong visual elements" and "engaging stories." Do not invent themes, motifs, or subject matter that are absent from primaryAffinities, keywords, and tasteModes. Unsupported or generic copy is discarded even if a craft name is present.
- Be concrete. No marketing language. No em dashes.
- Raw JSON object only.
"#;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateAssessment {
    pub id: String,
    #[serde(default)]
    pub fit: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub concerns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteGap {
    pub facet: String,
    #[serde(default)]
    pub missing_from_shortlist: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryQuery {
    pub query: String,
    #[serde(default)]
    pub target_facet: String,
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CriticReport {
    #[serde(default)]
    pub candidate_assessments: Vec<CandidateAssessment>,
    #[serde(default)]
    pub taste_gaps: Vec<TasteGap>,
    #[serde(default)]
    pub discovery_queries: Vec<DiscoveryQuery>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonerPick {
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub rhymes_with: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReasonerReport {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub affinities: Vec<crate::taste::TasteAffinity>,
    #[serde(default)]
    pub aversions: Vec<crate::taste::TasteAffinity>,
    #[serde(default)]
    pub dimensions: Vec<crate::taste::TasteDimension>,
    #[serde(default)]
    pub picks: Vec<ReasonerPick>,
}

pub fn parse_critic(raw: &Value) -> Result<CriticReport, String> {
    if raw.get("picks").is_some() {
        return Err("Call 1 must not emit picks".into());
    }
    serde_json::from_value(raw.clone()).map_err(|e| format!("Call 1 JSON: {e}"))
}

pub fn parse_reasoner(raw: &Value) -> Result<ReasonerReport, String> {
    serde_json::from_value(raw.clone()).map_err(|e| format!("Call 2 JSON: {e}"))
}

pub fn apply_critic_rerank(ranked: &mut [ScoredCandidate], critic: &CriticReport) {
    for candidate in ranked.iter_mut() {
        let tmdb_id = candidate
            .candidate
            .tmdb_id
            .map(|id| format!("tmdb:{id}"));
        let assessment = critic.candidate_assessments.iter().find(|assessment| {
            tmdb_id
                .as_deref()
                .map(|id| assessment.id.trim().eq_ignore_ascii_case(id))
                .unwrap_or(false)
                || assessment
                    .id
                    .trim()
                    .eq_ignore_ascii_case(candidate.candidate.title.trim())
        });
        let factor = assessment.and_then(|assessment| {
            if assessment.fit.eq_ignore_ascii_case("strong") {
                Some(1.12)
            } else if assessment.fit.eq_ignore_ascii_case("mixed") {
                Some(0.90)
            } else if assessment.fit.eq_ignore_ascii_case("superficial") {
                Some(0.50)
            } else {
                None
            }
        });
        if let Some(factor) = factor {
            candidate.score.total = (candidate.score.total * factor).clamp(-1.5, 1.5);
        }
    }
    crate::taste::confidence::sort_workspace(ranked);
}

pub fn ground_reasoner(report: ReasonerReport, profile: &FeatureProfile) -> ReasonerReport {
    ground_reasoner_with(report, profile, &[])
}

pub fn ground_reasoner_with(
    mut report: ReasonerReport,
    profile: &FeatureProfile,
    watchlist_titles: &[String],
) -> ReasonerReport {
    let original_summary = report.summary.clone();
    report.aversions.retain(|a| {
        aversion_supported(&a.label, profile) && !cites_watchlist(&a.evidence, watchlist_titles)
    });
    if !title_supported(&report.title, profile) {
        report.title = deterministic_title(profile);
    }
    if summary_has_dislike_language(&original_summary) && report.aversions.is_empty() {
        report.summary = deterministic_summary(profile);
    } else {
        report.summary = ground_summary(&report.summary, profile, watchlist_titles);
    }
    for dim in &mut report.dimensions {
        if claims_ungrounded_dislike(&dim.take, profile)
            || (cites_watchlist(&dim.take, watchlist_titles)
                && summary_has_dislike_language(&dim.take))
        {
            dim.take.clear();
        }
    }
    report
}

fn title_tokens(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

fn profile_signals(profile: &FeatureProfile) -> Vec<String> {
    let mut signals = Vec::new();
    for m in &profile.modes {
        signals.push(m.dimension.to_ascii_lowercase());
    }
    for d in &profile.dimensions {
        signals.push(d.name.to_ascii_lowercase());
    }
    for a in profile.affinities.iter().filter(|a| {
        a.citeable()
            && a.recommendation_mean > 0.05
            && matches!(
                a.key.family,
                crate::taste::features::FeatureFamily::Genre
                    | crate::taste::features::FeatureFamily::Director
                    | crate::taste::features::FeatureFamily::Writer
                    | crate::taste::features::FeatureFamily::Cinematographer
                    | crate::taste::features::FeatureFamily::Keyword
            )
    }) {
        signals.push(a.key.name.to_ascii_lowercase());
    }
    signals
}

fn title_supported(title: &str, profile: &FeatureProfile) -> bool {
    let tokens = title_tokens(title);
    if tokens.len() < 2 {
        return false;
    }
    let signals = profile_signals(profile);
    let hits = tokens
        .iter()
        .filter(|t| {
            signals.iter().any(|s| s == *t || s.contains(t.as_str()) || t.contains(s.as_str()))
        })
        .count();
    hits >= 2
}

pub fn deterministic_title(profile: &FeatureProfile) -> String {
    let mut modes: Vec<_> = profile
        .modes
        .iter()
        .filter(|m| m.strength > 0.15)
        .collect();
    modes.sort_by(|a, b| {
        b.strength
            .partial_cmp(&a.strength)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.dimension.cmp(&b.dimension))
    });
    let labels: Vec<String> = modes
        .iter()
        .take(2)
        .map(|m| {
            let mut c = m.dimension.chars();
            match c.next() {
                Some(f) => format!("{}{}", f.to_ascii_uppercase(), c.as_str()),
                None => m.dimension.clone(),
            }
        })
        .collect();
    match labels.as_slice() {
        [a, b] => format!("{a} {b}"),
        [a] => format!("{a} taste"),
        _ => "Taste".into(),
    }
}

fn has_negative_signal(profile: &FeatureProfile, needle: &str) -> bool {
    let n = needle.trim().to_ascii_lowercase();
    if n.is_empty() {
        return false;
    }
    profile.affinities.iter().any(|a| {
        if a.key.name.to_ascii_lowercase() != n {
            return false;
        }
        a.recommendation_mean <= -0.08
            && a.appearances >= 5
            && matches!(
                a.key.family,
                crate::taste::features::FeatureFamily::Genre
                    | crate::taste::features::FeatureFamily::Keyword
            )
    })
}

fn aversion_supported(label: &str, profile: &FeatureProfile) -> bool {
    has_negative_signal(profile, label)
}

fn summary_has_dislike_language(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "not enjoy",
        "dislike",
        "aversion",
        "bounce off",
        "do not like",
        "don't like",
        "may not enjoy",
        "may not always enjoy",
        "not always enjoy",
        "mixed reaction",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

fn cites_watchlist(text: &str, watchlist_titles: &[String]) -> bool {
    let lower = text.to_lowercase();
    watchlist_titles.iter().any(|title| {
        let t = title.trim().to_lowercase();
        t.len() >= 4 && lower.contains(&t)
    })
}

fn claims_ungrounded_dislike(sentence: &str, profile: &FeatureProfile) -> bool {
    if !summary_has_dislike_language(sentence) {
        return false;
    }
    let lower = sentence.to_lowercase();
    let topics = ["horror", "conspiracy", "comedy", "camp", "humor", "action", "thriller"];
    topics.iter().any(|topic| {
        lower.contains(topic) && !has_negative_signal(profile, topic)
    })
}

fn last_name(name: &str) -> String {
    name.split_whitespace()
        .rev()
        .find(|w| w.chars().filter(|c| c.is_ascii_alphabetic()).count() > 2)
        .unwrap_or(name)
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_lowercase()
}

fn top_craft_people(profile: &FeatureProfile) -> Vec<&crate::taste::features::FeatureAffinity> {
    let mut people: Vec<_> = profile
        .affinities
        .iter()
        .filter(|a| {
            a.citeable()
                && a.recommendation_mean > 0.08
                && matches!(
                    a.key.family,
                    crate::taste::features::FeatureFamily::Director
                        | crate::taste::features::FeatureFamily::Writer
                        | crate::taste::features::FeatureFamily::Cinematographer
                )
        })
        .collect();
    people.sort_by(|a, b| {
        let qa = a.scoring_affinity() * a.portability.clamp(0.0, 1.0);
        let qb = b.scoring_affinity() * b.portability.clamp(0.0, 1.0);
        qb.partial_cmp(&qa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.family.sort_key().cmp(&b.key.family.sort_key()))
            .then_with(|| a.key.name.cmp(&b.key.name))
            .then_with(|| a.key.id.cmp(&b.key.id))
    });
    people.truncate(3);
    people
}

fn summary_names_craft(summary: &str, people: &[&crate::taste::features::FeatureAffinity]) -> bool {
    if people.is_empty() {
        return true;
    }
    let lower = summary.to_lowercase();
    people.iter().any(|a| {
        let full = a.key.name.to_ascii_lowercase();
        lower.contains(&full) || lower.contains(&last_name(&a.key.name))
    })
}

fn summary_is_generic(summary: &str) -> bool {
    let lower = summary.to_lowercase();
    const MARKERS: &[&str] = &[
        "action, adventure, and humor",
        "action, adventure and humor",
        "mix of action",
        "eclectic mix",
        "wide range of",
        "variety of genres",
        "diverse range",
        "you enjoy a mix",
        "you like a mix",
        "across a wide range",
        "variety of films",
        "diverse set of",
        "tree or game",
        "game themes",
        "this user",
        "this person",
        "the user enjoys",
        "the user likes",
        "strong visual elements",
        "engaging stories and characters",
        "clear character arcs",
        "emotional resonance",
        "overly complex",
        "convoluted plots",
        "they also enjoy",
        "they tend to",
        "they enjoy films",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    if lower.contains("they enjoy") && lower.contains("they tend") {
        return true;
    }
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    let soup = ["action", "adventure", "humor", "comedy"]
        .iter()
        .filter(|g| lower.contains(*g))
        .count();
    soup >= 3
}

fn sentence_is_generic(sentence: &str) -> bool {
    summary_is_generic(sentence)
}

fn allowed_theme_tokens(profile: &FeatureProfile) -> Vec<String> {
    let mut tokens = Vec::new();
    for a in &profile.affinities {
        if !a.citeable() {
            continue;
        }
        tokens.push(a.key.name.to_ascii_lowercase());
        for film in a
            .positive_evidence
            .iter()
            .chain(a.negative_evidence.iter())
        {
            tokens.push(film.title.to_ascii_lowercase());
            tokens.extend(film.keywords.iter().map(|k| k.to_ascii_lowercase()));
            tokens.extend(film.genres.iter().map(|g| g.to_ascii_lowercase()));
        }
    }
    for m in &profile.modes {
        tokens.push(m.dimension.to_ascii_lowercase());
    }
    tokens
}

fn theme_claims(sentence: &str) -> Vec<String> {
    let lower = sentence.to_lowercase();
    let Some(idx) = lower.find("theme").or_else(|| lower.find("motif")) else {
        return Vec::new();
    };
    const STOP: &[&str] = &[
        "the", "and", "or", "of", "your", "you", "with", "from", "that", "this", "about",
        "around", "including", "into", "for", "its", "their", "recurring", "visual", "story",
        "strongest", "lanes",
    ];
    lower[..idx]
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2 && !STOP.contains(w))
        .rev()
        .take(4)
        .map(|s| s.to_string())
        .collect()
}

fn sentence_has_unsupported_theme(sentence: &str, profile: &FeatureProfile) -> bool {
    let claims = theme_claims(sentence);
    if claims.is_empty() {
        return false;
    }
    let allowed = allowed_theme_tokens(profile);
    claims.iter().any(|claimed| {
        !allowed.iter().any(|a| a.contains(claimed) || claimed.contains(a))
    })
}

fn ground_summary(summary: &str, profile: &FeatureProfile, watchlist_titles: &[String]) -> String {
    let people = top_craft_people(profile);
    let kept: Vec<&str> = summary
        .split_inclusive(['.', '!', '?'])
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && !claims_ungrounded_dislike(s, profile)
                && !(summary_has_dislike_language(s) && cites_watchlist(s, watchlist_titles))
                && !sentence_is_generic(s)
                && !sentence_has_unsupported_theme(s, profile)
        })
        .collect();
    if kept.is_empty() {
        return deterministic_summary(profile);
    }
    let joined = kept.join(" ");
    if summary_is_generic(&joined) || !summary_names_craft(&joined, &people) {
        return deterministic_summary(profile);
    }
    joined
}

fn deterministic_summary(profile: &FeatureProfile) -> String {
    let title = deterministic_title(profile);
    let people = top_craft_people(profile);
    if people.is_empty() {
        return format!("Strongest lanes are {}.", title.to_lowercase());
    }
    let names: Vec<&str> = people.iter().map(|a| a.key.name.as_str()).collect();
    let mut films = Vec::new();
    for aff in &people {
        for film in &aff.positive_evidence {
            if film.rating >= 3.5
                && films.len() < 4
                && !films
                    .iter()
                    .any(|t: &String| t.eq_ignore_ascii_case(&film.title))
            {
                films.push(film.title.clone());
            }
        }
    }
    if films.is_empty() {
        format!(
            "Strongest lanes are {}. Recurring craft includes {}.",
            title.to_lowercase(),
            names.join(", ")
        )
    } else {
        format!(
            "Strongest lanes are {}. Recurring craft includes {}. Close films include {}.",
            title.to_lowercase(),
            names.join(", "),
            films.join(", ")
        )
    }
}

fn film_sample(film: &FilmRecord) -> Option<Value> {
    let rating = film.rating?;
    Some(json!({
        "title": film.title,
        "year": film.year,
        "tmdbId": film.tmdb_id,
        "rating": rating,
        "absolute": film.signal.as_ref().map(|s| s.preference.absolute),
        "heart": film.liked,
        "viewings": film.viewings,
    }))
}

fn representative_history(films: &[FilmRecord]) -> Value {
    let mut rated: Vec<&FilmRecord> = films.iter().filter(|f| f.rating.is_some()).collect();
    rated.sort_by(|a, b| {
        b.rating
            .unwrap()
            .partial_cmp(&a.rating.unwrap())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.tmdb_id.cmp(&b.tmdb_id))
            .then_with(|| a.title.cmp(&b.title))
    });
    let favorites: Vec<Value> = rated
        .iter()
        .filter(|f| f.rating.unwrap() >= 4.5)
        .take(10)
        .filter_map(|f| film_sample(f))
        .collect();
    let liked: Vec<Value> = rated
        .iter()
        .filter(|f| {
            let r = f.rating.unwrap();
            r >= 3.5 && r < 4.5
        })
        .take(10)
        .filter_map(|f| film_sample(f))
        .collect();
    let mixed: Vec<Value> = rated
        .iter()
        .filter(|f| {
            let r = f.rating.unwrap();
            r >= 2.6 && r < 3.5
        })
        .take(8)
        .filter_map(|f| film_sample(f))
        .collect();
    let disliked: Vec<Value> = rated
        .iter()
        .rev()
        .filter(|f| f.rating.unwrap() <= 2.5)
        .take(10)
        .filter_map(|f| film_sample(f))
        .collect();
    json!({
        "favorites": favorites,
        "liked": liked,
        "mixed": mixed,
        "disliked": disliked,
    })
}

fn titles_of(films: &[EvidenceFilm], n: usize) -> Vec<String> {
    films.iter().map(|e| e.title.clone()).take(n).collect()
}

fn slim_matched(features: &[MatchedFeatureView], n: usize) -> Vec<Value> {
    features
        .iter()
        .take(n)
        .map(|f| {
            let limited = f.appearances <= 2;
            json!({
                "name": f.name,
                "family": f.family,
                "appearances": f.appearances,
                "affinity": f.recommendation_mean,
                "scoringAffinity": f.scoring_affinity,
                "cited": f.cited,
                "limitedEvidence": limited,
                "keywordStrength": if f.family == "keyword" || f.family.is_empty() {
                    Value::String(keyword_strength_label(&f.name).into())
                } else {
                    Value::Null
                },
            })
        })
        .collect()
}

fn taste_profile_json(profile: &FeatureProfile) -> Value {
    json!({
        "primaryAffinities": affinity_json(profile, true),
        "strongAversions": affinity_json(profile, false),
        "contextualSignals": contextual_json(profile),
        "polarizingFeatures": profile.polarizing.iter().take(6).map(|p| json!({
            "feature": p.feature,
            "family": p.family,
            "positiveEvidence": titles_of(&p.positive_evidence, 4),
            "negativeEvidence": titles_of(&p.negative_evidence, 4),
        })).collect::<Vec<_>>(),
        "recentChanges": profile.shifts.iter().take(6).map(|s| json!({
            "feature": s.feature,
            "delta": (s.delta as f64 * 100.0).round() / 100.0,
            "longTermEvidence": titles_of(&s.long_term_evidence, 3),
            "recentEvidence": titles_of(&s.recent_evidence, 3),
        })).collect::<Vec<_>>(),
        "tasteDimensions": profile.dimensions.iter().map(|d| json!({
            "name": d.name,
            "strength": (d.strength as f64 * 100.0).round() / 100.0,
            "evidence": titles_of(&d.evidence, 4),
        })).collect::<Vec<_>>(),
        "tasteModes": profile.modes.iter().map(|m| json!({
            "dimension": m.dimension,
            "strength": (m.strength as f64 * 100.0).round() / 100.0,
            "members": titles_of(&m.members, 4),
        })).collect::<Vec<_>>(),
        "modeShifts": profile.mode_shifts,
    })
}

fn affinity_json(profile: &FeatureProfile, positive: bool) -> Vec<Value> {
    let mut items: Vec<_> = profile
        .affinities
        .iter()
        .filter(|a| {
            let allowed_family = a.citeable();
            if !allowed_family {
                return false;
            }
            if positive {
                a.recommendation_mean > 0.08
            } else {
                a.preference_mean < -0.08 || a.negative_weight > a.positive_weight
            }
        })
        .take(12)
        .map(|a| {
            json!({
                "feature": a.key.name,
                "family": a.key.family,
                "affinity": (a.recommendation_mean as f64 * 100.0).round() / 100.0,
                "preferenceMean": (a.preference_mean as f64 * 100.0).round() / 100.0,
                "confidence": (a.confidence as f64 * 100.0).round() / 100.0,
                "portability": (a.portability as f64 * 100.0).round() / 100.0,
                "appearances": a.appearances,
                "positiveEvidence": titles_of(&a.positive_evidence, 4),
                "negativeEvidence": titles_of(&a.negative_evidence, 4),
            })
        })
        .collect();
    if !positive {
        items.truncate(8);
    }
    items
}

fn contextual_json(profile: &FeatureProfile) -> Vec<Value> {
    profile
        .affinities
        .iter()
        .filter(|a| a.key.family.is_contextual())
        .take(8)
        .map(|a| {
            json!({
                "feature": a.key.name,
                "family": a.key.family,
                "preferenceMean": (a.preference_mean as f64 * 100.0).round() / 100.0,
                "recommendationMean": (a.recommendation_mean as f64 * 100.0).round() / 100.0,
                "portability": (a.portability as f64 * 100.0).round() / 100.0,
                "confidence": (a.confidence as f64 * 100.0).round() / 100.0,
                "note": "Catalog exposure, not a recommendation target unless portability is high.",
                "positiveEvidence": titles_of(&a.positive_evidence, 4),
            })
        })
        .collect()
}

fn candidate_payload(c: &ScoredCandidate, rank: usize, critic: Option<&CriticReport>) -> Value {
    let (origin, origin_label) = crate::taste::explain::primary_retrieval(&c.candidate.sources);
    let id = c
        .candidate
        .tmdb_id
        .map(|id| format!("tmdb:{id}"))
        .unwrap_or_else(|| c.candidate.title.clone());
    let assess = critic.and_then(|cr| {
        cr.candidate_assessments.iter().find(|a| {
            a.id == id || a.id.eq_ignore_ascii_case(&c.candidate.title)
        })
    });
    let limited = c.matched_features.iter().any(|f| f.cited && f.appearances <= 2)
        || c.display_reasons.iter().any(|r| r.contains("limited evidence"));
    json!({
        "rank": rank,
        "id": id,
        "title": c.candidate.title,
        "year": c.candidate.year,
        "tmdbId": c.candidate.tmdb_id,
        "origin": origin,
        "originLabel": origin_label,
        "sources": c.candidate.sources.iter().take(6).cloned().collect::<Vec<_>>(),
        "watchlist": c.candidate.watchlist,
        "deterministicRank": rank,
        "deterministicScore": (c.score.total as f64 * 100.0).round() / 100.0,
        "overallTotal": (c.score.total as f64 * 100.0).round() / 100.0,
        "semanticFit": (c.score.semantic_fit as f64 * 100.0).round() / 100.0,
        "evidenceGrade": evidence_grade(c),
        "evidenceLimited": limited,
        "scoreBreakdown": {
            "content": (c.score.content as f64 * 100.0).round() / 100.0,
            "tmdbRelated": (c.score.tmdb_related as f64 * 100.0).round() / 100.0,
            "friend": (c.score.friend_affinity as f64 * 100.0).round() / 100.0,
            "recent": (c.score.recent_taste as f64 * 100.0).round() / 100.0,
            "watchlist": (c.score.watchlist as f64 * 100.0).round() / 100.0,
            "novelty": (c.score.novelty as f64 * 100.0).round() / 100.0,
            "negative": (c.score.negative_evidence as f64 * 100.0).round() / 100.0,
            "semanticFit": (c.score.semantic_fit as f64 * 100.0).round() / 100.0,
            "overallTotal": (c.score.total as f64 * 100.0).round() / 100.0,
        },
        "reasons": c.display_reasons.iter().take(4).cloned().collect::<Vec<_>>(),
        "scoringReasons": c.scoring_reasons.iter().take(4).cloned().collect::<Vec<_>>(),
        "displayReasons": c.display_reasons.iter().take(4).cloned().collect::<Vec<_>>(),
        "evidence": c.evidence.iter().take(6).cloned().collect::<Vec<_>>(),
        "positiveFeatures": c.positive_features.iter().take(6).cloned().collect::<Vec<_>>(),
        "negativeFeatures": c.negative_features.iter().take(4).cloned().collect::<Vec<_>>(),
        "matchedFeatures": slim_matched(&c.matched_features, 6),
        "eligibility": c.eligibility,
        "directors": c.candidate.directors.iter().take(3).cloned().collect::<Vec<_>>(),
        "genres": c.candidate.genres.iter().take(4).cloned().collect::<Vec<_>>(),
        "modes": c.candidate.modes.iter().take(4).cloned().collect::<Vec<_>>(),
        "contextualOnly": c.contextual_only,
        "call1Fit": assess.map(|a| a.fit.clone()),
        "call1Concerns": assess.map(|a| a.concerns.clone()).unwrap_or_default(),
        "call1Reason": assess.map(|a| a.reason.clone()),
    })
}

pub fn call1_payload(
    films: &[FilmRecord],
    profile: &FeatureProfile,
    shortlist: &[ScoredCandidate],
) -> Value {
    json!({
        "tasteProfile": taste_profile_json(profile),
        "representativeHistory": representative_history(films),
        "candidates": shortlist.iter().enumerate().map(|(i, c)| candidate_payload(c, i + 1, None)).collect::<Vec<_>>(),
    })
}

pub fn call2_payload(
    films: &[FilmRecord],
    profile: &FeatureProfile,
    shortlist: &[ScoredCandidate],
    critic: &CriticReport,
    discoveries: &[ScoredCandidate],
) -> Value {
    json!({
        "tasteProfile": taste_profile_json(profile),
        "representativeHistory": representative_history(films),
        "originalShortlist": shortlist.iter().enumerate().map(|(i, c)| candidate_payload(c, i + 1, Some(critic))).collect::<Vec<_>>(),
        "call1": {
            "candidateAssessments": critic.candidate_assessments.iter().map(|a| json!({
                "id": a.id,
                "fit": a.fit,
                "reason": a.reason,
                "concerns": a.concerns,
            })).collect::<Vec<_>>(),
            "tasteGaps": critic.taste_gaps.iter().map(|g| json!({
                "facet": g.facet,
                "missingFromShortlist": g.missing_from_shortlist,
            })).collect::<Vec<_>>(),
        },
        "validatedDiscoveries": discoveries.iter().enumerate().map(|(i, c)| candidate_payload(c, i + 1, Some(critic))).collect::<Vec<_>>(),
        "constraints": {
            "narrativeOnly": true,
            "doNotSelectOrRank": true,
            "discoveryOptional": true,
        }
    })
}

pub fn payload_has_breakdown(payload: &Value) -> bool {
    payload["candidates"]
        .as_array()
        .or_else(|| payload["originalShortlist"].as_array())
        .and_then(|arr| arr.first())
        .map(|c| c.get("scoreBreakdown").is_some() && c.get("evidence").is_some() && c.get("reasons").is_some())
        .unwrap_or(false)
}

pub fn empty_critic() -> CriticReport {
    CriticReport {
        candidate_assessments: Vec::new(),
        taste_gaps: Vec::new(),
        discovery_queries: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::features::{
        EvidenceFilm, FeatureAffinity, FeatureFamily, FeatureKey, FeatureProfile, PolarizingFeature,
    };
    use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
    use crate::taste::score::{CandidateScore, CandidateView};

    fn scored() -> ScoredCandidate {
        ScoredCandidate {
            candidate: CandidateView {
                tmdb_id: Some(1),
                title: "Heat".into(),
                year: Some(1995),
                poster: None,
                watchlist: false,
                sources: vec![RetrievalSource {
                    kind: RetrievalKind::Related,
                    label: "x".into(),
                    seed_tmdb_id: Some(2),
                    seed_rating: None,
                    similarity: None,
                    neighbor_rank: None,
                }],
                directors: vec!["Michael Mann".into()],
                genres: vec!["Crime".into()],
                modes: vec![],
                media_kind: crate::taste::retrieve::MediaKind::Movie,
                runtime: Some(110),
                vote_count: Some(400),
                semantic_cluster: None,
            },
            score: CandidateScore {
                content: 0.7,
                tmdb_related: 0.5,
                friend_affinity: 0.1,
                recent_taste: 0.2,
                watchlist: 0.0,
                novelty: -0.1,
                negative_evidence: -0.05,
                semantic_fit: 0.5,
                semantic_coverage: false,
                total: 0.74,
            },
            reasons: vec!["director affinity".into()],
            evidence: vec!["Heat".into(), "Collateral".into()],
            positive_features: vec!["Michael Mann".into()],
            negative_features: vec!["Blackhat".into()],
            contextual_only: false,
            person_keys: vec![],
            display_reasons: vec![],
            scoring_reasons: vec![],
            matched_features: vec![],
            hidden_features: vec![],
            eligibility: Default::default(),
            quality_prior: 0.0,
            has_quality_prior: false,
        }
    }

    #[test]
    fn call1_rejects_picks() {
        let v = json!({"picks":[{"title":"Heat"}]});
        assert!(parse_critic(&v).is_err());
    }

    #[test]
    fn payloads_include_breakdown_and_evidence() {
        let profile = FeatureProfile::default();
        let p = call1_payload(&[], &profile, &[scored()]);
        assert!(payload_has_breakdown(&p));
        assert!(p["candidates"][0]["scoreBreakdown"]["content"].is_number());
        assert!(p["candidates"][0]["semanticFit"].is_number());
        assert!(p["candidates"][0]["overallTotal"].is_number());
        assert!(p["candidates"][0]["evidence"].as_array().unwrap().len() >= 1);
        assert!(p["candidates"][0]["negativeFeatures"].as_array().unwrap().len() >= 1);
        assert!(p["candidates"][0]["displayReasons"].is_array());
        assert!(p["candidates"][0]["eligibility"].is_object());
        assert!(p["tasteProfile"]["contextualSignals"].is_array());
        assert!(p["tasteProfile"]["tasteModes"].is_array());
        assert!(p["tasteProfile"]["primaryAffinities"].is_array());
        let critic = empty_critic();
        let p2 = call2_payload(&[], &profile, &[scored()], &critic, &[]);
        assert!(payload_has_breakdown(&p2));
        assert_eq!(p2["constraints"]["narrativeOnly"], true);
        assert_eq!(p2["constraints"]["doNotSelectOrRank"], true);
        assert!(p2["originalShortlist"][0]["evidenceGrade"].is_number());
        assert!(p2["originalShortlist"][0]["deterministicRank"].is_number());
        assert!(CALL2_SYSTEM.contains("do NOT select, rank"));
    }

    #[test]
    fn critic_rerank_demotes_superficial_below_strong_at_same_total() {
        let mut superficial = scored();
        superficial.candidate.tmdb_id = Some(10);
        superficial.candidate.title = "Surface Match".into();
        superficial.score.total = 0.5;
        let mut strong = scored();
        strong.candidate.tmdb_id = Some(20);
        strong.candidate.title = "Deep Match".into();
        strong.score.total = 0.5;
        let critic = CriticReport {
            candidate_assessments: vec![
                CandidateAssessment {
                    id: "tmdb:10".into(),
                    fit: "superficial".into(),
                    reason: String::new(),
                    concerns: vec![],
                },
                CandidateAssessment {
                    id: "deep match".into(),
                    fit: "strong".into(),
                    reason: String::new(),
                    concerns: vec![],
                },
            ],
            taste_gaps: vec![],
            discovery_queries: vec![],
        };
        let mut ranked = vec![superficial, strong];

        apply_critic_rerank(&mut ranked, &critic);

        assert_eq!(ranked[0].candidate.tmdb_id, Some(20));
        assert!((ranked[0].score.total - 0.56).abs() < f32::EPSILON);
        assert!((ranked[1].score.total - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn llm_payloads_send_titles_not_credit_lists() {
        let mut profile = FeatureProfile::default();
        profile.polarizing.push(PolarizingFeature {
            feature: "Michael Mann".into(),
            family: FeatureFamily::Director,
            id: Some(1),
            confidence: 0.8,
            affinity: 0.4,
            variance: 0.2,
            positive_evidence: vec![EvidenceFilm {
                title: "Heat".into(),
                rating: 5.0,
                tmdb_id: Some(949),
                people: (0..80).map(|i| format!("Cast{i}")).collect(),
                keywords: vec!["crime".into(); 40],
                genres: vec!["Crime".into()],
                year: Some(1995),
                runtime: Some(170),
            }],
            negative_evidence: vec![],
        });
        let raw = call2_payload(&[], &profile, &[scored()], &empty_critic(), &[]).to_string();
        assert!(raw.contains("Heat"));
        assert!(!raw.contains("Cast79"), "{raw}");
        assert!(!raw.contains("hiddenFeatures"));
    }

    #[test]
    fn nonportable_decade_stays_contextual() {
        use crate::taste::features::{build_profile, observations_from_film};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = Vec::new();
        for i in 0..40 {
            let s = interaction_signal(4.5, &p, Some(8.0), 6, false);
            obs.extend(observations_from_film(
                &format!("kid{i}"),
                4.5,
                Some(i),
                &s,
                Some(8.0),
                &["Comedy".into()],
                &[],
                &[],
                Some(2005),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let payload = call1_payload(&[], &profile, &[scored()]);
        let primary = payload["tasteProfile"]["primaryAffinities"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(primary.iter().all(|v| v["family"] != "decade"));
        let ctx = payload["tasteProfile"]["contextualSignals"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(ctx.iter().any(|v| v["feature"] == "2000s"));
    }

    #[test]
    fn singleton_person_is_not_a_primary_affinity() {
        use crate::taste::features::{build_profile, observations_from_film, Credit};
        use crate::taste::preference::{interaction_signal, rating_profile};
        let p = rating_profile(&[4.0; 8]).unwrap();
        let s = interaction_signal(5.0, &p, Some(0.1), 1, false);
        let obs = observations_from_film(
            "The SpongeBob SquarePants Movie",
            5.0,
            Some(0),
            &s,
            Some(0.1),
            &["Animation".into()],
            &[Credit {
                id: Some(1),
                name: "Stephen Hillenburg".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2004),
            None,
        );
        let profile = build_profile(&obs);
        let payload = call1_payload(&[], &profile, &[scored()]);
        let names: Vec<String> = payload["tasteProfile"]["primaryAffinities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["feature"].as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            !names.iter().any(|n| n.contains("Hillenburg")),
            "n=1 0.91 people must not enter LLM hunt context: {names:?}"
        );
    }

    fn visual_story_profile() -> FeatureProfile {
        let mut profile = FeatureProfile::default();
        profile.modes = vec![
            crate::taste::dimensions::TasteMode {
                dimension: "visual".into(),
                strength: 0.8,
                members: vec![],
                recent_share: 0.4,
                long_term_share: 0.4,
            },
            crate::taste::dimensions::TasteMode {
                dimension: "story".into(),
                strength: 0.7,
                members: vec![],
                recent_share: 0.3,
                long_term_share: 0.4,
            },
            crate::taste::dimensions::TasteMode {
                dimension: "comedy".into(),
                strength: 0.4,
                members: vec![],
                recent_share: 0.2,
                long_term_share: 0.2,
            },
        ];
        profile
    }

    fn ungated_report() -> ReasonerReport {
        ReasonerReport {
            title: "Dark Fantasy Adventure".into(),
            summary: "They may not enjoy films with too much horror or conspiracy elements, as seen in their dislike of Batman & Robin and Assassin's Creed. Visual craft still matters.".into(),
            affinities: vec![],
            aversions: vec![crate::taste::TasteAffinity {
                label: "horror".into(),
                evidence: "Batman & Robin".into(),
            }],
            dimensions: vec![],
            picks: vec![],
        }
    }

    #[test]
    fn summary_does_not_infer_dislike_without_negative_evidence() {
        let profile = visual_story_profile();
        let grounded = ground_reasoner(ungated_report(), &profile);
        assert!(
            grounded.aversions.is_empty(),
            "horror aversion needs negative horror evidence, got {:?}",
            grounded.aversions
        );
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("not enjoy") && !lower.contains("horror"),
            "summary must not invent a dislike, got {}",
            grounded.summary
        );
    }

    #[test]
    fn summary_cannot_infer_horror_aversion_from_batman_and_robin() {
        let profile = visual_story_profile();
        let grounded = ground_reasoner(ungated_report(), &profile);
        let blob = format!("{} {}", grounded.summary, grounded.aversions.iter().map(|a| a.evidence.clone()).collect::<Vec<_>>().join(" "));
        assert!(
            !blob.to_lowercase().contains("batman"),
            "Batman & Robin must not ground horror/conspiracy aversion, got {blob}"
        );
    }

    #[test]
    fn profile_title_must_be_supported_by_multiple_deterministic_signals() {
        let profile = visual_story_profile();
        let grounded = ground_reasoner(ungated_report(), &profile);
        let lower = grounded.title.to_lowercase();
        assert!(
            !lower.contains("fantasy") && !lower.contains("adventure"),
            "ungrounded genre identity must not survive, got {}",
            grounded.title
        );
        assert!(
            lower.contains("visual") && lower.contains("story"),
            "title should come from top modes, got {}",
            grounded.title
        );
        let ok = ReasonerReport {
            title: "Visual Story".into(),
            summary: "Cinematography and plot construction show up across the log.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let kept = ground_reasoner(ok, &profile);
        assert_eq!(kept.title, "Visual Story");
    }

    fn genre_aff(name: &str, rec: f32, n: u32, pos: f32, neg: f32) -> FeatureAffinity {
        FeatureAffinity {
            key: FeatureKey::new(FeatureFamily::Genre, None, name),
            appearances: n,
            weighted_mean: rec,
            preference_mean: rec,
            recommendation_mean: rec,
            weighted_variance: 0.0,
            positive_weight: pos,
            negative_weight: neg,
            recent_weight: 0.0,
            long_term_weight: 0.0,
            confidence: 0.9,
            feature_strength: rec.abs(),
            portability: 1.0,
            feedback_adjustment: 0.0,
            positive_evidence: vec![],
            negative_evidence: vec![],
            evidence_cluster: Default::default(),
        }
    }

    #[test]
    fn mixed_positive_genre_does_not_support_category_aversion() {
        let mut profile = visual_story_profile();
        profile.affinities.push(genre_aff("Horror", 0.07, 29, 10.0, 12.0));
        profile.affinities.push({
            let mut a = genre_aff("conspiracy", 0.20, 8, 4.0, 9.0);
            a.key.family = FeatureFamily::Keyword;
            a.key.name = "conspiracy".into();
            a
        });
        let mut report = ungated_report();
        report.title = "Visual Story".into();
        report.summary = "The user enjoys films with strong visual elements. However, they tend to dislike films with excessive action, horror, or conspiracy themes.".into();
        report.aversions = vec![
            crate::taste::TasteAffinity {
                label: "Horror".into(),
                evidence: "The Holy Mountain, Batman & Robin, Assassin's Creed".into(),
            },
            crate::taste::TasteAffinity {
                label: "Conspiracy".into(),
                evidence: "Batman v Superman: Dawn of Justice".into(),
            },
        ];
        let grounded = ground_reasoner(report, &profile);
        assert!(
            grounded.aversions.is_empty(),
            "liked-or-mixed categories must not become You bounce off, got {:?}",
            grounded.aversions
        );
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("horror") && !lower.contains("conspiracy") && !lower.contains("dislike"),
            "summary must not invent category aversions from mixed genres, got {}",
            grounded.summary
        );
    }

    fn craft_person(family: FeatureFamily, name: &str, rec: f32, n: u32, film: &str) -> FeatureAffinity {
        FeatureAffinity {
            key: FeatureKey::new(family, Some(1), name),
            appearances: n,
            weighted_mean: rec,
            preference_mean: rec,
            recommendation_mean: rec,
            weighted_variance: 0.0,
            positive_weight: 8.0,
            negative_weight: 0.0,
            recent_weight: 0.0,
            long_term_weight: 0.0,
            confidence: 0.9,
            feature_strength: rec.abs(),
            portability: 1.0,
            feedback_adjustment: 0.0,
            positive_evidence: vec![EvidenceFilm {
                title: film.into(),
                rating: 5.0,
                tmdb_id: Some(10),
                people: vec![name.into()],
                keywords: vec![],
                genres: vec!["Drama".into()],
                year: Some(2022),
                runtime: Some(120),
            }],
            negative_evidence: vec![],
            evidence_cluster: Default::default(),
        }
    }

    #[test]
    fn generic_action_adventure_humor_summary_is_replaced() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Mauro Fiore",
            0.41,
            5,
            "Avatar",
        ));
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Wally Pfister",
            0.38,
            8,
            "Inception",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "You enjoy a mix of action, adventure, and humor across a wide range of films.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let grounded = ground_reasoner(report, &profile);
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("action, adventure, and humor"),
            "generic blurb must not survive, got {}",
            grounded.summary
        );
        assert!(
            lower.contains("fraser") || lower.contains("fiore") || lower.contains("pfister"),
            "summary must cite profile craft, got {}",
            grounded.summary
        );
    }

    #[test]
    fn summary_that_cites_craft_is_kept() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "Fraser's photography on The Batman is the through-line.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let grounded = ground_reasoner(report, &profile);
        assert!(grounded.summary.contains("Fraser"));
        assert!(!grounded.summary.contains("Recurring craft"));
    }

    #[test]
    fn generic_opener_with_craft_name_is_replaced() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "You enjoy a mix of action, adventure, and humor. Fraser's photography on The Batman is the through-line.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let grounded = ground_reasoner(report, &profile);
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("you enjoy a mix"),
            "generic opener must not survive, got {}",
            grounded.summary
        );
        assert!(
            grounded.summary.contains("Fraser"),
            "grounded craft sentence should remain, got {}",
            grounded.summary
        );
    }

    #[test]
    fn unsupported_theme_claim_is_replaced() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "Fraser often returns to tree or game themes across the log.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let grounded = ground_reasoner(report, &profile);
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("tree or game") && !lower.contains("game themes"),
            "unsupported theme must not survive, got {}",
            grounded.summary
        );
        assert!(
            lower.contains("fraser") || lower.contains("recurring craft"),
            "fallback should cite craft, got {}",
            grounded.summary
        );
    }

    #[test]
    fn may_not_always_enjoy_horror_is_replaced_with_craft_summary() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "Your taste in films is influenced by directors like Christopher Nolan. However, you may not always enjoy films with intense action or horror elements, as seen in your mixed reaction to films like Aliens and The Conjuring: Last Rites.".into(),
            affinities: vec![],
            aversions: vec![crate::taste::TasteAffinity {
                label: "horror".into(),
                evidence: "Aliens, The Conjuring: Last Rites".into(),
            }],
            dimensions: vec![crate::taste::TasteDimension {
                name: "intensity".into(),
                take: "mixed reaction to intense action and horror elements".into(),
            }],
            picks: vec![],
        };
        let grounded = ground_reasoner_with(report, &profile, &["Aliens".into()]);
        assert!(
            grounded.aversions.is_empty(),
            "watchlist Aliens must not ground a horror aversion, got {:?}",
            grounded.aversions
        );
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("horror")
                && !lower.contains("aliens")
                && !lower.contains("may not always enjoy"),
            "ungrounded aversion must yield the craft summary, got {}",
            grounded.summary
        );
        assert!(
            lower.contains("fraser"),
            "fallback should cite craft, got {}",
            grounded.summary
        );
        let intensity = grounded
            .dimensions
            .iter()
            .find(|d| d.name == "intensity")
            .map(|d| d.take.to_lowercase())
            .unwrap_or_default();
        assert!(
            !intensity.contains("horror") && !intensity.contains("mixed reaction"),
            "dimension take must not repeat the invented aversion, got {intensity}"
        );
    }

    #[test]
    fn third_person_visual_storytelling_blurb_is_replaced() {
        let mut profile = visual_story_profile();
        profile.affinities.push(craft_person(
            FeatureFamily::Cinematographer,
            "Greig Fraser",
            0.47,
            6,
            "The Batman",
        ));
        let report = ReasonerReport {
            title: "Visual Story".into(),
            summary: "This user enjoys films with strong visual elements, such as cinematography and spectacle, as well as engaging stories and characters. Directors like Christopher Nolan and Edgar Wright are favorites, and they also enjoy the work of cinematographers like Greig Fraser and Mauro Fiore.".into(),
            affinities: vec![],
            aversions: vec![],
            dimensions: vec![],
            picks: vec![],
        };
        let grounded = ground_reasoner(report, &profile);
        let lower = grounded.summary.to_lowercase();
        assert!(
            !lower.contains("this user") && !lower.contains("strong visual elements"),
            "third-person boilerplate must not survive, got {}",
            grounded.summary
        );
        assert!(
            lower.contains("fraser") || lower.contains("recurring craft"),
            "fallback should cite craft, got {}",
            grounded.summary
        );
    }
}
