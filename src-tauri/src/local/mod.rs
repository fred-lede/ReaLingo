mod asr;
mod glossary;
mod session;
mod translation;

use asr::WhisperEngine;
pub use session::LocalSession;
use std::path::{Path, PathBuf};
use translation::LocalTranslator;

pub struct LocalModels {
    pub asr: WhisperEngine,
    pub translator: Option<LocalTranslator>,
}

#[derive(Default)]
pub struct LocalModelCache {
    paths: Option<(PathBuf, Option<PathBuf>)>,
    models: Option<LocalModels>,
}

impl LocalModelCache {
    pub fn load_for(&mut self, whisper_path: &Path, translation_path: Option<&Path>) -> Result<(), String> {
        let paths = (whisper_path.to_path_buf(), translation_path.map(Path::to_path_buf));
        if self.paths.as_ref() == Some(&paths) {
            return Ok(());
        }
        let asr = WhisperEngine::load(whisper_path)?;
        let translator = translation_path.map(LocalTranslator::load).transpose()?;
        self.models = Some(LocalModels { asr, translator });
        self.paths = Some(paths);
        Ok(())
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{LocalModelCache, LocalTranslator};
    use std::path::Path;
    use std::process::Command;

    #[test]
    #[ignore = "requires a local Whisper model"]
    fn transcription_only_loads_whisper_without_a_translation_file() {
        crate::configure_metal_backend();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let whisper = repo.join("models/whisper/ggml-small.en-q8_0.bin");
        let mut cache = LocalModelCache::default();
        cache.load_for(&whisper, None).unwrap();
        assert!(cache.models_mut().translator.is_none());
        let missing = repo.join("models/translation/missing.gguf");
        assert!(cache.load_for(&whisper, Some(&missing)).unwrap_err().contains("Translation GGUF file not found"));
        let qwen = repo.join("models/translation/Qwen3-4B-Q4_K_M.gguf");
        cache.load_for(&whisper, Some(&qwen)).unwrap();
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
