//! Offline probe of SenseVoice's timestamps and the overlap-clip logic, run
//! against the bundled int8 model. `cargo test --lib sv_probe -- --nocapture`.
#[cfg(test)]
mod sv_probe {
    use crate::local::asr::AsrEngine;
    use crate::local::sensevoice::SenseVoiceEngine;

    fn engine() -> Option<SenseVoiceEngine> {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest.parent().unwrap().join("models/sensevoice");
        (!dir.join("model.onnx").is_file() && !dir.join("model.int8.onnx").is_file())
            .then(|| {
                eprintln!("SKIP: no SenseVoice model under {}", dir.display());
                None
            })
            .unwrap_or_else(|| SenseVoiceEngine::load(&dir).ok())
    }

    #[test]
    fn timestamps_and_overlap_clipping_on_real_audio() {
        let engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("models/sensevoice");

        for name in ["zh.wav", "en.wav"] {
            let wave = match sherpa_onnx::Wave::read(dir.join(name).to_str().unwrap()) {
                Some(w) => w,
                None => {
                    eprintln!("{name}: wave not readable");
                    continue;
                }
            };
            let samples = wave.samples().to_vec();

            // No cursor: the full text, one segment.
            let full = engine.transcribe_samples(&samples, None, 0, None).unwrap();
            eprintln!(
                "{name} full: {} segs, \"{}\" span {}..{}",
                full.len(),
                full.first().map(|s| s.text.as_str()).unwrap_or(""),
                full.first().map(|s| s.start_ms).unwrap_or(0),
                full.last().map(|s| s.end_ms).unwrap_or(0)
            );

            // The log's situation: the window is re-transcribed 4s later and
            // 6s of it is already committed. The fresh tail must survive.
            let clipped = engine.transcribe_samples(&samples, None, 0, Some(4_000)).unwrap();
            eprintln!(
                "{name} cursor=4000: {} segs, \"{}\"",
                clipped.len(),
                clipped.first().map(|s| s.text.as_str()).unwrap_or("")
            );

            // And 8s committed out of the window: only the last 2s are new.
            let stale = engine.transcribe_samples(&samples, None, 0, Some(8_000)).unwrap();
            eprintln!(
                "{name} cursor=8000: {} segs ({} expected: overlap dominates)",
                stale.len(),
                if stale.is_empty() { "0" } else { "nonzero" }
            );
        }
    }

    #[test]
    fn window_tail_stability_determines_commit() {
        let engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("models/sensevoice");

        // The en.wav is 7s of speech with no trailing silence. In the session
        // the window grows to the 10 s cap, so the model sees 3 s of silence
        // after the last word. Simulate that: speech + 3 s of zero.
        let wave = sherpa_onnx::Wave::read(dir.join("en.wav").to_str().unwrap()).unwrap();
        let speech = wave.samples().to_vec();
        let mut window = speech.clone();
        window.extend(std::iter::repeat(0.0f32).take(3 * 16_000)); // 3 s silence
        eprintln!("simulated window: {} s ({} speech + 3 s silence)", window.len() as f64 / 16_000.0, speech.len() as f64 / 16_000.0);

        // No cursor: what does the model report as the last token time?
        let segs = engine.transcribe_samples(&window, None, 0, None).unwrap();
        let span_end = segs.last().map(|s| s.end_ms).unwrap_or(0);
        eprintln!("window segment: \"{}\" span 0..{}",
            segs.first().map(|s| s.text.as_str()).unwrap_or(""),
            span_end);
        // SenseVoice declares itself a stable-window engine: no 2 s
        // margin, so a window segment is committable the moment it is
        // out. The probe asserts the engine really does say so.
        assert!(engine.is_stable_window_engine(), "the session relies on this");
    }
}
