# Local Inference Backend Status — Design

## Intent

Show which inference backend the current local session actually initialized for Whisper ASR and, when translation is enabled, the GGUF translator. This helps diagnose high fan speed and unexpected CPU fallback. The status is not a GPU utilization or temperature meter.

## Scope

- macOS Apple Silicon: Metal or CPU. Windows NVIDIA: CUDA or CPU.
- Show separate Whisper and translation statuses in the existing local performance panel. In Transcription only mode, show only Whisper.
- Do not add a setting, change model selection, alter inference scheduling, or change cloud providers.
- Keep the status transient. It belongs to the current run and is not written to settings or history.

## Evidence and labels

Each stage reports `detecting`, `metal`, `cuda`, `cpu`, or `unknown`. The UI translates these values; the Rust event carries stable codes.

- `metal` or `cuda` requires evidence that the native library initialized that backend for this model. For GGUF, evidence must also show at least one model layer was offloaded and a model context was successfully initialized. A compiled GPU feature, an available device, a requested GPU parameter, or a successful model load alone is insufficient.
- `cpu` requires evidence that the stage initialized without GPU acceleration or took the explicit CPU fallback path. CPU work alongside GPU inference does not change a confirmed Metal or CUDA label.
- `detecting` covers the period before evidence is available. Whisper may not initialize its execution backend until its first inference state is created.
- `unknown` is the final label if the library does not expose trustworthy evidence after its backend initialization completes. Never turn missing log output into a CPU or GPU claim.

The pinned `whisper-rs` and `llama-cpp-2` versions do not expose a complete per-model backend query through their safe Rust APIs. The implementation should capture narrowly scoped native backend initialization and GGUF layer-offload information at the existing model/state creation points. It must not infer the result solely from `use_gpu(true)` or `with_n_gpu_layers(1000)`. Native messages are version-dependent, so their recognized forms need focused tests. If a platform's messages cannot be captured reliably, that platform reports `unknown` until the design is revised.

On the tested Apple M5, Whisper's confirmed Metal message is `using MTL0 backend`; accept that native device name as Metal evidence. Run Metal probes outside the restricted test sandbox, which cannot obtain a Metal device.

The current GGUF retry uses `LlamaModelParams::default()`, whose pinned llama.cpp default requests GPU layers. Replace that retry with an explicit `with_n_gpu_layers(0)` and empty GPU device list; otherwise a displayed CPU fallback would not correspond to the model-loading and context behavior.

## Data flow and lifecycle

1. The local model wrappers collect backend evidence while creating the Whisper inference state or loading the GGUF model. GGUF retains a candidate until its first model context initializes successfully. They retain only the resulting status, not the full native log.
2. The local session emits a small `local.backend` diagnostic event through the existing `rt://raw` path when a stage becomes known. A cached model must still report its retained status at the start of a later run.
3. `src/local-diagnostics.ts` stores the two statuses for the current run. Starting another run resets them to `detecting`; the performance panel hides after the run stops. Remote sessions do not display these fields.
4. The main panel shows `Whisper: Metal/CUDA/CPU/Unknown` and, when translation is active, `Translation: Metal/CUDA/CPU/Unknown`. Transcription only mode does not load a GGUF model or show a translation status.

Do not parse the status from performance timings or operating-system-wide GPU load. The backend event contains no model path, transcript, prompt, or credential.

## Approaches considered

1. **Capture native initialization evidence — selected.** Fits the existing local diagnostics path and distinguishes actual GPU initialization/offload from a requested setting. The trade-off is coupling to the pinned native libraries' log formats; unrecognized output yields `unknown`.
2. **Report the requested GPU flag or successful load.** Smaller change, but can falsely say Metal or CUDA after an internal CPU fallback or zero-layer offload. Rejected for this feature's purpose.
3. **Add a native C/C++ inspection shim or poll OS GPU counters.** Potentially more detailed, but requires a new compatibility surface or system monitoring and does not fit this per-model status request.

## Verification and acceptance

- First run a focused feasibility probe on the pinned native libraries: confirm that Metal initialization and forced CPU fallback produce distinguishable evidence without interfering with inference or another library's logging. If this fails, stop and revise the design before adding UI labels.
- Unit tests cover native-message interpretation, unknown output, GPU-requested-but-not-used cases, cached model status, diagnostic event handling, and hiding the translation row in Transcription only mode.
- Verify that the GGUF retry passes zero GPU layers rather than native default parameters.
- On Apple Silicon, run a real Whisper model and GGUF model with Metal, then a forced CPU path, and compare the labels with native backend evidence. Verify that normal and Transcription only runs report their respective stages.
- Build the Windows CUDA target and perform a Windows NVIDIA runtime smoke test before claiming CUDA verified. Until then, report Windows runtime validation as outstanding.
- Confirm remote providers, translation output, stopping, and restarting still behave as before.

## Implementation boundary

Limit changes to the local model wrappers/cache, local diagnostic event emission, diagnostic state, main panel, and their text labels and focused tests. No GPU percentage, temperature, device selector, model downloader, or new inference engine is included.
