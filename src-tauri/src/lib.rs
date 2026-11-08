mod audio;
pub mod config;
pub mod decode;
mod history;
#[cfg(any(target_os = "macos", windows))]
mod local;
mod player;
mod recording;
pub mod realtime;
pub mod pulse;
pub mod resample;
pub mod volc;
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
pub struct AppState {
    running: Mutex<Option<Arc<AtomicBool>>>,
    capture: Mutex<Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>>,
    exiting: AtomicBool,
    level: audio::InputLevel,
    #[cfg(any(target_os = "macos", windows))]
    local_models: Arc<Mutex<local::LocalModelCache>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    Device { id: String, #[serde(default)] record: bool, #[serde(default)] format: RecordingFormat },
    File { path: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingFormat {
    #[default]
    Wav,
    Mp3,
}

impl Source {
    fn recording_extension(&self) -> Option<&'static str> {
        match self {
            Self::Device { id, record: true, format }
                if id.starts_with("mic|") || (cfg!(any(target_os = "macos", windows)) && id.starts_with("sys|")) => Some(match format {
                RecordingFormat::Wav => "wav",
                RecordingFormat::Mp3 => "mp3",
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod recording_source_tests {
    use super::*;

    #[test]
    fn old_device_requests_default_to_wav_and_mp3_can_be_selected() {
        let old: Source = serde_json::from_value(serde_json::json!({
            "kind": "device", "id": "mic|default", "record": true
        })).unwrap();
        let mp3: Source = serde_json::from_value(serde_json::json!({
            "kind": "device", "id": "mic|default", "record": true, "format": "mp3"
        })).unwrap();
        assert_eq!(old.recording_extension(), Some("wav"));
        assert_eq!(mp3.recording_extension(), Some("mp3"));
    }

    #[test]
    fn system_audio_can_be_recorded_only_when_enabled() {
        let system: Source = serde_json::from_value(serde_json::json!({
            "kind": "device", "id": "sys|default", "record": true, "format": "mp3"
        })).unwrap();
        let disabled: Source = serde_json::from_value(serde_json::json!({
            "kind": "device", "id": "sys|default", "record": false, "format": "mp3"
        })).unwrap();
        assert_eq!(system.recording_extension(), cfg!(any(target_os = "macos", windows)).then_some("mp3"));
        assert_eq!(disabled.recording_extension(), None);
    }
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
fn input_level(state: State<AppState>) -> audio::InputLevels {
    audio::read_levels(&state.level)
}

fn recordings_dir(app: AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

#[tauri::command]
fn open_recordings_dir(app: AppHandle) -> Result<(), String> {
    let dir = recordings_dir(app.clone())?;
    app.opener().open_path(dir, None::<&str>).map_err(|e| e.to_string())
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
        let translation_path = (!settings.transcribe_only).then(|| PathBuf::from(&settings.translation_model_path));
        tauri::async_runtime::spawn_blocking(move || {
            models.lock().unwrap().load_for(&whisper_path, translation_path.as_deref())
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
    // Live local capture needs headroom for GPU stalls; file decoding blocks when its
    // smaller queue fills instead of reading far ahead of ASR.
    let queue_chunks = if settings.mode == ExecutionMode::Local {
        if matches!(&source, Source::File { .. }) { 32 } else { 256 }
    } else { 64 };
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<i16>>(queue_chunks);

    #[cfg(any(target_os = "macos", windows))]
    let live_capture = matches!(&source, Source::Device { .. });
    let recording_extension = source.recording_extension();
    match source {
        Source::Device { id, .. } => {
            let recording_path = if let Some(extension) = recording_extension {
                let dir = PathBuf::from(recordings_dir(app.clone())?);
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?.as_nanos();
                let prefix = if id.starts_with("sys|") { "system" } else { "microphone" };
                Some(dir.join(format!("{prefix}-{stamp}.{extension}")))
            } else { None };
            let recording_name = recording_path.as_ref().and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            let app_recording = app.clone();
            // Only needed when we make sound ourselves; otherwise the plain loopback path.
            let capture = audio::start(&id, settings.sample_rate(), tx, state.level.clone(), dropped.clone(), stop.clone(), settings.speak, recording_path,
                move |result| {
                    let event = match result {
                        Ok(path) => serde_json::json!({ "status": "saved", "file": path.file_name().unwrap_or_default().to_string_lossy() }),
                        Err(error) => {
                            realtime::note(&app_recording, "warn", format!("Audio recording was not saved: {error}"));
                            serde_json::json!({ "status": "error", "message": error.to_string() })
                        }
                    };
                    let _ = app_recording.emit("audio://recording", event);
                })
                .map_err(|e| e.to_string())?;
            if let Some(capture) = capture {
                *state.capture.lock().unwrap() = Some((stop.clone(), capture));
            }
            if let Some(file) = recording_name {
                let _ = app.emit("audio://recording", serde_json::json!({ "status": "recording", "file": file }));
            }
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
        join_capture(state, &stop);
    }
    state.level.reset();
}

fn join_capture(state: &AppState, stop: &Arc<AtomicBool>) {
    let capture = {
        let mut capture = state.capture.lock().unwrap();
        if capture.as_ref().is_some_and(|(current, _)| Arc::ptr_eq(current, stop)) {
            capture.take()
        } else { None }
    };
    if let Some((_, handle)) = capture { let _ = handle.join(); }
}

pub(crate) fn request_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.exiting.store(true, Ordering::SeqCst);
    let stop = state.running.lock().unwrap().clone();
    if let Some(stop) = stop {
        stop.store(true, Ordering::Relaxed);
        join_capture(&state, &stop);
        state.level.reset();
    } else {
        finish_shutdown(app, &state);
    }
}

fn finish_session(app: &AppHandle, stop: &Arc<AtomicBool>) {
    let state = app.state::<AppState>();
    stop.store(true, Ordering::Relaxed);
    join_capture(&state, stop);
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
    if let Some((stop, handle)) = state.capture.lock().unwrap().take() {
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }
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
            open_recordings_dir,
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
            history::history_open_dir,
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
