//! Tauri 2 desktop shell around the PAX bridge server (see `bridge/`).
//!
//! The bridge itself is a plain axum server (`bridge::start_bridge`) that
//! knows nothing about Tauri. This module owns its lifecycle: starting it on
//! a background tokio task, stopping it via a `CancellationToken`, tracking
//! its status/port for the UI, and wiring up the tray, single-instance lock,
//! autostart, settings persistence.

pub mod bridge;

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Bridge lifecycle state
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub status: String, // stopped | starting | running | error
    pub port: u16,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntry {
    ts: i64,
    stream: String,
    line: String,
}

const MAX_LOG_LINES: usize = 500;

struct BridgeRuntime {
    status: AsyncMutex<BridgeStatus>,
    cancel: AsyncMutex<Option<CancellationToken>>,
    generation: AtomicU64,
    logs: AsyncMutex<Vec<LogEntry>>,
}

impl BridgeRuntime {
    fn new() -> Self {
        Self {
            status: AsyncMutex::new(BridgeStatus { status: "stopped".into(), port: bridge::config::port() }),
            cancel: AsyncMutex::new(None),
            generation: AtomicU64::new(0),
            logs: AsyncMutex::new(Vec::new()),
        }
    }
}

struct PaxManaged {
    runtime: Arc<BridgeRuntime>,
    is_quitting: Arc<AtomicBool>,
}

async fn push_log(app: &AppHandle, rt: &Arc<BridgeRuntime>, stream: &str, line: impl Into<String>) {
    let entry = LogEntry { ts: chrono::Utc::now().timestamp_millis(), stream: stream.to_string(), line: line.into() };
    {
        let mut logs = rt.logs.lock().await;
        logs.push(entry.clone());
        if logs.len() > MAX_LOG_LINES {
            logs.remove(0);
        }
    }
    let _ = app.emit("bridge:log", &entry);
}

async fn set_status(app: &AppHandle, rt: &Arc<BridgeRuntime>, status: &str) {
    let next = {
        let mut guard = rt.status.lock().await;
        guard.status = status.to_string();
        guard.clone()
    };
    let _ = app.emit("bridge:status", &next);
}

/// Polls a TCP connect to `127.0.0.1:port` until it succeeds or times out.
/// Cheap stand-in for an HTTP health check — the bridge starts accepting
/// connections as soon as `axum::serve` begins its loop.
async fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn start_bridge_internal(app: AppHandle, rt: Arc<BridgeRuntime>) {
    {
        let guard = rt.cancel.lock().await;
        if guard.is_some() {
            return; // already starting/running
        }
    }

    let port = bridge::config::port();
    {
        let mut status = rt.status.lock().await;
        status.port = port;
    }
    set_status(&app, &rt, "starting").await;
    push_log(&app, &rt, "sys", format!("Starting Salesgent Pax Bridge on port {}…", port)).await;

    let token = CancellationToken::new();
    {
        let mut guard = rt.cancel.lock().await;
        *guard = Some(token.clone());
    }
    let generation = rt.generation.fetch_add(1, Ordering::SeqCst) + 1;

    let app_for_server = app.clone();
    let rt_for_server = rt.clone();
    let shutdown_token = token.clone();
    tauri::async_runtime::spawn(async move {
        let result = bridge::start_bridge(async move { shutdown_token.cancelled().await }).await;

        // Only touch state if nothing has superseded this run (e.g. a fast restart).
        if rt_for_server.generation.load(Ordering::SeqCst) == generation {
            *rt_for_server.cancel.lock().await = None;
            match result {
                Ok(()) => {
                    push_log(&app_for_server, &rt_for_server, "sys", "Bridge stopped.").await;
                    set_status(&app_for_server, &rt_for_server, "stopped").await;
                }
                Err(err) => {
                    push_log(&app_for_server, &rt_for_server, "err", format!("Bridge error: {err}")).await;
                    set_status(&app_for_server, &rt_for_server, "error").await;
                }
            }
        }
    });

    let app_for_health = app.clone();
    let rt_for_health = rt.clone();
    tauri::async_runtime::spawn(async move {
        let ok = wait_for_port(port, Duration::from_secs(10)).await;
        if rt_for_health.generation.load(Ordering::SeqCst) != generation {
            return; // superseded by a stop/restart
        }
        if ok {
            set_status(&app_for_health, &rt_for_health, "running").await;
            push_log(&app_for_health, &rt_for_health, "sys", format!("Bridge is live at http://localhost:{port}")).await;
        } else {
            let still_starting = rt_for_health.status.lock().await.status == "starting";
            if still_starting {
                set_status(&app_for_health, &rt_for_health, "error").await;
                push_log(&app_for_health, &rt_for_health, "err", "Bridge did not become healthy in time.").await;
            }
        }
    });
}

async fn stop_bridge_internal(app: AppHandle, rt: Arc<BridgeRuntime>) {
    let token = { rt.cancel.lock().await.take() };
    if let Some(token) = token {
        push_log(&app, &rt, "sys", "Stopping bridge…").await;
        rt.generation.fetch_add(1, Ordering::SeqCst); // invalidate any in-flight health check
        token.cancel();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    set_status(&app, &rt, "stopped").await;
}

async fn restart_bridge_internal(app: AppHandle, rt: Arc<BridgeRuntime>) {
    stop_bridge_internal(app.clone(), rt.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    start_bridge_internal(app, rt).await;
}

// ---------------------------------------------------------------------------
// Settings (JSON file in the app data dir)
// ---------------------------------------------------------------------------

fn default_settings() -> Value {
    json!({
        "launchAtLogin": false,
        "autoUpdate": true,
        "startBridgeOnLaunch": true,
        "minimizeToTray": true,
        "port": bridge::config::DEFAULT_PORT,
    })
}

/// Ports below 1024 need OS privileges on most platforms and well-known ports
/// above that (like 5432, 3306, ...) are likely to collide with something
/// else already running — keep the picker in the unprivileged range.
const MIN_USER_PORT: u64 = 1024;
const MAX_PORT: u64 = 65535;

fn valid_port(v: &Value) -> Option<u16> {
    let n = v.as_u64()?;
    if (MIN_USER_PORT..=MAX_PORT).contains(&n) {
        Some(n as u16)
    } else {
        None
    }
}

fn settings_path(app: &AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir()).join("settings.json")
}

fn read_settings(app: &AppHandle) -> Value {
    let mut merged = default_settings();
    if let Ok(text) = std::fs::read_to_string(settings_path(app)) {
        if let Ok(Value::Object(existing)) = serde_json::from_str::<Value>(&text) {
            if let Value::Object(base) = &mut merged {
                for (k, v) in existing {
                    base.insert(k, v);
                }
            }
        }
    }
    merged
}

fn write_settings(app: &AppHandle, patch: Value) -> Value {
    let mut current = read_settings(app);
    if let (Value::Object(cur_map), Value::Object(patch_map)) = (&mut current, patch) {
        for (k, v) in patch_map {
            cur_map.insert(k, v);
        }
    }
    let path = settings_path(app);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&current).unwrap_or_default());
    current
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let mgr = app.autolaunch();
    let result = if enabled { mgr.enable() } else { mgr.disable() };
    if let Err(err) = result {
        tracing::warn!("[autostart] could not update: {err}");
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn app_info(app: AppHandle) -> Value {
    json!({
        "version": app.package_info().version.to_string(),
        "platform": std::env::consts::OS,
    })
}

#[tauri::command]
async fn bridge_state(state: State<'_, PaxManaged>) -> Result<BridgeStatus, String> {
    Ok(state.runtime.status.lock().await.clone())
}

#[tauri::command]
async fn bridge_start(app: AppHandle, state: State<'_, PaxManaged>) -> Result<BridgeStatus, String> {
    start_bridge_internal(app, state.runtime.clone()).await;
    Ok(state.runtime.status.lock().await.clone())
}

#[tauri::command]
async fn bridge_stop(app: AppHandle, state: State<'_, PaxManaged>) -> Result<BridgeStatus, String> {
    stop_bridge_internal(app, state.runtime.clone()).await;
    Ok(state.runtime.status.lock().await.clone())
}

#[tauri::command]
async fn bridge_restart(app: AppHandle, state: State<'_, PaxManaged>) -> Result<BridgeStatus, String> {
    restart_bridge_internal(app, state.runtime.clone()).await;
    Ok(state.runtime.status.lock().await.clone())
}

#[tauri::command]
async fn bridge_logs(state: State<'_, PaxManaged>) -> Result<Vec<LogEntry>, String> {
    Ok(state.runtime.logs.lock().await.clone())
}

#[tauri::command]
fn settings_get(app: AppHandle) -> Value {
    read_settings(&app)
}

#[tauri::command]
fn settings_set(app: AppHandle, patch: Value) -> Result<Value, String> {
    let mut patch = patch;
    if let Value::Object(map) = &mut patch {
        if let Some(port_val) = map.get("port") {
            match valid_port(port_val) {
                Some(port) => {
                    // Takes effect on the next start/restart — config::port() reads
                    // this env var fresh every time the bridge starts.
                    std::env::set_var("PORT", port.to_string());
                }
                None => {
                    return Err(format!(
                        "Port must be a number between {MIN_USER_PORT} and {MAX_PORT}."
                    ));
                }
            }
        }
    }
    let next = write_settings(&app, patch);
    if let Some(enabled) = next.get("launchAtLogin").and_then(Value::as_bool) {
        apply_autostart(&app, enabled);
    }
    Ok(next)
}

fn open_with_system(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(target).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/C", "start", "", target]).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn open_user_data(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    open_with_system(&dir.to_string_lossy())
}

#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    open_with_system(&url)
}

#[tauri::command]
fn quit_app(app: AppHandle, state: State<'_, PaxManaged>) {
    state.is_quitting.store(true, Ordering::SeqCst);
    app.exit(0);
}

#[tauri::command]
async fn logs_download(app: AppHandle, state: State<'_, PaxManaged>) -> Result<Value, String> {
    let logs = state.runtime.logs.lock().await.clone();
    if logs.is_empty() {
        return Ok(json!({ "ok": false, "reason": "empty" }));
    }
    let content = logs
        .iter()
        .map(|e| {
            let time = chrono::DateTime::from_timestamp_millis(e.ts).map(|d| d.to_rfc3339()).unwrap_or_default();
            format!("{time} [{}] {}", e.stream.to_uppercase(), e.line)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let default_name = format!("salesgent-pax-bridge-logs-{}.log", chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S"));
    let dir = app.path().download_dir().or_else(|_| app.path().app_data_dir()).map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    let path_buf = dir.join(default_name);
    std::fs::write(&path_buf, content).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "filePath": path_buf.to_string_lossy() }))
}

#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Value, String> {
    let current = app.package_info().version.to_string();
    let _ = app.emit("update:event", json!({ "type": "none", "current": current }));
    Ok(json!({ "available": false, "disabled": true }))
}

#[tauri::command]
async fn update_download() -> Result<(), String> {
    Err("In-app updates are disabled in this build. Download the latest release from GitHub.".into())
}

#[tauri::command]
fn update_install() {}

// ---------------------------------------------------------------------------
// App bootstrap
// ---------------------------------------------------------------------------

pub fn run() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .invoke_handler(tauri::generate_handler![
            app_info,
            bridge_state,
            bridge_start,
            bridge_stop,
            bridge_restart,
            bridge_logs,
            settings_get,
            settings_set,
            open_user_data,
            open_external,
            quit_app,
            logs_download,
            update_check,
            update_download,
            update_install,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // All bridge state (config + JSON db) lives under the OS-standard app
            // data dir unless the environment already pins PAX_HOME (e.g. tests).
            if std::env::var("PAX_HOME").ok().filter(|v| !v.trim().is_empty()).is_none() {
                if let Ok(dir) = app.path().app_data_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    std::env::set_var("PAX_HOME", &dir);
                }
            }

            let runtime = Arc::new(BridgeRuntime::new());
            let is_quitting = Arc::new(AtomicBool::new(false));
            app.manage(PaxManaged { runtime: runtime.clone(), is_quitting: is_quitting.clone() });

            // --- Tray icon: Open / Restart bridge / Quit ---
            // This is the ONLY tray icon. Do not also declare `app.trayIcon` in
            // tauri.conf.json: that makes Tauri spawn a second, menu-less icon
            // next to this one (same image, does nothing when clicked).
            let settings = read_settings(app);
            let show_tray = settings.get("showTrayIcon").and_then(Value::as_bool).unwrap_or(true);

            if show_tray {
                let open_item = MenuItem::with_id(app, "open", "Open window", true, None::<&str>)?;
                let restart_item = MenuItem::with_id(app, "restart", "Restart bridge", true, None::<&str>)?;
                let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
                let tray_menu = Menu::with_items(app, &[&open_item, &restart_item, &quit_item])?;

                let is_quitting_for_tray = is_quitting.clone();
                TrayIconBuilder::with_id("main-tray")
                    .icon(app.default_window_icon().cloned().expect("default window icon should be embedded via tauri.conf.json bundle.icon"))
                    .tooltip("Salesgent Pax Bridge")
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app, event| match event.id.as_ref() {
                        "open" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "restart" => {
                            let app2 = app.clone();
                            let rt2 = app.state::<PaxManaged>().runtime.clone();
                            tauri::async_runtime::spawn(async move { restart_bridge_internal(app2, rt2).await });
                        }
                        "quit" => {
                            is_quitting_for_tray.store(true, Ordering::SeqCst);
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click { .. } = event {
                            let app = tray.app_handle();
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    })
                    .build(app)?;
            }

            // --- Apply persisted settings ---
            let settings = read_settings(&handle);
            let start_on_launch = settings.get("startBridgeOnLaunch").and_then(Value::as_bool).unwrap_or(true);
            let launch_at_login = settings.get("launchAtLogin").and_then(Value::as_bool).unwrap_or(false);
            apply_autostart(&handle, launch_at_login);
            // A saved port (if the merchant picked a non-default one) must be in
            // the environment before the bridge's first start, since config::port()
            // just reads $PORT.
            if let Some(port) = settings.get("port").and_then(valid_port) {
                std::env::set_var("PORT", port.to_string());
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            if start_on_launch {
                let app2 = handle.clone();
                let rt2 = runtime.clone();
                tauri::async_runtime::spawn(async move { start_bridge_internal(app2, rt2).await });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let app = window.app_handle();
                    let is_quitting = app.state::<PaxManaged>().is_quitting.load(Ordering::SeqCst);
                    let minimize_to_tray = read_settings(app).get("minimizeToTray").and_then(Value::as_bool).unwrap_or(true);
                    if minimize_to_tray && !is_quitting {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the Salesgent Pax Bridge app")
        .run(|_app_handle, event| {
            if let RunEvent::ExitRequested { .. } = event {
                // Nothing to veto here — quitting always tears the bridge down via
                // the OS killing the process; graceful shutdown isn't required.
            }
        });
}
