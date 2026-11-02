use std::{ffi::{c_char, c_void, CStr}, sync::{atomic::{AtomicBool, Ordering}, Mutex, Once}};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendStatus {
    Detecting,
    Metal,
    Cuda,
    Cpu,
    Unknown,
}

pub fn gpu_backend_for_platform() -> BackendStatus {
    if cfg!(target_os = "macos") { BackendStatus::Metal } else { BackendStatus::Cuda }
}

pub fn classify_whisper(lines: &[&str], forced_cpu: bool) -> BackendStatus {
    if forced_cpu { return BackendStatus::Cpu; }
    let lines: Vec<_> = lines.iter().map(|line| line.to_ascii_lowercase()).collect();
    if lines.iter().any(|line| line.contains("failed to initialize") && line.contains("backend"))
        || lines.iter().any(|line| line.contains("no gpu found")) {
        return BackendStatus::Cpu;
    }
    if lines.iter().any(|line| line.contains("using metal backend") || (line.contains("using mtl") && line.contains(" backend"))) { BackendStatus::Metal }
    else if lines.iter().any(|line| line.contains("using cuda") && line.contains(" backend")) { BackendStatus::Cuda }
    else { BackendStatus::Unknown }
}

pub fn classify_llama(lines: &[&str], forced_cpu: bool) -> BackendStatus {
    if forced_cpu { return BackendStatus::Cpu; }
    let count = lines.iter().find_map(|line| {
        if !line.contains("layers to GPU") { return None; }
        line.split("offloaded ").nth(1)?.split('/').next()?.parse::<u32>().ok()
    });
    match count { Some(0) => BackendStatus::Cpu, Some(_) => gpu_backend_for_platform(), None => BackendStatus::Unknown }
}

#[cfg(test)]
mod tests {
    use super::{classify_llama, classify_whisper, gpu_backend_for_platform, BackendStatus};

    #[test]
    fn whisper_requires_initialized_gpu_evidence() {
        assert_eq!(classify_whisper(&["whisper_backend_init_gpu: using Metal backend"], false), BackendStatus::Metal);
        assert_eq!(classify_whisper(&["whisper_backend_init_gpu: using MTL0 backend"], false), BackendStatus::Metal);
        assert_eq!(classify_whisper(&["whisper_backend_init_gpu: using CUDA0 backend"], false), BackendStatus::Cuda);
        assert_eq!(classify_whisper(&["whisper_backend_init_gpu: no GPU found"], false), BackendStatus::Cpu);
        assert_eq!(classify_whisper(&["whisper_backend_init_gpu: using Metal backend", "whisper_backend_init_gpu: failed to initialize Metal backend"], false), BackendStatus::Cpu);
        assert_eq!(classify_whisper(&["unrecognized native output"], false), BackendStatus::Unknown);
    }

    #[test]
    fn llama_requires_positive_offload_count() {
        assert_eq!(classify_llama(&["llama_model_load: offloaded 0/33 layers to GPU"], false), BackendStatus::Cpu);
        assert_eq!(classify_llama(&["llama_model_load: offloaded 33/33 layers to GPU"], false), gpu_backend_for_platform());
        assert_eq!(classify_llama(&["unrecognized native output"], false), BackendStatus::Unknown);
        assert_eq!(classify_llama(&[], true), BackendStatus::Cpu);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires local models"]
    fn native_backend_evidence_from_real_models() {
        use super::{capture_llama, capture_whisper};
        use llama_cpp_2::{context::params::LlamaContextParams, llama_backend::LlamaBackend, model::{params::LlamaModelParams, LlamaModel}};
        use std::path::Path;
        use whisper_rs::{WhisperContext, WhisperContextParameters};

        crate::configure_metal_backend();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let whisper = repo.join("models/whisper/ggml-small.en-q8_0.bin");
        let mut params = WhisperContextParameters::default();
        params.use_gpu(false);
        let context = WhisperContext::new_with_params(whisper.to_str().unwrap(), params).unwrap();
        let (state, evidence) = capture_whisper(|| context.create_state());
        let _state = state.unwrap();
        let lines: Vec<_> = evidence.iter().map(String::as_str).collect();
        eprintln!("Whisper backend evidence: {lines:?}");
        assert_eq!(classify_whisper(&lines, true), BackendStatus::Cpu);

        let backend = LlamaBackend::init().unwrap();
        let qwen = repo.join("models/translation/Qwen3-4B-Q4_K_M.gguf");
        let cpu_params = LlamaModelParams::default().with_n_gpu_layers(0).with_devices(&[]).unwrap();
        let (model, evidence) = capture_llama(|| LlamaModel::load_from_file(&backend, &qwen, &cpu_params));
        let lines: Vec<_> = evidence.iter().map(String::as_str).collect();
        eprintln!("GGUF CPU evidence: {lines:?}");
        assert_eq!(classify_llama(&lines, true), BackendStatus::Cpu);
        let model = model.unwrap();
        let context_params = LlamaContextParams::default().with_n_ctx(std::num::NonZeroU32::new(512));
        let _context = model.new_context(&backend, context_params).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires a local Whisper model and Metal device"]
    fn whisper_metal_backend_evidence_from_real_model() {
        use super::capture_whisper;
        use std::path::Path;
        use whisper_rs::{WhisperContext, WhisperContextParameters};

        crate::configure_metal_backend();
        let model = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("models/whisper/ggml-small.en-q8_0.bin");
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);
        let context = WhisperContext::new_with_params(model.to_str().unwrap(), params).unwrap();
        let (state, evidence) = capture_whisper(|| context.create_state());
        let _state = state.unwrap();
        let lines: Vec<_> = evidence.iter().map(String::as_str).collect();
        eprintln!("Whisper Metal backend evidence: {lines:?}");
        assert_eq!(classify_whisper(&lines, false), BackendStatus::Metal);
    }
}
static WHISPER_CAPTURE_LOCK: Mutex<()> = Mutex::new(());
static WHISPER_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WHISPER_ACTIVE: AtomicBool = AtomicBool::new(false);
static LLAMA_CAPTURE_LOCK: Mutex<()> = Mutex::new(());
static LLAMA_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LLAMA_ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn whisper_callback(_level: whisper_rs_sys::ggml_log_level, text: *const c_char, _data: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if text.is_null() { return; }
        let line = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        eprint!("{line}");
        if WHISPER_ACTIVE.load(Ordering::Acquire) && line.contains("whisper_backend_init_gpu:") {
            if let Ok(mut lines) = WHISPER_LINES.lock() {
                if lines.len() < 32 { lines.push(line.into_owned()); }
            }
        }
    });
}

unsafe extern "C" fn llama_callback(_level: llama_cpp_sys_2::ggml_log_level, text: *const c_char, _data: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if text.is_null() { return; }
        let line = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        eprint!("{line}");
        if LLAMA_ACTIVE.load(Ordering::Acquire) && line.contains("offloaded ") {
            if let Ok(mut lines) = LLAMA_LINES.lock() {
                if lines.len() < 32 { lines.push(line.into_owned()); }
            }
        }
    });
}

pub fn capture_whisper<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe { whisper_rs::set_log_callback(Some(whisper_callback), std::ptr::null_mut()) });
    let _lock = WHISPER_CAPTURE_LOCK.lock().unwrap();
    WHISPER_LINES.lock().unwrap().clear();
    WHISPER_ACTIVE.store(true, Ordering::Release);
    let result = work();
    WHISPER_ACTIVE.store(false, Ordering::Release);
    (result, std::mem::take(&mut *WHISPER_LINES.lock().unwrap()))
}

pub fn capture_llama<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe { llama_cpp_sys_2::llama_log_set(Some(llama_callback), std::ptr::null_mut()) });
    let _lock = LLAMA_CAPTURE_LOCK.lock().unwrap();
    LLAMA_LINES.lock().unwrap().clear();
    LLAMA_ACTIVE.store(true, Ordering::Release);
    let result = work();
    LLAMA_ACTIVE.store(false, Ordering::Release);
    (result, std::mem::take(&mut *LLAMA_LINES.lock().unwrap()))
}
