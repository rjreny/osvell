use crate::catalog::tmdb;
use crate::letterboxd::import::import_zip_discovery;
use crate::letterboxd::rss::{rss_url, sync_rss};
use crate::letterboxd::zip::discover_zip;
use crate::migration::migrate_legacy;
use crate::models::{
    AppSession, EnrichReport, FilmArtwork, FilmDetail, FriendSyncResult, HomeViewModel, ImportDiagnostics,
    ImportResult, ImportSummary, InstallInfo, JobProgress, LegacyLibrary, LibraryCoverage,
    LibraryPage, LibraryQuery, MigrationResult, SetArtworkInput, SetRatingInput, StatsSnapshot, SyncResult, TmdbKeyStatus,
};
use crate::queries::{get_film, get_home, get_library, get_stats, parse_tmdb_ref, resolve_source_movie_ids};
use crate::storage::db::Database;
use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

pub struct AppState {
    pub db: Mutex<Database>,
    pub db_path: PathBuf,
    pub job: crate::jobs::JobSlot,
}

fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("studio.db"))
}

pub fn init_state(app: &AppHandle) -> Result<AppState, String> {
    crate::app_log::write(app, "app started");
    let path = db_path(app)?;
    let db = Database::open(&path)?;
    Ok(AppState {
        db: Mutex::new(db),
        db_path: path,
        job: Arc::new(Mutex::new(None)),
    })
}

#[tauri::command]
pub fn get_coverage(state: State<'_, AppState>) -> Result<LibraryCoverage, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.compute_coverage()
}

#[tauri::command]
pub fn get_session(state: State<'_, AppState>) -> Result<AppSession, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_session()
}

#[tauri::command]
pub fn set_self_username(username: String, state: State<'_, AppState>) -> Result<(), String> {
    let clean = username.trim().trim_start_matches('@').to_string();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    if clean.is_empty() {
        db.conn()
            .execute("DELETE FROM app_meta WHERE key = 'self_username'", [])
            .map_err(|e| e.to_string())?;
    } else {
        db.set_meta("self_username", &clean)?;
    }
    Ok(())
}

fn install_kind(exe: &std::path::Path) -> &'static str {
    let path = exe.to_string_lossy().to_lowercase();
    if path.contains("target\\debug")
        || path.contains("target/debug")
        || path.contains("target\\release")
        || path.contains("target/release")
        || path.contains("cursor-sandbox")
    {
        "dev"
    } else if path.contains("\\programs\\") || path.contains("/programs/") {
        "installed"
    } else {
        "portable"
    }
}

fn find_uninstaller(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("uninstall") && name.ends_with(".exe") {
            return Some(entry.path());
        }
    }
    None
}

#[tauri::command]
pub fn get_install_info(app: AppHandle) -> Result<InstallInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let db_path = db_path(&app)?;
    let exe = std::env::current_exe().ok();
    let kind = exe
        .as_ref()
        .map(|p| install_kind(p).to_string())
        .unwrap_or_else(|| "unknown".into());
    let uninstaller = exe.as_ref().and_then(|p| find_uninstaller(p));
    Ok(InstallInfo {
        version: app
            .config()
            .version
            .clone()
            .unwrap_or_else(|| "0.0.0".into()),
        install_kind: kind,
        app_data_dir: data_dir.to_string_lossy().into_owned(),
        database_path: db_path.to_string_lossy().into_owned(),
        executable_path: exe.map(|p| p.to_string_lossy().into_owned()),
        uninstaller_path: uninstaller.map(|p| p.to_string_lossy().into_owned()),
        log_path: crate::app_log::log_path(&app)?
            .to_string_lossy()
            .into_owned(),
        data_bytes: dir_size(&data_dir),
    })
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            total += dir_size(&child);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[tauri::command]
pub fn reset_all_data(state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.reset_all_data()
}

#[tauri::command]
pub fn launch_uninstaller(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninstaller =
        find_uninstaller(&exe).ok_or_else(|| "No uninstaller found for this build".to_string())?;
    std::process::Command::new(&uninstaller)
        .spawn()
        .map_err(|e| e.to_string())?;
    let _ = app;
    Ok(())
}

#[tauri::command]
pub fn library_get(query: LibraryQuery, state: State<'_, AppState>) -> Result<LibraryPage, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    get_library(&db, &query)
}

#[tauri::command]
pub fn stats_get(state: State<'_, AppState>) -> Result<StatsSnapshot, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    get_stats(&db)
}

#[tauri::command]
pub fn film_get(id: String, state: State<'_, AppState>) -> Result<FilmDetail, String> {
    let detail = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        get_film(&db, &id)
    };

    match detail {
        Ok(d) if d.tmdb_id.is_some() && (!d.collection_hydrated || !d.detail_metadata_hydrated) => {
            if let Some(tmdb_id) = d.tmdb_id {
                if let Ok(worker) = crate::jobs::open_worker_db(&state.db_path) {
                    let _ = tmdb::refresh_movie_catalog(&worker, tmdb_id, true);
                }
            }
            let db = state.db.lock().map_err(|e| e.to_string())?;
            Ok(get_film(&db, &id).unwrap_or(d))
        }
        Ok(d) => Ok(d),
        Err(err) => {
            if let Some(tmdb_id) = parse_tmdb_ref(&id) {
                if let Ok(worker) = crate::jobs::open_worker_db(&state.db_path) {
                    if tmdb::refresh_movie_catalog(&worker, tmdb_id, true).is_ok() {
                        let db = state.db.lock().map_err(|e| e.to_string())?;
                        return get_film(&db, &id);
                    }
                }
            }
            Err(err)
        }
    }
}

#[tauri::command]
pub fn film_artwork_get(id: String, state: State<'_, AppState>) -> Result<FilmArtwork, String> {
    let db = state.db.lock().map_err(|error| error.to_string())?;
    tmdb::film_artwork(&db, &id)
}

#[tauri::command]
pub fn film_artwork_set(
    input: SetArtworkInput,
    state: State<'_, AppState>,
) -> Result<FilmDetail, String> {
    let db = state.db.lock().map_err(|error| error.to_string())?;
    tmdb::set_film_artwork(&db, &input)?;
    get_film(&db, &input.id)
}

#[tauri::command]
pub fn home_get(state: State<'_, AppState>) -> Result<HomeViewModel, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    get_home(&db)
}

#[tauri::command]
pub fn import_export_zip(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::jobs::spawn_job(
        app,
        state.job.clone(),
        state.db_path.clone(),
        "import",
        move |app, db_path| {
            crate::app_log::write(app, &format!("zip import start {path}"));
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "import".into(),
                    label: "Importing Letterboxd ZIP…".into(),
                    total: 1,
                    ..Default::default()
                },
            );
            let discovery = discover_zip(&path)?;
            crate::app_log::write(
                app,
                &format!(
                    "zip discovered files={} unknown={} warnings={}",
                    discovery.files.len(),
                    discovery.unknown_paths.len(),
                    discovery.warnings.len()
                ),
            );
            let mut db = crate::jobs::open_worker_db(&db_path)?;
            let result = import_zip_discovery(&mut db, &discovery)?;
            crate::app_log::write(
                app,
                &format!(
                    "zip import done movies={} viewings={} ratings={} skipped={}",
                    result.movies, result.viewings, result.ratings, result.skipped
                ),
            );
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "import".into(),
                    label: format!(
                        "Imported {} films · {} viewings",
                        result.movies, result.viewings
                    ),
                    current: 1,
                    total: 1,
                    done: true,
                    import: Some(result),
                    ..Default::default()
                },
            );
            Ok(())
        },
    )
}

#[tauri::command]
pub fn import_get_diagnostics(state: State<'_, AppState>) -> Result<ImportDiagnostics, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = db
        .conn()
        .prepare("SELECT id, content_hash, imported_at, status FROM imports ORDER BY imported_at DESC LIMIT 20")
        .map_err(|e| e.to_string())?;
    let imports = stmt
        .query_map([], |row| {
            Ok(ImportSummary {
                id: row.get(0)?,
                content_hash: row.get(1)?,
                imported_at: row.get(2)?,
                status: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let coverage = db.compute_coverage()?;
    Ok(ImportDiagnostics {
        imports,
        warnings: coverage.warnings,
    })
}

#[tauri::command]
pub fn sync_self(username: String, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    crate::jobs::spawn_job(
        app,
        state.job.clone(),
        state.db_path.clone(),
        "sync",
        move |app, db_path| {
            let clean = username.trim().trim_start_matches('@').to_string();
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "sync".into(),
                    label: format!("Syncing @{clean}…"),
                    total: 1,
                    ..Default::default()
                },
            );
            let url = rss_url(&clean);
            let xml = match crate::fetch_rss(&url, None) {
                crate::RssFetch::Xml { body, .. } => body,
                crate::RssFetch::NotModified => {
                    let _ = app.emit(
                        "studio-job",
                        JobProgress {
                            job: "sync".into(),
                            label: format!("@{clean} diary already current"),
                            current: 1,
                            total: 1,
                            done: true,
                            ..Default::default()
                        },
                    );
                    return Ok(());
                }
                crate::RssFetch::RateLimited { .. } => {
                    return Err("Letterboxd asked us to slow down".into());
                }
                crate::RssFetch::Forbidden => {
                    return Err("Letterboxd blocked the public diary request".into());
                }
                crate::RssFetch::Failed(err) => return Err(err),
            };
            let mut db = crate::jobs::open_worker_db(&db_path)?;
            let result = sync_rss(&mut db, &clean, &xml)?;
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "sync".into(),
                    label: format!(
                        "Synced @{clean} · {} new of {} seen",
                        result.entries_added, result.entries_seen
                    ),
                    current: 1,
                    total: 1,
                    done: true,
                    ..Default::default()
                },
            );
            Ok(())
        },
    )
}

#[tauri::command]
pub fn sync_feeds(
    force: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    crate::letterboxd::feeds::spawn_feed_sync(
        app,
        state.job.clone(),
        state.db_path.clone(),
        force,
    )
}

#[tauri::command]
pub fn sync_friends(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    crate::letterboxd::feeds::spawn_feed_sync(
        app,
        state.job.clone(),
        state.db_path.clone(),
        true,
    )
    .map(|_| ())
}

#[tauri::command]
pub fn import_friend_usernames(text: String, state: State<'_, AppState>) -> Result<u32, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut added = 0u32;
    for line in text.lines() {
        let username = line
            .trim()
            .trim_start_matches('@')
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if username.is_empty()
            || !username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let exists: bool = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM friends WHERE username = ?1",
                params![username],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())?;
        if exists {
            continue;
        }
        db.conn()
            .execute(
                "INSERT INTO friends(id, username, enabled) VALUES (?1, ?2, 1)",
                params![Uuid::new_v4().to_string(), username],
            )
            .map_err(|e| e.to_string())?;
        added += 1;
    }
    Ok(added)
}

#[tauri::command]
pub fn film_set_rating(
    input: SetRatingInput,
    state: State<'_, AppState>,
) -> Result<FilmDetail, String> {
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();
    let smr_id: String = resolve_source_movie_ids(&db, &input.id)?
        .into_iter()
        .next()
        .ok_or_else(|| "Film not found".to_string())?;
    let rating_key = format!("manual|{smr_id}|{now}");
    db.conn()
        .execute(
            "INSERT INTO rating_events(
              id, source_movie_record_id, source_record_key, rating,
              occurred_at, published_at, observed_at, imported_at, source_type, import_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?5, NULL, 'manual', NULL)",
            params![
                Uuid::new_v4().to_string(),
                smr_id,
                rating_key,
                input.rating,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
    let mut tx = db.transaction()?;
    Database::rebuild_projections(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    get_film(&db, &input.id)
}

#[tauri::command]
pub fn migrate_from_legacy(
    legacy: LegacyLibrary,
    state: State<'_, AppState>,
) -> Result<MigrationResult, String> {
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    migrate_legacy(&mut db, &legacy)
}

#[tauri::command]
pub fn tmdb_set_key(key: String, app: AppHandle) -> Result<TmdbKeyStatus, String> {
    let status = tmdb::store_api_key(&key)?;
    crate::app_log::write(
        &app,
        &format!(
            "tmdb key save stored={} valid={:?} kind={:?} error={:?}",
            status.stored, status.valid, status.kind, status.last_error
        ),
    );
    Ok(status)
}

#[tauri::command]
pub fn tmdb_clear_key(app: AppHandle) -> Result<TmdbKeyStatus, String> {
    tmdb::clear_api_key()?;
    crate::app_log::write(&app, "tmdb key cleared");
    Ok(TmdbKeyStatus {
        stored: false,
        valid: None,
        kind: None,
        last_error: None,
    })
}

#[tauri::command]
pub fn tmdb_has_key() -> Result<bool, String> {
    Ok(tmdb::get_api_key()?
        .filter(|k| !k.trim().is_empty())
        .is_some())
}

#[tauri::command]
pub fn tmdb_key_status() -> Result<TmdbKeyStatus, String> {
    tmdb::key_status()
}

#[tauri::command]
pub fn tmdb_enrich(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    crate::jobs::spawn_job(
        app,
        state.job.clone(),
        state.db_path.clone(),
        "enrich",
        move |app, db_path| {
            let log_file = crate::app_log::log_path(app)?;
            crate::app_log::write(app, "enrich started");
            let db = crate::jobs::open_worker_db(&db_path)?;
            db.conn()
                .execute(
                    "UPDATE movie_links SET match_state = 'unmatched'
                     WHERE match_state = 'ambiguous'",
                    [],
                )
                .map_err(|e| e.to_string())?;
            let app_for_progress = app.clone();
            let mut report = tmdb::enrich_catalog_with(&db, &mut |progress| {
                let _ = app_for_progress.emit("studio-job", &progress);
            })?;
            report.log_path = Some(log_file.to_string_lossy().into_owned());
            crate::app_log::write(
                app,
                &format!(
                    "enrich finished has_key={} key_valid={:?} matched={} posters={} remaining_unmatched={} remaining_without_poster={} errors={} last_error={:?}",
                    report.has_key,
                    report.key_valid,
                    report.matched,
                    report.posters,
                    report.remaining_unmatched,
                    report.remaining_without_poster,
                    report.errors,
                    report.last_error
                ),
            );
            let poster_status = if report.remaining_without_poster == 0 {
                "all posters available".to_string()
            } else {
                format!("{} still without a poster", report.remaining_without_poster)
            };
            let finished_label = match (report.has_key, report.key_valid) {
                (false, _) => format!(
                    "TMDB key missing · {poster_status}"
                ),
                (_, Some(false)) => format!(
                    "TMDB key rejected · {poster_status}"
                ),
                _ if report.errors > 0 => format!(
                    "Finished · {} posters · {poster_status} · {} errors",
                    report.posters, report.errors
                ),
                _ => format!(
                    "Finished · {} posters · {poster_status}",
                    report.posters
                ),
            };
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "enrich".into(),
                    label: finished_label,
                    current: report.attempted,
                    total: report.attempted.max(1),
                    posters: report.posters,
                    errors: report.errors,
                    done: true,
                    enrich: Some(report),
                    ..Default::default()
                },
            );
            Ok(())
        },
    )
}

#[tauri::command]
pub fn remove_friend(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.remove_friend(&id)
}

#[tauri::command]
pub fn list_friends(
    state: State<'_, AppState>,
) -> Result<Vec<(String, String, Option<String>, Option<String>)>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, username, last_sync_at, last_sync_error FROM friends ORDER BY username",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[derive(Serialize)]
pub struct UpdatePreflight {
    pub signing_configured: bool,
    pub endpoint: String,
    pub http_status: Option<u16>,
    pub reachable: bool,
    pub message: String,
    pub update_available: bool,
    pub available_version: Option<String>,
}

fn updater_config(app: &AppHandle) -> (String, String) {
    let plugins = app.config().plugins.clone();
    let updater = plugins.0.get("updater").cloned().unwrap_or_default();
    let pubkey = updater
        .get("pubkey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let endpoint = updater
        .get("endpoints")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    (pubkey, endpoint)
}

fn update_preflight_message(signing_configured: bool, status: u16) -> String {
    if !signing_configured {
        if status == 404 {
            return "You're on the current build. Signed auto-updates aren't configured for this install yet."
                .to_string();
        }
        return format!(
            "Signed auto-updates aren't configured yet (release server HTTP {})",
            status
        );
    }
    if status == 204 || status == 404 {
        return "You're up to date".to_string();
    }
    if status >= 200 && status < 300 {
        return "Update available — click Update to download and install".to_string();
    }
    format!("Release server returned HTTP {}", status)
}

fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('v').to_string()
}

fn version_parts(version: &str) -> Vec<u32> {
    normalize_version(version)
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    let a = version_parts(candidate);
    let b = version_parts(current);
    let len = a.len().max(b.len());
    for idx in 0..len {
        let av = a.get(idx).copied().unwrap_or(0);
        let bv = b.get(idx).copied().unwrap_or(0);
        if av > bv {
            return true;
        }
        if av < bv {
            return false;
        }
    }
    false
}

fn finish_update_preflight(
    signing_configured: bool,
    endpoint: String,
    status: u16,
    body: &str,
    current_version: &str,
) -> UpdatePreflight {
    let mut update_available = false;
    let mut available_version = None;
    let mut message = update_preflight_message(signing_configured, status);

    if status >= 200 && status < 300 && !body.is_empty() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(release) = json.get("version").and_then(|v| v.as_str()) {
                let current = normalize_version(current_version);
                let release_norm = normalize_version(release);
                if version_is_newer(&release_norm, &current) {
                    update_available = true;
                    available_version = Some(release.to_string());
                    message = if signing_configured {
                        format!("Update available: {}", release)
                    } else {
                        format!(
                            "Release {} is published — run signer:generate and set pubkey to install",
                            release
                        )
                    };
                } else {
                    message = "You're up to date".to_string();
                }
            }
        }
    }

    UpdatePreflight {
        signing_configured,
        endpoint,
        http_status: Some(status),
        reachable: true,
        message,
        update_available,
        available_version,
    }
}

#[tauri::command]
pub fn update_preflight(app: AppHandle) -> Result<UpdatePreflight, String> {
    let (pubkey, endpoint) = updater_config(&app);
    let signing_configured = !pubkey.is_empty();
    let current_version = app
        .config()
        .version
        .as_deref()
        .unwrap_or("0.0.0");

    if endpoint.is_empty() {
        return Ok(UpdatePreflight {
            signing_configured,
            endpoint,
            http_status: None,
            reachable: false,
            message: "No update endpoint configured in tauri.conf.json".to_string(),
            update_available: false,
            available_version: None,
        });
    }

    let response = ureq::get(&endpoint)
        .set("User-Agent", "Osvell/0.1 (update check)")
        .set("Accept", "application/json")
        .call();

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            Ok(finish_update_preflight(
                signing_configured,
                endpoint,
                status,
                &body,
                &current_version,
            ))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok(finish_update_preflight(
                signing_configured,
                endpoint,
                code,
                &body,
                &current_version,
            ))
        }
        Err(e) => Ok(UpdatePreflight {
            signing_configured,
            endpoint,
            http_status: None,
            reachable: false,
            message: format!("Could not reach release server: {}", e),
            update_available: false,
            available_version: None,
        }),
    }
}

#[tauri::command]
pub fn taste_key_status(state: State<'_, AppState>) -> Result<crate::taste::TasteKeyStatus, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::key_status(&db)
}

#[tauri::command]
pub fn taste_set_key(
    key: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<crate::taste::TasteKeyStatus, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let status = crate::taste::store_api_key(&db, &key)?;
    crate::app_log::write(
        &app,
        &format!(
            "taste key save stored={} valid={:?} error={:?}",
            status.stored, status.valid, status.last_error
        ),
    );
    Ok(status)
}

#[tauri::command]
pub fn taste_clear_key(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<crate::taste::TasteKeyStatus, String> {
    crate::taste::clear_api_key()?;
    crate::app_log::write(&app, "taste key cleared");
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::key_status(&db)
}

#[tauri::command]
pub fn taste_set_model(model: String, state: State<'_, AppState>) -> Result<crate::taste::TasteKeyStatus, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::set_model(&db, &model)?;
    crate::taste::stored_status(&db)
}

#[tauri::command]
pub fn taste_set_web(enabled: bool, state: State<'_, AppState>) -> Result<crate::taste::TasteKeyStatus, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::set_web(&db, enabled)?;
    crate::taste::stored_status(&db)
}

#[tauri::command]
pub fn taste_get(state: State<'_, AppState>) -> Result<crate::taste::TasteState, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::load_state(&db)
}

#[tauri::command]
pub fn film_taste_detail(
    id: String,
    state: State<'_, AppState>,
) -> Result<crate::taste::FilmTasteFit, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::film_taste_detail(&db, &id)
}

#[tauri::command]
pub fn taste_analyze(
    force_refresh: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let force = force_refresh.unwrap_or(false);
    crate::jobs::spawn_job(
        app,
        state.job.clone(),
        state.db_path.clone(),
        "taste",
        move |app, db_path| {
            let db = crate::jobs::open_worker_db(&db_path)?;
            let model = crate::taste::stored_model(&db).unwrap_or_else(|_| "unknown".into());
            let web = crate::taste::stored_web(&db).unwrap_or(true);
            crate::app_log::write(
                app,
                &format!("taste analyze started model={model} web={web} force={force}"),
            );
            let app_for_progress = app.clone();
            let run_dir = app
                .path()
                .app_data_dir()
                .ok()
                .map(|p| p.join("taste-runs"));
            let report = crate::taste::analyze_with_run_log(
                &db,
                &mut |progress| {
                    crate::app_log::write(&app_for_progress, &format!("taste · {}", progress.label));
                    let _ = app_for_progress.emit("studio-job", &progress);
                },
                run_dir.as_deref(),
                force,
            )?;
            crate::app_log::write(
                app,
                &format!(
                    "taste analyze finished model={} picks={} rated={}",
                    report.model,
                    report.picks.len(),
                    report.rated_count
                ),
            );
            let _ = app.emit(
                "studio-job",
                JobProgress {
                    job: "taste".into(),
                    label: format!("Taste ready · {} picks", report.picks.len()),
                    done: true,
                    taste: Some(report),
                    ..Default::default()
                },
            );
            Ok(())
        },
    )
}

#[tauri::command]
pub fn taste_feedback_set(
    tmdb_id: i64,
    action: String,
    reason: Option<String>,
    exposure_id: Option<String>,
    target_feature_key: Option<String>,
    mood_scope: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::taste::feedback::TasteFeedback, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::feedback::set_feedback_with_exposure(
        &db,
        crate::taste::feedback::TasteFeedbackRequest {
            tmdb_id,
            action,
            reason,
            exposure_id,
            target_feature_key,
            mood_scope,
        },
    )
}

#[tauri::command]
pub fn taste_feedback_clear(tmdb_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    crate::taste::feedback::clear_feedback(&db, tmdb_id)
}
