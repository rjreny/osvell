use super::fingerprint::{row_fingerprint, source_record_key};
use super::import::upsert_source_movie;
use super::normalize::{normalize_title, parse_year};
use super::posters::{poster_from_rss_body, tmdb_id_from_rss_body, SourceMovieMeta};
use crate::models::SyncResult;
use crate::storage::db::Database;
use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

pub fn rss_url(username: &str) -> String {
    let clean = username.trim().trim_start_matches('@').to_lowercase();
    format!("https://letterboxd.com/{clean}/rss/")
}

pub fn sync_rss(db: &mut Database, username: &str, xml: &str) -> Result<SyncResult, String> {
    let feed_url = rss_url(username);
    let now = Utc::now().to_rfc3339();
    db.set_meta("self_username", username)?;
    let tx = db.transaction()?;
    let mut added = 0u32;
    let mut changed = false;
    let items = parse_items(xml);
    let entries_seen = items.len() as u32;

    for item in items {
        let guid = item.guid.clone().unwrap_or_else(|| {
            row_fingerprint(&[
                ("link", &item.link),
                ("title", &item.title),
                ("published", &item.published),
            ])
        });
        let event_fp = row_fingerprint(&[("guid", &guid), ("feed", &feed_url)]);
        let viewing_key = source_record_key("letterboxd_rss", &feed_url, &event_fp);
        let rating_key = format!("{viewing_key}|rating");

        let existing_smr: Option<String> = tx
            .query_row(
                "SELECT source_movie_record_id FROM viewings WHERE source_record_key = ?1",
                params![viewing_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        if let Some(smr_id) = existing_smr {
            if let Some(rating) = item.rating {
                if upsert_rating(
                    &tx,
                    &smr_id,
                    &rating_key,
                    rating,
                    item.watched_date.as_deref(),
                    &item.published,
                    &now,
                )? {
                    changed = true;
                }
            }
            continue;
        }

        if let Some(smr_id) = matching_export_viewing(&tx, &item)? {
            if let Some(rating) = item.rating {
                if upsert_rating(
                    &tx,
                    &smr_id,
                    &rating_key,
                    rating,
                    item.watched_date.as_deref(),
                    &item.published,
                    &now,
                )? {
                    changed = true;
                }
            }
            continue;
        }

        let movie_fp = row_fingerprint(&[
            ("name", &item.film_title),
            (
                "year",
                &item.year.map(|y| y.to_string()).unwrap_or_default(),
            ),
            ("uri", &item.link),
        ]);
        let movie_key = source_record_key("letterboxd_rss", "film", &movie_fp);
        let meta = SourceMovieMeta {
            poster: item.poster.clone(),
            tmdb_id: item.tmdb_id,
            ..Default::default()
        };
        let smr_id = upsert_source_movie(
            &tx,
            "letterboxd_rss",
            &movie_key,
            &item.film_title,
            item.year,
            &item.link,
            &meta,
        )?;

        tx.execute(
            "INSERT INTO viewings(
              id, source_movie_record_id, source_record_key, occurred_at, published_at,
              observed_at, imported_at, source_type, import_id, diary_entry_id, rewatch, raw_payload
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'letterboxd_rss', NULL, ?8, 0, ?9)",
            params![
                Uuid::new_v4().to_string(),
                smr_id,
                viewing_key,
                item.watched_date,
                item.published,
                now,
                now,
                guid,
                item.raw
            ],
        )
        .map_err(|e| e.to_string())?;

        if let Some(rating) = item.rating {
            upsert_rating(
                &tx,
                &smr_id,
                &rating_key,
                rating,
                item.watched_date.as_deref(),
                &item.published,
                &now,
            )?;
        }
        added += 1;
        changed = true;
    }

    if changed {
        Database::rebuild_projections(&tx)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    db.set_meta("last_self_rss_sync_at", &now)?;
    db.set_meta("last_rss_sync_at", &now)?;

    let coverage = db.compute_coverage()?;
    Ok(SyncResult {
        username: username.to_string(),
        entries_seen,
        entries_added: added,
        coverage,
    })
}

fn matching_export_viewing(tx: &Transaction<'_>, item: &RssItem) -> Result<Option<String>, String> {
    let Some(watched_date) = item.watched_date.as_deref().filter(|date| !date.is_empty()) else {
        return Ok(None);
    };
    tx.query_row(
        "SELECT v.source_movie_record_id
         FROM viewings v
         JOIN source_movie_records smr ON smr.id = v.source_movie_record_id
         WHERE v.source_type = 'letterboxd_export'
           AND smr.normalized_title = ?1 AND smr.release_year IS ?2
           AND v.occurred_at = ?3
         LIMIT 1",
        params![normalize_title(&item.film_title), item.year, watched_date],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn sync_friend_rss(
    db: &mut Database,
    friend_id: &str,
    username: &str,
    xml: &str,
) -> Result<u32, String> {
    let feed_url = rss_url(username);
    let tx = db.transaction()?;
    let mut added = 0u32;

    for item in parse_items(xml) {
        let guid = item
            .guid
            .clone()
            .unwrap_or_else(|| row_fingerprint(&[("link", &item.link), ("title", &item.title)]));
        let event_fp = row_fingerprint(&[("guid", &guid), ("feed", &feed_url)]);
        let activity_key = source_record_key("letterboxd_rss", &feed_url, &event_fp);
        let movie_fp = row_fingerprint(&[
            ("name", &item.film_title),
            (
                "year",
                &item.year.map(|y| y.to_string()).unwrap_or_default(),
            ),
            ("uri", &item.link),
        ]);
        let movie_key = source_record_key("letterboxd_rss", "film", &movie_fp);
        let meta = SourceMovieMeta {
            poster: item.poster.clone(),
            tmdb_id: item.tmdb_id,
            ..Default::default()
        };
        let smr = upsert_source_movie(
            &tx,
            "letterboxd_rss",
            &movie_key,
            &item.film_title,
            item.year,
            &item.link,
            &meta,
        )?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM friend_activity WHERE source_record_key = ?1",
                params![activity_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(id) = existing {
            tx.execute(
                "UPDATE friend_activity
                 SET rating = ?2, review = COALESCE(?3, review), raw_payload = ?4,
                     poster_url = COALESCE(?5, poster_url),
                     source_movie_record_id = COALESCE(source_movie_record_id, ?6)
                 WHERE id = ?1",
                params![id, item.rating, item.review, item.raw, item.poster, smr],
            )
            .map_err(|e| e.to_string())?;
            continue;
        }

        tx.execute(
            "INSERT INTO friend_activity(
              id, friend_id, source_movie_record_id, source_record_key, activity_type,
              published_at, watched_at, rating, review, source_guid, raw_payload, poster_url
            ) VALUES (?1, ?2, ?3, ?4, 'diary', ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                Uuid::new_v4().to_string(),
                friend_id,
                smr,
                activity_key,
                item.published,
                item.watched_date,
                item.rating,
                item.review,
                guid,
                item.raw,
                item.poster
            ],
        )
        .map_err(|e| e.to_string())?;
        added += 1;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(added)
}

fn upsert_rating(
    tx: &Transaction<'_>,
    smr_id: &str,
    rating_key: &str,
    rating: f64,
    watched_date: Option<&str>,
    published: &str,
    now: &str,
) -> Result<bool, String> {
    let existing: Option<f64> = tx
        .query_row(
            "SELECT rating FROM rating_events WHERE source_record_key = ?1",
            params![rating_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing == Some(rating) {
        return Ok(false);
    }
    if existing.is_some() {
        tx.execute(
            "UPDATE rating_events
             SET rating = ?2, occurred_at = ?3, published_at = ?4, observed_at = ?5
             WHERE source_record_key = ?1",
            params![rating_key, rating, watched_date, published, now],
        )
        .map_err(|e| e.to_string())?;
        return Ok(true);
    }
    tx.execute(
        "INSERT INTO rating_events(
          id, source_movie_record_id, source_record_key, rating,
          occurred_at, published_at, observed_at, imported_at, source_type, import_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'letterboxd_rss', NULL)",
        params![
            Uuid::new_v4().to_string(),
            smr_id,
            rating_key,
            rating,
            watched_date,
            published,
            now,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

pub(crate) struct RssItem {
    guid: Option<String>,
    title: String,
    film_title: String,
    year: Option<i32>,
    link: String,
    published: String,
    watched_date: Option<String>,
    rating: Option<f64>,
    poster: Option<String>,
    tmdb_id: Option<i64>,
    review: Option<String>,
    raw: String,
}

pub(crate) fn parse_items(xml: &str) -> Vec<RssItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let body = chunk.split("</item>").next()?;
            let film_title = tag(body, "filmTitle").unwrap_or_default();
            let title = tag(body, "title").unwrap_or_default();
            let link = tag(body, "link").unwrap_or_default();
            // A Letterboxd RSS feed also contains list activity. It has no
            // filmTitle and no /film/ URL, so it must not become a faux movie
            // record that later appears as a missing-poster failure.
            if film_title.is_empty() && !is_film_link(&link) {
                return None;
            }
            let name = if film_title.is_empty() {
                title.split(" - ").next()?.trim().to_string()
            } else {
                film_title
            };
            if name.is_empty() {
                return None;
            }
            let year_raw = tag(body, "filmYear");
            let year = year_raw.as_deref().and_then(parse_year).or_else(|| {
                title
                    .chars()
                    .collect::<String>()
                    .split(',')
                    .last()
                    .and_then(|y| parse_year(y.trim()))
            });
            let rating_raw = tag(body, "memberRating");
            let rating = rating_raw
                .as_deref()
                .and_then(|v| v.parse().ok())
                .or_else(|| stars_from_title(&title));
            Some(RssItem {
                guid: tag(body, "guid"),
                title,
                film_title: decode(&name),
                year,
                link,
                published: tag(body, "pubDate").unwrap_or_default(),
                watched_date: tag(body, "watchedDate"),
                rating,
                poster: poster_from_rss_body(body),
                tmdb_id: tmdb_id_from_rss_body(body),
                review: tag(body, "description").and_then(|d| review_from_description(&d)),
                raw: body.to_string(),
            })
        })
        .collect()
}

fn is_film_link(link: &str) -> bool {
    link.to_lowercase().contains("/film/")
}

fn tag(xml: &str, name: &str) -> Option<String> {
    for marker in [name, &format!("letterboxd:{name}")] {
        let open = format!("<{marker}");
        let xml_lower = xml.to_lowercase();
        let marker_lower = open.to_lowercase();
        let Some(start) = xml_lower.find(&marker_lower) else {
            continue;
        };
        let after = &xml[start..];
        let content_start = after.find('>').map(|i| i + 1)?;
        let rest = &after[content_start..];
        let close = format!("</{marker}>");
        let end = rest.to_lowercase().find(&close.to_lowercase())?;
        return Some(decode(&rest[..end]));
    }
    None
}

fn decode(value: &str) -> String {
    value
        .replace("<![CDATA[", "")
        .replace("]]>", "")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace('★', "")
        .trim()
        .to_string()
}

/// Parse a friend-activity or viewing payload into display title and year.
pub fn parse_activity_payload(raw: &str) -> (String, Option<i32>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(title) = v.get("title").and_then(|t| t.as_str()) {
            let year = v.get("year").and_then(|y| y.as_i64()).map(|y| y as i32);
            return (title.to_string(), year);
        }
    }

    let film_title = tag(raw, "filmTitle").unwrap_or_default();
    let title_tag = tag(raw, "title").unwrap_or_default();
    let name = if film_title.is_empty() {
        title_tag
            .split(" - ")
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        film_title
    };
    let name = decode(&name);
    if name.is_empty() {
        return (raw.trim().to_string(), None);
    }
    let year = tag(raw, "filmYear")
        .and_then(|y| parse_year(&y))
        .or_else(|| {
            title_tag
                .split(',')
                .last()
                .and_then(|y| parse_year(y.trim()))
        });
    (name, year)
}

fn review_from_description(html: &str) -> Option<String> {
    let decoded = decode(html);
    let mut plain = String::new();
    let mut in_tag = false;
    for c in decoded.chars() {
        match c {
            '<' => {
                if !in_tag && !plain.is_empty() && !plain.ends_with('\n') {
                    plain.push('\n');
                }
                in_tag = true;
            }
            '>' => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let text = plain
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .filter(|line| !line.to_ascii_lowercase().starts_with("watched on "))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn stars_from_title(title: &str) -> Option<f64> {
    let stars: String = title.chars().filter(|&c| c == '★').collect();
    if stars.is_empty() {
        return None;
    }
    let half = title.contains('½') || title.contains("1/2");
    Some(stars.len() as f64 + if half { 0.5 } else { 0.0 })
}

pub fn unique_movie_count(db: &Database) -> Result<u32, String> {
    db.conn()
        .query_row(
            "SELECT COUNT(DISTINCT smr.id) FROM source_movie_records smr
             WHERE EXISTS (SELECT 1 FROM viewings v WHERE v.source_movie_record_id = smr.id)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::letterboxd::import::count_events;
    use crate::storage::db::Database;

    fn fixture_50_47() -> String {
        let mut xml = String::from(r#"<?xml version="1.0"?><rss><channel>"#);
        for i in 0..47 {
            xml.push_str(&format!(
                r#"<item><title>Film {i} - ★★★★</title><link>https://letterboxd.com/film/f{i}/</link>
                <guid isPermaLink="false">guid-{i}</guid><pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate>
                <letterboxd:filmTitle>Film {i}</letterboxd:filmTitle><letterboxd:filmYear>2020</letterboxd:filmYear>
                <letterboxd:watchedDate>2024-01-01</letterboxd:watchedDate>
                <letterboxd:memberRating>4.0</letterboxd:memberRating></item>"#
            ));
        }
        for (idx, film) in [(0, 0), (1, 1), (2, 2)] {
            xml.push_str(&format!(
                r#"<item><title>Film {film} - ★★★★★</title><link>https://letterboxd.com/film/f{film}/</link>
                <guid isPermaLink="false">guid-rewatch-{idx}</guid><pubDate>Mon, 02 Jan 2024 00:00:00 GMT</pubDate>
                <letterboxd:filmTitle>Film {film}</letterboxd:filmTitle><letterboxd:filmYear>2020</letterboxd:filmYear>
                <letterboxd:watchedDate>2024-02-01</letterboxd:watchedDate>
                <letterboxd:memberRating>5.0</letterboxd:memberRating>
                <letterboxd:rewatch>Yes</letterboxd:rewatch></item>"#
            ));
        }
        xml.push_str("</channel></rss>");
        xml
    }

    #[test]
    fn parse_activity_payload_reads_rss_item() {
        let raw = r#"<title>Ant-Man, 2015 - ★★★★</title>
        <letterboxd:filmTitle>Ant-Man</letterboxd:filmTitle>
        <letterboxd:filmYear>2015</letterboxd:filmYear>"#;
        let (title, year) = parse_activity_payload(raw);
        assert_eq!(title, "Ant-Man");
        assert_eq!(year, Some(2015));
    }

    #[test]
    fn rss_ignores_non_film_list_activity() {
        let xml = r#"<?xml version="1.0"?><rss><channel>
            <item><title>Most Excited For in 2026</title>
            <link>https://letterboxd.com/example/list/most-excited-for-in-2026/</link>
            <guid>list-1</guid><pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate></item>
        </channel></rss>"#;
        assert!(parse_items(xml).is_empty());
    }

    #[test]
    fn rss_preserves_50_events_47_movies() {
        let mut db = Database::in_memory().unwrap();
        let xml = fixture_50_47();
        let result = sync_rss(&mut db, "testuser", &xml).unwrap();
        assert_eq!(result.entries_added, 50);
        let (viewings, smr, _) = count_events(&db).unwrap();
        assert_eq!(viewings, 50);
        assert_eq!(smr, 47);
    }

    #[test]
    fn rss_updates_rating_on_existing_diary_guid() {
        let mut db = Database::in_memory().unwrap();
        let first = r#"<?xml version="1.0"?><rss><channel>
            <item><title>Heat - ★★★</title><link>https://letterboxd.com/film/heat/</link>
            <guid isPermaLink="false">guid-heat</guid><pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate>
            <letterboxd:filmTitle>Heat</letterboxd:filmTitle><letterboxd:filmYear>1995</letterboxd:filmYear>
            <letterboxd:watchedDate>2024-01-01</letterboxd:watchedDate>
            <letterboxd:memberRating>3.0</letterboxd:memberRating></item>
            </channel></rss>"#;
        sync_rss(&mut db, "me", first).unwrap();
        let later = first.replace("3.0", "5.0").replace("★★★<", "★★★★★<");
        let result = sync_rss(&mut db, "me", &later).unwrap();
        assert_eq!(result.entries_added, 0);
        let rating: f64 = db
            .conn()
            .query_row("SELECT rating FROM rating_events LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rating, 5.0);
    }

    #[test]
    fn review_strips_watched_on_boilerplate() {
        let html = "<p>Watched on Monday January 1, 2024.</p><p>Pretty good actually.</p>";
        assert_eq!(
            review_from_description(html).as_deref(),
            Some("Pretty good actually.")
        );
    }
}
