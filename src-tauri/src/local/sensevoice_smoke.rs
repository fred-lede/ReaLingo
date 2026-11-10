//! Smoke test: load the real SenseVoice ONNX model and transcribe two short
//! WAV files. Run with `cargo test --lib sensevoice_smoke -- --nocapture`.
//! Skips quietly when the model directory is not present.
#[cfg(test)]
mod sensevoice_smoke {
    use crate::local::asr::AsrEngine;
    use crate::local::sensevoice::SenseVoiceEngine;

    #[test]
    fn the_bundled_sensevoice_model_transcribes_chinese_and_english() {
        // CARGO_MANIFEST_DIR is src-tauri; the models directory is at the
        // repo root, two levels up.
        // CARGO_MANIFEST_DIR = <repo>/src-tauri; models/ is at <repo>/models.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest.parent().unwrap().join("models/sensevoice");
        if !dir.join("model.onnx").is_file() && !dir.join("model.int8.onnx").is_file() {
            eprintln!("SKIP: no SenseVoice model under {}", dir.display());
            return;
        }

        let engine = SenseVoiceEngine::load(&dir).unwrap();

        let wave = sherpa_onnx::Wave::read(dir.join("zh.wav").to_str().unwrap()).unwrap();
        let segments = engine
            .transcribe_samples(wave.samples(), None, 0, None)
            .expect("transcription should succeed");
        assert!(!segments.is_empty(), "expected at least one segment");
        let text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
        eprintln!("zh.wav -> {}", text);

        let wave_en =
            sherpa_onnx::Wave::read(dir.join("en.wav").to_str().unwrap()).unwrap();
        let segments_en = engine
            .transcribe_samples(wave_en.samples(), None, 0, None)
            .expect("transcription should succeed");
        let text_en = segments_en
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("en.wav -> {}", text_en);
        assert!(!text_en.trim().is_empty());
    }
}
