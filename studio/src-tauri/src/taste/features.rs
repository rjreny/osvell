use crate::taste::preference::{
    weighted_mean, weighted_variance, InteractionSignal,
};
use serde::{Deserialize, Serialize};

pub const DIRECTOR_W: f32 = 1.00;
pub const WRITER_W: f32 = 0.85;
pub const CINEMATOGRAPHER_W: f32 = 0.80;
pub const COMPOSER_W: f32 = 0.65;
pub const ACTOR_W: f32 = 0.55;
pub const KEYWORD_W: f32 = 0.45;
pub const GENRE_W: f32 = 0.35;
pub const DECADE_W: f32 = 0.20;
pub const RUNTIME_W: f32 = 0.15;

const PEOPLE_K: f32 = 4.0;
const BROAD_K: f32 = 6.0;
const CINEMATOGRAPHER_K: f32 = 2.5;
pub const RECENT_YEARS: f32 = 1.5;
const SHIFT_RECENT_N: usize = 8;
const SHIFT_LONG_N: usize = 20;
const SHIFT_CONF: f32 = 0.4;
const SHIFT_DELTA: f32 = 0.35;
const POLARIZING_VAR: f32 = 0.12;
pub const PORTABILITY_SKIP: f32 = 0.15;
pub const PORTABLE_CONTEXTUAL: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureFamily {
    Director,
    Writer,
    Cinematographer,
    Composer,
    Actor,
    Genre,
    Keyword,
    Decade,
    Runtime,
}

impl FeatureFamily {
    pub fn weight(self) -> f32 {
        match self {
            Self::Director => DIRECTOR_W,
            Self::Writer => WRITER_W,
            Self::Cinematographer => CINEMATOGRAPHER_W,
            Self::Composer => COMPOSER_W,
            Self::Actor => ACTOR_W,
            Self::Keyword => KEYWORD_W,
            Self::Genre => GENRE_W,
            Self::Decade => DECADE_W,
            Self::Runtime => RUNTIME_W,
        }
    }

    pub fn is_contextual(self) -> bool {
        matches!(self, Self::Decade | Self::Runtime)
    }

    pub fn is_primary(self) -> bool {
        !self.is_contextual()
    }

    fn k(self) -> f32 {
        match self {
            Self::Cinematographer => CINEMATOGRAPHER_K,
            Self::Genre | Self::Decade | Self::Runtime => BROAD_K,
            _ => PEOPLE_K,
        }
    }

    pub fn top_k(self) -> usize {
        match self {
            Self::Director | Self::Writer | Self::Cinematographer | Self::Composer => 2,
            Self::Actor => 3,
            Self::Keyword => 5,
            _ => 4,
        }
    }

    pub fn sort_key(self) -> u8 {
        match self {
            Self::Director => 0,
            Self::Writer => 1,
            Self::Cinematographer => 2,
            Self::Composer => 3,
            Self::Actor => 4,
            Self::Keyword => 5,
            Self::Genre => 6,
            Self::Decade => 7,
            Self::Runtime => 8,
        }
    }
}

pub fn family_for_job(job: &str) -> Option<FeatureFamily> {
    match job {
        "Director" => Some(FeatureFamily::Director),
        "Writer" | "Screenplay" | "Original Screenplay" | "Story" => Some(FeatureFamily::Writer),
        "Director of Photography" | "Cinematography" | "Cinematographer" => {
            Some(FeatureFamily::Cinematographer)
        }
        "Original Music Composer" | "Music" => Some(FeatureFamily::Composer),
        "Actor" => Some(FeatureFamily::Actor),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureKey {
    pub family: FeatureFamily,
    pub id: Option<i64>,
    pub name: String,
}

impl FeatureKey {
    pub fn new(family: FeatureFamily, id: Option<i64>, name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            family,
            id,
            name: name.trim().to_string(),
        }
    }

    pub fn storage_key(&self) -> String {
        let name = self.name.to_lowercase();
        match self.id {
            Some(id) => format!("{:?}:{id}:{name}", self.family),
            None => format!("{:?}:{name}", self.family),
        }
    }

    pub fn is_person_or_keyword(&self) -> bool {
        matches!(
            self.family,
            FeatureFamily::Director
                | FeatureFamily::Writer
                | FeatureFamily::Cinematographer
                | FeatureFamily::Composer
                | FeatureFamily::Actor
                | FeatureFamily::Keyword
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceFilm {
    pub title: String,
    pub rating: f32,
    pub tmdb_id: Option<i64>,
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub runtime: Option<i32>,
}

pub(crate) fn evidence_film_id(e: &EvidenceFilm) -> String {
    match e.tmdb_id {
        Some(id) => format!("tmdb:{id}"),
        None => e.title.trim().to_lowercase(),
    }
}

/// How a TMDB keyword may be used. Signal keywords are specific enough that
/// matching them should retrieve another film. Contextual keywords are kept
/// on the profile (location, cartoon, format) but are not hunt targets.
/// Ignore keywords are catalog junk and are not observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordRole {
    Signal,
    Contextual,
    Ignore,
}

/// Strength class for keywords that already made it past ignore/contextual.
/// Frozen preference math is unchanged; this only gates retrieval carry
/// and final selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordStrength {
    Strong,
    Thematic,
    Broad,
    Contextual,
    Ignore,
}

pub fn keyword_role(name: &str) -> KeywordRole {
    match keyword_strength(name) {
        KeywordStrength::Ignore => KeywordRole::Ignore,
        KeywordStrength::Contextual => KeywordRole::Contextual,
        KeywordStrength::Strong | KeywordStrength::Thematic | KeywordStrength::Broad => {
            KeywordRole::Signal
        }
    }
}

pub fn keyword_can_carry_alone(name: &str) -> bool {
    keyword_strength(name) == KeywordStrength::Strong
}

pub fn keyword_strength_label(name: &str) -> &'static str {
    match keyword_strength(name) {
        KeywordStrength::Strong => "strong",
        KeywordStrength::Thematic => "thematic",
        KeywordStrength::Broad => "broad",
        KeywordStrength::Contextual => "contextual",
        KeywordStrength::Ignore => "ignore",
    }
}

/// Headline reasons on a pick card. Adjectival metadata may still score;
/// it should not be presented as a core taste signal.
pub fn keyword_is_display_reason(name: &str) -> bool {
    if is_adjectival_keyword(name) {
        return false;
    }
    matches!(
        keyword_strength(name),
        KeywordStrength::Strong | KeywordStrength::Thematic
    )
}

fn is_adjectival_keyword(name: &str) -> bool {
    let compact: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    is_reaction_keyword(&compact)
        || matches!(
            compact.as_str(),
            "bold"
                | "enthusiastic"
                | "amused"
                | "adoring"
                | "joyful"
                | "nostalgic"
                | "dramatic"
                | "excited"
                | "whimsical"
                | "romantic"
                | "anxious"
        )
}

pub fn keyword_strength(name: &str) -> KeywordStrength {
    let lower = name.trim().to_ascii_lowercase();
    let compact: String = lower.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if compact.contains("stinger")
        || compact.contains("duringcredits")
        || compact.contains("aftercredits")
        || compact.starts_with("basedon")
    {
        return KeywordStrength::Ignore;
    }
    if is_geographic_keyword(&lower)
        || is_generic_keyword(&compact)
        || is_reaction_keyword(&compact)
    {
        return KeywordStrength::Contextual;
    }
    if is_strong_keyword(&lower, &compact) {
        return KeywordStrength::Strong;
    }
    if is_broad_keyword(&lower, &compact) {
        return KeywordStrength::Broad;
    }
    if is_thematic_keyword(&compact) {
        return KeywordStrength::Thematic;
    }
    // TMDB emits many descriptive/marketing attributes that look like taste
    // signals but are not stable enough to drive retrieval or membership.
    KeywordStrength::Contextual
}

fn is_thematic_keyword(compact: &str) -> bool {
    matches!(
        compact,
        "comingofage"
            | "dysfunctionalfamily"
            | "fathersonrelationship"
            | "fatherdaughterrelationship"
            | "motherdaughterrelationship"
            | "parentchildrelationship"
            | "siblingrelationship"
            | "lossoflovedone"
            | "foundfamily"
            | "heist"
            | "creature"
            | "supernaturalhorror"
    )
}

fn is_strong_keyword(lower: &str, compact: &str) -> bool {
    compact.contains("neonoir")
        || compact.contains("filmnoir")
        || compact.contains("nonlinear")
        || compact.contains("longtake")
        || compact == "oner"
        || compact.contains("oneshot")
        || compact.contains("timeloop")
        || lower.contains("neo-noir")
        || lower.contains("film noir")
        || lower.contains("long take")
        || lower.contains("one-shot")
        || lower.contains("non-linear")
}

fn is_broad_keyword(lower: &str, compact: &str) -> bool {
    matches!(
        compact,
        "drugs"
            | "drug"
            | "murder"
            | "battle"
            | "friends"
            | "friendship"
            | "bestfriend"
            | "war"
            | "violence"
            | "fight"
            | "dramatic"
            | "spacecraft"
            | "spinoff"
            | "prequel"
            | "sequel"
            | "phonecall"
            | "workaholic"
            | "teenmovie"
            | "highschool"
            | "sports"
            | "revenge"
            | "crime"
    ) || lower == "best friend"
        || lower == "phone call"
        || lower == "spin off"
        || lower == "teen movie"
        || lower == "high school"
}

fn is_geographic_keyword(lower: &str) -> bool {
    if lower.contains(',') {
        return true;
    }
    lower.ends_with(" city") || lower.ends_with(" town") || lower.ends_with(" county")
}

fn is_generic_keyword(compact: &str) -> bool {
    matches!(
        compact,
        "cartoon"
            | "antihero"
            | "liveaction"
            | "liveactionandanimation"
            | "cgi"
            | "3d"
            | "imax"
            | "stopmotion"
    )
}

/// Mood/reaction adjectives are too broad to retrieve with. They stay on the
/// profile as context, like location tags, but they do not hunt or rank.
fn is_reaction_keyword(compact: &str) -> bool {
    matches!(
        compact,
        "hilarious"
            | "admiring"
            | "funny"
            | "amusing"
            | "entertaining"
            | "cute"
            | "cool"
            | "awesome"
            | "weird"
            | "quirky"
            | "sad"
            | "scary"
            | "exciting"
            | "emotional"
            | "touching"
            | "heartwarming"
            | "feelgood"
            | "feelgoodmovie"
    )
}

/// Observed keywords. Contextual locations/generics are still observed.
pub fn keyword_is_taste_signal(name: &str) -> bool {
    keyword_role(name) != KeywordRole::Ignore
}

/// Explicit execution-language signals from personal reviews. A phrase must
/// occur in at least two separate reviews before it becomes profile evidence;
/// one-off jokes, sarcasm, and isolated reactions stay out of retrieval.
pub fn repeated_execution_signals(reviews: &[&str]) -> Vec<String> {
    EXECUTION_LEXICON
        .iter()
        .filter(|(_, phrases)| {
            reviews
                .iter()
                .filter(|review| substantive_review(review))
                .filter(|review| review_mentions_any(review, phrases))
                .count()
                >= 2
        })
        .map(|(label, _)| (*label).to_string())
        .collect()
}

/// Turn only already-repeated review signals into candidate/profile keywords.
/// The returned names are deliberately human-readable because they can appear
/// in explanations and profile exports.
pub fn execution_keywords_for_review(review: &str, accepted: &[String]) -> Vec<Keyword> {
    if !substantive_review(review) {
        return Vec::new();
    }
    EXECUTION_LEXICON
        .iter()
        .filter(|(label, phrases)| {
            accepted.iter().any(|signal| signal == label)
                && review_mentions_any(review, phrases)
        })
        .map(|(label, _)| Keyword {
            id: None,
            name: (*label).to_string(),
        })
        .collect()
}

/// Personal reviews are optional supplementary evidence. A one-liner or joke
/// cannot infer an aspect preference even if it happens to contain a keyword.
pub fn substantive_review(review: &str) -> bool {
    review.split_whitespace().count() >= 25
        && EXECUTION_LEXICON
            .iter()
            .any(|(_, phrases)| review_mentions_any(review, phrases))
}

/// Polarity for the small, repeated review lexicon. These labels are only
/// admitted after `repeated_execution_signals`, so a single joke or aside
/// cannot become a durable preference.
pub fn execution_signal_polarity(name: &str) -> Option<f32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "strong performances"
        | "beautiful cinematography"
        | "polished visual effects"
        | "tight pacing"
        | "strong dialogue" => Some(1.0),
        "weak performances"
        | "poor visual effects"
        | "slow pacing"
        | "overlong"
        | "weak dialogue" => {
            Some(-1.0)
        }
        _ => None,
    }
}

const EXECUTION_LEXICON: &[(&str, &[&str])] = &[
    (
        "strong performances",
        &[
            "great acting",
            "excellent acting",
            "strong acting",
            "acting was great",
            "acting was excellent",
            "great performances",
            "excellent performances",
            "strong performances",
            "well acted",
            "well-acted",
            "phenomenal acting",
            "brilliant acting",
        ],
    ),
    (
        "beautiful cinematography",
        &[
            "beautiful cinematography",
            "great cinematography",
            "stunning cinematography",
            "beautifully shot",
            "stunning visuals",
        ],
    ),
    (
        "polished visual effects",
        &[
            "great visual effects",
            "excellent visual effects",
            "impressive visual effects",
            "great effects",
            "excellent effects",
        ],
    ),
    (
        "tight pacing",
        &[
            "tight pacing",
            "pacing was tight",
            "well paced",
            "well-paced",
            "good pacing",
            "pacing was good",
        ],
    ),
    (
        "strong dialogue",
        &[
            "great dialogue",
            "excellent dialogue",
            "strong dialogue",
            "sharp dialogue",
            "witty dialogue",
        ],
    ),
    (
        "weak performances",
        &[
            "bad acting",
            "poor acting",
            "weak acting",
            "bad performances",
            "poor performances",
            "weak performances",
            "badly acted",
        ],
    ),
    (
        "poor visual effects",
        &[
            "bad visual effects",
            "poor visual effects",
            "cheap visual effects",
            "bad cgi",
            "poor cgi",
            "cheap cgi",
            "terrible cgi",
        ],
    ),
    (
        "slow pacing",
        &["slow pacing", "poor pacing", "dragged", "dragging"],
    ),
    (
        "overlong",
        &["too long", "overlong", "way too long", "felt too long"],
    ),
    (
        "weak dialogue",
        &[
            "bad dialogue",
            "poor dialogue",
            "weak dialogue",
            "clunky dialogue",
            "wooden dialogue",
            "terrible dialogue",
        ],
    ),
];

fn review_mentions_any(review: &str, phrases: &[&str]) -> bool {
    let normalized = review
        .to_ascii_lowercase()
        .replace(['-', '–', '—'], " ");
    phrases.iter().any(|phrase| {
        normalized.contains(&phrase.to_ascii_lowercase().replace(['-', '–', '—'], " "))
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCluster {
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

impl EvidenceCluster {
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty() && self.modes.is_empty() && self.keywords.is_empty()
    }

    pub fn overlaps(
        &self,
        genres: &[String],
        keywords: &[Keyword],
        modes: &[String],
    ) -> bool {
        if self.is_empty() {
            return true;
        }
        let genre_hit = self.genres.iter().any(|g| {
            genres.iter().any(|c| c.eq_ignore_ascii_case(g))
        });
        let mode_hit = self.modes.iter().any(|m| {
            modes.iter().any(|c| c.eq_ignore_ascii_case(m))
        });
        let keyword_hit = self.keywords.iter().any(|k| {
            keywords.iter().any(|c| c.name.eq_ignore_ascii_case(k))
        });
        genre_hit || mode_hit || keyword_hit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureAffinity {
    pub key: FeatureKey,
    pub appearances: u32,
    pub weighted_mean: f32,
    pub preference_mean: f32,
    pub recommendation_mean: f32,
    pub weighted_variance: f32,
    pub positive_weight: f32,
    pub negative_weight: f32,
    pub recent_weight: f32,
    pub long_term_weight: f32,
    pub confidence: f32,
    pub feature_strength: f32,
    pub portability: f32,
    /// Bounded, event-sourced feedback applied only to the content-score term.
    /// It never changes citeability, evidence grades, retrieval, or embeddings.
    #[serde(default)]
    pub feedback_adjustment: f32,
    pub positive_evidence: Vec<EvidenceFilm>,
    pub negative_evidence: Vec<EvidenceFilm>,
    #[serde(default)]
    pub evidence_cluster: EvidenceCluster,
}

impl FeatureAffinity {
    pub fn scoring_affinity(&self) -> f32 {
        (self.recommendation_mean + self.feedback_adjustment)
            * self.confidence
            * self.key.family.weight()
            * self.portability
    }

    pub fn polarizing(&self) -> bool {
        self.weighted_variance >= POLARIZING_VAR
            && !self.positive_evidence.is_empty()
            && !self.negative_evidence.is_empty()
    }

    /// Singleton high ratings must not become hunt targets (the 0.91 Hillenburg case).
    pub fn citeable(&self) -> bool {
        if self.key.family.is_contextual() {
            return false;
        }
        if self.key.family == FeatureFamily::Keyword {
            match keyword_role(&self.key.name) {
                KeywordRole::Ignore | KeywordRole::Contextual => return false,
                KeywordRole::Signal => {}
            }
        }
        if self.key.is_person_or_keyword() {
            return self.appearances >= 2;
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolarizingFeature {
    pub feature: String,
    pub family: FeatureFamily,
    pub id: Option<i64>,
    pub confidence: f32,
    pub affinity: f32,
    pub variance: f32,
    pub positive_evidence: Vec<EvidenceFilm>,
    pub negative_evidence: Vec<EvidenceFilm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentShift {
    pub feature: String,
    pub family: FeatureFamily,
    pub long_term: f32,
    pub recent: f32,
    pub delta: f32,
    pub long_term_evidence: Vec<EvidenceFilm>,
    pub recent_evidence: Vec<EvidenceFilm>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FeatureProfile {
    pub affinities: Vec<FeatureAffinity>,
    pub polarizing: Vec<PolarizingFeature>,
    pub shifts: Vec<RecentShift>,
    #[serde(default)]
    pub dimensions: Vec<crate::taste::dimensions::TasteDimensionView>,
    #[serde(default)]
    pub modes: Vec<crate::taste::dimensions::TasteMode>,
    #[serde(default)]
    pub mode_shifts: Vec<crate::taste::dimensions::ModeShift>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credit {
    pub id: Option<i64>,
    pub name: String,
    pub job: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub id: Option<i64>,
    pub name: String,
}

pub fn runtime_bucket(rt: i32) -> &'static str {
    if rt < 90 {
        "under 90"
    } else if rt < 120 {
        "90-119"
    } else if rt < 150 {
        "120-149"
    } else {
        "150+"
    }
}

pub fn decade_label(year: i32) -> String {
    format!("{}s", (year / 10) * 10)
}

pub struct FeatureObservation {
    pub key: FeatureKey,
    pub affinity_preference: f32,
    pub preference_weight: f32,
    pub recommendation_weight: f32,
    pub discovery_strength: f32,
    pub age_years: Option<f32>,
    pub evidence: EvidenceFilm,
    pub positive: f32,
    pub negative: f32,
    pub genres: Vec<String>,
}

pub fn build_profile(obs: &[FeatureObservation]) -> FeatureProfile {
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<&FeatureObservation>> = HashMap::new();
    for o in obs {
        groups.entry(o.key.storage_key()).or_default().push(o);
    }
    let mut affinities = Vec::new();
    for group in groups.values() {
        let key = group[0].key.clone();
        let pairs_pref: Vec<(f32, f32)> = group
            .iter()
            .map(|o| (o.affinity_preference, o.preference_weight))
            .collect();
        let pairs_rec: Vec<(f32, f32)> = group
            .iter()
            .map(|o| (o.affinity_preference, o.recommendation_weight))
            .collect();
        let Some(preference_mean) = weighted_mean(&pairs_pref) else {
            continue;
        };
        let recommendation_mean = weighted_mean(&pairs_rec).unwrap_or(preference_mean);
        let var = weighted_variance(&pairs_pref, preference_mean);
        let appearances = distinct_film_count(group);
        let confidence = 1.0 - (-(appearances as f32) / key.family.k()).exp();
        let portability = feature_portability(&key, group);
        let mut positive_weight = 0.0;
        let mut negative_weight = 0.0;
        let mut recent_pairs = Vec::new();
        let mut long_pairs = Vec::new();
        let mut positive_evidence = Vec::new();
        let mut negative_evidence = Vec::new();
        let mut seen_pos = std::collections::HashSet::new();
        let mut seen_neg = std::collections::HashSet::new();
        for o in group {
            positive_weight += o.positive * o.preference_weight;
            negative_weight += o.negative * o.preference_weight;
            let recent = o.age_years.map(|y| y <= RECENT_YEARS).unwrap_or(false);
            if recent {
                recent_pairs.push((o.affinity_preference, o.recommendation_weight));
            } else {
                long_pairs.push((o.affinity_preference, o.recommendation_weight));
            }
            let id = evidence_film_id(&o.evidence);
            if o.positive > 0.0 && seen_pos.insert(id.clone()) && positive_evidence.len() < 6 {
                positive_evidence.push(o.evidence.clone());
            }
            if o.negative > 0.0 && seen_neg.insert(id) && negative_evidence.len() < 6 {
                negative_evidence.push(o.evidence.clone());
            }
        }
        let evidence_cluster = cluster_from_group(&key, group);
        affinities.push(FeatureAffinity {
            key,
            appearances,
            weighted_mean: preference_mean,
            preference_mean,
            recommendation_mean,
            weighted_variance: var,
            positive_weight,
            negative_weight,
            recent_weight: weighted_mean(&recent_pairs).unwrap_or(0.0),
            long_term_weight: weighted_mean(&long_pairs).unwrap_or(preference_mean),
            confidence,
            feature_strength: recommendation_mean.abs(),
            portability,
            feedback_adjustment: 0.0,
            positive_evidence,
            negative_evidence,
            evidence_cluster,
        });
    }
    affinities.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.scoring_affinity(), b.scoring_affinity())
            .then_with(|| a.key.family.sort_key().cmp(&b.key.family.sort_key()))
            .then_with(|| a.key.name.cmp(&b.key.name))
            .then_with(|| a.key.id.cmp(&b.key.id))
    });

    let polarizing: Vec<PolarizingFeature> = affinities
        .iter()
        .filter(|a| a.polarizing())
        .take(12)
        .map(|a| PolarizingFeature {
            feature: a.key.name.clone(),
            family: a.key.family,
            id: a.key.id,
            confidence: a.confidence,
            affinity: a.weighted_mean,
            variance: a.weighted_variance,
            positive_evidence: a.positive_evidence.clone(),
            negative_evidence: a.negative_evidence.clone(),
        })
        .collect();

    let recent_n = obs
        .iter()
        .filter(|o| o.age_years.map(|y| y <= RECENT_YEARS).unwrap_or(false))
        .count();
    let long_n = obs.len().saturating_sub(recent_n);
    let mut shifts = Vec::new();
    if recent_n >= SHIFT_RECENT_N && long_n >= SHIFT_LONG_N {
        for a in &affinities {
            if a.confidence < SHIFT_CONF {
                continue;
            }
            let delta = a.recent_weight - a.long_term_weight;
            if delta.abs() < SHIFT_DELTA {
                continue;
            }
            let recent_ev: Vec<EvidenceFilm> = obs
                .iter()
                .filter(|o| {
                    o.key.storage_key() == a.key.storage_key()
                        && o.age_years.map(|y| y <= RECENT_YEARS).unwrap_or(false)
                })
                .map(|o| o.evidence.clone())
                .take(6)
                .collect();
            if recent_ev.len() < 3 {
                continue;
            }
            shifts.push(RecentShift {
                feature: a.key.name.clone(),
                family: a.key.family,
                long_term: a.long_term_weight,
                recent: a.recent_weight,
                delta,
                long_term_evidence: a.positive_evidence.iter().chain(a.negative_evidence.iter()).cloned().take(6).collect(),
                recent_evidence: recent_ev,
            });
        }
    }
    shifts.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.delta.abs(), b.delta.abs())
            .then_with(|| a.family.sort_key().cmp(&b.family.sort_key()))
            .then_with(|| a.feature.cmp(&b.feature))
    });
    shifts.truncate(8);

    FeatureProfile {
        affinities,
        polarizing,
        shifts,
        dimensions: Vec::new(),
        modes: Vec::new(),
        mode_shifts: Vec::new(),
    }
}

fn distinct_film_count(group: &[&FeatureObservation]) -> u32 {
    group
        .iter()
        .map(|o| evidence_film_id(&o.evidence))
        .collect::<std::collections::HashSet<_>>()
        .len() as u32
}

fn feature_portability(key: &FeatureKey, group: &[&FeatureObservation]) -> f32 {
    if key.family == FeatureFamily::Keyword && keyword_role(&key.name) == KeywordRole::Contextual {
        return 0.0;
    }
    if !key.family.is_contextual() {
        return 1.0;
    }
    let mut genres = std::collections::HashSet::new();
    let mut disc = 0.0;
    let mut n = 0.0;
    for o in group {
        if o.positive > 0.0 {
            for g in &o.genres {
                genres.insert(g.to_lowercase());
            }
        }
        disc += o.discovery_strength;
        n += 1.0;
    }
    let unique = genres.len();
    if unique < 3 {
        return 0.0;
    }
    let genre_port = ((unique as f32 - 2.0) / 4.0).clamp(0.0, 1.0);
    let mean_disc = if n > 0.0 { disc / n } else { 1.0 };
    genre_port * mean_disc
}

/// Portable facets shared by a majority of a person's liked films.
/// For two films, majority means both — so a composer on Panda + Minions
/// yields comedy/animation, not "any John Powell movie."
fn cluster_from_group(key: &FeatureKey, group: &[&FeatureObservation]) -> EvidenceCluster {
    use std::collections::{HashMap, HashSet};
    if !key.is_person_or_keyword() {
        return EvidenceCluster::default();
    }
    let mut films: HashMap<String, Vec<String>> = HashMap::new();
    for o in group.iter().filter(|o| o.positive > 0.0) {
        let id = evidence_film_id(&o.evidence);
        films.entry(id).or_insert_with(|| o.genres.clone());
    }
    let n = films.len();
    if n < 2 {
        return EvidenceCluster::default();
    }
    let mut genre_counts: HashMap<String, usize> = HashMap::new();
    let mut mode_counts: HashMap<String, usize> = HashMap::new();
    for genres in films.values() {
        let mut seen_g = HashSet::new();
        for g in genres {
            let g = g.to_lowercase();
            if seen_g.insert(g.clone()) {
                *genre_counts.entry(g).or_insert(0) += 1;
            }
        }
        let mut seen_m = HashSet::new();
        for m in modes_from_genres(genres) {
            if seen_m.insert(m.clone()) {
                *mode_counts.entry(m).or_insert(0) += 1;
            }
        }
    }
    let majority = |count: usize| count * 2 > n;
    EvidenceCluster {
        genres: genre_counts
            .into_iter()
            .filter(|(_, c)| majority(*c))
            .map(|(g, _)| g)
            .collect(),
        modes: mode_counts
            .into_iter()
            .filter(|(_, c)| majority(*c))
            .map(|(m, _)| m)
            .collect(),
        keywords: Vec::new(),
    }
}

fn modes_from_genres(genres: &[String]) -> Vec<String> {
    let has = |name: &str| genres.iter().any(|g| g.eq_ignore_ascii_case(name));
    let mut modes = Vec::new();
    if has("Drama") {
        modes.push("story".into());
    }
    if has("Thriller") || has("Horror") {
        modes.push("intensity".into());
    }
    if has("Comedy") {
        modes.push("comedy".into());
    }
    if has("Action") || has("Science Fiction") || has("Sci-Fi") || has("Fantasy") {
        modes.push("spectacle".into());
    }
    if has("Mystery") {
        modes.push("atmosphere".into());
    }
    modes
}

pub fn observations_from_film(
    title: &str,
    rating: f32,
    tmdb_id: Option<i64>,
    signal: &InteractionSignal,
    age_years: Option<f32>,
    genres: &[String],
    credits: &[Credit],
    keywords: &[Keyword],
    year: Option<i32>,
    runtime: Option<i32>,
) -> Vec<FeatureObservation> {
    let evidence = EvidenceFilm {
        title: title.to_string(),
        rating,
        tmdb_id,
        people: credits.iter().map(|c| c.name.clone()).collect(),
        keywords: keywords
            .iter()
            .filter(|k| keyword_is_taste_signal(&k.name))
            .map(|k| k.name.clone())
            .collect(),
        genres: genres.to_vec(),
        year,
        runtime,
    };
    let abs = signal.preference.absolute;
    let positive = abs.max(0.0) * crate::taste::preference::relative_strength(signal.preference.relative);
    let negative = (-abs).max(0.0);
    let mut out = Vec::new();
    let push = |out: &mut Vec<FeatureObservation>, key: FeatureKey| {
        out.push(FeatureObservation {
            key,
            affinity_preference: signal.preference.affinity_preference,
            preference_weight: signal.preference_weight,
            recommendation_weight: signal.recommendation_weight,
            discovery_strength: signal.discovery_strength,
            age_years,
            evidence: evidence.clone(),
            positive,
            negative,
            genres: genres.to_vec(),
        });
    };
    for g in genres {
        push(&mut out, FeatureKey::new(FeatureFamily::Genre, None, g));
    }
    for c in credits {
        let Some(family) = family_for_job(&c.job) else {
            continue;
        };
        push(&mut out, FeatureKey::new(family, c.id, &c.name));
    }
    for k in keywords {
        if !keyword_is_taste_signal(&k.name) {
            continue;
        }
        push(&mut out, FeatureKey::new(FeatureFamily::Keyword, k.id, &k.name));
    }
    if let Some(y) = year {
        push(&mut out, FeatureKey::new(FeatureFamily::Decade, None, decade_label(y)));
    }
    if let Some(rt) = runtime {
        push(
            &mut out,
            FeatureKey::new(FeatureFamily::Runtime, None, runtime_bucket(rt)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::preference::{interaction_signal, rating_profile};

    fn sig(rating: f32) -> InteractionSignal {
        let p = rating_profile(&[5.0, 4.5, 4.0, 3.5, 3.0, 2.5, 4.0, 4.5]).unwrap();
        interaction_signal(rating, &p, Some(1.0), 1, false)
    }

    fn two_film_keyword(name: &str) -> FeatureAffinity {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let kw = Keyword {
            id: Some(42),
            name: name.into(),
        };
        let mut obs = Vec::new();
        for i in 0..2i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("liked{i}"),
                4.5,
                Some(i + 1),
                &s,
                Some(0.4),
                &["Drama".into()],
                &[],
                &[kw.clone()],
                Some(2018),
                None,
            ));
        }
        build_profile(&obs)
            .affinities
            .into_iter()
            .find(|a| a.key.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("missing keyword {name}"))
    }

    #[test]
    fn location_keyword_is_contextual_not_citeable() {
        for name in ["new york city", "los angeles, california"] {
            let a = two_film_keyword(name);
            assert_eq!(a.appearances, 2, "{name} should still be observed");
            assert!(!a.citeable(), "{name} must not be a hunt target");
            assert!(
                a.portability < PORTABILITY_SKIP,
                "{name} should be non-portable, got {}",
                a.portability
            );
        }
    }

    #[test]
    fn cartoon_and_anti_hero_are_not_citeable_engines() {
        for name in ["cartoon", "anti hero"] {
            let a = two_film_keyword(name);
            assert_eq!(a.appearances, 2, "{name} should still be observed");
            assert!(!a.citeable(), "{name}");
        }
    }

    #[test]
    fn reaction_keywords_are_not_citeable_engines() {
        for name in ["hilarious", "admiring"] {
            let a = two_film_keyword(name);
            assert_eq!(a.appearances, 2, "{name} should still be observed");
            assert!(!a.citeable(), "{name}");
        }
    }

    #[test]
    fn visual_quality_keywords_remain_citeable() {
        for name in ["neo-noir", "long take"] {
            let a = two_film_keyword(name);
            assert!(a.citeable(), "{name}");
            assert!(
                (a.portability - 1.0).abs() < 1e-5,
                "{name} portability {}",
                a.portability
            );
        }
    }

    #[test]
    fn specific_repeated_keyword_remains_citeable() {
        let a = two_film_keyword("heist");
        assert!(a.citeable());
        assert_eq!(a.appearances, 2);
    }

    #[test]
    fn polarizing_needs_both_sides() {
        let heat = observations_from_film(
            "Heat",
            5.0,
            Some(1),
            &sig(5.0),
            Some(3.0),
            &["Crime".into()],
            &[Credit {
                id: Some(10),
                name: "Michael Mann".into(),
                job: "Director".into(),
            }],
            &[],
            Some(1995),
            Some(170),
        );
        let blackhat = observations_from_film(
            "Blackhat",
            2.0,
            Some(2),
            &sig(2.0),
            Some(4.0),
            &["Crime".into()],
            &[Credit {
                id: Some(10),
                name: "Michael Mann".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2015),
            Some(133),
        );
        let mut all = heat;
        all.extend(blackhat);
        // repeat to raise variance confidence
        let extra = observations_from_film(
            "Collateral",
            4.5,
            Some(3),
            &sig(4.5),
            Some(2.0),
            &["Crime".into()],
            &[Credit {
                id: Some(10),
                name: "Michael Mann".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2004),
            Some(120),
        );
        all.extend(extra);
        let profile = build_profile(&all);
        let mann = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Michael Mann")
            .unwrap();
        assert!(!mann.positive_evidence.is_empty());
        assert!(!mann.negative_evidence.is_empty());
    }

    #[test]
    fn recent_shift_is_gated() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = Vec::new();
        for i in 0..25 {
            let s = interaction_signal(2.5, &p, Some(4.0), 1, false);
            obs.extend(observations_from_film(
                &format!("old{i}"),
                2.5,
                Some(i),
                &s,
                Some(4.0),
                &["Animation".into()],
                &[],
                &[],
                Some(2000),
                None,
            ));
        }
        let profile = build_profile(&obs);
        assert!(profile.shifts.is_empty());
    }

    #[test]
    fn weighted_mean_uses_affinity_preference() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let s5 = interaction_signal(5.0, &p, Some(1.0), 1, false);
        let s4 = interaction_signal(4.0, &p, Some(1.0), 1, false);
        let mut obs = observations_from_film(
            "A",
            5.0,
            Some(1),
            &s5,
            Some(1.0),
            &["Drama".into()],
            &[],
            &[],
            None,
            None,
        );
        obs.extend(observations_from_film(
            "B",
            4.0,
            Some(2),
            &s4,
            Some(1.0),
            &["Drama".into()],
            &[],
            &[],
            None,
            None,
        ));
        let profile = build_profile(&obs);
        let drama = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Drama")
            .unwrap();
        let expected = (s5.preference.affinity_preference * s5.preference_weight
            + s4.preference.affinity_preference * s4.preference_weight)
            / (s5.preference_weight + s4.preference_weight);
        assert!((drama.preference_mean - expected).abs() < 1e-4);
        assert!((drama.weighted_mean - drama.preference_mean).abs() < 1e-6);
        assert!((drama.portability - 1.0).abs() < 1e-5);
    }

    #[test]
    fn nostalgia_decade_is_not_portable() {
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
        let decade = profile
            .affinities
            .iter()
            .find(|a| a.key.family == FeatureFamily::Decade)
            .unwrap();
        assert_eq!(decade.portability, 0.0);
        assert!(decade.scoring_affinity().abs() < 1e-5);
        assert!(decade.preference_mean > 0.0);
    }

    #[test]
    fn diverse_first_watch_decade_is_portable() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let genres = ["Drama", "Thriller", "Action", "Horror", "Comedy"];
        let mut obs = Vec::new();
        for (i, g) in genres.iter().cycle().take(20).enumerate() {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("new{i}"),
                4.5,
                Some(i as i64),
                &s,
                Some(0.4),
                &[(*g).into()],
                &[],
                &[],
                Some(2005),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let decade = profile
            .affinities
            .iter()
            .find(|a| a.key.family == FeatureFamily::Decade)
            .unwrap();
        assert!(decade.portability >= 0.4, "got {}", decade.portability);
        assert!(decade.scoring_affinity() > 0.0);
    }

    #[test]
    fn execution_language_requires_repeated_explicit_phrases() {
        let one = ["The acting was great, honestly."].as_slice();
        assert!(repeated_execution_signals(&one).is_empty());

        let repeated = [
            "The acting was great and the pacing was tight throughout the entire film, with every scene feeling purposeful, energetic, and carefully shaped instead of rushed or repetitive.",
            "Great acting with tight pacing throughout, and the performances kept each emotional turn clear while the film moved confidently through its story without losing the smaller details.",
        ];
        let signals = repeated_execution_signals(&repeated);
        assert!(signals.iter().any(|s| s == "strong performances"));
        assert!(signals.iter().any(|s| s == "tight pacing"));
        let keywords = execution_keywords_for_review(&repeated[0], &signals);
        assert!(keywords.iter().any(|k| k.name == "strong performances"));
        assert!(keywords.iter().any(|k| k.name == "tight pacing"));
        assert_eq!(execution_signal_polarity("strong performances"), Some(1.0));
        assert_eq!(execution_signal_polarity("slow pacing"), Some(-1.0));
    }

    #[test]
    fn two_films_establish_cinematographer() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            Some(176),
        );
        obs.extend(observations_from_film(
            "Project Hail Mary",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp],
            &[],
            Some(2026),
            Some(140),
        ));
        let profile = build_profile(&obs);
        let fraser = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Greig Fraser")
            .unwrap();
        assert_eq!(fraser.positive_evidence.len(), 2);
        assert!(fraser.confidence > 0.4, "got {}", fraser.confidence);
        assert!(fraser.scoring_affinity() > 0.0);
        assert!((fraser.portability - 1.0).abs() < 1e-5);
        assert!(fraser.citeable());
    }

    #[test]
    fn hillenburg_evidence_does_not_include_twilight() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = observations_from_film(
            "The SpongeBob SquarePants Movie",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Family".into()],
            &[Credit {
                id: Some(10),
                name: "Stephen Hillenburg".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2004),
            Some(87),
        );
        obs.extend(observations_from_film(
            "The Twilight Saga: Breaking Dawn - Part 1",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.8), 1, false),
            Some(0.8),
            &["Drama".into(), "Fantasy".into()],
            &[Credit {
                id: Some(99),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2011),
            Some(117),
        ));
        let profile = build_profile(&obs);
        let hill = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Stephen Hillenburg")
            .unwrap();
        assert!(hill.positive_evidence.iter().any(|e| e.title.contains("SpongeBob")));
        assert!(hill.positive_evidence.iter().all(|e| !e.title.contains("Twilight")));
        assert!(!hill.citeable(), "one film must not be a hunt target");
        let drama = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Drama")
            .unwrap();
        assert!(drama.positive_evidence.iter().any(|e| e.title.contains("Twilight")));
    }

    /// Real 627-film run: Hillenburg n=4, every evidence title is SpongeBob.
    /// Rewatches / duplicate credit rows must not turn one film into a hunt target.
    #[test]
    fn four_viewings_of_one_film_do_not_make_a_person_citeable() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = Vec::new();
        for _ in 0..4 {
            obs.extend(observations_from_film(
                "The SpongeBob SquarePants Movie",
                5.0,
                Some(1),
                &interaction_signal(5.0, &p, Some(1.0), 1, false),
                Some(1.0),
                &["Comedy".into(), "Family".into()],
                &[Credit {
                    id: Some(10),
                    name: "Stephen Hillenburg".into(),
                    job: "Director".into(),
                }],
                &[],
                Some(2004),
                Some(87),
            ));
        }
        let profile = build_profile(&obs);
        let hill = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Stephen Hillenburg")
            .unwrap();
        assert_eq!(
            hill.positive_evidence
                .iter()
                .map(|e| e.tmdb_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert!(
            !hill.citeable(),
            "one film rated four times is still a singleton, appearances={}",
            hill.appearances
        );
    }

    #[test]
    fn same_tmdb_id_does_not_merge_different_names() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let mut obs = observations_from_film(
            "A",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Drama".into()],
            &[Credit {
                id: Some(7),
                name: "Stephen Hillenburg".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2004),
            None,
        );
        obs.extend(observations_from_film(
            "Twilight",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Drama".into()],
            &[Credit {
                id: Some(7),
                name: "Bill Condon".into(),
                job: "Director".into(),
            }],
            &[],
            Some(2011),
            None,
        ));
        let profile = build_profile(&obs);
        let hill = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Stephen Hillenburg")
            .unwrap();
        assert_eq!(hill.positive_evidence.len(), 1);
        assert_eq!(hill.positive_evidence[0].title, "A");
    }

    #[test]
    fn portable_decade_is_still_not_citeable() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let genres = ["Drama", "Thriller", "Comedy", "Action", "Horror"];
        let mut obs = Vec::new();
        for i in 0..12i64 {
            let s = interaction_signal(4.5, &p, Some(0.4), 1, false);
            obs.extend(observations_from_film(
                &format!("new{i}"),
                4.5,
                Some(i),
                &s,
                Some(0.4),
                &[genres[i as usize % 5].into()],
                &[],
                &[],
                Some(2005),
                None,
            ));
        }
        let profile = build_profile(&obs);
        let decade = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "2000s")
            .unwrap();
        assert!(
            decade.portability >= PORTABILITY_SKIP,
            "this dataset makes decade look portable: {}",
            decade.portability
        );
        assert!(
            !decade.citeable(),
            "contextual features must never become hunt targets"
        );
    }

    #[test]
    fn powell_evidence_cluster_is_shared_comedy_not_any_powell_film() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let composer = Credit {
            id: Some(50),
            name: "John Powell".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "Kung Fu Panda",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Animation".into(), "Family".into()],
            &[composer.clone()],
            &[],
            Some(2008),
            None,
        );
        obs.extend(observations_from_film(
            "Minions & Monsters",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(1.0), 1, false),
            Some(1.0),
            &["Comedy".into(), "Animation".into(), "Family".into()],
            &[composer],
            &[],
            Some(2010),
            None,
        ));
        let profile = build_profile(&obs);
        let powell = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "John Powell")
            .unwrap();
        assert!(powell.citeable());
        assert!(powell.appearances >= 2);
        assert!(
            powell.recommendation_mean > 0.5,
            "two strong likes should produce a high mean, got {}",
            powell.recommendation_mean
        );
        assert!(
            powell.evidence_cluster.genres.iter().any(|g| g == "comedy"),
            "cluster {:?}",
            powell.evidence_cluster
        );
        assert!(powell.evidence_cluster.modes.iter().any(|m| m == "comedy"));
        assert!(powell.evidence_cluster.overlaps(
            &["Comedy".into(), "Animation".into()],
            &[],
            &["comedy".into()]
        ));
        assert!(
            !powell.evidence_cluster.overlaps(
                &["Drama".into(), "Thriller".into()],
                &[],
                &["intensity".into(), "story".into()]
            ),
            "United 93 / Bourne must not inherit Panda+Minions"
        );
    }

    #[test]
    fn cinematographer_across_unrelated_genres_has_empty_cluster() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let dp = Credit {
            id: Some(77),
            name: "Greig Fraser".into(),
            job: "Director of Photography".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.5), 1, false),
            Some(0.5),
            &["Crime".into()],
            &[dp.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Project Hail Mary",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.2), 1, false),
            Some(0.2),
            &["Science Fiction".into()],
            &[dp],
            &[],
            Some(2026),
            None,
        ));
        let profile = build_profile(&obs);
        let fraser = profile
            .affinities
            .iter()
            .find(|a| a.key.name == "Greig Fraser")
            .unwrap();
        assert!(fraser.citeable());
        assert!(
            fraser.evidence_cluster.is_empty(),
            "person is the transferable facet when evidence genres do not overlap: {:?}",
            fraser.evidence_cluster
        );
        assert!(fraser.evidence_cluster.overlaps(
            &["Comedy".into()],
            &[],
            &["comedy".into()]
        ));
    }

    #[test]
    fn keyword_strength_classes() {
        assert_eq!(keyword_strength("neo-noir"), KeywordStrength::Strong);
        assert_eq!(keyword_strength("nonlinear timeline"), KeywordStrength::Strong);
        assert_eq!(keyword_strength("long take"), KeywordStrength::Strong);
        assert_eq!(keyword_strength("coming of age"), KeywordStrength::Thematic);
        assert_eq!(keyword_strength("dysfunctional family"), KeywordStrength::Thematic);
        assert_eq!(keyword_strength("drugs"), KeywordStrength::Broad);
        assert_eq!(keyword_strength("murder"), KeywordStrength::Broad);
        assert_eq!(keyword_strength("battle"), KeywordStrength::Broad);
        assert_eq!(keyword_strength("friends"), KeywordStrength::Broad);
        assert_eq!(keyword_strength("hilarious"), KeywordStrength::Contextual);
        assert_eq!(keyword_strength("based on novel"), KeywordStrength::Ignore);
        assert!(keyword_can_carry_alone("neo-noir"));
        assert!(!keyword_can_carry_alone("coming of age"));
        assert!(!keyword_can_carry_alone("drugs"));
        assert!(!keyword_can_carry_alone("hilarious"));
        assert!(keyword_is_display_reason("neo-noir"));
        assert!(keyword_is_display_reason("coming of age"));
        assert!(keyword_is_display_reason("supernatural horror"));
        assert!(!keyword_is_display_reason("bold"));
        assert!(!keyword_is_display_reason("enthusiastic"));
        assert!(!keyword_is_display_reason("amused"));
        assert!(!keyword_is_display_reason("dramatic"));
    }
}
