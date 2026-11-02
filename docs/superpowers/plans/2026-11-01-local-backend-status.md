# Local Inference Backend Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the verified backend used by local Whisper ASR and GGUF translation during each run.

**Architecture:** Capture the pinned native libraries' backend initialization evidence at model/state creation, classify it into a small Rust enum, and emit one diagnostic event per stage through the existing `rt://raw` channel. The frontend adds two values to the existing local performance panel. Unverifiable native output becomes `unknown`, never an inferred GPU claim.

**Tech Stack:** Rust/Tauri, `whisper-rs 0.16.0`, `llama-cpp-2 0.1.160`, their matching sys crates, Vue 3/TypeScript.

**Spec:** `docs/superpowers/specs/2026-11-01-local-backend-status-design.md`

## Global Constraints

- macOS Apple Silicon labels are Metal or CPU; Windows NVIDIA labels are CUDA or CPU.
- Whisper and GGUF report separately. Transcription only does not load or display a GGUF backend.
- A GPU label requires observed native GPU initialization; GGUF additionally requires at least one offloaded layer and a successfully initialized model context.
- An unrecognized or absent native report produces `unknown`, not an inferred CPU or GPU label.
- Do not add settings, persistence, GPU percentage, temperature, or remote-provider behavior.
- Do not track model files. Do not commit or push without a separate explicit user request.

## Review Focus

- GPU requested but Whisper silently initializes CPU: Task 1's native probe and parser test must label CPU or unknown, never Metal/CUDA.
- GGUF load succeeds with zero offloaded layers: Task 1's parser test must label CPU, not Metal/CUDA.
- GPU load fails before the GGUF CPU retry: Task 2's parameter test and real-model probe must confirm `n_gpu_layers(0)` is used and CPU is reported only after that load succeeds.
- A model is reused from `LocalModelCache`: Task 3's test must re-emit its retained backend for the new run.
- A user switches to Transcription only and starts again: Task 4's diagnostic-state test and UI smoke check must reset the old translation status and hide its row.

---

### Task 1: Prove and classify native backend evidence

**Files:**
- Create: `src-tauri/src/local/backend_status.rs`
- Modify: `src-tauri/src/local/mod.rs` (module declaration only)
- Modify: `src-tauri/Cargo.toml` (add `whisper-rs-sys = "0.15.0"` and `llama-cpp-sys-2 = "0.1.160"` to the macOS and Windows dependency sections)
- Test: `src-tauri/src/local/backend_status.rs`

**Interfaces:**
- Produces: `BackendStatus` (`Detecting`, `Metal`, `Cuda`, `Cpu`, `Unknown`), `capture_whisper`, `capture_llama`, `classify_whisper`, and `classify_llama` for Task 2.
- `capture_*` accepts one synchronous closure, returns its result plus bounded native evidence, and preserves existing native logging. Keep callbacks scoped to their own library so concurrent Whisper and llama output cannot be confused.

- [ ] **Step 1: Add failing parser tests.** Cover `using MTL0 backend` (the observed macOS log), `using Metal backend`, `using CUDA0 backend`, `no GPU found`, unrecognized output, and `offloaded 0/33` versus `offloaded 33/33`.

```rust
assert_eq!(classify_whisper(&["whisper_backend_init_gpu: using Metal backend"], false), BackendStatus::Metal);
assert_eq!(classify_whisper(&["whisper_backend_init_gpu: no GPU found"], false), BackendStatus::Cpu);
assert_eq!(classify_whisper(&["unexpected native message"], false), BackendStatus::Unknown);
assert_eq!(classify_llama(&["llama_model_load: offloaded 0/33 layers to GPU"], false), BackendStatus::Cpu);
assert_eq!(classify_llama(&["llama_model_load: offloaded 33/33 layers to GPU"], false), gpu_backend_for_platform());
```

- [ ] **Step 2: Run `cargo test local::backend_status::tests -- --nocapture` from `src-tauri`; confirm the new tests fail before implementation.**
- [ ] **Step 3: Implement the enum and the two parsers.** A CPU claim based on `no GPU found` is valid only after state initialization succeeds; pass `forced_cpu=true` only after an explicit CPU load succeeds. For llama, parse the integer before `/` following `offloaded ` and require a positive count for GPU.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendStatus { Detecting, Metal, Cuda, Cpu, Unknown }

pub fn gpu_backend_for_platform() -> BackendStatus {
    if cfg!(target_os = "macos") { BackendStatus::Metal } else { BackendStatus::Cuda }
}

pub fn classify_whisper(lines: &[&str], forced_cpu: bool) -> BackendStatus {
    if forced_cpu { return BackendStatus::Cpu; }
    let evidence: Vec<_> = lines.iter().map(|line| line.to_ascii_lowercase()).collect();
    if evidence.iter().any(|line| line.contains("using metal backend")) { BackendStatus::Metal }
    else if evidence.iter().any(|line| line.contains("using cuda") && line.contains(" backend")) { BackendStatus::Cuda }
    else if evidence.iter().any(|line| line.contains("no gpu found")) { BackendStatus::Cpu }
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
```

- [ ] **Step 4: Add Whisper log capture.** Install `whisper_rs::set_log_callback` once. Its `extern "C"` callback converts valid C strings, mirrors native messages to stderr, and keeps at most 32 backend-related lines only while `WHISPER_ACTIVE` is true. Guard the callback body against unwinding. `capture_whisper` locks the capture window around one `create_state()` call and returns those lines with the call result.

```rust
static WHISPER_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static WHISPER_LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static WHISPER_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn install_whisper_callback_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        whisper_rs::set_log_callback(Some(whisper_callback), std::ptr::null_mut());
    });
}

unsafe extern "C" fn whisper_callback(
    _level: whisper_rs_sys::ggml_log_level,
    text: *const std::ffi::c_char,
    _user_data: *mut std::ffi::c_void,
) {
    let _ = std::panic::catch_unwind(|| {
        if text.is_null() { return; }
        let line = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
        eprint!("{line}");
        if WHISPER_ACTIVE.load(std::sync::atomic::Ordering::Acquire)
            && (line.contains("using ") || line.contains("no GPU found"))
        {
            if let Ok(mut lines) = WHISPER_LINES.lock() {
                if lines.len() < 32 { lines.push(line.into_owned()); }
            }
        }
    });
}

pub fn capture_whisper<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
    install_whisper_callback_once();
    let _window = WHISPER_CAPTURE_LOCK.lock().unwrap();
    WHISPER_LINES.lock().unwrap().clear();
    WHISPER_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    let result = work();
    WHISPER_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    (result, std::mem::take(&mut *WHISPER_LINES.lock().unwrap()))
}
```

- [ ] **Step 5: Add llama log capture.** Install `llama_cpp_sys_2::llama_log_set` once with a separate callback and 32-line buffer. Mirror logs to stderr, keep only offload lines, and use an independent mutex around one `LlamaModel::load_from_file` call. Expose `capture_llama` with the same closure/result interface as `capture_whisper`. Native callbacks must not unwind or retain full model logs.

```rust
static LLAMA_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static LLAMA_LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static LLAMA_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn llama_callback(
    _level: llama_cpp_sys_2::ggml_log_level,
    text: *const std::ffi::c_char,
    _user_data: *mut std::ffi::c_void,
) {
    let _ = std::panic::catch_unwind(|| {
        if text.is_null() { return; }
        let line = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
        eprint!("{line}");
        if LLAMA_ACTIVE.load(std::sync::atomic::Ordering::Acquire) && line.contains("offloaded ") {
            if let Ok(mut lines) = LLAMA_LINES.lock() {
                if lines.len() < 32 { lines.push(line.into_owned()); }
            }
        }
    });
}

fn install_llama_callback_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        llama_cpp_sys_2::llama_log_set(Some(llama_callback), std::ptr::null_mut());
    });
}

pub fn capture_llama<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
    install_llama_callback_once();
    let _window = LLAMA_CAPTURE_LOCK.lock().unwrap();
    LLAMA_LINES.lock().unwrap().clear();
    LLAMA_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    let result = work();
    LLAMA_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    (result, std::mem::take(&mut *LLAMA_LINES.lock().unwrap()))
}
```
- [ ] **Step 6: Run the parser suite and a macOS ignored real-model probe.** The probe uses ignored `models/whisper/ggml-small.en-q8_0.bin` and `models/translation/Qwen3-4B-Q4_K_M.gguf`; create a Whisper state and load the GGUF with explicit CPU parameters, then create a context. A separate Metal run is required before claiming Metal runtime verified. If capture is unreliable or changes library logging globally in a way that breaks normal operation, stop here and revise the spec rather than writing UI code.

### Task 2: Retain verified backend status in model wrappers

**Files:**
- Modify: `src-tauri/src/local/asr.rs`
- Modify: `src-tauri/src/local/translation.rs`
- Modify: `src-tauri/src/local/mod.rs`
- Test: focused tests in those Rust modules

**Interfaces:**
- Consumes: Task 1's `BackendStatus`, capture helpers, and classifiers.
- Produces: `WhisperEngine::backend_status() -> BackendStatus` and `LocalTranslator::backend_status() -> BackendStatus`; `LocalModels` retains both across cached runs.

- [ ] **Step 1: Add failing tests for the two wrappers.** Verify Whisper remains `Detecting` until its first successful state initialization, and a successful forced-CPU GGUF load reports `Cpu`. Include a test that the fallback parameters have `n_gpu_layers() == 0`; the current `LlamaModelParams::default()` retry does not satisfy that condition.

```rust
let cpu_params = LlamaModelParams::default().with_n_gpu_layers(0).with_devices(&[]).unwrap();
assert_eq!(cpu_params.n_gpu_layers(), 0);
```

- [ ] **Step 2: Run `cargo test local:: -- --nocapture`; verify the new tests fail.**
- [ ] **Step 3: Capture Whisper backend initialization around `create_state()` in `transcribe_samples()`, retain the classified result in the engine, and expose `backend_status()`.** Change the current tuple wrapper to `WhisperEngine { context: WhisperContext, forced_cpu: bool, backend: Mutex<BackendStatus> }`. A successful GPU-requested context alone must not set the status. Preserve the current CPU retry if GPU context construction fails.

```rust
let (state_result, evidence) = capture_whisper(|| self.context.create_state());
let mut state = state_result.map_err(|e| format!("Whisper state: {e}"))?;
*self.backend.lock().unwrap() = classify_whisper(&evidence.iter().map(String::as_str).collect::<Vec<_>>(), self.forced_cpu);
```

- [ ] **Step 4: Capture GGUF load evidence for the GPU attempt and an explicit CPU attempt.** Replace the existing fallback's default parameters with `with_n_gpu_layers(0).with_devices(&[])`. Retain the classification as a candidate until a model context initializes. Return the existing load error if both loads fail.

```rust
let gpu_params = LlamaModelParams::default().with_n_gpu_layers(1000);
let cpu_params = LlamaModelParams::default().with_n_gpu_layers(0).with_devices(&[]).unwrap();
let (gpu_result, gpu_log) = capture_llama(|| LlamaModel::load_from_file(backend, path, &gpu_params));
let (model, status) = match gpu_result {
    Ok(model) => (model, classify_llama(&gpu_log.iter().map(String::as_str).collect::<Vec<_>>(), false)),
    Err(_) => {
        let (result, _) = capture_llama(|| LlamaModel::load_from_file(backend, path, &cpu_params));
        (result.map_err(|e| format!("Could not load translation GGUF {}: {e}", path.display()))?, BackendStatus::Cpu)
    }
};
```

- [ ] **Step 5: Run the focused Rust tests and the ignored macOS real-model probe.** Confirm the CPU model path works and the resulting status remains stored when `LocalModelCache::load_for` sees the same paths again.

### Task 3: Publish status once per local run

**Files:**
- Modify: `src-tauri/src/local/session.rs`
- Test: `src-tauri/src/local/session.rs` or a focused pure event-helper test

**Interfaces:**
- Consumes: Task 2's two `backend_status()` methods.
- Produces: `rt://raw` payload `{ "type": "local.backend", "stage": "asr" | "translation", "backend": "metal" | "cuda" | "cpu" | "unknown" }`.

- [ ] **Step 1: Add a failing event test.** A cached verified Whisper status must be emitted when a second run starts; a fresh Whisper status is emitted after the first inference; Transcription only emits no translation event. Test the event payload without launching a UI window by extracting `backend_event(stage, status)` and `initial_backend_events(asr_status, translation_status)` pure helpers.

```rust
assert_eq!(backend_event("asr", BackendStatus::Metal), serde_json::json!({
    "type": "local.backend", "stage": "asr", "backend": "metal"
}));
assert_eq!(initial_backend_events(BackendStatus::Metal, None), vec![backend_event("asr", BackendStatus::Metal)]);
assert_eq!(initial_backend_events(BackendStatus::Metal, Some(BackendStatus::Cpu)).len(), 2);
assert!(initial_backend_events(BackendStatus::Detecting, None).is_empty());
```

- [ ] **Step 2: Run `cargo test local::session::tests -- --nocapture`; confirm the new test fails.**
- [ ] **Step 3: Emit translation status at session start when a translator exists. Emit a cached Whisper status at session start, or emit its first new status after `process_window` completes.** Keep a run-local `last_reported` value so repeated ASR windows do not spam `rt://raw`; when backend initialization completes without recognized evidence, emit `unknown`.

```rust
if let Some(translator) = models.translator.as_ref() {
    let _ = app.emit(realtime::RAW_EVENT, backend_event("translation", translator.backend_status()));
}
```

- [ ] **Step 4: Run the local Rust suite.** Check that an ASR-only session never starts the translation worker or reports a stale translator status.

### Task 4: Show the current run's two statuses

**Files:**
- Modify: `src/local-diagnostics.ts`
- Modify: `src/App.vue`
- Modify: `src/i18n.ts`
- Create: `tests/local-diagnostics.test.mjs`

**Interfaces:**
- Consumes: Task 3's `local.backend` payload.
- Produces: `LocalDiagnostics.asrBackend` and `.translationBackend` for the existing local performance panel.

- [ ] **Step 1: Add a failing `node --test tests/local-diagnostics.test.mjs` test.** Node 26 in this checkout can import the pure TypeScript module. Keep the test outside `src/` so Vue's TypeScript config does not need Node types. Cover valid stage/status values, unknown values, event ordering, and a fresh `emptyLocalDiagnostics()` reset.

```js
import assert from "node:assert/strict";
import { emptyLocalDiagnostics, updateLocalDiagnostics } from "../src/local-diagnostics.ts";
assert.equal(updateLocalDiagnostics(emptyLocalDiagnostics(), {
  type: "local.backend", stage: "asr", backend: "metal",
}).asrBackend, "metal");
assert.equal(updateLocalDiagnostics(emptyLocalDiagnostics(), {
  type: "local.backend", stage: "translation", backend: "cuda",
}).translationBackend, "cuda");
```

- [ ] **Step 2: Run the Node test; confirm the new assertions fail.**
- [ ] **Step 3: Add the two status fields to `LocalDiagnostics`, reset them to `detecting`, and accept only the five defined backend values.** Unknown event shapes leave the previous state unchanged.

```ts
export type BackendStatus = "detecting" | "metal" | "cuda" | "cpu" | "unknown";
// In updateLocalDiagnostics, after validating type, stage, and backend:
return event.stage === "asr"
  ? { ...stats, asrBackend: event.backend as BackendStatus }
  : { ...stats, translationBackend: event.backend as BackendStatus };
```

- [ ] **Step 4: Add localized labels and two rows to the existing `.local-perf-grid`.** Define `backendLabel(status: BackendStatus)` in `App.vue` as a map from status values to i18n keys. Show the Whisper row for all local runs and the translation row only when `!transcribingOnly`. Render `detecting` as “偵測中” and `unknown` as “未確認”; do not imply GPU utilization.

```ts
const backendKeys = { detecting: "backendDetecting", metal: "backendMetal", cuda: "backendCuda", cpu: "backendCpu", unknown: "backendUnknown" } as const;
const backendLabel = (status: BackendStatus) => backendKeys[status];
```

```vue
<span>{{ t("diagAsrBackend") }} <b>{{ t(backendLabel(localDiagnostics.asrBackend)) }}</b></span>
<span v-if="!transcribingOnly">{{ t("diagTranslationBackend") }} <b>{{ t(backendLabel(localDiagnostics.translationBackend)) }}</b></span>
```

- [ ] **Step 5: Run `node --test tests/local-diagnostics.test.mjs` and `npm run build`.** In the app, start Transcription only, switch back to translation after stopping, and confirm the GGUF row appears only in the latter run and no old value flashes at start.

### Task 5: Cross-platform verification and handoff

**Files:** No product changes unless a verification failure is traced to this feature.

- [ ] Run `cargo test --quiet` from `src-tauri`, `node --test tests/local-diagnostics.test.mjs`, `npm run build`, and `git diff --check`.
- [ ] On Apple Silicon, use real Whisper and GGUF models to compare displayed labels with captured native initialization evidence. Use the ignored real-model probe for forced CPU, and verify no microphone transcript or translation regression in the normal UI run.
- [ ] On a Windows NVIDIA CUDA build, run the corresponding real-model smoke test and verify CUDA and CPU labels. A macOS build cannot establish Windows runtime behavior; report Windows as pending if no Windows host is available.
- [ ] Review the diff against the spec. Report the exact checks completed, any `unknown` result, and any unverified platform. Leave the work uncommitted until the user explicitly requests a commit.
