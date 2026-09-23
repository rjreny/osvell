//! Login startup, close-to-tray, and bringing the window back.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_store::StoreExt;

pub const AUTOSTART_ARG: &str = "--osvell-autostart";
const TRAY_ID: &str = "osvell";
const STORE_FILE: &str = "studio.json";

pub struct ResidentFlags {
    pub close_minimizes: AtomicBool,
    pub allow_exit: AtomicBool,
}

impl ResidentFlags {
    fn new(close_minimizes: bool) -> Self {
        Self {
            close_minimizes: AtomicBool::new(close_minimizes),
            allow_exit: AtomicBool::new(false),
        }
    }
}

pub fn launched_by_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

fn store_flag(app: &AppHandle, key: &str) -> bool {
    app.store(STORE_FILE)
        .ok()
        .and_then(|store| store.get(key))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn present_on_launch(app: &AppHandle) {
    let close_minimizes = store_flag(app, "closeMinimizes");
    let start_minimized = store_flag(app, "startMinimized");
    app.manage(ResidentFlags::new(close_minimizes));
    if let Some(window) = app.get_webview_window("main") {
        attach_close(&window);
    }
    sync_tray(app, close_minimizes || start_minimized);
    if !(launched_by_autostart() && start_minimized) {
        reveal_main(app, false);
    }
}

pub fn reveal_main(app: &AppHandle, home_if_hidden: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let was_hidden = !window.is_visible().unwrap_or(false);
    if home_if_hidden && was_hidden {
        let _ = window.emit("osvell-open-home", ());
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    #[cfg(windows)]
    crate::windows_icon::apply(&window);
}

fn attach_close(window: &WebviewWindow) {
    let win = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            let Some(flags) = win.try_state::<ResidentFlags>() else {
                return;
            };
            if flags.allow_exit.load(Ordering::Relaxed) {
                return;
            }
            if flags.close_minimizes.load(Ordering::Relaxed) {
                api.prevent_close();
                let _ = win.hide();
            }
        }
    });
}

fn sync_tray(app: &AppHandle, wanted: bool) {
    if wanted {
        if app.tray_by_id(TRAY_ID).is_none() {
            if let Err(err) = build_tray(app) {
                crate::app_log::write(app, format!("tray unavailable: {err}"));
            }
        }
    } else if let Some(icon) = app.remove_tray_by_id(TRAY_ID) {
        drop(icon);
    }
}

fn build_tray(app: &AppHandle) -> Result<(), String> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "missing window icon".to_string())?;
    let open = MenuItem::with_id(app, "open", "Open Osvell", true, None::<&str>)
        .map_err(|err| err.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "Quit Osvell", true, None::<&str>)
        .map_err(|err| err.to_string())?;
    let menu = Menu::with_items(app, &[&open, &quit]).map_err(|err| err.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Osvell")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => reveal_main(app, true),
            "quit" => quit_app(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let open = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if open {
                reveal_main(tray.app_handle(), true);
            }
        })
        .build(app)
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_resident_prefs(
    app: AppHandle,
    close_minimizes: bool,
    start_minimized: bool,
) -> Result<(), String> {
    let Some(flags) = app.try_state::<ResidentFlags>() else {
        return Err("startup preferences are not ready".into());
    };
    flags
        .close_minimizes
        .store(close_minimizes, Ordering::Relaxed);
    sync_tray(&app, close_minimizes || start_minimized);
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    if let Some(flags) = app.try_state::<ResidentFlags>() {
        flags.allow_exit.store(true, Ordering::Relaxed);
    }
    app.exit(0);
}
