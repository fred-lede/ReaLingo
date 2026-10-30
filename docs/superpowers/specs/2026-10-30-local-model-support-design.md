# Local Model Support — Design

## Intent

Add an on-device inference mode to ReaLingo so users can transcribe and translate speech without sending audio or text to a cloud provider. Keep the existing Qwen, OpenAI, and Gemini modes available and unchanged.

## Goals

- Support local speech recognition and translation on macOS and Windows.
- Release hardware targets are Apple Silicon Macs using Metal and Windows PCs with NVIDIA GPUs using CUDA; both platforms can fall back to CPU inference.
- Reuse ReaLingo's existing microphone, system-audio, file-input, and subtitle presentation paths wherever practical.
- Keep the full language list currently shown by ReaLingo. The selected local translation model may vary in quality or language coverage; the UI does not hide languages based on a hard-coded local-model allowlist.
- Load user-selected Whisper and GGUF model files in-process, and keep loaded models available across utterances instead of reloading for every segment.
- Persist model file paths with the existing app settings. Never persist model weights or credentials as part of this feature.
- Preserve the user's speech-output preference so local speech synthesis can be added later.

## Non-goals for the first release

- Linux local inference. Linux audio capture and existing cloud modes remain supported.
- Downloading, updating, or managing model weights.
- Local speech synthesis. In local mode, speech output is unavailable and the UI explains this; the user's speech preference remains intact when switching modes.
- A local HTTP/WebSocket model server or a compatibility API for third-party local servers.
- Guaranteeing translation quality for every language in the full selector; quality depends on the chosen GGUF model.

## Approaches considered

1. **In-process Whisper plus llama.cpp — selected.** Follow the reference repo's local inference pattern: `whisper-rs` for ASR and `llama-cpp-2` for GGUF translation. This supports an offline path without another server process and fits ReaLingo's Rust/Tauri backend. The trade-off is native dependency, model-loading, and platform-build complexity.
2. **Connect to a local OpenAI-compatible server.** This avoids embedding inference libraries, but requires users to install and configure another service, and OpenAI-compatible chat APIs do not by themselves define the streaming audio-to-translation behavior this app needs.

## User-facing behavior

- Add a Local option alongside the existing cloud provider choices.
- Local settings provide separate file selectors for the Whisper ASR model and GGUF translation model. Do not download either file automatically.
- Require a `whisper-rs`-compatible Whisper `.bin` file and a GGUF translation model with an embedded chat template. Use Whisper large-v3-turbo and Qwen3-4B Q4_K_M as the initial acceptance baseline; other compatible user-selected models remain allowed.
- Keep the full existing source- and target-language selectors. Pass the selected source-language preference to ASR and the target language to the translation prompt.
- Use the local pipeline for microphone, system audio, and audio-file input. While audio is captured or decoded, buffer it into speech segments, transcribe each segment, translate its text, and publish source/translation updates through the existing subtitle event path.
- If a required model path is missing or a model fails to load, show a specific error and do not silently fall back to a cloud provider.
- In Local mode, disable speech output with a clear unavailable explanation, without clearing or overwriting the saved speech preference. Keep the design open for a later local TTS backend.

## Architecture

- Add a local-versus-remote execution choice without changing the meaning of the existing remote `Protocol` variants or their wire formats. Existing saved settings with no execution-mode field continue to select their current remote protocol.
- Store the Whisper and translation model paths in the existing persisted settings. Keep inference objects in Rust `AppState`, keyed by the selected model path, and perform model loading/inference on blocking workers rather than the UI or async event loop.
- Reuse current audio capture and file decoding. Local mode feeds mono 16 kHz PCM to the ASR pipeline; the remote `start_stream` WebSocket path remains separate and unchanged.
- Add a bounded streaming segmenter between PCM capture and ASR. It should handle silence/utterance boundaries and avoid unbounded buffering. Translate finalized recognized segments and emit normalized source and target events so the current stream, overlay, and history continue to work.
- Use Metal on Apple Silicon and CUDA on Windows with NVIDIA GPUs; retain CPU fallback on both platforms. Intel Macs and non-NVIDIA Windows GPU acceleration are outside the first-release acceptance matrix. Report model/backend load errors clearly.
- Do not send local-mode audio, transcripts, model paths, or prompts to an external service.

## Acceptance criteria

- Existing installs default to their previous cloud protocol after the settings change.
- On macOS and Windows, selecting Local and choosing valid model files permits microphone and audio-file translation with the full language selector; supported system-audio capture follows the same local path.
- Local mode produces source and translated subtitle updates during the session, and stopping input flushes the final buffered segment.
- Missing/invalid model files and inference failures produce visible errors, with no cloud fallback.
- Local mode makes no external inference requests. Existing remote provider behavior remains available.
- Switching between Local and a remote provider preserves remote settings and the speech-output preference.
- Local speech output is unavailable in the first release but can be enabled later by adding a local speech backend without replacing the local ASR/translation flow.

## Implementation choices left to the builder

- Select concrete `whisper-rs` and `llama-cpp-2` features compatible with the current Tauri toolchain and macOS/Windows packaging.
- Choose bounded segment/window and overlap values based on correctness and latency in the existing streaming pipeline.
- Choose precise error wording and placement in the current settings/status UI.

## Reference

- Reference local ASR and capture: `fred-lede/real-time-subtitles`, `app/src-tauri/src/audio.rs` and `app/src-tauri/src/audio_capture.rs`.
- Reference GGUF translation: `fred-lede/real-time-subtitles`, `app/src-tauri/src/translation.rs`.
- Reference native build features: `fred-lede/real-time-subtitles`, `app/src-tauri/Cargo.toml`.
