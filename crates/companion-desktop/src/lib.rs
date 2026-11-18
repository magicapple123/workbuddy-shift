#[cfg(target_os = "macos")]
mod background_cursor;

mod hit_test;

mod integration_folder;

mod locks;

#[cfg(target_os = "macos")]
mod panel;

mod rail_position;

mod rail_settings;

mod session;

pub const RAIL: &str = "companion-rail";

pub const SETTINGS: &str = "companion-settings";

#[derive(Clone)]
pub struct Config {
    pub assets: String,
    pub runtime: PathBuf,
    /// Whether this host owns login-item registration. Embedded hosts set false.
    pub manage_autostart: bool,
    /// Default on a fresh install. Embedded hosts can opt in only after consent.
    pub default_enabled: bool,
    pub show_rail: bool,
}

pub fn init(config: Config) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("companion")
        .on_window_ready(|window| {
            // WKWebView can remain document-visible while its native window is minimized.
            std::thread::spawn(move || {
                let mut previous = None;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let (Ok(visible), Ok(minimized)) = (window.is_visible(), window.is_minimized())
                    else {
                        break;
                    };
                    let active = visible && !minimized;
                    if previous != Some(active) {
                        let _ = window.emit_to(
                            window.label(),
                            "companion-window-active",
                            serde_json::json!({"label":window.label(),"active":active}),
                        );
                        previous = Some(active);
                    }
                }
            });
        })
        .invoke_handler(tauri::generate_handler![
            monitor_state,
            collector_request,
            set_hit_regions,
            rail_settings::rail_settings_get,
            rail_settings::rail_settings_set,
            rail_reset_position,
            close_settings,
            open_view,
            session::open_session_url,
            integration_folder::open_integration_folder
        ])
        .setup(move |app, _| {
            // Plugin initialization holds Tauri's plugin store lock. Queue window
            // creation from another thread so preparation runs after it unlocks.
            let app = app.clone();
            std::thread::spawn(move || {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Err(error) = start(&handle, config) {
                        eprintln!("companion-desktop initialization: {error}");
                        let _ = handle.emit("monitor-warning", error.to_string());
                    }
                });
            });
            Ok(())
        })
        .on_event(|app, event| match event {
            // The rail hides on close; settings close normally and release their WebView.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == RAIL => {
                api.prevent_close();
                if let Some(w) = app.get_webview_window(RAIL) {
                    let _ = w.hide();
                }
            }
            // Native drags run a modal loop; the worker thread only reads this
            // timestamp and acts once the moves have gone quiet.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Moved(_),
                ..
            } if label == RAIL => {
                if let Some(watch) = app.try_state::<Arc<MoveWatch>>() {
                    watch.touch();
                }
            }
            tauri::RunEvent::Exit => {
                if let Some(w) = app.get_webview_window(RAIL) {
                    if let (Ok(p), Ok(scale), Some(file)) =
                        (w.outer_position(), w.scale_factor(), position_path(app))
                    {
                        let p = p.to_logical::<f64>(scale);
                        if let Some(parent) = file.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(file, json!({"x":p.x,"y":p.y}).to_string());
                    }
                }
                if let Some(s) = app.try_state::<Arc<Service>>() {
                    s.stop.store(true, Ordering::Relaxed);
                    s.wake.notify_all();
                    if let Ok(mut c) = s.client.try_lock() {
                        *c = None;
                    }
                }
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                hit_test::remove();
            }
            _ => {}
        })
        .build()
}

mod plugin;
use companion_runtime::Client;
pub use plugin::*;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
