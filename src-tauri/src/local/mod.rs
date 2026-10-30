mod asr;
mod session;
mod translation;

use asr::WhisperEngine;
pub use session::LocalSession;
use std::path::{Path, PathBuf};
use translation::LocalTranslator;

pub struct LocalModels {
    pub asr: WhisperEngine,
    pub translator: LocalTranslator,
}

#[derive(Default)]
pub struct LocalModelCache {
    paths: Option<(PathBuf, PathBuf)>,
    models: Option<LocalModels>,
}

impl LocalModelCache {
    pub fn load_for(&mut self, whisper_path: &Path, translation_path: &Path) -> Result<(), String> {
        let paths = (whisper_path.to_path_buf(), translation_path.to_path_buf());
        if self.paths.as_ref() == Some(&paths) {
            return Ok(());
        }
        let asr = WhisperEngine::load(whisper_path)?;
        let translator = LocalTranslator::load(translation_path)?;
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
