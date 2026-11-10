//! Local transcription with the FunAudioLLM SenseVoice Small ONNX model,
//! run through sherpa-onnx. SenseVoice is not a Whisper model — it is a
//! non-autoregressive encoder + CTC stack that the ONNX Runtime runs on CPU
//! in a fraction of the time Whisper needs for the same window — which is
//! exactly what the VAD window loop is asking for.
//!
//! The model files are the standard sherpa-onnx sense-voice bundle:
//! `model.onnx`, `tokens.txt`, and the language file
//! (`language_model.bin`). The picker offers a directory; `load` resolves
//! the files inside it.
use std::path::Path;

use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
};

use super::asr::{AsrEngine, SubtitleSegment};
use super::backend_status::BackendStatus;

pub struct SenseVoiceEngine {
    recognizer: OfflineRecognizer,
}

impl SenseVoiceEngine {
    /// Load the SenseVoice ONNX model bundle from `dir`. The directory must
    /// contain `model.onnx` (or `model.int8.onnx`) and `tokens.txt`.
    /// The int8 build is preferred when both are present: same ASR accuracy,
    /// a quarter of the memory, and faster on CPU.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let model = if dir.join("model.int8.onnx").is_file() {
            dir.join("model.int8.onnx")
        } else {
            dir.join("model.onnx")
        };
        let tokens = dir.join("tokens.txt");
        for required in [model.as_path(), tokens.as_path()] {
            if !required.is_file() {
                return Err(format!(
                    "SenseVoice model file not found: {}",
                    required.display()
                ));
            }
        }

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            language: None, // picked per call via the stream option; None = auto
            use_itn: true,
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        // SenseVoice always decodes a 30-second window at once; give the
        // encoder a few threads without stealing from the translation worker.
        config.model_config.num_threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get().min(4) as i32);
        config.model_config.model_type = Some("sense_voice".into());

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| format!("SenseVoice model could not be loaded from {}", dir.display()))?;
        Ok(Self { recognizer })
    }

    fn language_token(language: Option<&str>) -> Option<String> {
        // SenseVoice's language file names one of: zh, en, ja, ko, yue, auto.
        // The app's source-language codes are RFC-ish ("zh", "en", …), so pass
        // through only the codes the model actually knows and let everything
        // else fall back to auto-detect.
        let code = language?;
        matches!(code, "zh" | "en" | "ja" | "ko" | "yue").then(|| code.to_string())
    }
}

impl AsrEngine for SenseVoiceEngine {
    fn backend_status(&self) -> BackendStatus {
        // The ONNX Runtime here is CPU-only; the diagnostics drawer should say so.
        BackendStatus::Cpu
    }

    fn transcribe_samples(
        &self,
        samples: &[f32],
        language: Option<&str>,
        offset_ms: i64,
        clip_after_ms: Option<i64>,
    ) -> Result<Vec<SubtitleSegment>, String> {
        let stream = self.recognizer.create_stream();
        if let Some(lang) = Self::language_token(language) {
            stream.set_option("lang", &lang);
        }
        stream.accept_waveform(16_000, samples);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| "SenseVoice produced no result".to_string())?;

        let text = result.text.trim().to_owned();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let tokens = result.tokens;

        // SenseVoice reports one token per timestamp. Build the window-local
        // segments and, when the session has already committed past the middle
        // of this window, keep only the tail after the commit cursor — the
        // same contract Whisper's token-timestamp clipping makes.
        let token_times: Vec<i64> = result
            .timestamps
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|seconds| (*seconds as i64) * 1000 + offset_ms)
            .collect();

        // Even out the timestamps across the window when the model gives no
        // per-token times (older bundles): better a monotone guess than all
        // segments pinned to the window start.
        let times: Vec<i64> = if !token_times.is_empty() {
            token_times
        } else {
            let window_ms = samples.len() as i64 * 1000 / 16_000;
            (0..tokens.len())
                .map(|i| offset_ms + window_ms * (i as i64 + 1) / tokens.len() as i64)
                .collect()
        };

        let mut out = Vec::new();
        if tokens.len() <= 1 {
            let end_ms = times.last().copied().unwrap_or(offset_ms);
            if clip_after_ms.is_some_and(|cursor| end_ms <= cursor) {
                return Ok(out);
            }
            out.push(SubtitleSegment { start_ms: offset_ms, end_ms, text });
            return Ok(out);
        }

        // One segment per token: map each token's character span onto the text
        // and take the next token's timestamp as this piece's end.
        let mut chars_so_far = 0usize;
        for (i, token) in tokens.iter().enumerate() {
            let from = chars_so_far;
            chars_so_far += token.chars().count();
            let to = chars_so_far;
            let piece = text.get(from..to).unwrap_or("").to_string();
            if piece.trim().is_empty() {
                continue;
            }
            let start_ms = times.get(i).copied().unwrap_or(offset_ms);
            let end_ms = times.get(i + 1).copied().unwrap_or(start_ms);
            if clip_after_ms.is_some_and(|cursor| end_ms <= cursor) {
                continue;
            }
            out.push(SubtitleSegment { start_ms, end_ms, text: piece });
        }
        if out.is_empty() {
            out.push(SubtitleSegment {
                start_ms: offset_ms,
                end_ms: times.last().copied().unwrap_or(offset_ms),
                text,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::SenseVoiceEngine;

    #[test]
    fn only_sensevoice_language_codes_are_forced() {
        assert_eq!(SenseVoiceEngine::language_token(Some("zh")), Some("zh".into()));
        assert_eq!(SenseVoiceEngine::language_token(Some("yue")), Some("yue".into()));
        // A code the model does not know must not be forced onto it.
        assert_eq!(SenseVoiceEngine::language_token(Some("es")), None);
        assert_eq!(SenseVoiceEngine::language_token(None), None);
    }
}
