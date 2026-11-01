use super::{asr::SubtitleSegment, glossary::GlossaryMatcher, LocalModels, LocalTranslator, WhisperEngine};
use crate::{
    config::Settings,
    realtime::{self, Event},
};
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Receiver;

const RATE: usize = 16_000;
const HOP: usize = RATE * 2;
const MAX_PENDING: usize = RATE * 16;
// Whisper can move a segment's start backward between overlapping windows.
const TIMESTAMP_JITTER_MS: i64 = 1_000;

fn committable(segment: &SubtitleSegment, committed_until_ms: i64, stable_before_ms: i64) -> bool {
    segment.end_ms > committed_until_ms
        && segment.end_ms <= stable_before_ms
        && segment.start_ms >= committed_until_ms.saturating_sub(TIMESTAMP_JITTER_MS)
}

fn letters(text: &str) -> Vec<(char, usize)> {
    text.char_indices()
        .filter_map(|(index, character)| {
            character.is_alphanumeric().then(|| {
                (character.to_lowercase().next().unwrap_or(character), index + character.len_utf8())
            })
        })
        .collect()
}

fn trim_repeated_prefix(previous: &str, current: &str) -> String {
    let old = letters(previous);
    let new = letters(current);
    for count in (4..=old.len().min(new.len())).rev() {
        if old[old.len() - count..].iter().zip(&new[..count]).all(|(a, b)| a.0 == b.0) {
            let remaining = &current[new[count - 1].1..];
            if new[count - 1].0.is_ascii_alphabetic()
                && remaining.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            return remaining.trim_start_matches(|c: char| !c.is_alphanumeric()).to_owned();
        }
    }
    current.to_owned()
}

pub struct TranscriptionWindow {
    pub samples: Vec<f32>,
    pub offset_ms: i64,
}

#[derive(Default)]
pub struct SpeechSegmenter {
    pending: Vec<i16>,
    since_window: usize,
    consumed: usize,
}

impl SpeechSegmenter {
    pub fn push(&mut self, samples: &[i16]) -> Vec<TranscriptionWindow> {
        let mut windows = Vec::new();
        for sample in samples {
            self.pending.push(*sample);
            self.since_window += 1;
            if self.pending.len() >= MAX_PENDING {
                windows.push(self.window());
                self.advance();
            } else if self.since_window >= HOP {
                windows.push(self.window());
                self.since_window = 0;
            }
        }
        windows
    }
    pub fn flush(&mut self) -> Option<TranscriptionWindow> {
        if self.pending.is_empty() {
            return None;
        }
        let window = self.window();
        self.pending.clear();
        self.since_window = 0;
        Some(window)
    }
    fn window(&self) -> TranscriptionWindow {
        TranscriptionWindow {
            samples: self.pending.iter().map(|s| *s as f32 / 32768.0).collect(),
            offset_ms: (self.consumed * 1000 / RATE) as i64,
        }
    }
    fn advance(&mut self) {
        let drain = HOP.min(self.pending.len());
        self.pending.drain(..drain);
        self.consumed += drain;
        self.since_window = 0;
    }
}

pub struct LocalSession;

struct TranslationJob {
    id: String,
    text: String,
    hints: Vec<(String, String)>,
    queued_at: Instant,
}

impl LocalSession {
    pub fn run(
        app: &AppHandle,
        settings: &Settings,
        target_language_name: &str,
        mut audio_rx: Receiver<Vec<i16>>,
        stop: &Arc<AtomicBool>,
        models: &mut LocalModels,
    ) -> Result<(), String> {
        realtime::note(app, "status", "connecting");
        realtime::note(app, "status", "connected");
        let matcher = GlossaryMatcher::new(&settings.glossary, &settings.target_lang)?;
        let (translation_tx, translation_rx) = std::sync::mpsc::channel();
        let (asr, translator) = (&models.asr, &mut models.translator);
        let asr_prompt_tokens = asr.prompt_tokens(&matcher.asr_terms());
        std::thread::scope(|scope| {
            let translation_matcher = &matcher;
            let translation = scope.spawn(move || {
                Self::translate_loop(app, settings, target_language_name, translator, translation_matcher, translation_rx)
            });
            let asr_result = Self::run_asr(app, settings, &mut audio_rx, stop, asr, &matcher, &asr_prompt_tokens, &translation_tx);
            drop(translation_tx);
            let translation_result = translation
                .join()
                .map_err(|_| "Translation worker panicked".to_string())?;
            translation_result?;
            asr_result
        })?;
        realtime::note(app, "status", "closed");
        Ok(())
    }

    fn run_asr(
        app: &AppHandle,
        settings: &Settings,
        audio_rx: &mut Receiver<Vec<i16>>,
        stop: &Arc<AtomicBool>,
        asr: &WhisperEngine,
        matcher: &GlossaryMatcher,
        asr_prompt_tokens: &[i32],
        translation_tx: &Sender<TranslationJob>,
    ) -> Result<(), String> {
        let mut segmenter = SpeechSegmenter::default();
        let mut committed_until_ms = -1i64;
        let mut last_source = String::new();
        let mut turn = 0u64;
        loop {
            let chunk = audio_rx.blocking_recv();
            if let Some(samples) = chunk {
                for window in segmenter.push(&samples) {
                    Self::process_window(
                        app,
                        settings,
                        asr,
                        matcher,
                        asr_prompt_tokens,
                        translation_tx,
                        window,
                        false,
                        &mut committed_until_ms,
                        &mut last_source,
                        &mut turn,
                        audio_rx.len(),
                    )?;
                }
            } else {
                break;
            }
            if stop.load(Ordering::Relaxed) && audio_rx.is_closed() {
                break;
            }
        }
        if let Some(window) = segmenter.flush() {
            Self::process_window(
                app,
                settings,
                asr,
                matcher,
                asr_prompt_tokens,
                translation_tx,
                window,
                true,
                &mut committed_until_ms,
                &mut last_source,
                &mut turn,
                audio_rx.len(),
            )?;
        }
        Ok(())
    }

    fn process_window(
        app: &AppHandle,
        settings: &Settings,
        asr: &WhisperEngine,
        matcher: &GlossaryMatcher,
        asr_prompt_tokens: &[i32],
        translation_tx: &Sender<TranslationJob>,
        window: TranscriptionWindow,
        force: bool,
        committed_until_ms: &mut i64,
        last_source: &mut String,
        turn: &mut u64,
        audio_queue_len: usize,
    ) -> Result<(), String> {
        let lang = (settings.source_lang != "auto").then_some(settings.source_lang.as_str());
        let started = Instant::now();
        let segments = asr.transcribe_samples(&window.samples, lang, asr_prompt_tokens, window.offset_ms)?;
        let _ = app.emit(realtime::RAW_EVENT, serde_json::json!({
            "type": "local.asr",
            "windowMs": window.samples.len() * 1000 / RATE,
            "elapsedMs": started.elapsed().as_millis(),
            "segments": segments.len(),
            "audioQueueChunks": audio_queue_len,
        }));
        let window_end_ms = window.offset_ms + (window.samples.len() * 1000 / RATE) as i64;
        let stable_before_ms = if force {
            i64::MAX
        } else {
            window_end_ms - 2_000
        };
        for segment in segments {
            if !committable(&segment, *committed_until_ms, stable_before_ms) {
                continue;
            }
            let raw_text = if segment.start_ms < *committed_until_ms {
                trim_repeated_prefix(last_source, &segment.text)
            } else {
                segment.text.clone()
            };
            if raw_text.trim().is_empty() {
                continue;
            }
            *committed_until_ms = segment.end_ms;
            *last_source = raw_text.clone();
            let matched = matcher.apply(raw_text.trim());
            let text = matched.text;
            *turn += 1;
            let id = format!("local-{turn}");
            let input = format!("{id}-input");
            realtime::emit(
                app,
                Event {
                    kind: "link",
                    text: input.clone(),
                    id: id.clone(),
                    done: true,
                    ..Default::default()
                },
            );
            realtime::emit(
                app,
                Event {
                    kind: "source",
                    text: text.clone(),
                    lang: if settings.source_lang == "auto" {
                        String::new()
                    } else {
                        settings.source_lang.clone()
                    },
                    id: input,
                    done: true,
                    ..Default::default()
                },
            );
            translation_tx
                .send(TranslationJob {
                    id,
                    text,
                    hints: matched.hints,
                    queued_at: Instant::now(),
                })
                .map_err(|_| "Translation worker stopped".to_string())?;
        }
        Ok(())
    }

    fn translate_loop(
        app: &AppHandle,
        settings: &Settings,
        target: &str,
        translator: &mut LocalTranslator,
        matcher: &GlossaryMatcher,
        jobs: std::sync::mpsc::Receiver<TranslationJob>,
    ) -> Result<(), String> {
        let mut context = VecDeque::with_capacity(2);
        for job in jobs {
            let queue_wait_ms = job.queued_at.elapsed().as_millis();
            let context_text = context.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
            let possible = matcher.possible_terms(&job.text, &context_text).into_iter()
                .filter(|term| !job.hints.contains(term)).collect::<Vec<_>>();
            let started = Instant::now();
            let result = translator.translate(&job.text, target, &context_text, settings.target_lang == "zh", &job.hints, &possible);
            let _ = app.emit(realtime::RAW_EVENT, serde_json::json!({
                "type": "local.translation",
                "id": &job.id,
                "queueWaitMs": queue_wait_ms,
                "elapsedMs": started.elapsed().as_millis(),
                "ok": result.is_ok(),
            }));
            realtime::emit(app, Event {
                kind: "target",
                text: result?,
                id: job.id,
                done: true,
                ..Default::default()
            });
            if context.len() == 2 {
                context.pop_front();
            }
            context.push_back(job.text);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{committable, trim_repeated_prefix, SubtitleSegment};
    use crate::{config::GlossaryEntry, local::glossary::GlossaryMatcher};

    #[test]
    fn raw_overlap_dedup_precedes_glossary_correction() {
        let matcher = GlossaryMatcher::new(&[GlossaryEntry {
            source: "LoRA".into(), aliases: vec!["Loari".into()],
            target: "低秩适配".into(), target_lang: "zh".into(),
        }], "zh").unwrap();
        let raw = trim_repeated_prefix("about Loari", "about Loari and GGUF");
        assert_eq!(raw, "and GGUF");
        let corrected = matcher.apply("Loari and GGUF");
        assert_eq!(corrected.text, "LoRA and GGUF");
        assert_eq!(corrected.hints.len(), 1);
    }

    #[test]
    fn repeated_prefix_is_trimmed_without_losing_new_words() {
        assert_eq!(trim_repeated_prefix("optimizations.", "optimizations. Tested CUDA presets"), "Tested CUDA presets");
        assert_eq!(trim_repeated_prefix("into GGUF packages.", "GGUF packages."), "");
        assert_eq!(trim_repeated_prefix("前面的人工智能", "人工智能技術正在發展"), "技術正在發展");
        assert_eq!(trim_repeated_prefix("previous sentence", "new sentence"), "new sentence");
    }

    #[test]
    fn timestamp_jitter_does_not_discard_a_new_sentence() {
        let sentence = SubtitleSegment { start_ms: 16_000, end_ms: 19_000, text: "Performance modes".into() };
        assert!(committable(&sentence, 16_400, 24_000));
        let mostly_repeated = SubtitleSegment { start_ms: 4_000, end_ms: 9_600, text: "old text".into() };
        assert!(!committable(&mostly_repeated, 9_260, 18_000));
        let unfinished = SubtitleSegment { start_ms: 20_000, end_ms: 24_000, text: "unfinished".into() };
        assert!(!committable(&unfinished, 19_000, 22_000));
    }
}
