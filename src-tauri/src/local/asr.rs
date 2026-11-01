use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone)]
pub struct SubtitleSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub struct WhisperEngine(WhisperContext);

impl WhisperEngine {
    pub fn prompt_tokens(&self, terms: &[&str]) -> Vec<i32> {
        const MAX_TOKENS: usize = 96;
        let mut prompt = String::new();
        let mut tokens = Vec::new();
        for term in terms.iter().take(32).map(|term| term.trim()) {
            if term.is_empty() || term.contains(['\0', '\r', '\n']) {
                continue;
            }
            let candidate = if prompt.is_empty() {
                term.to_owned()
            } else {
                format!("{prompt}, {term}")
            };
            if let Ok(next) = self.0.tokenize(&candidate, MAX_TOKENS) {
                prompt = candidate;
                tokens = next;
            }
        }
        tokens
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("Whisper model file not found: {}", path.display()));
        }
        let model_path = path.to_string_lossy();
        let mut gpu_params = WhisperContextParameters::default();
        gpu_params.use_gpu(true);
        match WhisperContext::new_with_params(model_path.as_ref(), gpu_params) {
            Ok(context) => Ok(Self(context)),
            Err(gpu_error) => {
                let mut cpu_params = WhisperContextParameters::default();
                cpu_params.use_gpu(false);
                WhisperContext::new_with_params(model_path.as_ref(), cpu_params)
                    .map(Self)
                    .map_err(|cpu_error| {
                        format!(
                            "Could not load Whisper model {} (GPU: {gpu_error}; CPU: {cpu_error})",
                            path.display()
                        )
                    })
            }
        }
    }

    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        language: Option<&str>,
        prompt_tokens: &[i32],
        offset_ms: i64,
    ) -> Result<Vec<SubtitleSegment>, String> {
        let mut state = self
            .0
            .create_state()
            .map_err(|e| format!("Whisper state: {e}"))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(
            std::thread::available_parallelism().map_or(2, |n| n.get().min(8) as i32),
        );
        params.set_language(language);
        if !prompt_tokens.is_empty() {
            params.set_tokens(prompt_tokens);
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        state
            .full(params, samples)
            .map_err(|e| format!("Whisper transcription failed: {e}"))?;
        Ok(state
            .as_iter()
            .filter_map(|segment| {
                let text = segment.to_string().trim().to_owned();
                (!text.is_empty()).then_some(SubtitleSegment {
                    start_ms: offset_ms + segment.start_timestamp() * 10,
                    end_ms: offset_ms + segment.end_timestamp() * 10,
                    text,
                })
            })
            .collect())
    }
}
