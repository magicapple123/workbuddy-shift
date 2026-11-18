//! 插件状态、初始化与悬浮栏窗口创建。

use super::*;

pub(crate) struct Service {
    pub(crate) client: Mutex<Option<Client>>,
    pub(crate) snapshot: Mutex<Value>,
    pub(crate) connected: AtomicBool,
    pub(crate) stop: AtomicBool,
    pub(crate) enabled: AtomicBool,
    pub(crate) wake: Condvar,
    pub(crate) config: Config,
}

pub(crate) fn enabled_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("companion-enabled.json"))
}

pub fn is_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    app.try_state::<Arc<Service>>()
        .map(|s| s.enabled.load(Ordering::Acquire))
        .unwrap_or_else(|| read_enabled(app, false))
}

pub(crate) fn read_enabled<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    default_enabled: bool,
) -> bool {
    enabled_path(app)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<bool>(&b).ok())
        .unwrap_or(default_enabled)
}

/// Called from an async host command, outside Tauri's plugin setup lock.
pub fn set_enabled(app: &tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let state = app
        .try_state::<Arc<Service>>()
        .ok_or("内置会话悬浮窗正在初始化，请稍后重试")?;
    let mut client = crate::locks::guard(&state.client, "采集器连接")?;
    if enabled && state.config.show_rail {
        if let Some(rail) = app.get_webview_window(RAIL) {
            rail.show().map_err(|e| e.to_string())?;
        } else {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let handle = app.clone();
            let config = state.config.clone();
            app.run_on_main_thread(move || {
                let _ = tx.send(create_rail(&handle, &config).map_err(|e| e.to_string()));
            })
            .map_err(|e| e.to_string())?;
            rx.recv().map_err(|e| e.to_string())??;
        }
    }
    let path = enabled_path(app)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, if enabled { "true" } else { "false" }).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    state.enabled.store(enabled, Ordering::Release);
    state.wake.notify_all();
    if !enabled {
        *client = None; // Release this host's lease; other applications retain theirs.
        state.connected.store(false, Ordering::Relaxed);
        // 关闭悬浮窗时把快照清空；快照是采集器派生缓存，锁中毒时取回旧值覆盖即可。
        *crate::locks::recover(&state.snapshot) = Value::Null;
        for label in [RAIL, SETTINGS] {
            if let Some(w) = app.get_webview_window(label) {
                let _ = w.hide();
            }
        }
        let _ = app.emit("monitor-connection", "offline");
    }
    let _ = app.emit("companion:enabled", enabled);
    Ok(enabled)
}

pub(crate) fn position_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|p| p.join("companion-rail-position.json"))
}

pub(crate) fn start(
    app: &tauri::AppHandle,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = read_enabled(app, config.default_enabled);
    let regions: hit_test::Regions = Arc::new(Mutex::new(vec![]));
    app.manage(regions);
    let state = Arc::new(Service {
        client: Mutex::new(None),
        snapshot: Mutex::new(Value::Null),
        connected: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        enabled: AtomicBool::new(enabled),
        wake: Condvar::new(),
        config: config.clone(),
    });
    app.manage(state.clone());
    if enabled {
        create_rail(app, &config)?;
    }
    // Correct a rail that ended up outside every work area (dragged off screen,
    // or its monitor was unplugged). The 500 ms of silence keeps the write out
    // of the platform's modal drag loop, and the swap makes sure each settled
    // position runs through `plan` exactly once.
    {
        let watch = Arc::new(MoveWatch::new());
        app.manage(watch.clone());
        let app = app.clone();
        let state = state.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if state.stop.load(Ordering::Relaxed) {
                break;
            }
            let last = watch.last.load(Ordering::Acquire);
            if last == 0 || now_ms().saturating_sub(last) < 500 {
                continue;
            }
            // A move that landed after the snapshot is not quiet yet. Don't
            // claim the old stamp; the next tick waits out the new one.
            if watch.last.load(Ordering::Acquire) != last {
                continue;
            }
            if watch.checked.swap(last, Ordering::AcqRel) == last {
                continue;
            }
            if let Some(rail) = app.get_webview_window(RAIL) {
                // Snapshot before queueing so a reset that runs first on the
                // main thread invalidates this correction (stale `from`
                // would jump the window back off-screen).
                let generation = rail_position::animation_generation();
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                let read_rail = rail.clone();
                if app
                    .run_on_main_thread(move || {
                        let _ = tx.send(rail_position::plan_at(&read_rail, generation));
                    })
                    .is_err()
                {
                    continue;
                }
                if let Ok(Ok(Some(target))) = rx.recv() {
                    rail_position::glide(&rail, target);
                }
            }
        });
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut previous_settings = Value::Null;
        while !state.stop.load(Ordering::Relaxed) {
            let mut guard = crate::locks::recover(&state.client);
            guard = state
                .wake
                .wait_while(guard, |_| {
                    !state.enabled.load(Ordering::Acquire) && !state.stop.load(Ordering::Relaxed)
                })
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stop.load(Ordering::Relaxed) {
                break;
            }
            if guard.is_none() {
                *guard = Client::connect(companion_runtime::home(), &state.config.runtime).ok();
            }
            let result = guard
                .as_ref()
                .ok_or("连接中".to_string())
                .and_then(Client::poll);
            match result {
                Ok(v) => {
                    state
                        .connected
                        .store(v["snapshot"]["ready"] == true, Ordering::Relaxed);
                    // 快照是派生缓存：锁中毒时取回旧值并覆盖为最新一轮结果。
                    *crate::locks::recover(&state.snapshot) = v["snapshot"].clone();
                    let _ = app.emit("monitor-state", &v["snapshot"]);
                    if v["settings"] != previous_settings {
                        previous_settings = v["settings"].clone();
                        let _ = app.emit("monitor-settings", &previous_settings);
                    }
                    for a in v["notifications"].as_array().into_iter().flatten() {
                        let kind = match a["kind"].as_str() {
                            Some("wait") => "需要确认",
                            Some("error") => "任务失败",
                            _ => "任务完成",
                        };
                        let mut n = app
                            .notification()
                            .builder()
                            .title(format!("WorkBuddy Shift · {kind}"))
                            .body(a["title"].as_str().unwrap_or("会话状态已更新"));
                        if a["sound"] == true {
                            n = n.sound("default");
                        }
                        let _ = n.show();
                    }
                }
                Err(_) => {
                    state.connected.store(false, Ordering::Relaxed);
                    let _ = app.emit("monitor-connection", "offline");
                    *guard = None;
                }
            }
            drop(guard);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
    Ok(())
}

pub(crate) fn create_rail(
    app: &tauri::AppHandle,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let rail = WebviewWindowBuilder::new(
        app,
        RAIL,
        WebviewUrl::App(format!("{}desktop.html", config.assets).into()),
    )
    .title("WorkBuddy Shift · 会话栏")
    .inner_size(368., 600.)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .accept_first_mouse(true)
    .visible_on_all_workspaces(true)
    .visible(config.show_rail)
    .build()?;
    // Restore through the shared visibility rule; the first launch keeps the
    // original top-right default. Losing the monitor list only skips the move.
    let saved: Value = position_path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let saved = match (saved["x"].as_f64(), saved["y"].as_f64()) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
        _ => None,
    };
    if let Err(error) = rail_position::place(&rail, saved) {
        eprintln!("会话悬浮窗位置: {error}");
    }
    #[cfg(target_os = "macos")]
    {
        let ns_window = rail.ns_window()? as usize;
        // Clicking the rail must not activate the host application (which
        // would pull every other host window to the front). A failed
        // conversion keeps the plain window, which still works.
        if let Err(error) = panel::convert(ns_window) {
            eprintln!("会话悬浮栏窗口转换: {error}");
        }
        hit_test::install(
            ns_window,
            app.state::<hit_test::Regions>().inner().clone(),
            {
                let window = rail.clone();
                move |point| {
                    let payload = point.map(|(x, y)| json!({ "x": x, "y": y }));
                    let _ = window.emit("companion-pointer", payload);
                }
            },
        );
    }
    #[cfg(target_os = "windows")]
    hit_test::install(
        rail.clone(),
        app.state::<hit_test::Regions>().inner().clone(),
    );
    Ok(())
}
