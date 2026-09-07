mod app_log;
mod catalog;
mod commands;
mod jobs;
mod letterboxd;
mod migration;
mod models;
mod queries;
mod storage;
mod taste;
#[cfg(windows)]
mod windows_icon;

use commands::init_state;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tauri::Manager;

#[derive(Serialize)]
struct FileInfo {
    name: String,
    bytes: u64,
}

#[tauri::command]
fn file_info(path: String) -> Result<FileInfo, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(path);
    Ok(FileInfo {
        name,
        bytes: meta.len(),
    })
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_zip_texts(path: String) -> Result<HashMap<String, String>, String> {
    let file = File::open(&path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = Path::new(entry.name())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.to_ascii_lowercase().ends_with(".csv") {
            continue;
        }
        let mut buf = String::new();
        if entry.read_to_string(&mut buf).is_err() {
            continue;
        }
        out.insert(name.to_ascii_lowercase(), buf);
    }
    Ok(out)
}

const OSVELL_UA: &str = concat!(
    "Osvell/",
    env!("CARGO_PKG_VERSION"),
    " (personal Letterboxd RSS reader)"
);

fn allowed_fetch(url: &str) -> bool {
    url.starts_with("https://letterboxd.com/")
        && url.chars().all(|c| c.is_ascii() && !c.is_control())
}

pub fn fetch_url(url: &str) -> Result<String, String> {
    if !allowed_fetch(url) {
        return Err("blocked host".into());
    }
    ureq::get(url)
        .set("User-Agent", OSVELL_UA)
        .set(
            "Accept",
            "application/rss+xml, application/xml, text/xml, */*",
        )
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

pub enum RssFetch {
    Xml {
        body: String,
        etag: Option<String>,
    },
    NotModified,
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    Forbidden,
    Failed(String),
}

pub fn is_letterboxd_diary_rss(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://letterboxd.com/") else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let Some(user) = rest.strip_suffix("/rss") else {
        return false;
    };
    !user.is_empty()
        && !user.contains('/')
        && user
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub fn fetch_rss(url: &str, etag: Option<&str>) -> RssFetch {
    if !is_letterboxd_diary_rss(url) {
        return RssFetch::Failed("blocked host".into());
    }
    let mut req = ureq::get(url)
        .set("User-Agent", OSVELL_UA)
        .set(
            "Accept",
            "application/rss+xml, application/xml, text/xml;q=0.9, */*;q=0.8",
        );
    if let Some(tag) = etag.map(str::trim).filter(|t| !t.is_empty()) {
        req = req.set("If-None-Match", tag);
    }
    match req.call() {
        Ok(resp) => {
            let etag = resp.header("etag").map(|v| v.to_string());
            match resp.into_string() {
                Ok(body) => RssFetch::Xml { body, etag },
                Err(err) => RssFetch::Failed(err.to_string()),
            }
        }
        Err(ureq::Error::Status(304, _)) => RssFetch::NotModified,
        Err(ureq::Error::Status(429, resp)) => RssFetch::RateLimited {
            retry_after_secs: resp
                .header("retry-after")
                .and_then(|v| v.parse().ok())
                .filter(|n| *n > 0),
        },
        Err(ureq::Error::Status(403, _)) => RssFetch::Forbidden,
        Err(err) => RssFetch::Failed(err.to_string()),
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::is_letterboxd_diary_rss;

    #[test]
    fn diary_rss_urls_only() {
        assert!(is_letterboxd_diary_rss("https://letterboxd.com/ryan/rss/"));
        assert!(is_letterboxd_diary_rss("https://letterboxd.com/ryan/rss"));
        assert!(!is_letterboxd_diary_rss("https://letterboxd.com/film/heat/"));
        assert!(!is_letterboxd_diary_rss("https://letterboxd.com/ryan/films/ratings/"));
        assert!(!is_letterboxd_diary_rss("https://example.com/ryan/rss/"));
    }
}

#[tauri::command]
fn fetch_text(url: String) -> Result<String, String> {
    fetch_url(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        use tauri_plugin_window_state::StateFlags;

        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    #[cfg(windows)]
                    windows_icon::apply(&window);
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }))
            .plugin(
                tauri_plugin_window_state::Builder::new()
                    .with_state_flags(
                        StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED,
                    )
                    .build(),
            );
    }

    builder
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = init_state(app.handle())?;
            let job = state.job.clone();
            let db_path = state.db_path.clone();
            app.manage(state);
            crate::letterboxd::feeds::start_scheduler(app.handle().clone(), job, db_path);

            #[cfg(desktop)]
            {
                use tauri_plugin_window_state::{StateFlags, WindowExt};

                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.restore_state(
                        StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED,
                    );
                    #[cfg(windows)]
                    windows_icon::apply(&window);
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                    #[cfg(windows)]
                    windows_icon::apply(&window);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            file_info,
            read_text_file,
            read_zip_texts,
            fetch_text,
            commands::get_coverage,
            commands::get_session,
            commands::set_self_username,
            commands::get_install_info,
            commands::reset_all_data,
            commands::launch_uninstaller,
            commands::library_get,
            commands::stats_get,
            commands::film_get,
            commands::film_artwork_get,
            commands::film_artwork_set,
            commands::home_get,
            commands::import_export_zip,
            commands::import_get_diagnostics,
            commands::sync_self,
            commands::sync_friends,
            commands::sync_feeds,
            commands::import_friend_usernames,
            commands::film_set_rating,
            commands::migrate_from_legacy,
            commands::tmdb_set_key,
            commands::tmdb_clear_key,
            commands::tmdb_has_key,
            commands::tmdb_key_status,
            commands::tmdb_enrich,
            commands::taste_key_status,
            commands::taste_set_key,
            commands::taste_clear_key,
            commands::taste_set_model,
            commands::taste_set_web,
            commands::taste_get,
            commands::film_taste_detail,
            commands::taste_analyze,
            commands::taste_feedback_set,
            commands::taste_feedback_clear,
            commands::list_friends,
            commands::remove_friend,
            commands::update_preflight,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
