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

        // The window is transcribed as one piece. Whisper's overlapping
        // windows produce one or two segments each; SenseVoice's non-
        // autoregressive decoder returns the whole window in one shot, so
        // the contract is one segment per window with the full text. The
        // grouping loop then turns the window into a sentence, which is
        // what the subtitle row is built from.
        let mut text = result.text.trim().to_owned();
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // The session may already have committed past the start of this
        // window (the VAD kept it open across a short pause, so the two
        // engines re-transcribe the overlap). Keep only the fresh text:
        // the fraction of the window that lies past the commit cursor,
        // measured on the audio clock — never on the model's own
        // timestamps, which are relative to the window and do not track
        // how much of it is new.
        let window_ms = samples.len() as i64 * 1000 / 16_000;
        if let Some(cursor) = clip_after_ms.filter(|cursor| *cursor >= 0) {
            let fresh = window_ms - (cursor.saturating_sub(offset_ms).min(window_ms));
            if fresh * 4 < window_ms {
                // Less than a quarter of the window is new speech: the
                // overlap dominates, and the tail will come back in the
                // next, shorter window. Committing it here would stall
                // the commit cursor for a stretch of audio that is
                // already committed.
                return Ok(Vec::new());
            }
            let kept_chars = (text.chars().count() as f64 * fresh as f64 / window_ms as f64)
                .max(1.0) as usize;
            text = text.chars().rev().take(kept_chars).collect::<Vec<_>>().iter().rev().collect();
            if text.trim().is_empty() {
                return Ok(Vec::new());
            }
        }
        // The model reports when each token landed, on the window clock.
        // The last one is where the speech actually ends — not where the
        // (possibly much longer) window does. The commit cursor and the
        // stability margin are both on this clock, so the segment's span
        // has to be the speech span.
        let last_token_ms = result
            .timestamps
            .as_deref()
            .and_then(|ts| ts.last())
            .map(|seconds| (seconds * 1000.0) as i64)
            .unwrap_or(window_ms.saturating_sub(2_000));
        Ok(vec![SubtitleSegment {
            start_ms: offset_ms,
            end_ms: (offset_ms + last_token_ms).max(offset_ms + 100),
            text,
        }])
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
