use super::rss::{rss_url, sync_friend_rss, sync_rss};
use crate::jobs::JobSlot;
use crate::models::{FeedSyncReport, JobProgress};
use crate::storage::db::Database;
use crate::{fetch_rss, RssFetch};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const AUTO_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(30 * 60);
pub const AUTO_SYNC_PERIOD: Duration = Duration::from_secs(60 * 60);
pub const FEED_REQUEST_GAP: Duration = Duration::from_millis(1600);
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);
pub const FORBIDDEN_BACKOFF: Duration = Duration::from_secs(24 * 60 * 60);

const META_LAST_SYNC: &str = "last_rss_sync_at";
const META_SELF_SYNC: &str = "last_self_rss_sync_at";
const META_BACKOFF: &str = "rss_backoff_until";

pub fn is_due(
    last_rfc3339: Option<&str>,
    now: DateTime<Utc>,
    min_interval: Duration,
) -> bool {
    let Some(raw) = last_rfc3339.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let Ok(parsed) = DateTime::parse_from_rfc3339(raw) else {
        return true;
    };
    let elapsed = now.signed_duration_since(parsed.with_timezone(&Utc));
    elapsed >= ChronoDuration::from_std(min_interval).unwrap_or(ChronoDuration::minutes(30))
}

pub fn is_paused(backoff_until: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(raw) = backoff_until.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Ok(parsed) = DateTime::parse_from_rfc3339(raw) else {
        return false;
    };
    parsed.with_timezone(&Utc) > now
}

pub fn start_scheduler(app: AppHandle, slot: JobSlot, db_path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("studio-feeds".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(8));
            loop {
                loop {
                    match spawn_feed_sync(app.clone(), slot.clone(), db_path.clone(), false) {
                        Ok(_) => break,
                        Err(err) if err.to_ascii_lowercase().contains("already running") => {
                            std::thread::sleep(Duration::from_secs(45));
                        }
                        Err(_) => break,
                    }
                }
                std::thread::sleep(AUTO_SYNC_PERIOD);
            }
        });
}

pub fn spawn_feed_sync(
    app: AppHandle,
    slot: JobSlot,
    db_path: PathBuf,
    force: bool,
) -> Result<bool, String> {
    {
        let db = Database::open(&db_path)?;
        if !force && !work_is_due(&db)? {
            return Ok(false);
        }
        if self_username(&db)?.is_none() && enabled_friends(&db)?.is_empty() {
            return Ok(false);
        }
    }
    let quiet = !force;
    crate::jobs::spawn_job(app, slot, db_path, "feeds", quiet, move |app, db_path| {
        let mut db = crate::jobs::open_worker_db(&db_path)?;
        let report = run_feed_sync(&mut db, force, |label, current, total| {
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "feeds".into(),
                    label: label.to_string(),
                    current,
                    total,
                    quiet,
                    ..Default::default()
                },
            );
        })?;
        let label = if report.skipped {
            if report.paused_until.is_some() {
                "Diary RSS paused — Letterboxd asked us to wait".into()
            } else {
                "Diary RSS already up to date".into()
            }
        } else {
            let mut parts = Vec::new();
            if report.self_synced {
                parts.push("your diary".into());
            }
            if report.friends_synced > 0 {
                parts.push(format!(
                    "{} friend feed{}",
                    report.friends_synced,
                    if report.friends_synced == 1 { "" } else { "s" }
                ));
            }
            if parts.is_empty() {
                "No public diary feeds to refresh".into()
            } else {
                format!(
                    "Refreshed {} · {} new",
                    parts.join(" + "),
                    report.entries_added
                )
            }
        };
        let _ = app.emit(
            "studio-job",
            JobProgress {
                job: "feeds".into(),
                label,
                current: 1,
                total: 1,
                errors: report.errors.len() as u32,
                done: true,
                quiet,
                feeds: Some(report),
                ..Default::default()
            },
        );
        Ok(())
    })?;
    Ok(true)
}

pub fn run_feed_sync(
    db: &mut Database,
    force: bool,
    mut on_progress: impl FnMut(&str, u32, u32),
) -> Result<FeedSyncReport, String> {
    let now = Utc::now();
    let paused_until = db.get_meta(META_BACKOFF)?;
    if is_paused(paused_until.as_deref(), now) {
        return Ok(FeedSyncReport {
            skipped: true,
            last_sync_at: db.get_meta(META_LAST_SYNC)?,
            paused_until,
            ..Default::default()
        });
    }

    let username = self_username(db)?;
    let friends = enabled_friends(db)?;
    let self_due = username.is_some()
        && (force
            || is_due(
                db.get_meta(META_SELF_SYNC)?.as_deref(),
                now,
                AUTO_SYNC_MIN_INTERVAL,
            ));
    let friend_due: Vec<(String, String)> = friends
        .into_iter()
        .filter(|(_, _, last)| force || is_due(last.as_deref(), now, AUTO_SYNC_MIN_INTERVAL))
        .map(|(id, name, _)| (id, name))
        .collect();

    if !self_due && friend_due.is_empty() {
        return Ok(FeedSyncReport {
            skipped: true,
            last_sync_at: db.get_meta(META_LAST_SYNC)?,
            paused_until: None,
            ..Default::default()
        });
    }

    let total = u32::from(self_due) + friend_due.len() as u32;
    let mut current = 0u32;
    let mut report = FeedSyncReport::default();
    let mut gap = false;

    if self_due {
        if let Some(name) = username.clone() {
            current += 1;
            on_progress(&format!("Refreshing @{name}…"), current, total.max(1));
            maybe_gap(&mut gap);
            match pull_feed(db, &name)? {
                FeedPull::Xml(xml) => {
                    let result = sync_rss(db, &name, &xml)?;
                    report.self_synced = true;
                    report.entries_added += result.entries_added;
                }
                FeedPull::NotModified => {
                    report.self_synced = true;
                    let now_s = Utc::now().to_rfc3339();
                    db.set_meta(META_SELF_SYNC, &now_s)?;
                    db.set_meta(META_LAST_SYNC, &now_s)?;
                }
                FeedPull::Stop { until, reason } => {
                    db.set_meta(META_BACKOFF, &until)?;
                    report.paused_until = Some(until);
                    report.errors.push(reason);
                    report.last_sync_at = db.get_meta(META_LAST_SYNC)?;
                    return Ok(report);
                }
            }
        }
    }

    for (friend_id, name) in friend_due {
        current += 1;
        on_progress(
            &format!("Refreshing @{name} · {current}/{}", total.max(1)),
            current,
            total.max(1),
        );
        maybe_gap(&mut gap);
        match pull_feed(db, &name)? {
            FeedPull::Xml(xml) => match sync_friend_rss(db, &friend_id, &name, &xml) {
                Ok(added) => {
                    report.friends_synced += 1;
                    report.entries_added += added;
                    let now_s = Utc::now().to_rfc3339();
                    db.conn()
                        .execute(
                            "UPDATE friends SET last_sync_at = ?2, last_sync_error = NULL WHERE id = ?1",
                            rusqlite::params![friend_id, now_s],
                        )
                        .map_err(|e| e.to_string())?;
                    db.set_meta(META_LAST_SYNC, &now_s)?;
                }
                Err(err) => {
                    report.errors.push(format!("@{name}: {err}"));
                    let _ = db.conn().execute(
                        "UPDATE friends SET last_sync_error = ?2 WHERE id = ?1",
                        rusqlite::params![friend_id, err],
                    );
                }
            },
            FeedPull::NotModified => {
                report.friends_synced += 1;
                let now_s = Utc::now().to_rfc3339();
                db.conn()
                    .execute(
                        "UPDATE friends SET last_sync_at = ?2, last_sync_error = NULL WHERE id = ?1",
                        rusqlite::params![friend_id, now_s],
                    )
                    .map_err(|e| e.to_string())?;
                db.set_meta(META_LAST_SYNC, &now_s)?;
            }
            FeedPull::Stop { until, reason } => {
                db.set_meta(META_BACKOFF, &until)?;
                report.paused_until = Some(until);
                report.errors.push(format!("@{name}: {reason}"));
                break;
            }
        }
    }

    report.last_sync_at = db.get_meta(META_LAST_SYNC)?;
    Ok(report)
}

enum FeedPull {
    Xml(String),
    NotModified,
    Stop { until: String, reason: String },
}

fn pull_feed(db: &Database, username: &str) -> Result<FeedPull, String> {
    let url = rss_url(username);
    let etag_key = format!("rss_etag:{username}");
    let etag = db.get_meta(&etag_key)?;
    match fetch_rss(&url, etag.as_deref()) {
        RssFetch::Xml { body, etag } => {
            if let Some(tag) = etag {
                db.set_meta(&etag_key, &tag)?;
            }
            Ok(FeedPull::Xml(body))
        }
        RssFetch::NotModified => Ok(FeedPull::NotModified),
        RssFetch::RateLimited { retry_after_secs } => {
            let wait = retry_after_secs
                .map(|s| Duration::from_secs(s.max(60)))
                .unwrap_or(RATE_LIMIT_BACKOFF);
            Ok(FeedPull::Stop {
                until: (Utc::now() + ChronoDuration::from_std(wait).unwrap_or(ChronoDuration::hours(6)))
                    .to_rfc3339(),
                reason: "Letterboxd asked us to slow down".into(),
            })
        }
        RssFetch::Forbidden => Ok(FeedPull::Stop {
            until: (Utc::now()
                + ChronoDuration::from_std(FORBIDDEN_BACKOFF).unwrap_or(ChronoDuration::hours(24)))
            .to_rfc3339(),
            reason: "Letterboxd blocked the request".into(),
        }),
        RssFetch::Failed(err) => Err(err),
    }
}

fn maybe_gap(need_gap: &mut bool) {
    if *need_gap {
        std::thread::sleep(FEED_REQUEST_GAP);
    }
    *need_gap = true;
}

fn work_is_due(db: &Database) -> Result<bool, String> {
    let now = Utc::now();
    if is_paused(db.get_meta(META_BACKOFF)?.as_deref(), now) {
        return Ok(false);
    }
    if self_username(db)?.is_some()
        && is_due(
            db.get_meta(META_SELF_SYNC)?.as_deref(),
            now,
            AUTO_SYNC_MIN_INTERVAL,
        )
    {
        return Ok(true);
    }
    for (_, _, last) in enabled_friends(db)? {
        if is_due(last.as_deref(), now, AUTO_SYNC_MIN_INTERVAL) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn self_username(db: &Database) -> Result<Option<String>, String> {
    Ok(db
        .get_meta("self_username")?
        .map(|u| u.trim().trim_start_matches('@').to_lowercase())
        .filter(|u| !u.is_empty()))
}

fn enabled_friends(db: &Database) -> Result<Vec<(String, String, Option<String>)>, String> {
    let mut stmt = db
        .conn()
        .prepare("SELECT id, username, last_sync_at FROM friends WHERE enabled = 1 ORDER BY username")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_when_never_synced() {
        assert!(is_due(None, Utc::now(), AUTO_SYNC_MIN_INTERVAL));
    }

    #[test]
    fn not_due_inside_minimum_window() {
        let now = Utc::now();
        let recent = (now - ChronoDuration::minutes(5)).to_rfc3339();
        assert!(!is_due(Some(&recent), now, AUTO_SYNC_MIN_INTERVAL));
    }

    #[test]
    fn due_after_minimum_window() {
        let now = Utc::now();
        let old = (now - ChronoDuration::hours(2)).to_rfc3339();
        assert!(is_due(Some(&old), now, AUTO_SYNC_MIN_INTERVAL));
    }

    #[test]
    fn paused_until_future() {
        let now = Utc::now();
        let later = (now + ChronoDuration::hours(1)).to_rfc3339();
        assert!(is_paused(Some(&later), now));
        assert!(!is_paused(Some(&now.to_rfc3339()), now + ChronoDuration::seconds(1)));
    }
}
