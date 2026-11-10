use std::{path::Path, sync::Mutex};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
use super::backend_status::{capture_whisper, classify_whisper, BackendStatus};

#[derive(Debug, Clone)]
pub struct SubtitleSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// One local transcription backend. Whisper and SenseVoice are two
/// implementations of the same offline window-to-text call, so the session
/// loop only sees this trait and never the concrete engine.
pub trait AsrEngine: Send {
    /// Which backend this engine reports in the diagnostics drawer.
    fn backend_status(&self) -> BackendStatus;

    /// Whether the engine's window is stable the moment a segment comes
    /// out: SenseVoice runs on a VAD window that only closes on a pause,
    /// so its one segment is final — no 2 s stability margin is needed.
    /// Whisper's sliding window can still correct its last segment, so
    /// the margin stays.
    fn is_stable_window_engine(&self) -> bool {
        false
    }

    /// Transcribe one VAD window. `language` is the forced language ("auto" is
    /// already filtered by the caller), `offset_ms` places the result on the
    /// input-audio clock, and `clip_after_ms` says how far of the window is
    /// already committed, so the engine can hand back only the new tail.
    fn transcribe_samples(
        &self,
        samples: &[f32],
        language: Option<&str>,
        offset_ms: i64,
        clip_after_ms: Option<i64>,
    ) -> Result<Vec<SubtitleSegment>, String>;

    /// Run one minimal decode so the backend's status moves out of
    /// "detecting". Whisper picks its GPU/CPU backend on its first state;
    /// the engine calls this instead of waiting for the first live window.
    /// The default is a no-op: engines whose backend is known the moment
    /// the model loads (SenseVoice is CPU-onnx) have nothing to warm.
    fn ready(&mut self) -> Result<(), String> {
        Ok(())
    }
}

struct TimedPiece {
    end_ms: i64,
    bytes: Vec<u8>,
}

fn suffix_after_cursor(pieces: &[TimedPiece], cursor_ms: i64, ascii: bool) -> Option<String> {
    let mut first = pieces.iter().position(|piece| piece.end_ms > cursor_ms)?;
    if ascii {
        while first > 0 && !pieces[first].bytes.first().is_some_and(u8::is_ascii_whitespace) {
            first -= 1;
        }
    }
    for start in (0..=first).rev() {
        let bytes = pieces[start..].iter().flat_map(|piece| piece.bytes.iter().copied()).collect();
        if let Ok(text) = String::from_utf8(bytes) {
            let text = text.trim().to_owned();
            if !text.is_empty() {
                // A long word cannot be spoken in a few milliseconds; this is stale overlap.
                if pieces.last().is_some_and(|piece| piece.end_ms - cursor_ms < 100)
                    && text
                        .chars()
                        .filter(|character| character.is_alphanumeric())
                        .count()
                        >= 4
                {
                    return None;
                }
                return Some(text);
            }
        }
        if ascii {
            break;
        }
    }
    None
}

pub struct WhisperEngine {
    context: WhisperContext,
    forced_cpu: bool,
    backend: Mutex<BackendStatus>,
}

impl WhisperEngine {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("Whisper model file not found: {}", path.display()));
        }
        let model_path = path.to_string_lossy();
        let mut gpu_params = WhisperContextParameters::default();
        gpu_params.use_gpu(true);
        let (context, forced_cpu) = match WhisperContext::new_with_params(model_path.as_ref(), gpu_params) {
            Ok(context) => (context, false),
            Err(gpu_error) => {
                let mut cpu_params = WhisperContextParameters::default();
                cpu_params.use_gpu(false);
                let context = WhisperContext::new_with_params(model_path.as_ref(), cpu_params)
                    .map_err(|cpu_error| {
                        format!(
                            "Could not load Whisper model {} (GPU: {gpu_error}; CPU: {cpu_error})",
                            path.display()
                        )
                    })?;
                (context, true)
            }
        };
        let mut engine = Self { context, forced_cpu, backend: Mutex::new(BackendStatus::Detecting) };
        engine.ready()?;
        Ok(engine)
    }

    fn ready(&mut self) -> Result<(), String> {
        // Whisper picks its backend on the first state. That call prints
        // the lines `classify_whisper` reads, so the probe is the very
        // operation that turns "detecting" into Metal/CUDA/CPU. A 3 s
        // silence is enough: the first frame is a warm-up, the rest is
        // the backend's own init.
        let silence = vec![0f32; 16_000 * 3];
        let params = self.probe_params();
        let (result, evidence) = capture_whisper(|| self.context.create_state());
        let mut state = result.map_err(|e| format!("Whisper probe state: {e}"))?;
        let lines: Vec<_> = evidence.iter().map(String::as_str).collect();
        *self.backend.lock().unwrap() = classify_whisper(&lines, self.forced_cpu);
        state.full(params, &silence).map_err(|e| format!("Whisper warm-up failed: {e}"))?;
        Ok(())
    }

    fn probe_params(&self) -> FullParams<'_, '_> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(std::thread::available_parallelism().map_or(2, |n| n.get().min(8) as i32));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_token_timestamps(false);
        params
    }

    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        language: Option<&str>,
        offset_ms: i64,
        clip_after_ms: Option<i64>,
    ) -> Result<Vec<SubtitleSegment>, String> {
        let (state, evidence) = capture_whisper(|| self.context.create_state());
        let mut state = state.map_err(|e| format!("Whisper state: {e}"))?;
        let lines: Vec<_> = evidence.iter().map(String::as_str).collect();
        *self.backend.lock().unwrap() = classify_whisper(&lines, self.forced_cpu);
        let mut params = self.probe_params();
        params.set_language(language);
        params.set_token_timestamps(clip_after_ms.is_some());
        state
            .full(params, samples)
            .map_err(|e| format!("Whisper transcription failed: {e}"))?;
        Ok(state
            .as_iter()
            .filter_map(|segment| {
                let mut text = segment.to_string().trim().to_owned();
                let mut start_ms = offset_ms + segment.start_timestamp() * 10;
                let end_ms = offset_ms + segment.end_timestamp() * 10;
                if let Some(cursor_ms) = clip_after_ms.filter(|cursor| {
                    start_ms < cursor.saturating_sub(1_000) && end_ms > *cursor
                }) {
                    let pieces: Vec<_> = (0..segment.n_tokens())
                        .filter_map(|index| {
                            let token = segment.get_token(index)?;
                            if token.token_id() >= self.context.token_eot() {
                                return None;
                            }
                            Some(TimedPiece {
                                end_ms: offset_ms + token.token_data().t1 * 10,
                                bytes: token.to_bytes().ok()?.to_vec(),
                            })
                        })
                        .collect();
                    if let Some(suffix) = suffix_after_cursor(&pieces, cursor_ms, text.is_ascii()) {
                        text = suffix;
                        start_ms = cursor_ms.saturating_sub(1);
                    }
                }
                (!text.is_empty()).then_some(SubtitleSegment {
                    start_ms,
                    end_ms,
                    text,
                })
            })
            .collect())
    }
}




impl AsrEngine for WhisperEngine {
    fn backend_status(&self) -> BackendStatus {
        *self.backend.lock().unwrap()
    }

    fn transcribe_samples(
        &self,
        samples: &[f32],
        language: Option<&str>,
        offset_ms: i64,
        clip_after_ms: Option<i64>,
    ) -> Result<Vec<SubtitleSegment>, String> {
        WhisperEngine::transcribe_samples(self, samples, language, offset_ms, clip_after_ms)
    }
}


#[cfg(test)]
mod tests {
    use super::{suffix_after_cursor, TimedPiece};

    #[test]
    fn overlap_does_not_reemit_a_word_from_a_twenty_ms_tail() {
        let pieces = vec![TimedPiece {
            end_ms: 164_000,
            bytes: b" changing.".to_vec(),
        }];
        assert_eq!(suffix_after_cursor(&pieces, 163_980, true), None);
    }

    #[test]
    fn final_overlap_keeps_the_whole_word_crossing_the_commit_cursor() {
        let pieces = [
            TimedPiece { end_ms: 185_890, bytes: b" supercom".to_vec() },
            TimedPiece { end_ms: 186_160, bytes: b"put".to_vec() },
            TimedPiece { end_ms: 186_430, bytes: b"ers".to_vec() },
            TimedPiece { end_ms: 186_670, bytes: b" are".to_vec() },
            TimedPiece { end_ms: 187_130, bytes: b" based".to_vec() },
        ];
        assert_eq!(suffix_after_cursor(&pieces, 186_000, true).as_deref(), Some("supercomputers are based"));
        assert_eq!(suffix_after_cursor(&pieces, 188_000, true), None);
    }

    #[test]
    fn final_overlap_does_not_split_a_multibyte_character() {
        let pieces = [
            TimedPiece { end_ms: 100, bytes: vec![0xe4, 0xb8] },
            TimedPiece { end_ms: 200, bytes: vec![0xad] },
            TimedPiece { end_ms: 300, bytes: "文".as_bytes().to_vec() },
        ];
        assert_eq!(suffix_after_cursor(&pieces, 150, false).as_deref(), Some("中文"));
        assert_eq!(suffix_after_cursor(&pieces, 250, false).as_deref(), Some("文"));
    }
}
