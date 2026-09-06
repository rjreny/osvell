use crate::taste::features::{
    keyword_is_display_reason, keyword_strength, FeatureAffinity, FeatureFamily, KeywordStrength,
};
use crate::taste::retrieve::{RetrievalKind, RetrievalSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MatchedFeatureView {
    #[serde(default)]
    pub feature_key: String,
    pub name: String,
    pub family: String,
    pub appearances: u32,
    pub recommendation_mean: f32,
    pub scoring_affinity: f32,
    pub confidence: f32,
    pub portability: f32,
    pub citeable: bool,
    pub cited: bool,
}

impl MatchedFeatureView {
    pub fn from_affinity(aff: &FeatureAffinity, cited: bool) -> Self {
        Self {
            feature_key: aff.key.storage_key(),
            name: aff.key.name.clone(),
            family: family_label(aff.key.family).into(),
            appearances: aff.appearances,
            recommendation_mean: round2(aff.recommendation_mean),
            scoring_affinity: round2(aff.scoring_affinity()),
            confidence: round2(aff.confidence),
            portability: round2(aff.portability),
            citeable: aff.citeable(),
            cited,
        }
    }
}

fn default_candidate_fit() -> f32 {
    1.0
}

/// Displayable New-for-you evidence. `None` is a retrieval lead, not a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceGrade {
    #[default]
    None,
    Medium,
    Strong,
}

impl EvidenceGrade {
    pub fn displayable(self) -> bool {
        matches!(self, Self::Medium | Self::Strong)
    }

    /// Numeric rank used only as a component of internal selection scores.
    /// Membership still reads the enum itself, so a missing grade cannot be
    /// reconstructed from whatever fields happen to be present on a row.
    pub fn rank(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Medium => 2,
            Self::Strong => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityTrace {
    pub portable_evidence_required: bool,
    pub passed: bool,
    pub passed_because: Vec<String>,
    /// How well *this* movie matches liked evidence, independent of person affinity.
    /// 1.0 = specific visual/story/craft overlap; ~0.3 = person-only.
    #[serde(default = "default_candidate_fit")]
    pub candidate_fit: f32,
    /// Legacy Strong/Medium grade — logged only; New admission ignores this after C1.
    #[serde(default)]
    pub evidence_grade: EvidenceGrade,
    /// Content Fit_v1 on 0..=1 (`(score + 1) * 0.5`).
    #[serde(default)]
    pub predicted_fit: f32,
    /// Content-family confidence (orthogonal to fit).
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub hydration_completeness: f32,
    /// C1 decision state: recommended | exploratory | held.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub primary_reason: String,
}

impl Default for EligibilityTrace {
    fn default() -> Self {
        Self {
            portable_evidence_required: false,
            passed: false,
            passed_because: Vec::new(),
            candidate_fit: 1.0,
            evidence_grade: EvidenceGrade::None,
            predicted_fit: 0.0,
            confidence: 0.0,
            hydration_completeness: 0.0,
            state: "held".into(),
            primary_reason: String::new(),
        }
    }
}

pub fn primary_retrieval(sources: &[RetrievalSource]) -> (String, String) {
    let Some(src) = sources.first() else {
        return ("unknown".into(), String::new());
    };
    (retrieval_kind_label(src.kind).into(), src.label.clone())
}

pub fn retrieval_kind_label(kind: RetrievalKind) -> &'static str {
    match kind {
        RetrievalKind::Related
        | RetrievalKind::RelatedRecommendations
        | RetrievalKind::RelatedSimilar => "related",
        RetrievalKind::Filmography => "filmography",
        RetrievalKind::Collection => "collection",
        RetrievalKind::SemanticFilmLocal => "semanticFilmLocal",
        RetrievalKind::SemanticProfile => "semanticProfile",
        RetrievalKind::Friend => "friend",
        RetrievalKind::Watchlist => "watchlist",
        RetrievalKind::Exploration => "exploration",
        RetrievalKind::Discovery => "discovery",
    }
}

pub fn family_label(family: FeatureFamily) -> &'static str {
    match family {
        FeatureFamily::Director => "director",
        FeatureFamily::Writer => "writer",
        FeatureFamily::Cinematographer => "cinematographer",
        FeatureFamily::Composer => "composer",
        FeatureFamily::Actor => "actor",
        FeatureFamily::Genre => "genre",
        FeatureFamily::Keyword => "keyword",
        FeatureFamily::Decade => "decade",
        FeatureFamily::Runtime => "runtime",
    }
}

/// Display rank, not scoring weight. Craft and specific story/visual keywords
/// explain a recommendation better than family tropes or broad genres.
pub fn display_priority(aff: &FeatureAffinity) -> i32 {
    match aff.key.family {
        FeatureFamily::Cinematographer | FeatureFamily::Director => 100,
        FeatureFamily::Writer => 90,
        FeatureFamily::Composer => 80,
        FeatureFamily::Keyword => match keyword_strength(&aff.key.name) {
            KeywordStrength::Strong => 78,
            KeywordStrength::Thematic if is_relational_keyword(&aff.key.name) => 25,
            KeywordStrength::Thematic => 52,
            KeywordStrength::Broad => 12,
            _ => 8,
        },
        FeatureFamily::Actor => 60,
        FeatureFamily::Genre => 20,
        FeatureFamily::Decade | FeatureFamily::Runtime => 5,
    }
}

pub fn is_relational_keyword(name: &str) -> bool {
    let compact: String = name
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    matches!(
        compact.as_str(),
        "dysfunctionalfamily"
            | "fathersonrelationship"
            | "fatherson"
            | "fatherdaughterrelationship"
            | "fatherdaughter"
            | "motherdaughterrelationship"
            | "motherdaughter"
            | "mothersonrelationship"
            | "motherson"
            | "parentchildrelationship"
            | "parentchild"
            | "siblingrelationship"
            | "friends"
            | "friendship"
            | "comingofage"
            | "lossoflovedone"
            | "lossofalovedone"
    )
}

pub fn format_display_reason(aff: &FeatureAffinity) -> String {
    let n = aff.appearances;
    let films = if n == 1 {
        "1 film".to_string()
    } else {
        format!("{n} films")
    };
    if n <= 2 {
        format!("{} · {} · limited evidence", aff.key.name, films)
    } else {
        format!("{} · {}", aff.key.name, films)
    }
}

fn display_collapse_key(aff: &FeatureAffinity) -> String {
    if aff.key.is_person_or_keyword() && aff.key.family != FeatureFamily::Keyword {
        format!("person:{}", aff.key.name.to_ascii_lowercase())
    } else {
        format!("{:?}:{}", aff.key.family, aff.key.name.to_ascii_lowercase())
    }
}

pub fn select_display_reasons(cited: &[&FeatureAffinity], extras: &[String]) -> Vec<String> {
    let mut ranked: Vec<&FeatureAffinity> = cited
        .iter()
        .copied()
        .filter(|a| {
            a.key.family != FeatureFamily::Keyword || keyword_is_display_reason(&a.key.name)
        })
        .collect();
    ranked.sort_by(|a, b| {
        display_priority(b)
            .cmp(&display_priority(a))
            .then_with(|| {
                b.scoring_affinity()
                    .partial_cmp(&a.scoring_affinity())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.key.family.sort_key().cmp(&b.key.family.sort_key()))
            .then_with(|| a.key.name.cmp(&b.key.name))
            .then_with(|| a.key.id.cmp(&b.key.id))
    });
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for aff in ranked {
        if !seen.insert(display_collapse_key(aff)) {
            continue;
        }
        out.push(format_display_reason(aff));
        if out.len() >= 3 {
            break;
        }
    }
    for extra in extras {
        if out.len() >= 4 {
            break;
        }
        if !out.iter().any(|r| r.eq_ignore_ascii_case(extra)) {
            out.push(extra.clone());
        }
    }
    canonicalize_reason_lines(out)
}

/// Collapse duplicate "Name · N films" lines that survived from director+writer
/// or repeated extras.
pub fn canonicalize_reason_lines(lines: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in lines {
        let key = line
            .split(" · ")
            .next()
            .unwrap_or(&line)
            .trim()
            .to_ascii_lowercase();
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        out.push(line);
    }
    out
}

/// Drop generated copy that cites features not on the visible card.
pub fn ground_why(why: &str, display: &[String], hidden: &[MatchedFeatureView]) -> String {
    let why = why.trim();
    if why.is_empty() {
        return String::new();
    }
    let allowed: Vec<String> = display
        .iter()
        .filter_map(|line| {
            let name = line.split(" · ").next()?.trim().to_ascii_lowercase();
            if name.is_empty() || name == "on your watchlist" {
                None
            } else {
                Some(name)
            }
        })
        .collect();
    let lower = why.to_lowercase();
    for hidden_feat in hidden {
        let name = hidden_feat.name.to_ascii_lowercase();
        if name.len() < 4 {
            continue;
        }
        if lower.contains(&name) && !allowed.iter().any(|a| a.contains(&name) || name.contains(a)) {
            return String::new();
        }
    }
    const BROAD: &[&str] = &[
        "mystery",
        "crime",
        "thriller",
        "drama",
        "horror",
        "action",
        "comedy",
        "suspense",
        "suspenseful",
        "conspiracy",
        "adventure",
        "fantasy",
    ];
    for word in BROAD {
        if lower.contains(word) && !allowed.iter().any(|a| a.contains(word)) {
            return String::new();
        }
    }
    why.to_string()
}

pub fn format_provenance(sources: &[RetrievalSource]) -> String {
    let mut parts = Vec::new();
    let mut seen_kind: Vec<RetrievalKind> = Vec::new();
    for src in sources {
        if seen_kind.contains(&src.kind) {
            continue;
        }
        seen_kind.push(src.kind);
        match src.kind {
            RetrievalKind::Watchlist => parts.push("On your watchlist".into()),
            RetrievalKind::Related
            | RetrievalKind::RelatedRecommendations
            | RetrievalKind::RelatedSimilar => {
                let seed = src
                    .label
                    .trim()
                    .strip_prefix("similar to ")
                    .or_else(|| src.label.trim().strip_prefix("Similar to "))
                    .or_else(|| src.label.trim().strip_prefix("recommended from "))
                    .unwrap_or(src.label.trim());
                if seed.is_empty() || seed.eq_ignore_ascii_case("related") {
                    parts.push("Related".into());
                } else if src.kind == RetrievalKind::RelatedRecommendations {
                    parts.push(format!("Recommended from {seed}"));
                } else {
                    parts.push(format!("Related to {seed}"));
                }
            }
            RetrievalKind::Filmography => {
                let name = src.label.trim();
                if name.is_empty() || name.eq_ignore_ascii_case("filmography") {
                    parts.push("Filmography".into());
                } else {
                    parts.push(format!("Filmography · {name}"));
                }
            }
            RetrievalKind::Collection => {
                let name = src.label.trim();
                if name.is_empty() {
                    parts.push("Same collection".into());
                } else {
                    parts.push(name.to_string());
                }
            }
            RetrievalKind::SemanticFilmLocal => parts.push("Semantically near a liked film".into()),
            RetrievalKind::SemanticProfile => parts.push("Matches your overall taste".into()),
            RetrievalKind::Friend => parts.push("Friends".into()),
            RetrievalKind::Discovery => parts.push("Discovery".into()),
            RetrievalKind::Exploration => parts.push("Exploration".into()),
        }
    }
    parts.join(" · ")
}

pub fn eligibility_trace(
    cited: &[&FeatureAffinity],
    genre_only: bool,
    passed: bool,
) -> EligibilityTrace {
    let portable: Vec<String> = cited
        .iter()
        .filter(|a| a.key.is_person_or_keyword())
        .map(|a| a.key.name.clone())
        .collect();
    let passed_because = if !portable.is_empty() {
        portable
    } else if genre_only {
        let mut names: Vec<String> = cited.iter().map(|a| a.key.name.clone()).collect();
        names.push("genre-only".into());
        names
    } else {
        cited.iter().map(|a| a.key.name.clone()).collect()
    };
    EligibilityTrace {
        portable_evidence_required: true,
        passed,
        passed_because,
        candidate_fit: 1.0,
        evidence_grade: EvidenceGrade::None,
        ..Default::default()
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::features::{
        build_profile, observations_from_film, Credit, FeatureAffinity, Keyword,
    };
    use crate::taste::preference::{interaction_signal, rating_profile};

    fn two_film_keyword(name: &str) -> FeatureAffinity {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let kw = Keyword {
            id: Some(1),
            name: name.into(),
        };
        let mut obs = observations_from_film(
            "Liked A",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[],
            &[kw.clone()],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Liked B",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Fantasy".into()],
            &[],
            &[kw],
            Some(2023),
            None,
        ));
        build_profile(&obs)
            .affinities
            .into_iter()
            .find(|a| a.key.name == name)
            .expect("keyword")
    }

    #[test]
    fn family_trope_loses_display_to_creature_keyword() {
        let family = two_film_keyword("dysfunctional family");
        let creature = two_film_keyword("creature");
        let cited = [&family, &creature];
        let display = select_display_reasons(&cited, &[]);
        assert!(
            display[0].to_lowercase().contains("creature"),
            "display should lead with the meaningful keyword, got {display:?}"
        );
        assert!(
            !display[0].to_lowercase().contains("dysfunctional"),
            "family trope must not be the headline, got {display:?}"
        );
    }

    #[test]
    fn thin_person_display_names_sample_size() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let giacchino = Credit {
            id: Some(9),
            name: "Michael Giacchino".into(),
            job: "Original Music Composer".into(),
        };
        let mut obs = observations_from_film(
            "The Batman",
            5.0,
            Some(1),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Crime".into()],
            &[giacchino.clone()],
            &[],
            Some(2022),
            None,
        );
        obs.extend(observations_from_film(
            "Spider-Man: No Way Home",
            5.0,
            Some(2),
            &interaction_signal(5.0, &p, Some(0.3), 1, false),
            Some(0.3),
            &["Action".into()],
            &[giacchino],
            &[],
            Some(2021),
            None,
        ));
        let aff = build_profile(&obs)
            .affinities
            .into_iter()
            .find(|a| a.key.name == "Michael Giacchino")
            .unwrap();
        let line = format_display_reason(&aff);
        assert!(line.contains("2 films"), "{line}");
        assert!(line.contains("limited evidence"), "{line}");
        assert!(!line.contains("0.91"), "{line}");
    }

    fn two_film_person(name: &str, job: &str) -> FeatureAffinity {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let credit = Credit {
            id: Some(1),
            name: name.into(),
            job: job.into(),
        };
        let mut obs = observations_from_film(
            "Liked A",
            4.5,
            Some(1),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Drama".into()],
            &[credit.clone()],
            &[],
            Some(2006),
            None,
        );
        obs.extend(observations_from_film(
            "Liked B",
            4.5,
            Some(2),
            &interaction_signal(4.5, &p, Some(0.4), 1, false),
            Some(0.4),
            &["Thriller".into()],
            &[credit],
            &[],
            Some(2008),
            None,
        ));
        build_profile(&obs)
            .affinities
            .into_iter()
            .find(|a| a.key.name == name)
            .expect("person")
    }

    #[test]
    fn duplicate_feature_reasons_are_collapsed() {
        let director = two_film_person("James Cameron", "Director");
        let writer = two_film_person("James Cameron", "Writer");
        let actor = two_film_person("Bill Paxton", "Actor");
        let display = select_display_reasons(&[&director, &writer, &actor], &[]);
        let cameron = display
            .iter()
            .filter(|l| l.to_lowercase().contains("james cameron"))
            .count();
        assert_eq!(cameron, 1, "director+writer must render once, got {display:?}");
        assert!(
            display.iter().any(|l| l.to_lowercase().contains("bill paxton")),
            "{display:?}"
        );
    }

    #[test]
    fn duplicate_watchlist_origins_are_collapsed() {
        let sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Watchlist,
                label: "watchlist".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Watchlist,
                label: "watchlist".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        assert_eq!(format_provenance(&sources), "On your watchlist");
    }

    #[test]
    fn multiple_identical_sources_render_once() {
        let sources = vec![
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Christopher Nolan".into(),
                seed_tmdb_id: Some(155),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Wally Pfister".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Filmography,
                label: "Christopher Nolan".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
            RetrievalSource {
                kind: RetrievalKind::Watchlist,
                label: "watchlist".into(),
                seed_tmdb_id: None,
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            },
        ];
        let line = format_provenance(&sources);
        assert_eq!(line, "Filmography · Christopher Nolan · On your watchlist");
        assert_eq!(
            format_provenance(&[RetrievalSource {
                kind: RetrievalKind::Related,
                label: "similar to A Minecraft Movie".into(),
                seed_tmdb_id: Some(1),
                seed_rating: None,
                similarity: None,
                neighbor_rank: None,
            }]),
            "Related to A Minecraft Movie"
        );
    }

    #[test]
    fn generic_adjectives_do_not_lead_pick_reasons() {
        let bold = two_film_keyword("bold");
        let enthusiastic = two_film_keyword("enthusiastic");
        let zimmer = two_film_person("Hans Zimmer", "Original Music Composer");
        let display = select_display_reasons(&[&bold, &enthusiastic, &zimmer], &[]);
        assert!(
            display[0].contains("Hans Zimmer"),
            "craft should lead, got {display:?}"
        );
        assert!(
            display.iter().all(|l| !l.to_lowercase().contains("bold")
                && !l.to_lowercase().contains("enthusiastic")),
            "adjectives must not be headline reasons, got {display:?}"
        );
    }

    #[test]
    fn structural_keywords_remain_displayable() {
        let noir = two_film_keyword("neo-noir");
        let display = select_display_reasons(&[&noir], &[]);
        assert!(
            display.iter().any(|l| l.to_lowercase().contains("neo-noir")),
            "{display:?}"
        );
    }

    #[test]
    fn thematic_keywords_require_explanatory_value() {
        let coming = two_film_keyword("coming of age");
        let zimmer = two_film_person("Hans Zimmer", "Original Music Composer");
        let display = select_display_reasons(&[&coming, &zimmer], &[]);
        assert!(
            display[0].contains("Hans Zimmer"),
            "person should outrank thematic keyword, got {display:?}"
        );
        assert!(
            display.iter().any(|l| l.to_lowercase().contains("coming of age")),
            "thematic keyword still belongs on the card when it explains the pick, got {display:?}"
        );
    }

    #[test]
    fn loss_of_loved_one_does_not_headline_over_person() {
        let loss = two_film_keyword("loss of loved one");
        let collette = two_film_person("Toni Collette", "Actor");
        let thriller = two_film_keyword("psychological thriller");
        let display = select_display_reasons(&[&loss, &collette, &thriller], &[]);
        assert!(
            !display[0].to_lowercase().contains("loss of loved one"),
            "relational loss tag must not lead, got {display:?}"
        );
        assert!(
            display[0].contains("Toni Collette")
                || display[0].to_lowercase().contains("psychological thriller"),
            "{display:?}"
        );
    }

    #[test]
    fn generated_why_cannot_mention_hidden_or_genre_features() {
        let hidden = vec![MatchedFeatureView {
            feature_key: String::new(),
            name: "mystery".into(),
            family: "genre".into(),
            appearances: 22,
            recommendation_mean: 0.25,
            scoring_affinity: 0.09,
            confidence: 0.97,
            portability: 1.0,
            citeable: true,
            cited: false,
        }];
        let prestige = ground_why(
            "strong fit due to matching features such as Christopher Nolan and mystery",
            &["Christopher Nolan · 4 films".into(), "On your watchlist".into()],
            &hidden,
        );
        assert!(
            prestige.is_empty() || !prestige.to_lowercase().contains("mystery"),
            "{prestige}"
        );
        let seven = ground_why(
            "strong fit due to matching features such as crime and thriller",
            &["Darius Khondji · 3 films".into(), "On your watchlist".into()],
            &[],
        );
        assert!(seven.is_empty(), "genre-led why must not survive, got {seven}");
        let ok = ground_why(
            "Christopher Nolan, from films you already like",
            &["Christopher Nolan · 4 films".into()],
            &[],
        );
        assert!(ok.to_lowercase().contains("nolan"), "{ok}");
    }
}
