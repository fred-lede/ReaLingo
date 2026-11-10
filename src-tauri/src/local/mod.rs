mod asr;
mod backend_status;
mod glossary;
pub mod sensevoice;
mod session;
#[cfg(test)]
mod sensevoice_smoke;
#[cfg(test)]
mod sv_probe;
mod translation;

pub use asr::{AsrEngine, WhisperEngine};
pub use backend_status::BackendStatus;
pub use sensevoice::SenseVoiceEngine;
pub use session::LocalSession;
use std::path::{Path, PathBuf};
use translation::LocalTranslator;

/// Which local ASR engine the session uses. Whisper keeps its GPU path;
/// SenseVoice is a fast CPU alternative for the same VAD window loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AsrBackend {
    #[default]
    Whisper,
    SenseVoice,
}

pub struct LocalModels {
    pub asr: Box<dyn AsrEngine>,
    pub translator: Option<LocalTranslator>,
}

#[derive(Default)]
pub struct LocalModelCache {
    paths: Option<(AsrBackend, PathBuf, Option<PathBuf>)>,
    models: Option<LocalModels>,
}

impl LocalModelCache {
    /// Load and probe the models. The probe is what turns the backends out
    /// of "detecting", so the caller is expected to surface the resulting
    /// statuses (`backend_report`) in the UI while the user is choosing
    /// settings — not while a sentence is in flight.
    pub fn load_for(
        &mut self,
        backend: AsrBackend,
        asr_path: &Path,
        translation_path: Option<&Path>,
    ) -> Result<(), String> {
        let paths = (backend, asr_path.to_path_buf(), translation_path.map(Path::to_path_buf));
        if self.paths.as_ref() == Some(&paths) {
            return Ok(());
        }
        let mut asr = match backend {
            AsrBackend::Whisper => Box::new(WhisperEngine::load(asr_path)?) as Box<dyn AsrEngine>,
            AsrBackend::SenseVoice => Box::new(SenseVoiceEngine::load(asr_path)?) as Box<dyn AsrEngine>,
        };
        let mut translator = translation_path.map(LocalTranslator::load).transpose()?;
        asr.ready()?;
        if let Some(t) = translator.as_mut() {
            t.ready()?;
        }
        self.models = Some(LocalModels { asr, translator });
        self.paths = Some(paths);
        Ok(())
    }

    /// The backends as the last load left them. `None` entries are the
    /// stages that have no model (a transcription-only run has no
    /// translator), which the UI reports as "not applicable" rather than
    /// "detecting forever".
    pub fn backend_report(&self) -> (Option<BackendStatus>, Option<BackendStatus>) {
        let models = match self.models.as_ref() {
            Some(models) => models,
            None => return (None, None),
        };
        (Some(models.asr.backend_status()), models.translator.as_ref().map(|t| t.backend_status()))
    }

    pub fn models_mut(&mut self) -> &mut LocalModels {
        self.models
            .as_mut()
            .expect("models loaded before local session")
    }

    pub fn clear(&mut self) {
        self.models = None;
        self.paths = None;
    }
}

impl AsrBackend {
    /// The engine the settings point at: the explicit `asr_engine` choice,
    /// falling back to whichever model path is filled in.
    pub fn from_settings(settings: &crate::config::Settings) -> Self {
        settings.asr_backend()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{AsrBackend, LocalModelCache, LocalTranslator};
    use std::path::Path;
    use std::process::Command;

    #[test]
    #[ignore = "requires a local Whisper model"]
    fn transcription_only_loads_whisper_without_a_translation_file() {
        crate::configure_metal_backend();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let whisper = repo.join("models/whisper/ggml-small.en-q8_0.bin");
        let mut cache = LocalModelCache::default();
        cache.load_for(AsrBackend::Whisper, &whisper, None).unwrap();
        assert!(cache.models_mut().translator.is_none());
        let missing = repo.join("models/translation/missing.gguf");
        cache.load_for(AsrBackend::Whisper, &whisper, Some(&missing)).unwrap_err();
        let qwen = repo.join("models/translation/Qwen3-4B-Q4_K_M.gguf");
        cache.load_for(AsrBackend::Whisper, &whisper, Some(&qwen)).unwrap();
        assert!(cache.models_mut().translator.is_some());
    }

    #[test]
    #[ignore = "requires the local Qwen GGUF model"]
    fn metal_process_exit_with_live_translation_model_does_not_abort() {
        const CHILD: &str = "REALINGO_METAL_EXIT_PROBE";
        if std::env::var_os(CHILD).is_some() {
            crate::configure_metal_backend();
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
            let qwen = repo.join("models/translation/Qwen3-4B-Q4_K_M.gguf");
            let mut translator = LocalTranslator::load(&qwen).unwrap();
            translator.translate("Hello, world.", "Traditional Chinese", "", true, &[], &[]).unwrap();
            // A native quit can reach C++ finalizers while a Metal model is still live.
            // Exit directly so this child checks that finalization path, not Rust Drop.
            let _translator = translator;
            std::process::exit(0);
        }

        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "local::tests::metal_process_exit_with_live_translation_model_does_not_abort", "--ignored"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success(), "Metal model teardown aborted: {status}");
    }
}
