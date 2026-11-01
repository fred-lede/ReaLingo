mod audio;
pub mod config;
pub mod decode;
mod history;
#[cfg(any(target_os = "macos", windows))]
mod local;
mod player;
pub mod realtime;
pub mod pulse;
pub mod resample;
mod secret;
#[cfg(target_os = "macos")]
mod tap;
mod tray;
#[cfg(windows)]
mod wasapi;

use audio::DeviceInfo;
use config::{ExecutionMode, Protocol, Settings};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct AppState {
    running: Mutex<Option<Arc<AtomicBool>>>,
    exiting: AtomicBool,
    level: Arc<AtomicU32>,
    #[cfg(any(target_os = "macos", windows))]
    local_models: Arc<Mutex<local::LocalModelCache>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    Device { id: String },
    File { path: String },
}

#[tauri::command]
fn list_devices() -> Vec<DeviceInfo> {
    audio::list_devices()
}

/// What the host platform can and cannot do, so the UI can say why a list is empty.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
    pub loopback: bool,
    /// False on a Linux box with no Secret Service provider: the key then stays in
    /// settings.json, and the settings page says so.
    pub keyring: bool,
}

#[tauri::command]
fn platform() -> Platform {
    Platform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        loopback: audio::loopback_supported(),
        keyring: secret::available(),
    }
}

#[tauri::command]
fn get_api_key(protocol: Protocol) -> String {
    secret::get(protocol).unwrap_or_default()
}

#[tauri::command]
fn set_api_key(protocol: Protocol, key: String) -> Result<(), String> {
    secret::set(protocol, &key)
}

#[tauri::command]
fn default_device(loopback: bool) -> Option<String> {
    audio::default_device_id(loopback)
}

#[tauri::command]
fn input_level(state: State<AppState>) -> f32 {
    audio::read_level(&state.level)
}

/// Shown read-only in Settings so it is obvious which host the app will dial.
#[tauri::command]
fn endpoint_url(settings: Settings) -> String {
    settings.ws_url()
}

#[tauri::command]
async fn start_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
    source: Source,
    target_language_name: String,
) -> Result<(), String> {
    halt(&state);

    #[cfg(target_os = "linux")]
    if settings.mode == ExecutionMode::Local {
        return Err("Local inference is supported on Apple Silicon macOS and Windows only".into());
    }

    #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
    if settings.mode == ExecutionMode::Local {
        return Err("Local inference requires Apple Silicon on macOS".into());
    }

    #[cfg(any(target_os = "macos", windows))]
    if settings.mode == ExecutionMode::Local {
        let models = state.local_models.clone();
        let whisper_path = PathBuf::from(&settings.whisper_model_path);
        let translation_path = PathBuf::from(&settings.translation_model_path);
        tauri::async_runtime::spawn_blocking(move || {
            models.lock().unwrap().load_for(&whisper_path, &translation_path)
        })
        .await
        .map_err(|e| e.to_string())??;
    }

    // The credential store wins over whatever the webview sent, so on a machine that has
    // one the key never has to travel through the frontend at all.
    if settings.mode == ExecutionMode::Remote {
        if let Some(key) = secret::get(settings.protocol) {
            *settings.key_mut() = key;
        }
    }

    // Before capture: on macOS the process has to be a Core Audio client already for the
    // capture to be able to exclude it.
    let player = if settings.mode == ExecutionMode::Remote && settings.speak {
        Some(player::Player::start().map_err(|e| e.to_string())?)
    } else {
        None
    };

    let stop = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicU64::new(0));
    // Local ASR can stall briefly on GPU work; keep more capture audio in flight there.
    // Sustained processing slower than realtime still reports dropped audio.
    let queue_chunks = if settings.mode == ExecutionMode::Local { 256 } else { 64 };
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<i16>>(queue_chunks);

    #[cfg(any(target_os = "macos", windows))]
    let live_capture = matches!(&source, Source::Device { .. });
    match source {
        Source::Device { id } => {
            // Only needed when we make sound ourselves; otherwise the plain loopback path.
            audio::start(&id, settings.sample_rate(), tx, state.level.clone(), dropped.clone(), stop.clone(), settings.speak)
                .map_err(|e| e.to_string())?;
            let (app2, stop2) = (app.clone(), stop.clone());
            tauri::async_runtime::spawn(async move {
                let mut reported = 0;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let total = dropped.load(Ordering::Relaxed);
                    if total > reported {
                        let delta = total - reported;
                        let _ = app2.emit(realtime::RAW_EVENT, serde_json::json!({
                            "type": "audio.capture_dropped",
                            "chunks": delta,
                            "totalChunks": total,
                        }));
                        realtime::note(&app2, "warn", format!("Audio input dropped {delta} chunks while processing fell behind"));
                        reported = total;
                    }
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                }
            });
        }
        Source::File { path } => {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(format!("file not found: {}", path.display()));
            }
            let (app2, stop2, rate) = (app.clone(), stop.clone(), settings.sample_rate());
            tauri::async_runtime::spawn_blocking(move || {
                let result = decode::stream_file(&path, rate, tx, stop2, |sent, total| {
                    let text = match total {
                        Some(t) => format!("{sent:.1}/{t:.1}"),
                        None => format!("{sent:.1}/?"),
                    };
                    realtime::note(&app2, "progress", text);
                });
                match result {
                    Ok(()) => realtime::note(&app2, "progress", "done"),
                    Err(e) => realtime::note(&app2, "error", e.to_string()),
                }
                // `tx` drops here, which is the end of input: the session finishes and
                // waits for the last sentence on its own.
            });
        }
    }

    *state.running.lock().unwrap() = Some(stop.clone());
    #[cfg(any(target_os = "macos", windows))]
    if settings.mode == ExecutionMode::Local {
        let models = state.local_models.clone();
        let app2 = app.clone();
        let stop2 = stop.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = {
                let mut models = models.lock().unwrap();
                local::LocalSession::run(
                    &app,
                    &settings,
                    &target_language_name,
                    live_capture,
                    rx,
                    &stop,
                    models.models_mut(),
                )
            };
            if let Err(error) = result {
                realtime::note(&app, "error", error);
            }
            realtime::emit(
                &app,
                realtime::Event {
                    kind: "status",
                    text: "closed".into(),
                    done: true,
                    ..Default::default()
                },
            );
            finish_session(&app2, &stop2);
        });
    } else {
        let app2 = app.clone();
        let stop2 = stop.clone();
        tauri::async_runtime::spawn(async move {
            realtime::run(app, settings, rx, stop, player).await;
            finish_session(&app2, &stop2);
        });
    }
    #[cfg(target_os = "linux")]
    {
        let app2 = app.clone();
        let stop2 = stop.clone();
        tauri::async_runtime::spawn(async move {
            realtime::run(app, settings, rx, stop, player).await;
            finish_session(&app2, &stop2);
        });
    }
    Ok(())
}

/// Writes a transcript export. The path comes from the user's own save dialog, so there is
/// no scope to police here beyond what the OS already enforces.
#[tauri::command]
fn write_text(path: String, contents: String) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_stream(state: State<AppState>) {
    halt(&state);
}

fn halt(state: &State<AppState>) {
    if let Some(stop) = state.running.lock().unwrap().take() {
        stop.store(true, Ordering::Relaxed);
    }
    state.level.store(0f32.to_bits(), Ordering::Relaxed);
}

pub(crate) fn request_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.exiting.store(true, Ordering::SeqCst);
    let stop = state.running.lock().unwrap().clone();
    if let Some(stop) = stop {
        stop.store(true, Ordering::Relaxed);
        state.level.store(0f32.to_bits(), Ordering::Relaxed);
    } else {
        finish_shutdown(app, &state);
    }
}

fn finish_session(app: &AppHandle, stop: &Arc<AtomicBool>) {
    let state = app.state::<AppState>();
    {
        let mut running = state.running.lock().unwrap();
        if running.as_ref().is_some_and(|current| Arc::ptr_eq(current, stop)) {
            running.take();
        }
        if running.is_some() || !state.exiting.load(Ordering::SeqCst) {
            return;
        }
    }
    finish_shutdown(app, &state);
}

fn finish_shutdown(app: &AppHandle, state: &AppState) {
    #[cfg(any(target_os = "macos", windows))]
    state.local_models.lock().unwrap().clear();
    app.exit(0);
}

#[cfg(target_os = "macos")]
fn configure_metal_backend() {
    // ggml's Metal device finalizer aborts if a residency set survives native app exit.
    // This must be set before either local model initializes its Metal backend.
    std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    configure_metal_backend();
    // WebKitGTK's DMA-BUF renderer produces nothing on software GL (VMs) and on some
    // proprietary drivers: the window is created and focused but never painted, which with
    // `transparent: true` looks like the app failed to start. Losing the DMA-BUF path costs
    // a buffer copy; losing the whole UI costs the app. Respect an explicit override.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::default())
        .setup(|app| {
            // The overlay is declared in tauri.conf.json but starts hidden; the main window
            // shows it on demand.
            if let Some(w) = app.get_webview_window("subtitle") {
                let _ = w.set_always_on_top(true);
            }
            tray::init(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window must end the app. The subtitle overlay is a second
            // window with skipTaskbar set, so without this the process lingers with no way
            // to reach it: the app looks closed but the overlay is still on screen.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    request_exit(&window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            platform,
            get_api_key,
            set_api_key,
            default_device,
            input_level,
            endpoint_url,
            start_stream,
            stop_stream,
            write_text,
            history::history_open,
            history::history_append,
            history::history_list,
            history::history_read,
            history::history_delete,
            history::history_clear,
            history::history_dir,
            tray::sync_tray
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = app.state::<AppState>();
            if !state.exiting.load(Ordering::SeqCst) {
                api.prevent_exit();
                request_exit(app);
            }
        }
    });
}
