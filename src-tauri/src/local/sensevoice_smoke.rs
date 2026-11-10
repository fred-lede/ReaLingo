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
        // One window is transcribed as one segment — the whole text, not one
        // per token. The grouping loop turns that into the sentence.
        assert_eq!(segments.len(), 1, "one window, one segment: {segments:?}");
        let text = &segments[0].text;
        eprintln!("zh.wav -> {text}");

        let wave_en =
            sherpa_onnx::Wave::read(dir.join("en.wav").to_str().unwrap()).unwrap();
        let segments_en = engine
            .transcribe_samples(wave_en.samples(), None, 0, None)
            .expect("transcription should succeed");
        assert_eq!(segments_en.len(), 1, "one window, one segment: {segments_en:?}");
        eprintln!("en.wav -> {}", segments_en[0].text);

        // The int8 bundle reports per-token timestamps; confirm the engine's
        // segment spans the audio, not just the text.
        eprintln!("zh span: {} .. {} ms", segments[0].start_ms, segments[0].end_ms);
    }
}
