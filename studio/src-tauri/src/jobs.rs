use crate::models::JobProgress;
use crate::storage::db::Database;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub type JobSlot = Arc<Mutex<Option<String>>>;

pub fn try_begin_job(slot: &Mutex<Option<String>>, name: &str) -> Result<(), String> {
    let mut guard = slot.lock().map_err(|e| e.to_string())?;
    if let Some(current) = guard.as_ref() {
        return Err(format!("Already running {current}"));
    }
    *guard = Some(name.to_string());
    Ok(())
}

pub fn end_job(slot: &Mutex<Option<String>>) {
    if let Ok(mut guard) = slot.lock() {
        *guard = None;
    }
}

pub fn spawn_job<F>(
    app: AppHandle,
    slot: JobSlot,
    db_path: PathBuf,
    name: &'static str,
    work: F,
) -> Result<(), String>
where
    F: FnOnce(&AppHandle, PathBuf) -> Result<(), String> + Send + 'static,
{
    try_begin_job(&slot, name).or_else(|err| {
        if err.contains("Already running") {
            std::thread::sleep(std::time::Duration::from_millis(80));
            try_begin_job(&slot, name)
        } else {
            Err(err)
        }
    })?;
    let _ = app.emit(
        "studio-job",
        JobProgress {
            job: name.into(),
            label: format!("Starting {name}…"),
            ..Default::default()
        },
    );
    let worker_slot = slot.clone();
    let thread = std::thread::Builder::new()
        .name(format!("studio-{name}"))
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(&app, db_path)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    crate::app_log::write(&app, &format!("{name} failed: {err}"));
                    if name == "taste" {
                        persist_taste_failure(&app, &err);
                    }
                    let _ = app.emit(
                        "studio-job",
                        JobProgress {
                            job: name.into(),
                            label: format!("{name} failed · {}", toast_err(&err)),
                            errors: 1,
                            done: true,
                            ..Default::default()
                        },
                    );
                }
                Err(panic) => {
                    let msg = panic_message(&panic);
                    crate::app_log::write(&app, &format!("{name} panicked: {msg}"));
                    if name == "taste" {
                        persist_taste_failure(&app, &format!("panicked: {msg}"));
                    }
                    let _ = app.emit(
                        "studio-job",
                        JobProgress {
                            job: name.into(),
                            label: format!("{name} failed · {msg}"),
                            errors: 1,
                            done: true,
                            ..Default::default()
                        },
                    );
                }
            }
            end_job(&worker_slot);
        })
        .map_err(|e| e.to_string());
    if let Err(err) = thread {
        end_job(&slot);
        return Err(err);
    }
    Ok(())
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unexpected panic".into()
    }
}

pub fn open_worker_db(path: &PathBuf) -> Result<Database, String> {
    Database::open(path)
}

fn toast_err(err: &str) -> String {
    let short = err.split(" · raw:").next().unwrap_or(err).trim();
    if short.chars().count() <= 180 {
        short.to_string()
    } else {
        format!("{}…", short.chars().take(177).collect::<String>())
    }
}

fn persist_taste_failure(app: &AppHandle, err: &str) {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let body = serde_json::json!({
        "ok": false,
        "at": chrono::Utc::now().to_rfc3339(),
        "error": err,
    })
    .to_string();
    if let Some(path) = crate::app_log::write_json(app, "taste-runs", &format!("{stamp}-failed.json"), &body)
    {
        let _ = crate::app_log::write_json(app, "taste-runs", "latest.json", &body);
        crate::app_log::write(app, &format!("taste · failure trace {}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_job_is_rejected_until_the_first_finishes() {
        let slot = Mutex::new(None);
        try_begin_job(&slot, "enrich").unwrap();
        let err = try_begin_job(&slot, "import").unwrap_err();
        assert!(err.contains("enrich"), "{err}");
        end_job(&slot);
        try_begin_job(&slot, "import").unwrap();
    }
}
