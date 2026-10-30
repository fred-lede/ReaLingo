# Local Model Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an offline, in-process Whisper ASR and GGUF translation mode to ReaLingo while preserving existing cloud protocols.

**Architecture:** Add a `mode` separate from the existing remote `protocol`; local mode loads user-selected Whisper and llama.cpp models in Rust, consumes the existing 16 kHz PCM input channel, and emits the normalized events already consumed by the UI. Reuse microphone, system-audio, file decode, subtitle, overlay, and history flows; do not route local input through a cloud socket.

**Tech Stack:** Tauri 2, Rust, Vue 3 / TypeScript, `whisper-rs`, `llama-cpp-2`, existing `cpal`, `symphonia`, and Tokio pipelines.

**Spec:** `docs/superpowers/specs/2026-10-30-local-model-support-design.md`

## Global Constraints

- First-release inference targets Apple Silicon macOS with Metal and Windows with NVIDIA CUDA; both can fall back to CPU inference.
- Linux local inference is out of scope; existing Linux audio capture and cloud providers remain supported.
- Keep the full current language list; model language quality varies and must not hide language choices.
- User selects a `whisper-rs`-compatible Whisper `.bin` model and a GGUF translation model with an embedded chat template; do not download models.
- Initial acceptance baseline: Whisper large-v3-turbo and Qwen3-4B Q4_K_M.
- First release has no local speech synthesis; preserve the speech preference for future local TTS.
- Missing or unusable local models must produce a visible error and must never trigger cloud fallback.
- Existing saved settings without a mode value continue using their existing remote protocol.

## Review Focus

- **Legacy settings without `mode`:** must still start the previously selected remote protocol. Pin with `saved_settings_without_mode_default_to_remote` in Task 1.
- **Missing, unreadable, or incompatible model files:** must report which local model failed and never fall back to a remote request. Pin load failures in Task 2 and the no-key/no-cloud branch in Task 5.
- **Full language list and auto source detection:** uncommon target choices must remain selectable and their English names must reach the local prompt, while `auto` must be passed to Whisper as automatic detection. Pin the prompt target in Task 2 and the UI language options in Task 5.
- **Long speech or no detected endpoint:** pending PCM must remain bounded at 16 seconds and continue making progress. Pin the cap/fallback behavior in Task 3.
- **Stop, file EOF, or capture-channel close with a partial tail:** final buffered speech must be transcribed and translated once. Pin tail flush and duplicate prevention in Task 3.

---

## File Structure

- `src-tauri/src/config.rs` — serde execution mode, local model paths, and local sample rate while preserving remote protocol semantics.
- `src-tauri/src/local/mod.rs` — local model cache and local session entry points.
- `src-tauri/src/local/asr.rs` — Whisper model loading and transcription wrapper.
- `src-tauri/src/local/translation.rs` — GGUF model loading and text translation wrapper.
- `src-tauri/src/local/session.rs` — bounded audio windows, transcript stabilization, translation dispatch, and normalized ReaLingo events.
- `src-tauri/src/lib.rs` — local model state, preflight loading, and mode branch in `start_stream`.
- `src-tauri/Cargo.toml` — target-specific native inference dependencies/features.
- `src/store.ts` — persisted remote/local mode and selected model paths.
- `src/components/Settings.vue` — mode and local model file selectors.
- `src/languages.ts` and `src/App.vue` — local language/capability behavior, API-key bypass, and speech-output gating.
- `src/stream.ts` — pass the selected target-language display name to the local backend and record the local model name in history while retaining existing start behavior.
- `src/i18n.ts` — Chinese and English strings for local mode, model selection, unsupported local speech, and errors.
- `README.md` and `README.en.md` — model formats, platform support, and first-release limitations.

### Task 1: Add a backward-compatible local execution mode

**Files:**
- Modify: `src-tauri/src/config.rs`
- Modify: `src/store.ts`
- Test: unit tests in `src-tauri/src/config.rs`

**Interfaces:**
- Produces Rust `ExecutionMode::{Remote, Local}` with serde names `remote` / `local` and default `Remote`.
- Produces TypeScript `ExecutionMode = "remote" | "local"`.
- Adds `Settings.mode`, `Settings.whisper_model_path`, and `Settings.translation_model_path` (serialized as `mode`, `whisperModelPath`, and `translationModelPath`).
- `Settings::sample_rate()` returns `16_000` when `mode == Local`; remote values continue to follow `Protocol`.

- [ ] **Step 1: Write the failing config tests**

Add `saved_settings_without_mode_default_to_remote`, `local_settings_round_trip_model_paths`, and `local_mode_uses_16khz_sample_rate`, asserting that old settings default to Remote, local paths round-trip, and Local returns 16 kHz even if the saved remote protocol is OpenAI.

- [ ] **Step 2: Run the focused tests and confirm they fail**

Run: `cargo test --lib config::tests --manifest-path src-tauri/Cargo.toml`
Expected: the tests fail because `ExecutionMode`, local fields, and the Local sample-rate branch do not exist yet.

- [ ] **Step 3: Implement the mode and persisted fields**

Add the serde enum and defaulted fields to `config.rs`; add the matching union and fields/defaults to `store.ts`. Do not add Local to `Protocol`, and do not change any remote URL or wire-payload methods.

- [ ] **Step 4: Run the focused tests and frontend type check**

Run: `cargo test --lib config::tests --manifest-path src-tauri/Cargo.toml`
Run: `npm run build`
Expected: all focused tests pass and Vue/TypeScript compilation succeeds.

### Task 2: Add model wrappers and native inference dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/local/mod.rs`
- Create: `src-tauri/src/local/asr.rs`
- Create: `src-tauri/src/local/translation.rs`

**Interfaces:**
- `WhisperEngine::load(model_path: &str) -> Result<WhisperEngine, String>` validates and loads a Whisper `.bin` file.
- `WhisperEngine::transcribe_samples(&self, samples: &[f32], language: Option<&str>, offset_ms: i64) -> Result<Vec<SubtitleSegment>, String>` returns timestamped non-empty segments; `SubtitleSegment` is the small local struct `{ start_ms: i64, end_ms: i64, text: String }`.
- `LocalTranslator::load(model_path: &str) -> Result<LocalTranslator, String>` requires a readable GGUF model and its embedded chat template.
- `LocalTranslator::translate(&self, text: &str, target_language: &str) -> Result<String, String>` returns one non-empty translation, using the model chat template and returning only user-visible text.
- `translation_instruction(target_language_name: &str) -> String` builds the shared instruction text used by the local translator.
- `LocalModels` owns both wrappers for reuse across a session.

- [ ] **Step 1: Add model-wrapper failure tests**

Add Rust tests `whisper_load_rejects_missing_path` and `gguf_load_rejects_missing_path`, asserting clear `Err` values instead of panics, plus `translation_instruction_includes_target_language` asserting the requested language name appears in the translation instruction.

- [ ] **Step 2: Run the focused tests and confirm they fail**

Run: `cargo test --lib local:: --manifest-path src-tauri/Cargo.toml`
Expected: compilation fails because the local module and wrappers are not defined.

- [ ] **Step 3: Add target-specific dependencies and wrappers**

Add `whisper-rs` and `llama-cpp-2` under macOS/Windows target sections, enabling Metal for Apple Silicon and CUDA for Windows. Keep a CPU inference path in both builds. Implement file checks, one-time backend initialization, model loading, Whisper transcription, GGUF chat-template formatting, bounded translation generation, and clear load/inference errors. Keep the inference wrappers in the focused local module; do not change the existing capture or cloud protocol code in this task.

- [ ] **Step 4: Run wrapper tests and compile the current host target**

Run: `cargo test --lib local:: --manifest-path src-tauri/Cargo.toml`
Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: focused tests pass and the host target builds with the new dependencies.

### Task 3: Implement bounded streaming segmentation and local events

**Files:**
- Create: `src-tauri/src/local/session.rs`
- Modify: `src-tauri/src/local/mod.rs`
- Test: unit tests in `src-tauri/src/local/session.rs`

**Interfaces:**
- `TranscriptionWindow` contains `samples: Vec<f32>` and `offset_ms: i64`.
- `SpeechSegmenter::push(&mut self, samples: &[i16]) -> Vec<TranscriptionWindow>` accepts mono PCM16 at 16 kHz and yields bounded transcription windows at each hop.
- `SpeechSegmenter::flush(&mut self) -> Option<TranscriptionWindow>` returns any remaining non-silent tail once.
- `LocalSession::run(app: AppHandle, settings: Settings, target_language_name: String, audio_rx: tokio::sync::mpsc::Receiver<Vec<i16>>, stop: Arc<AtomicBool>, models: Arc<Mutex<LocalModels>>) -> Result<(), String>` runs on a blocking worker, consumes audio with `blocking_recv`, transcribes and translates, and emits normalized `rt://event` events.
- Windows advance in 2-second hops and pending audio is capped at 16 seconds, following the reference implementation. Repeated transcription windows use stable-prefix comparison; max-pending fallback and final flush commit the latest transcript without duplicates.
- Each committed segment emits a `link`, `source`, and `target` event sharing one stable local turn id; both text events have `done: true`.

- [ ] **Step 1: Add failing segmentation tests**

Test that a stable prefix is committed once, pending audio never exceeds 16 seconds, the max-window fallback advances, and `flush()` emits a partial tail exactly once.

- [ ] **Step 2: Run the focused tests and confirm they fail**

Run: `cargo test --lib local::session::tests --manifest-path src-tauri/Cargo.toml`
Expected: the tests fail because `SpeechSegmenter` does not exist.

- [ ] **Step 3: Implement the segmenter and session runner**

Use 2-second window hops and a 16-second pending cap. Convert PCM16 to normalized f32 for Whisper, pass `None` for source language `auto`, then translate each finalized segment. Emit existing `realtime::Event` values with unique ids and the source/target ordering required by `src/stream.ts`; do not create a second UI event format.

- [ ] **Step 4: Run segmentation tests**

Run: `cargo test --lib local::session::tests --manifest-path src-tauri/Cargo.toml`
Expected: all bounds, stable-prefix, and tail-flush cases pass.

### Task 4: Connect local models to Tauri streaming and audio/file input

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/local/mod.rs`
- Modify: `src-tauri/src/realtime.rs` only if event emission needs a narrowly scoped shared helper

**Interfaces:**
- `AppState` owns `Arc<Mutex<LocalModelCache>>`; `LocalModelCache` stores `Option<(PathBuf, PathBuf)>` for the loaded model paths and `Option<LocalModels>`, and exposes `load_for(&mut self, whisper_path: &Path, translation_path: &Path) -> Result<(), String>`.
- `start_stream` becomes async and accepts `target_language_name: String` in addition to the existing settings/source arguments; clone the managed state handles before awaiting so a Tauri `State` borrow does not cross the await.
- In `mode == Local`, `start_stream` loads or reuses both selected models before starting audio capture; missing or invalid paths return a user-visible error before capture begins.
- Remote mode keeps using `realtime::run` and keyring lookup; Local mode does not read a cloud API key.

- [ ] **Step 1: Add a failing local dispatch/preflight test**

Add a test around a pure mode-dispatch/preflight helper asserting Local does not require an API key and rejects missing model paths before starting the source.

- [ ] **Step 2: Run the focused test and confirm it fails**

Run: `cargo test --lib local_dispatch --manifest-path src-tauri/Cargo.toml`
Expected: the test fails until local preflight/dispatch exists.

- [ ] **Step 3: Branch stream startup by execution mode**

Make local startup load models on a blocking worker, capture/decode at `Settings::sample_rate()` (16 kHz), and run `LocalSession` on a blocking worker. Guard Local as unsupported in Linux builds, while keeping Linux cloud mode intact. Keep the remote branch, keyring injection, and WebSocket behavior unchanged. Ensure stop and file EOF allow the local receiver to drain and flush the final tail.

- [ ] **Step 4: Run dispatch tests and compile**

Run: `cargo test --lib local_dispatch --manifest-path src-tauri/Cargo.toml`
Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: local preflight passes and the host target compiles.

### Task 5: Add mode-aware settings and UI behavior

**Files:**
- Modify: `src/store.ts`
- Modify: `src/components/Settings.vue`
- Modify: `src/languages.ts`
- Modify: `src/App.vue`
- Modify: `src/stream.ts`
- Modify: `src/i18n.ts`

**Interfaces:**
- Settings mode selector chooses Remote or Local; the remote protocol selection and model/URL settings remain saved while Local is active.
- Local settings expose Whisper `.bin` and translation `.gguf` file pickers using the existing Tauri dialog plugin and persist their paths.
- Local source/target selection uses every `LANGUAGES` entry; remote language filtering remains unchanged.
- Disable Local mode on Linux and show a localized unsupported-platform message; Rust also rejects a Local invocation on Linux.
- Local mode bypasses API-key gating, permits explicit source language or `auto`, and disables speaking with a localized explanation without mutating `settings.speak`.
- `stream.ts` passes the English target-language display name to `start_stream` for the local translation prompt and records the selected local model name in translation history.

- [ ] **Step 1: Add the localized local-mode labels and status messages**

Add Chinese and English labels for Local/Remote mode, Whisper model path, translation GGUF path, model-load errors, unsupported local speech, and the offline-mode hint.

- [ ] **Step 2: Add local settings and model selectors**

Add the mode picker and file pickers. Hide remote key/endpoint-only controls while Local is selected, but leave the remote protocol/model/URL settings untouched in the store.

- [ ] **Step 3: Make the main screen honor Local capabilities**

Update key-required status/start guards, language options, `autoSourceOnly`, `speakBlocked`, and the displayed model label. Preserve `settings.speak`; only pass `speak: false` to the local backend while Local mode is active.

- [ ] **Step 4: Pass the translation target name and run frontend compilation**

Pass `langName(settings.targetLang, "en")` through `stream.ts` to the Tauri command. Run: `npm run build`.
Expected: Vue/TypeScript compilation succeeds and remote mode retains its previous guards and language filtering.

- [ ] **Step 5: Verify local language and history behavior**

In the Settings/main window, verify Local exposes every entry in `LANGUAGES`, including Icelandic and Cantonese, while each remote mode retains its existing filtered list. Start a local session and confirm its history metadata and title use the selected GGUF model name rather than the previously selected cloud model.

### Task 6: Document support and verify release builds

**Files:**
- Modify: `README.md`
- Modify: `README.en.md`

- [ ] **Step 1: Document local setup and constraints**

Describe user-supplied Whisper `.bin` and GGUF files, the Whisper large-v3-turbo / Qwen3-4B Q4_K_M acceptance baseline, Apple Silicon Metal and Windows NVIDIA CUDA with CPU fallback, full language-selector behavior, no cloud fallback, and no local TTS in the first release.

- [ ] **Step 2: Run frontend and Rust verification**

Run: `npm run build`
Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: both commands pass for the current host target.

- [ ] **Step 3: Verify each release platform with local model files**

On Apple Silicon macOS, launch using Metal and confirm CPU fallback when Metal initialization is unavailable. On Windows with NVIDIA, launch using CUDA and confirm CPU fallback when CUDA is unavailable. For both platforms, use the baseline model files to verify microphone, audio file, supported system audio, all source/target choices, stop-tail flush, local TTS disabled, and no inference network request. Confirm existing remote protocols still connect normally.
