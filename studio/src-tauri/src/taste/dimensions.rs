use crate::taste::features::{family_for_job, Credit, EvidenceFilm, FeatureFamily, Keyword, RECENT_YEARS};
use crate::taste::preference::InteractionSignal;
use serde::{Deserialize, Serialize};

pub const MODE_MIN_MEMBERS: usize = 5;
pub const MODE_SHIFT_DELTA: f32 = 0.25;
const SHIFT_RECENT_N: usize = 8;
const SHIFT_LONG_N: usize = 20;
const COMFORT_FAMILIARITY: f32 = 0.6;

const VISUAL_KEYWORDS: &[&str] = &[
    "imax",
    "cinematography",
    "neo-noir",
    "film noir",
    "atmospheric",
    "one-shot",
    "long take",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExperienceDimension {
    Visual,
    Story,
    Intensity,
    Comedy,
    Spectacle,
    Atmosphere,
    Comfort,
}

impl ExperienceDimension {
    pub fn all() -> [Self; 7] {
        [
            Self::Visual,
            Self::Story,
            Self::Intensity,
            Self::Comedy,
            Self::Spectacle,
            Self::Atmosphere,
            Self::Comfort,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Visual => "visual",
            Self::Story => "story",
            Self::Intensity => "intensity",
            Self::Comedy => "comedy",
            Self::Spectacle => "spectacle",
            Self::Atmosphere => "atmosphere",
            Self::Comfort => "comfort",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteDimensionView {
    pub name: String,
    pub strength: f32,
    pub evidence: Vec<EvidenceFilm>,
    pub recent_share: f32,
    pub long_term_share: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteMode {
    pub dimension: String,
    pub strength: f32,
    pub members: Vec<EvidenceFilm>,
    pub recent_share: f32,
    pub long_term_share: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeShift {
    pub dimension: String,
    pub long_term: f32,
    pub recent: f32,
    pub delta: f32,
}

pub struct ModeFilm<'a> {
    pub title: &'a str,
    pub rating: Option<f32>,
    pub tmdb_id: Option<i64>,
    pub genres: &'a [String],
    pub credits: &'a [Credit],
    pub keywords: &'a [Keyword],
    pub signal: Option<&'a InteractionSignal>,
    pub age_years: Option<f32>,
}

pub fn predicted_modes(
    genres: &[String],
    credits: &[Credit],
    keywords: &[Keyword],
) -> Vec<String> {
    ExperienceDimension::all()
        .into_iter()
        .filter(|d| {
            *d != ExperienceDimension::Comfort
                && film_in_dimension(*d, genres, credits, keywords, 0.0)
        })
        .map(|d| d.as_str().to_string())
        .collect()
}

pub fn derive(films: &[ModeFilm<'_>]) -> (Vec<TasteDimensionView>, Vec<TasteMode>, Vec<ModeShift>) {
    let rated: Vec<&ModeFilm<'_>> = films
        .iter()
        .filter(|f| {
            f.signal
                .map(|s| s.preference.affinity_preference > 0.0)
                .unwrap_or(false)
        })
        .collect();
    let recent_n = rated
        .iter()
        .filter(|f| f.age_years.map(|y| y <= RECENT_YEARS).unwrap_or(false))
        .count();
    let long_n = rated.len().saturating_sub(recent_n);
    let total_rec: f32 = rated
        .iter()
        .filter_map(|f| f.signal.map(|s| s.recommendation_weight))
        .sum();

    let mut dimensions = Vec::new();
    let mut modes = Vec::new();
    for dim in ExperienceDimension::all() {
        let members: Vec<&&ModeFilm<'_>> = rated
            .iter()
            .filter(|f| {
                let fam = f.signal.map(|s| s.familiarity_strength).unwrap_or(0.0);
                film_in_dimension(dim, f.genres, f.credits, f.keywords, fam)
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        let strength: f32 = members
            .iter()
            .filter_map(|f| f.signal.map(|s| s.recommendation_weight))
            .sum();
        let evidence: Vec<EvidenceFilm> = members
            .iter()
            .filter_map(|f| {
                Some(EvidenceFilm {
                    title: f.title.to_string(),
                    rating: f.rating?,
                    tmdb_id: f.tmdb_id,
                    people: f.credits.iter().map(|c| c.name.clone()).collect(),
                    keywords: f
                        .keywords
                        .iter()
                        .filter(|k| crate::taste::features::keyword_is_taste_signal(&k.name))
                        .map(|k| k.name.clone())
                        .collect(),
                    genres: f.genres.to_vec(),
                    year: None,
                    runtime: None,
                })
            })
            .take(6)
            .collect();
        let recent_w: f32 = members
            .iter()
            .filter(|f| f.age_years.map(|y| y <= RECENT_YEARS).unwrap_or(false))
            .filter_map(|f| f.signal.map(|s| s.recommendation_weight))
            .sum();
        let long_w = (strength - recent_w).max(0.0);
        let recent_share = if total_rec > 0.0 { recent_w / total_rec } else { 0.0 };
        let long_share = if total_rec > 0.0 { long_w / total_rec } else { 0.0 };
        dimensions.push(TasteDimensionView {
            name: dim.as_str().to_string(),
            strength,
            evidence: evidence.clone(),
            recent_share,
            long_term_share: long_share,
        });
        if members.len() >= MODE_MIN_MEMBERS {
            modes.push(TasteMode {
                dimension: dim.as_str().to_string(),
                strength,
                members: evidence,
                recent_share,
                long_term_share: long_share,
            });
        }
    }
    modes.sort_by(|a, b| {
        crate::taste::ord::cmp_f32_desc(a.strength, b.strength)
            .then_with(|| a.dimension.cmp(&b.dimension))
    });

    let mut mode_shifts = Vec::new();
    if recent_n >= SHIFT_RECENT_N && long_n >= SHIFT_LONG_N {
        for mode in &modes {
            let delta = mode.recent_share - mode.long_term_share;
            if delta.abs() >= MODE_SHIFT_DELTA {
                mode_shifts.push(ModeShift {
                    dimension: mode.dimension.clone(),
                    long_term: mode.long_term_share,
                    recent: mode.recent_share,
                    delta,
                });
            }
        }
    }

    (dimensions, modes, mode_shifts)
}

fn film_in_dimension(
    dim: ExperienceDimension,
    genres: &[String],
    credits: &[Credit],
    keywords: &[Keyword],
    familiarity: f32,
) -> bool {
    let has_genre = |name: &str| {
        genres
            .iter()
            .any(|g| g.eq_ignore_ascii_case(name))
    };
    let has_visual_kw = keywords.iter().any(|k| {
        let n = k.name.to_lowercase();
        VISUAL_KEYWORDS.iter().any(|v| n.contains(v))
    });
    let has_dp = credits.iter().any(|c| {
        family_for_job(&c.job) == Some(FeatureFamily::Cinematographer)
    });
    let has_writer = credits
        .iter()
        .any(|c| family_for_job(&c.job) == Some(FeatureFamily::Writer));
    match dim {
        ExperienceDimension::Visual => has_dp || has_visual_kw,
        ExperienceDimension::Story => has_writer || has_genre("Drama"),
        ExperienceDimension::Intensity => has_genre("Thriller") || has_genre("Horror"),
        ExperienceDimension::Comedy => has_genre("Comedy"),
        ExperienceDimension::Spectacle => {
            has_genre("Action") || has_genre("Science Fiction") || has_genre("Sci-Fi") || has_genre("Fantasy")
        }
        ExperienceDimension::Atmosphere => has_genre("Mystery") || has_visual_kw,
        ExperienceDimension::Comfort => familiarity >= COMFORT_FAMILIARITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::preference::{interaction_signal, rating_profile};

    fn film<'a>(
        title: &'a str,
        genres: &'a [String],
        credits: &'a [Credit],
        signal: &'a InteractionSignal,
        tmdb_id: i64,
    ) -> ModeFilm<'a> {
        ModeFilm {
            title,
            rating: Some(4.5),
            tmdb_id: Some(tmdb_id),
            genres,
            credits,
            keywords: &[],
            signal: Some(signal),
            age_years: Some(0.4),
        }
    }

    #[test]
    fn comedy_and_visual_modes_coexist() {
        let p = rating_profile(&[4.0; 8]).unwrap();
        let signal = interaction_signal(4.5, &p, Some(0.4), 1, false);
        let comedy = vec!["Comedy".into()];
        let crime = vec!["Crime".into()];
        let director = vec![Credit {
            id: Some(1),
            name: "Someone".into(),
            job: "Director".into(),
        }];
        let visual = vec![
            Credit {
                id: Some(1),
                name: "Someone".into(),
                job: "Director".into(),
            },
            Credit {
                id: Some(77),
                name: "Greig Fraser".into(),
                job: "Director of Photography".into(),
            },
        ];
        let titles: Vec<String> = (0..6).map(|i| format!("Laugh{i}")).chain((0..6).map(|i| format!("Look{i}"))).collect();
        let mut films = Vec::new();
        for i in 0..6 {
            films.push(film(&titles[i], &comedy, &director, &signal, i as i64));
        }
        for i in 0..6 {
            films.push(film(&titles[6 + i], &crime, &visual, &signal, 100 + i as i64));
        }
        let (_dims, modes, _) = derive(&films);
        let names: Vec<_> = modes.iter().map(|m| m.dimension.as_str()).collect();
        assert!(names.contains(&"comedy"), "{names:?}");
        assert!(names.contains(&"visual"), "{names:?}");
    }
}
