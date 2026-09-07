use crate::letterboxd::normalize::normalize_title;
use crate::storage::db::Database;
use rusqlite::{params, Transaction};

#[derive(Default)]
pub struct SourceMovieMeta {
    pub poster: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tmdb_media_type: Option<String>,
}

#[derive(Default)]
pub struct PosterBackfillReport {
    pub attempted: u32,
    pub updated: u32,
    /// Records handled in this catalog pass. A failed request is retried on the
    /// next pass after Enrich clears its temporary failure marker.
    pub resolved: u32,
}

pub fn poster_from_rss_body(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let mut search_from = 0;
    while let Some(img_start) = lower[search_from..].find("<img") {
        let idx = search_from + img_start;
        let tag_end = lower[idx..].find('>')? + idx;
        let tag = &body[idx..=tag_end];
        if let Some(src) = extract_attr(tag, "src") {
            if src.contains("ltrbxd.com") || src.starts_with("http") {
                return Some(src);
            }
        }
        search_from = tag_end + 1;
    }
    None
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    for quote in ['"', '\''] {
        let pattern = format!("{name}={quote}");
        let pattern_lower = pattern.to_lowercase();
        let Some(start) = lower.find(&pattern_lower).map(|index| index + pattern.len()) else {
            continue;
        };
        let rest = &tag[start..];
        if let Some(end) = rest.find(quote) {
            return Some(rest[..end].to_string());
        }
    }
    None
}

pub fn full_poster_url(path: &str) -> String {
    poster_url(Some(path.to_string())).unwrap_or_default()
}

pub fn poster_url(path: Option<String>) -> Option<String> {
    tmdb_image_url(path, "w780")
}

pub fn backdrop_url(path: Option<String>) -> Option<String> {
    tmdb_image_url(path, "original")
}

pub fn is_banner_quality(url: &str) -> bool {
    if url.contains("image.tmdb.org/t/p/original")
        || url.contains("image.tmdb.org/t/p/w1280")
        || url.contains("image.tmdb.org/t/p/w1920")
    {
        return true;
    }
    if url.contains("ltrbxd.com") && url.contains("/sm/upload/") {
        return letterboxd_crop_width(url).unwrap_or(0) >= 1000;
    }
    false
}

pub fn tmdb_image_url(path: Option<String>, size: &str) -> Option<String> {
    path.filter(|p| !p.is_empty()).map(|p| {
        if p.starts_with("http") {
            upgrade_remote_image(&p, size == "original")
        } else {
            let p = if p.starts_with('/') {
                p
            } else {
                format!("/{p}")
            };
            format!("https://image.tmdb.org/t/p/{size}{p}")
        }
    })
}

pub fn upgrade_remote_image(url: &str, for_banner: bool) -> String {
    let mut out = url.to_string();
    if out.contains("image.tmdb.org/t/p/") {
        let target = if for_banner { "original" } else { "w780" };
        for size in [
            "w92", "w154", "w185", "w342", "w500", "w300", "w780", "w1280",
        ] {
            let from = format!("/t/p/{size}");
            if !out.contains(&from) {
                continue;
            }
            if !for_banner && (size == "w780" || size == "w1280") {
                break;
            }
            out = out.replace(&from, &format!("/t/p/{target}"));
            break;
        }
        return out;
    }
    if out.contains("ltrbxd.com") {
        return upgrade_letterboxd_resized(&out, for_banner);
    }
    out
}

fn upgrade_letterboxd_resized(url: &str, for_banner: bool) -> String {
    let target = if for_banner {
        "-0-2000-0-3000-crop"
    } else {
        "-0-1000-0-1500-crop"
    };
    for crop in [
        "-0-70-0-105-crop",
        "-0-150-0-225-crop",
        "-0-230-0-345-crop",
        "-0-250-0-375-crop",
        "-0-500-0-750-crop",
        "-0-600-0-900-crop",
    ] {
        if url.contains(crop) {
            return url.replace(crop, target);
        }
    }
    url.to_string()
}

fn letterboxd_crop_width(url: &str) -> Option<u32> {
    let marker = "-0-";
    let start = url.find(marker)? + marker.len();
    let rest = &url[start..];
    let end = rest.find('-')?;
    rest[..end].parse().ok()
}

pub fn letterboxd_oembed_poster(uri: &str) -> Result<Option<String>, String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let page = if trimmed.starts_with("http") {
        trimmed.to_string()
    } else {
        format!("https://letterboxd.com{trimmed}")
    };
    let encoded = percent_encode(&page);
    let endpoints = [
        format!("https://letterboxd.com/services/oembed?url={encoded}"),
        format!("https://letterboxd.com/oembed?url={encoded}"),
    ];
    let mut last_err = None;
    for url in endpoints {
        match ureq::get(&url)
            .set("User-Agent", "Osvell/0.1 (local film app)")
            .timeout(std::time::Duration::from_secs(15))
            .call()
        {
            Ok(resp) => {
                let body = resp.into_string().map_err(|e| e.to_string())?;
                let v: serde_json::Value =
                    serde_json::from_str(&body).map_err(|e| e.to_string())?;
                return Ok(v["thumbnail_url"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| upgrade_remote_image(s, false)));
            }
            Err(ureq::Error::Status(code, _)) if code == 404 => return Ok(None),
            Err(err) => last_err = Some(err.to_string()),
        }
    }
    Err(last_err.unwrap_or_else(|| "Letterboxd oEmbed failed".into()))
}

/// Read public Letterboxd page metadata only for a Letterboxd-owned film URL.
/// Its social image supplies a poster fallback, and its TMDB movie link lets
/// the normal catalog hydrator fetch the rest of the film data from TMDB.
pub fn letterboxd_page_metadata(uri: &str) -> Result<SourceMovieMeta, String> {
    let Some(page) = letterboxd_page_url(uri) else {
        return Ok(SourceMovieMeta::default());
    };
    let body = ureq::get(&page)
        .set("User-Agent", "Osvell/0.1 (local film app)")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|error| error.to_string())?
        .into_string()
        .map_err(|error| error.to_string())?;
    Ok(letterboxd_page_metadata_from_html(&body))
}

fn letterboxd_page_url(uri: &str) -> Option<String> {
    let trimmed = uri.trim();
    if trimmed.starts_with('/') {
        return Some(format!("https://letterboxd.com{trimmed}"));
    }
    let lower = trimmed.to_ascii_lowercase();
    [
        "https://letterboxd.com/",
        "http://letterboxd.com/",
        "https://www.letterboxd.com/",
        "http://www.letterboxd.com/",
        "https://boxd.it/",
        "http://boxd.it/",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    .then(|| trimmed.to_string())
}

fn letterboxd_page_metadata_from_html(body: &str) -> SourceMovieMeta {
    let lower = body.to_lowercase();
    let mut poster = None;
    let mut search_from = 0;
    while let Some(meta_start) = lower[search_from..].find("<meta") {
        let start = search_from + meta_start;
        let Some(meta_end) = lower[start..].find('>').map(|offset| start + offset) else {
            break;
        };
        let tag = &body[start..=meta_end];
        let tag_lower = tag.to_lowercase();
        if tag_lower.contains("og:image") || tag_lower.contains("twitter:image") {
            poster = extract_attr(tag, "content")
                .filter(|url| !url.trim().is_empty())
                .map(|url| upgrade_remote_image(&url, false));
            if poster.is_some() {
                break;
            }
        }
        search_from = meta_end + 1;
    }

    let tmdb = tmdb_id_from_html(&lower);
    SourceMovieMeta {
        poster,
        tmdb_id: tmdb.map(|(_, id)| id),
        tmdb_media_type: tmdb.map(|(kind, _)| kind.to_string()),
    }
}

fn tmdb_id_from_html(lower_html: &str) -> Option<(&'static str, i64)> {
    let (kind, marker, offset) = [("movie", "themoviedb.org/movie/"), ("tv", "themoviedb.org/tv/")]
        .into_iter()
        .filter_map(|(kind, marker)| lower_html.find(marker).map(|offset| (kind, marker, offset)))
        .min_by_key(|(_, _, offset)| *offset)?;
    let start = offset + marker.len();
    let digits = lower_html[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| (kind, id))
}

/// Letterboxd oEmbed is a last resort when TMDB has no usable artwork.
/// Failed lookups are skipped for this pass and retried on the next one.
pub fn backfill_letterboxd_posters(
    db: &Database,
    limit: u32,
) -> Result<PosterBackfillReport, String> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, source_record_key, external_id, raw_identity, release_year
             FROM source_movie_records
             WHERE external_id IS NOT NULL AND TRIM(external_id) != ''
             AND (cached_poster_url IS NULL OR TRIM(cached_poster_url) = '')
             AND poster_fetch_failed = 0
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;

    let rows: Vec<(String, String, String, String, Option<i32>)> = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut report = PosterBackfillReport {
        attempted: rows.len() as u32,
        ..Default::default()
    };
    for (smr_id, key, uri, raw_identity, year) in rows {
        // A previous item in this batch may have propagated the same exact
        // title/year cover. Do not fetch or report that cached sibling again.
        if source_has_cached_poster(db.conn(), &smr_id)? {
            continue;
        }
        let title = raw_identity_title(&raw_identity);
        let page_metadata = letterboxd_page_metadata(&uri).unwrap_or_default();
        merge_source_movie_metadata(
            db.conn(),
            &key,
            None,
            page_metadata.tmdb_id,
            page_metadata.tmdb_media_type.as_deref(),
        )?;
        let poster = match page_metadata.poster {
            Some(poster) => Ok(Some(poster)),
            None => match letterboxd_oembed_poster(&uri) {
                Ok(Some(poster)) => Ok(Some(poster)),
                Ok(None) | Err(_) => wikipedia_film_poster(&title, year),
            },
        };
        match poster {
            Ok(Some(poster)) => {
                // The same film can appear through several imports or diary
                // entries. Once a trusted source resolves its poster, share it
                // only with exact normalized-title/year siblings.
                cache_poster_for_siblings(db.conn(), &smr_id, &poster)?;
                report.updated += 1;
                report.resolved += 1;
            }
            Ok(None) => {
                mark_poster_fetch_failed(db.conn(), &smr_id)?;
                report.resolved += 1;
            }
            Err(_) => {
                // Do not let one provider error block the remainder of the
                // library. The temporary marker is cleared before the next pass.
                mark_poster_fetch_failed(db.conn(), &smr_id)?;
                report.resolved += 1;
            }
        }
    }
    Ok(report)
}

/// A conservative, no-key final fallback for a film that both TMDB and
/// Letterboxd's oEmbed endpoint could not decorate. Wikipedia's public search
/// response includes a lead thumbnail; use it only when a single, exact film
/// article matches the source title/year and the returned image is portrait.
fn wikipedia_film_poster(title: &str, year: Option<i32>) -> Result<Option<String>, String> {
    let query = match year {
        Some(year) => format!("{title} {year}"),
        None => title.to_string(),
    };
    if query.trim().is_empty() {
        return Ok(None);
    }
    let url = format!(
        "https://en.wikipedia.org/w/rest.php/v1/search/page?q={}&limit=6",
        percent_encode(&query)
    );
    let body = ureq::get(&url)
        .set("User-Agent", "Osvell/0.1 (local film app)")
        .timeout(std::time::Duration::from_secs(12))
        .call()
        .map_err(|error| error.to_string())?
        .into_string()
        .map_err(|error| error.to_string())?;
    let Some(article_title) = wikipedia_film_article_from_response(&body, title, year) else {
        return Ok(None);
    };
    let image_url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&format=json&formatversion=2&prop=pageimages&piprop=thumbnail&pithumbsize=780&pilimit=1&pilicense=any&titles={}",
        percent_encode(&article_title)
    );
    let image_body = ureq::get(&image_url)
        .set("User-Agent", "Osvell/0.1 (local film app)")
        .timeout(std::time::Duration::from_secs(12))
        .call()
        .map_err(|error| error.to_string())?
        .into_string()
        .map_err(|error| error.to_string())?;
    Ok(wikipedia_poster_from_pageimage_response(&image_body))
}

fn wikipedia_film_article_from_response(
    body: &str,
    title: &str,
    year: Option<i32>,
) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let source_title = wiki_title_key(title);
    let mut matches = value["pages"]
        .as_array()?
        .iter()
        .filter_map(|page| {
            let page_title = page["title"].as_str()?;
            let description = page["description"].as_str().unwrap_or("");
            let title_matches = wiki_title_key(page_title) == source_title;
            let is_film = description.to_lowercase().contains("film");
            let has_year = year.is_none_or(|year| {
                let text = format!("{page_title} {description}");
                text.contains(&year.to_string())
            });
            (title_matches && is_film && has_year).then(|| page_title.to_string())
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn wikipedia_poster_from_pageimage_response(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let thumbnail = value["query"]["pages"].as_array()?.first()?["thumbnail"].clone();
    let url = thumbnail["source"].as_str()?.trim();
    let width = thumbnail["width"].as_i64()?;
    let height = thumbnail["height"].as_i64()?;
    (width >= 120 && height * 10 >= width * 12 && !url.is_empty()).then(|| url.to_string())
}

fn raw_identity_title(raw_identity: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw_identity)
        .ok()
        .and_then(|value| value["title"].as_str().map(str::to_string))
        .unwrap_or_else(|| raw_identity.to_string())
}

fn wiki_title_key(title: &str) -> String {
    normalize_title(title.split('(').next().unwrap_or(title))
        .chars()
        .map(|character| if character.is_alphanumeric() { character } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn cache_poster_for_smr(
    conn: &rusqlite::Connection,
    smr_id: &str,
    poster_url: &str,
) -> Result<(), String> {
    if poster_url.trim().is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE source_movie_records
         SET cached_poster_url = ?2,
             raw_identity = json_set(COALESCE(NULLIF(raw_identity, ''), '{}'), '$.poster', json_quote(?2))
         WHERE id = ?1",
        params![smr_id, poster_url.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn cache_poster_for_siblings(
    conn: &rusqlite::Connection,
    smr_id: &str,
    poster_url: &str,
) -> Result<(), String> {
    cache_poster_for_smr(conn, smr_id, poster_url)?;
    let (normalized, year): (String, Option<i32>) = conn
        .query_row(
            "SELECT normalized_title, release_year FROM source_movie_records WHERE id = ?1",
            params![smr_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE source_movie_records
         SET cached_poster_url = ?3,
             raw_identity = json_set(COALESCE(NULLIF(raw_identity, ''), '{}'), '$.poster', json_quote(?3))
         WHERE normalized_title = ?1 AND release_year IS ?2
         AND (cached_poster_url IS NULL OR TRIM(cached_poster_url) = '')",
        params![normalized, year, poster_url.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn source_has_cached_poster(conn: &rusqlite::Connection, smr_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM source_movie_records
           WHERE id = ?1 AND COALESCE(TRIM(cached_poster_url), '') != ''
         )",
        params![smr_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|error| error.to_string())
}

fn mark_poster_fetch_failed(conn: &rusqlite::Connection, smr_id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE source_movie_records SET poster_fetch_failed = 1 WHERE id = ?1",
        params![smr_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn merge_source_movie_metadata(
    conn: &rusqlite::Connection,
    source_record_key: &str,
    poster: Option<&str>,
    tmdb_id: Option<i64>,
    tmdb_media_type: Option<&str>,
) -> Result<(), String> {
    if poster.is_none() && tmdb_id.is_none() && tmdb_media_type.is_none() {
        return Ok(());
    }

    if let Some(url) = poster.filter(|p| !p.trim().is_empty()) {
        conn.execute(
            "UPDATE source_movie_records
             SET cached_poster_url = ?2,
                 raw_identity = json_set(
                   COALESCE(NULLIF(raw_identity, ''), '{}'),
                   '$.poster', json_quote(?2)
                 )
             WHERE source_record_key = ?1
             AND (cached_poster_url IS NULL OR TRIM(cached_poster_url) = '')",
            params![source_record_key, url],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(id) = tmdb_id {
        conn.execute(
            "UPDATE source_movie_records
             SET raw_identity = json_set(
               COALESCE(NULLIF(raw_identity, ''), '{}'),
               '$.tmdb_id', json(?2)
             )
             WHERE source_record_key = ?1",
            params![source_record_key, id],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(kind) = tmdb_media_type.filter(|kind| matches!(*kind, "movie" | "tv")) {
        conn.execute(
            "UPDATE source_movie_records
             SET raw_identity = json_set(
               COALESCE(NULLIF(raw_identity, ''), '{}'),
               '$.tmdb_media_type', json_quote(?2)
             )
             WHERE source_record_key = ?1",
            params![source_record_key, kind],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub fn merge_source_movie_metadata_tx(
    tx: &Transaction<'_>,
    source_record_key: &str,
    poster: Option<&str>,
    tmdb_id: Option<i64>,
    tmdb_media_type: Option<&str>,
) -> Result<(), String> {
    merge_source_movie_metadata(tx, source_record_key, poster, tmdb_id, tmdb_media_type)
}

pub fn parse_tmdb_id(raw: &str) -> Option<i64> {
    if raw.trim().is_empty() {
        return None;
    }
    raw.trim().parse().ok().filter(|&id| id > 0)
}

pub fn tmdb_id_from_rss_body(body: &str) -> Option<i64> {
    for name in ["filmId", "movieId", "tmdbId", "tmdbMovieId"] {
        if let Some(raw) = rss_tag(body, name) {
            if let Some(id) = parse_tmdb_id(&raw) {
                return Some(id);
            }
        }
    }
    None
}

fn rss_tag(xml: &str, name: &str) -> Option<String> {
    for marker in [name, &format!("letterboxd:{name}")] {
        let open = format!("<{marker}");
        let lower = xml.to_lowercase();
        let start = lower.find(&open.to_lowercase())?;
        let after = &xml[start..];
        let content_start = after.find('>')? + 1;
        let rest = &after[content_start..];
        let close = format!("</{marker}>");
        let end = rest.to_lowercase().find(&close.to_lowercase())?;
        return Some(rest[..end].trim().to_string());
    }
    None
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::letterboxd::import::upsert_source_movie;
    use crate::storage::db::Database;

    #[test]
    fn extracts_poster_from_rss_description() {
        let body = r#"<description><![CDATA[<p><img src="https://a.ltrbxd.com/resized/film-poster/240/240/70/68/abc.jpg" alt="Ant-Man"/></p>]]></description>"#;
        assert_eq!(
            poster_from_rss_body(body),
            Some("https://a.ltrbxd.com/resized/film-poster/240/240/70/68/abc.jpg".into())
        );
    }

    #[test]
    fn reads_poster_and_tmdb_id_from_letterboxd_page_metadata() {
        let html = r#"
          <meta property="og:image" content="https://a.ltrbxd.com/resized/film-poster/1/2/3/film-0-230-0-345-crop.jpg">
          <a href="https://www.themoviedb.org/movie/12345-example">TMDB</a>
        "#;
        let metadata = letterboxd_page_metadata_from_html(html);
        assert_eq!(metadata.tmdb_id, Some(12345));
        assert_eq!(metadata.tmdb_media_type.as_deref(), Some("movie"));
        assert_eq!(
            metadata.poster.as_deref(),
            Some("https://a.ltrbxd.com/resized/film-poster/1/2/3/film-0-1000-0-1500-crop.jpg")
        );
        assert_eq!(
            letterboxd_page_url("https://example.test/not-letterboxd"),
            None
        );
        assert_eq!(
            tmdb_id_from_html(r#"<a href="https://www.themoviedb.org/tv/700">TMDB</a>"#),
            Some(("tv", 700))
        );
    }

    #[test]
    fn caches_http_poster_url_on_source_record() {
        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        let smr_id = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|inception",
            "Inception",
            Some(2010),
            "https://letterboxd.com/film/inception/",
            &SourceMovieMeta::default(),
        )
        .expect("smr");
        tx.commit().expect("commit");

        let poster = "https://image.tmdb.org/t/p/w500/9gk7adHYeDvHkCSEqAvQNLV5Uge.jpg";
        cache_poster_for_smr(db.conn(), &smr_id, poster).expect("cache poster URL");

        let cached: String = db
            .conn()
            .query_row(
                "SELECT cached_poster_url FROM source_movie_records WHERE id = ?1",
                rusqlite::params![smr_id],
                |row| row.get(0),
            )
            .expect("cached url");
        assert_eq!(cached, poster);

        let identity: String = db
            .conn()
            .query_row(
                "SELECT json_extract(raw_identity, '$.poster') FROM source_movie_records WHERE id = ?1",
                rusqlite::params![smr_id],
                |row| row.get(0),
            )
            .expect("identity poster");
        assert_eq!(identity, poster);
    }

    #[test]
    fn caches_a_verified_poster_for_exact_title_year_siblings() {
        let mut db = Database::in_memory().expect("db");
        let tx = db.transaction().expect("tx");
        let first = upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|obsession|one",
            "Obsession",
            Some(2025),
            "https://boxd.it/one",
            &SourceMovieMeta::default(),
        )
        .expect("first record");
        upsert_source_movie(
            &tx,
            "letterboxd_export",
            "export|obsession|two",
            "Obsession",
            Some(2025),
            "https://boxd.it/two",
            &SourceMovieMeta::default(),
        )
        .expect("sibling record");
        tx.commit().expect("commit");

        let poster = "https://example.test/obsession.jpg";
        cache_poster_for_siblings(db.conn(), &first, poster).expect("cache poster");
        let cached: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_movie_records WHERE cached_poster_url = ?1",
                rusqlite::params![poster],
                |row| row.get(0),
            )
            .expect("count cached records");
        assert_eq!(cached, 2);
    }

    #[test]
    fn percent_encodes_utf8_bytes() {
        assert_eq!(percent_encode("Amélie"), "Am%C3%A9lie");
    }

    #[test]
    fn wikipedia_fallback_requires_one_exact_film_article_and_a_portrait_pageimage() {
        let response = r#"{
          "pages": [{
            "title": "Talk to Me (2022 film)",
            "description": "2022 Australian supernatural horror film"
          }]
        }"#;
        assert_eq!(
            wikipedia_film_article_from_response(response, "Talk to Me", Some(2022)),
            Some("Talk to Me (2022 film)".into())
        );
        assert!(wikipedia_film_article_from_response(response, "Talk to You", Some(2022)).is_none());

        let pageimage = r#"{
          "query": {"pages": [{
            "thumbnail": {"source": "https://upload.wikimedia.org/poster.jpg", "width": 500, "height": 750}
          }]}
        }"#;
        assert_eq!(
            wikipedia_poster_from_pageimage_response(pageimage),
            Some("https://upload.wikimedia.org/poster.jpg".into())
        );
        let not_a_poster = pageimage.replace("\"height\": 750", "\"height\": 281");
        assert!(wikipedia_poster_from_pageimage_response(&not_a_poster).is_none());
    }

    #[test]
    fn upgrades_small_tmdb_and_letterboxd_urls() {
        assert_eq!(
            upgrade_remote_image(
                "https://image.tmdb.org/t/p/w342/abc.jpg",
                true
            ),
            "https://image.tmdb.org/t/p/original/abc.jpg"
        );
        assert_eq!(
            upgrade_remote_image(
                "https://a.ltrbxd.com/resized/film-poster/1/2/3/inception-0-230-0-345-crop.jpg",
                false
            ),
            "https://a.ltrbxd.com/resized/film-poster/1/2/3/inception-0-1000-0-1500-crop.jpg"
        );
        assert!(is_banner_quality(
            "https://image.tmdb.org/t/p/original/backdrop.jpg"
        ));
        assert!(!is_banner_quality(
            "https://a.ltrbxd.com/resized/film-poster/1/2/3/inception-0-230-0-345-crop.jpg"
        ));
    }
}
